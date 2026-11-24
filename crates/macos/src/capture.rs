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

use std::ptr::NonNull;
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
use objc2_core_audio_types::{kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, AudioBufferList};
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer, CMTime, CMTimeFlags,
};
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamErrorDomain, SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
};

/// `kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange` ('420v'): NV12, which VideoToolbox
/// encodes natively. BGRA would force a conversion.
const PIXEL_FORMAT_420V: u32 = 0x3432_3076;
/// `SCStreamErrorUserDeclined`.
const SC_ERROR_USER_DECLINED: isize = -3801;
const REPEAT_AFTER: Duration = Duration::from_millis(100);
/// Minimum spacing between emitted frames (µs); keeps durations non-zero in the mp4.
const MIN_FRAME_GAP_US: i64 = 5_000;
const CHANNEL_DEPTH: usize = 2;
/// ~10 ms chunks arrive 100/s; leave a couple of seconds of slack.
const AUDIO_CHANNEL_DEPTH: usize = 256;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGMainDisplayID() -> u32;
}

/// Initialize the CoreGraphics WindowServer connection on the main thread before any worker
/// thread calls ScreenCaptureKit (`initWithDesktopIndependentWindow` otherwise aborts in
/// `CGS_REQUIRE_INIT` on macOS).
pub fn init_window_server() {
    unsafe {
        let _ = CGMainDisplayID();
        let _ = CGPreflightScreenCaptureAccess();
    }
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

/// A chunk of captured system audio: interleaved f32 PCM, first frame at `pts_us` (host clock).
pub struct PcmChunk {
    pub pts_us: i64,
    pub sample_rate: u32,
    pub channels: u8,
    pub samples: Vec<f32>,
}

struct Item {
    frame: Frame,
    pts_us: i64,
}

struct SinkIvars {
    tx: Mutex<SyncSender<Item>>,
    atx: Mutex<SyncSender<PcmChunk>>,
    /// Frames dropped because the consumer fell behind.
    dropped: std::sync::atomic::AtomicU64,
    dropped_audio: std::sync::atomic::AtomicU64,
}

/// Copy an SCK audio sample buffer into interleaved f32. SCK delivers 32-bit float PCM, usually
/// non-interleaved (one plane per channel).
unsafe fn extract_pcm(sb: &CMSampleBuffer) -> Option<PcmChunk> {
    let fd = sb.format_description()?;
    let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(&fd).as_ref()?;
    if asbd.mFormatFlags & kAudioFormatFlagIsFloat == 0 || asbd.mBitsPerChannel != 32 {
        return None; // only f32 is handled
    }
    let ch = asbd.mChannelsPerFrame as usize;
    let nonint = asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;
    let frames = sb.num_samples() as usize;
    if ch == 0 || frames == 0 {
        return None;
    }
    // Two-step: ask how big the AudioBufferList is, then fetch it with exactly that size.
    let mut needed = 0usize;
    let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
    let _size_status = sb.audio_buffer_list_with_retained_block_buffer(
        &mut needed,
        std::ptr::null_mut(),
        0,
        None,
        None,
        0,
        std::ptr::null_mut(),
    );
    let mut storage = vec![0u64; needed.div_ceil(8).max(8)];
    let st = sb.audio_buffer_list_with_retained_block_buffer(
        std::ptr::null_mut(),
        storage.as_mut_ptr().cast(),
        needed,
        None,
        None,
        0,
        &mut block,
    );
    // Owns the +1 retain on the block buffer so the PCM stays valid until we're done.
    let _guard = NonNull::new(block).map(|b| CFRetained::from_raw(b));
    if st != 0 {
        return None;
    }
    let abl = &*(storage.as_ptr() as *const AudioBufferList);
    let bufs = std::slice::from_raw_parts(abl.mBuffers.as_ptr(), abl.mNumberBuffers as usize);
    let mut out = vec![0f32; frames * ch];
    if nonint {
        if bufs.len() < ch {
            return None;
        }
        for (c, b) in bufs.iter().take(ch).enumerate() {
            let src = b.mData as *const f32;
            if src.is_null() || (b.mDataByteSize as usize) < frames * 4 {
                return None;
            }
            for i in 0..frames {
                out[i * ch + c] = src.add(i).read_unaligned();
            }
        }
    } else {
        let b = bufs.first()?;
        if b.mData.is_null() || (b.mDataByteSize as usize) < frames * ch * 4 {
            return None;
        }
        std::ptr::copy_nonoverlapping(b.mData as *const f32, out.as_mut_ptr(), frames * ch);
    }
    Some(PcmChunk {
        pts_us: cmtime_to_us(sb.presentation_time_stamp()),
        sample_rate: asbd.mSampleRate as u32,
        channels: ch as u8,
        samples: out,
    })
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
            if ty == SCStreamOutputType::Screen {
                self.handle(sb);
            } else if ty == SCStreamOutputType::Audio {
                self.handle_audio(sb);
            }
        }
    }
);

