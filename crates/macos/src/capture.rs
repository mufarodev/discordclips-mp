//! M1: ScreenCaptureKit display capture.
//!
//! Frames stay GPU-resident (IOSurface-backed `CVPixelBuffer`, NV12 `420v`) and are handed
//! to the encoder untouched. SCK *pushes* frames on its own queue; `next_frame` pulls from a
//! small bounded channel and the SCK callback drops frames when it is full (never blocks the
//! SCK queue, which would starve its IOSurface pool).
//!
//! SCK only delivers frames when the screen changes. A static screen would otherwise yield a
//! clip of ~0 s, so `next_frame` re-submits the last complete buffer with an advanced pts when
//! nothing has arrived for `REPEAT_AFTER`.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use block2::RcBlock;
use clipsd_core::Capture;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamErrorDomain, SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
};

/// `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange` ('420v'): NV12, which VideoToolbox
/// encodes natively. BGRA would force a conversion.
const PIXEL_FORMAT_420V: u32 = 0x3432_3076;
/// `SCStreamErrorUserDeclined`.
const SC_ERROR_USER_DECLINED: isize = -3801;
const REPEAT_AFTER: Duration = Duration::from_millis(100);
const CHANNEL_DEPTH: usize = 4;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

/// A captured video frame: an IOSurface-backed pixel buffer.
pub struct Frame(pub CFRetained<CVPixelBuffer>);
// CVPixelBuffer is an atomically ref-counted CF object; it is safe to move between threads.
unsafe impl Send for Frame {}

#[derive(Debug)]
pub enum CaptureError {
    /// Screen Recording permission (TCC) not granted to the responsible process.
    PermissionDenied,
    NoDisplay,
    Sck(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::PermissionDenied => write!(
                f,
                "Screen Recording permission denied. Grant it to the app that launched this \
                 process (your terminal / IDE) in System Settings → Privacy & Security → \
                 Screen & System Audio Recording, then fully quit and relaunch that app."
            ),
            CaptureError::NoDisplay => write!(f, "no display available to capture"),
            CaptureError::Sck(m) => write!(f, "ScreenCaptureKit: {m}"),
        }
    }
}
impl std::error::Error for CaptureError {}

/// Map an SCK `NSError` to a `CaptureError`, recognising the TCC "user declined" case.
fn map_error(what: &str, err: &NSError) -> CaptureError {
    let domain = err.domain();
    let sck_domain = unsafe { SCStreamErrorDomain };
    if domain.isEqualToString(sck_domain) && err.code() == SC_ERROR_USER_DECLINED {
        return CaptureError::PermissionDenied;
    }
    CaptureError::Sck(format!("{what}: {} (domain {domain}, code {})", err.localizedDescription(), err.code()))
}

fn cmtime_to_us(t: CMTime) -> i64 {
    if !t.flags.contains(CMTimeFlags::Valid) || t.timescale == 0 {
        return 0;
    }
    (t.value as i128 * 1_000_000 / t.timescale as i128) as i64
}

struct Item {
    frame: Frame,
    pts_us: i64,
}

struct SinkIvars {
    tx: Mutex<SyncSender<Item>>,
    /// Frames dropped because the consumer fell behind.
    dropped: std::sync::atomic::AtomicU64,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "ClipsdFrameSink"]
    #[ivars = SinkIvars]
    struct FrameSink;

    unsafe impl NSObjectProtocol for FrameSink {}

    unsafe impl SCStreamOutput for FrameSink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(&self, _stream: &SCStream, sb: &CMSampleBuffer, ty: SCStreamOutputType) {
            if ty != SCStreamOutputType::Screen {
                return;
            }
            self.handle(sb);
        }
    }
);

impl FrameSink {
    fn new(tx: SyncSender<Item>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(SinkIvars {
            tx: Mutex::new(tx),
            dropped: Default::default(),
        });
        unsafe { msg_send![super(this), init] }
    }

    /// Frame status from the sample attachments; `None` if absent.
    fn status(sb: &CMSampleBuffer) -> Option<SCFrameStatus> {
        let arr = unsafe { sb.sample_attachments_array(false) }?;
        // CFArray of CFDictionary is toll-free bridged.
        let arr: &NSArray<NSDictionary<AnyObject, AnyObject>> =
            unsafe { &*(&*arr as *const _ as *const NSArray<NSDictionary<AnyObject, AnyObject>>) };
        let dict = arr.firstObject()?;
        let key: &AnyObject = unsafe { &*(SCStreamFrameInfoStatus as *const _ as *const AnyObject) };
        let v = dict.objectForKey(key)?;
        let n = v.downcast_ref::<NSNumber>()?;
        Some(SCFrameStatus(n.integerValue()))
    }