impl FrameSink {
    fn new(tx: SyncSender<Item>, atx: SyncSender<PcmChunk>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(SinkIvars {
            tx: Mutex::new(tx),
            atx: Mutex::new(atx),
            dropped: Default::default(),
            dropped_audio: Default::default(),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn handle_audio(&self, sb: &CMSampleBuffer) {
        let Some(chunk) = (unsafe { extract_pcm(sb) }) else { return };
        if let Err(TrySendError::Full(_)) = self.ivars().atx.lock().unwrap().try_send(chunk) {
            self.ivars().dropped_audio.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
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
    /// Also capture system audio (48 kHz stereo), excluding this process's own audio.
    pub audio: bool,
    /// Optional CGWindowID to capture; falls back to main display if absent or not found.
    pub window_id: Option<u32>,
}
impl Default for CaptureConfig {
    fn default() -> Self {
        Self { max_width: 1920, fps: 30, audio: true, window_id: None }
    }
}

pub struct SckCapture {
    stream: Retained<SCStream>,
    _sink: Retained<FrameSink>,
    _queue: DispatchRetained<DispatchQueue>,
    _audio_queue: DispatchRetained<DispatchQueue>,
    rx: Receiver<Item>,
    audio_rx: Option<Receiver<PcmChunk>>,
    /// Output size in pixels.
    pub width: usize,
    pub height: usize,
    last: Option<(CFRetained<CVPixelBuffer>, i64, Instant)>,
    last_emitted_us: i64,
    trace: bool,
    t0: Instant,
    last_real_pts: i64,
    repeats: u64,
}

fn fetch_content() -> Result<Retained<SCShareableContent>, CaptureError> {
    let (tx, rx) = mpsc::channel::<Result<Retained<SCShareableContent>, CaptureError>>();
    let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let r = unsafe {
            if let Some(e) = err.as_ref() {
                Err(map_error("shareable content", e))
            } else if let Some(c) = NonNull::new(content) {
                Ok(Retained::retain(c.as_ptr()).unwrap())
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

fn build_filter_and_size(
    content: &SCShareableContent,
    window_id: Option<u32>,
    max_width: usize,
    retina_scale: usize,
) -> Result<(Retained<SCContentFilter>, usize, usize), CaptureError> {
    if let Some(wid) = window_id {
        let windows = unsafe { content.windows() };
        for w in windows.iter() {
            if unsafe { w.windowID() } == wid {
                let frame = unsafe { w.frame() };
                let rw = (frame.size.width.max(2.0) as usize) * retina_scale;
                let rh = (frame.size.height.max(2.0) as usize) * retina_scale;
                let width = (rw.min(max_width).max(2)) & !1;
                let height = ((rh * width / rw.max(1)).max(2)) & !1;
                let filter = unsafe {
                    SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &w)
                };
                return Ok((filter, width, height));
            }
        }
        eprintln!("clipsd: window {wid} not found in shareable content; falling back to main display");
    }
    let display = unsafe { content.displays() }.firstObject().ok_or(CaptureError::NoDisplay)?;
    let (dw, dh) = unsafe { (display.width() as usize * retina_scale, display.height() as usize * retina_scale) };
    let width = (dw.min(max_width).max(2)) & !1;
    let height = ((dh * width / dw.max(1)).max(2)) & !1;
    let excluded = NSArray::new();
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &excluded)
    };
    Ok((filter, width, height))
}

impl SckCapture {
    /// Start capturing the requested window (if present) or the first (main) display.
    pub fn start(cfg: &CaptureConfig) -> Result<Self, CaptureError> {
        init_window_server();
        let content = fetch_content()?;
        if !unsafe { CGPreflightScreenCaptureAccess() } {
            // SCK may still have returned content (redacted); treat as denied.
            return Err(CaptureError::PermissionDenied);
        }
        let (filter, width, height) = build_filter_and_size(&content, cfg.window_id, cfg.max_width, 1)?;
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
            // SCK renders into a pool of this many IOSurfaces; every frame we hold (channel, repeat
            // buffer, encoder) is one it can't reuse, so keep the pool larger than what we hold.
            conf.setQueueDepth(8);
            conf.setShowsCursor(true);
            conf.setScalesToFit(true);
            if cfg.audio {
                conf.setCapturesAudio(true);
                conf.setSampleRate(48_000);
                conf.setChannelCount(2);
                conf.setExcludesCurrentProcessAudio(true);
            }
        }

        let (tx, rx) = mpsc::sync_channel(CHANNEL_DEPTH);
        let (atx, arx) = mpsc::sync_channel(AUDIO_CHANNEL_DEPTH);
        let sink = FrameSink::new(tx, atx);
        let queue = DispatchQueue::new("clipsd.capture.video", None);
        let audio_queue = DispatchQueue::new("clipsd.capture.audio", None);
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

        if cfg.audio {
            unsafe {
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(
                        ProtocolObject::from_ref(&*sink),
                        SCStreamOutputType::Audio,
                        Some(&audio_queue),
                    )
                    .map_err(|e| map_error("addStreamOutput(audio)", &e))?;
            }
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

        Ok(Self { stream, _sink: sink, _queue: queue, _audio_queue: audio_queue, rx, audio_rx: cfg.audio.then_some(arx), width, height, last: None, last_emitted_us: i64::MIN, trace: std::env::var_os("CLIPSD_TRACE").is_some(), t0: Instant::now(), last_real_pts: 0, repeats: 0 })
    }

    /// Discard frames queued while the consumer was still starting up (encoder init can take
    /// hundreds of ms while SCK is already streaming). Without this, the first frames submitted
    /// are stale and the next fresh one is a whole init-duration later: a hole in the clip.
    pub fn flush_backlog(&mut self) {
        while self.rx.try_recv().is_ok() {}
        self.last = None;
        self.last_emitted_us = i64::MIN;
    }

    /// Take the audio chunk receiver (once). `None` if audio capture is off or already taken.
    pub fn take_audio(&mut self) -> Option<Receiver<PcmChunk>> {
        self.audio_rx.take()
    }

    /// Audio chunks dropped because the consumer was too slow.
    pub fn dropped_audio(&self) -> u64 {
        self._sink.ivars().dropped_audio.load(std::sync::atomic::Ordering::Relaxed)
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

impl SckCapture {
    /// Wait up to `wait` for a frame. Real frames keep SCK's host-clock pts; if the screen is
    /// static for `REPEAT_AFTER`, the last frame is repeated with an advanced pts. pts is
    /// strictly increasing. Returns `Ok(None)` if nothing is available yet (no frame has ever
    /// arrived, or `wait` elapsed).
    pub fn poll_frame(&mut self, wait: Duration) -> Result<Option<(Frame, i64)>, String> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left.min(REPEAT_AFTER)) {
                Ok(item) => {
                    // Never drop a real frame (it may be the last change before a static
                    // stretch). A repeat may have run slightly ahead of SCK's clock, so nudge the
                    // timestamp forward instead to keep pts strictly increasing.
                    if self.trace {
                        let wall = self.t0.elapsed().as_micros() as i64;
                        let jump = item.pts_us - self.last_real_pts;
                        if self.last_real_pts != 0 && !(0..150_000).contains(&jump) {
                            eprintln!("[trace] wall={}ms real pts jump {}ms (emitted-ahead={}ms)", wall / 1000, jump / 1000, (self.last_emitted_us - item.pts_us) / 1000);
                        }
                        self.last_real_pts = item.pts_us;
                    }
                    let pts_us = item.pts_us.max(self.last_emitted_us.saturating_add(MIN_FRAME_GAP_US));
                    self.last = Some((item.frame.0.clone(), pts_us, Instant::now()));
                    self.last_emitted_us = pts_us;
                    return Ok(Some((item.frame, pts_us)));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some((buf, pts, at)) = &self.last {
                        if self.trace {
                            self.repeats += 1;
                            if self.repeats == 1 || self.repeats % 20 == 0 {
                                eprintln!("[trace] wall={}ms repeat #{} (screen static or SCK stalled)", self.t0.elapsed().as_millis(), self.repeats);
                            }
                        }
                        let pts_us = (pts + at.elapsed().as_micros() as i64).max(self.last_emitted_us.saturating_add(MIN_FRAME_GAP_US));
                        self.last_emitted_us = pts_us;
                        return Ok(Some((Frame(buf.clone()), pts_us)));
                    }
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Err("capture stream closed".into()),
            }
        }
    }
}

impl Capture for SckCapture {
    type Frame = Frame;

    /// Blocks until a frame is available.
    fn next_frame(&mut self) -> Result<(Frame, i64), String> {
        loop {
            if let Some(f) = self.poll_frame(Duration::from_secs(1))? {
                return Ok(f);
            }
        }
    }
}

/// One-shot screenshot of the main display (macOS 14+), full resolution.
pub fn screenshot_main_display() -> Result<CFRetained<objc2_core_graphics::CGImage>, CaptureError> {
    screenshot(None)
}

/// One-shot screenshot of a specific window (falling back to main display), macOS 14+.
pub fn screenshot(window_id: Option<u32>) -> Result<CFRetained<objc2_core_graphics::CGImage>, CaptureError> {
    use objc2_core_graphics::CGImage;
    use objc2_screen_capture_kit::SCScreenshotManager;
    let content = fetch_content()?;
    let (filter, width, height) = build_filter_and_size(&content, window_id, 4096, 2)?;
    let conf = unsafe { SCStreamConfiguration::new() };
    unsafe {
        conf.setWidth(width);
        conf.setHeight(height);
        conf.setShowsCursor(true);
    }
    let (tx, rx) = mpsc::channel::<Result<CFRetained<CGImage>, CaptureError>>();
    let block = RcBlock::new(move |img: *mut CGImage, err: *mut NSError| {
        let r = unsafe {
            if let Some(e) = err.as_ref() {
                Err(map_error("screenshot", e))
            } else if let Some(i) = NonNull::new(img) {
                Ok(CFRetained::retain(i))
            } else {
                Err(CaptureError::Sck("empty screenshot".into()))
            }
        };
        let _ = tx.send(r);
    });
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(&filter, &conf, Some(&block))
    };
    rx.recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|_| Err(CaptureError::Sck("timed out taking screenshot".into())))
}

impl Drop for SckCapture {
    fn drop(&mut self) {
        self.stop();
    }
}