    fn handle(&self, sb: &CMSampleBuffer) {
        // Only `Complete` frames carry an image buffer; Idle/Blank/Suspended do not.
        if let Some(st) = Self::status(sb) {
            if st != SCFrameStatus::Complete {
                return;
            }
        }
        let Some(img) = (unsafe { sb.image_buffer() }) else { return };
        let pts_us = cmtime_to_us(unsafe { sb.presentation_time_stamp() });
        let item = Item { frame: Frame(img), pts_us };
        if let Err(TrySendError::Full(_)) = self.ivars().tx.lock().unwrap().try_send(item) {
            self.ivars().dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

pub struct CaptureConfig {
    /// Output width cap in pixels (height follows the display aspect ratio). Apple's hardware
    /// H.264 encoder tops out around 4096 wide; 1920 is a safe default.
    pub max_width: usize,
    pub fps: i32,
}
impl Default for CaptureConfig {
    fn default() -> Self {
        Self { max_width: 1920, fps: 30 }
    }
}

pub struct SckCapture {
    stream: Retained<SCStream>,
    _sink: Retained<FrameSink>,
    _queue: DispatchRetained<DispatchQueue>,
    rx: Receiver<Item>,
    /// Output size in pixels.
    pub width: usize,
    pub height: usize,
    last: Option<(CFRetained<CVPixelBuffer>, i64, Instant)>,
    last_emitted_us: i64,
}

fn fetch_display() -> Result<Retained<SCDisplay>, CaptureError> {
    let (tx, rx) = mpsc::channel::<Result<Retained<SCDisplay>, CaptureError>>();
    let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let r = unsafe {
            if let Some(e) = err.as_ref() {
                Err(map_error("shareable content", e))
            } else if let Some(c) = content.as_ref() {
                c.displays().firstObject().ok_or(CaptureError::NoDisplay)
            } else {
                Err(CaptureError::Sck("empty shareable content".into()))
            }
        };
        let _ = tx.send(r);
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(r) => r,
        Err(_) => Err(CaptureError::Sck(
            "timed out fetching shareable content (is a permission dialog pending?)".into(),
        )),
    }
}

impl SckCapture {
    /// Start capturing the first (main) display.
    pub fn start(cfg: &CaptureConfig) -> Result<Self, CaptureError> {
        let display = fetch_display()?;
        if !unsafe { CGPreflightScreenCaptureAccess() } {
            // SCK may still have returned content (redacted); treat as denied.
            return Err(CaptureError::PermissionDenied);
        }
        let (dw, dh) = unsafe { (display.width() as usize, display.height() as usize) };
        let width = (dw.min(cfg.max_width).max(2)) & !1;
        let height = ((dh * width / dw.max(1)).max(2)) & !1;

        let excluded = NSArray::new();
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &excluded)
        };
        let conf = unsafe { SCStreamConfiguration::new() };
        unsafe {
            conf.setWidth(width);
            conf.setHeight(height);
            conf.setPixelFormat(PIXEL_FORMAT_420V);
            conf.setMinimumFrameInterval(CMTime {
                value: 1,
                timescale: cfg.fps,
                flags: CMTimeFlags::Valid,
                epoch: 0,
            });
            conf.setQueueDepth(5);
            conf.setShowsCursor(true);
            conf.setScalesToFit(true);
        }

        let (tx, rx) = mpsc::sync_channel(CHANNEL_DEPTH);
        let sink = FrameSink::new(tx);
        let queue = DispatchQueue::new("clipsd.capture", None);
        let stream = unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &conf, None) };
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*sink),
                    SCStreamOutputType::Screen,
                    Some(&queue),
                )
                .map_err(|e| map_error("addStreamOutput", &e))?;
        }

        let (stx, srx) = mpsc::channel::<Option<CaptureError>>();
        let done = RcBlock::new(move |err: *mut NSError| {
            let _ = stx.send(unsafe { err.as_ref() }.map(|e| map_error("startCapture", e)));
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&done)) };
        match srx.recv_timeout(Duration::from_secs(15)) {
            Ok(None) => {}
            Ok(Some(e)) => return Err(e),
            Err(_) => return Err(CaptureError::Sck("timed out starting capture".into())),
        }

        Ok(Self { stream, _sink: sink, _queue: queue, rx, width, height, last: None, last_emitted_us: i64::MIN })
    }

    /// Frames the SCK callback dropped because the consumer was too slow.
    pub fn dropped(&self) -> u64 {
        self._sink.ivars().dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn stop(&self) {
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |_err: *mut NSError| {
            let _ = tx.send(());
        });
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&done)) };
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }
}

impl Capture for SckCapture {
    type Frame = Frame;

    /// Blocks until a frame is available. Real frames keep SCK's host-clock pts; if the screen
    /// is static for `REPEAT_AFTER`, the last frame is repeated with an advanced pts. pts is
    /// strictly increasing.
    fn next_frame(&mut self) -> Result<(Frame, i64), String> {
        loop {
            match self.rx.recv_timeout(REPEAT_AFTER) {
                Ok(item) => {
                    if item.pts_us <= self.last_emitted_us {
                        continue; // keep pts strictly monotonic (a repeat may have run ahead)
                    }
                    self.last = Some((item.frame.0.clone(), item.pts_us, Instant::now()));
                    self.last_emitted_us = item.pts_us;
                    return Ok((item.frame, item.pts_us));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some((buf, pts, at)) = &self.last {
                        let pts_us = (pts + at.elapsed().as_micros() as i64).max(self.last_emitted_us + 1);
                        self.last_emitted_us = pts_us;
                        return Ok((Frame(buf.clone()), pts_us));
                    }
                    // nothing captured yet: keep waiting
                }
                Err(RecvTimeoutError::Disconnected) => return Err("capture stream closed".into()),
            }
        }
    }
}

impl Drop for SckCapture {
    fn drop(&mut self) {
        self.stop();
    }
}
