//! M6 & M8: PipeWire video + system/app audio capture.
//!
//! Connects to the PipeWire remote returned by `portal::open_screencast`, negotiates video
//! format (BGRA/BGRx/NV12, DMA-BUF or MemFd/MemPtr), and delivers frames with monotonic µs
//! timestamps plus static-screen frame repeats (matching `crates/macos/src/capture.rs`).
//!
//! NOTE: Written against `pipewire 0.8` APIs from macOS — verify SPA pod negotiation and
//! DMA-BUF import on a live Linux PipeWire session (see LINUX_HANDOFF.md).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use clipsd_core::Capture;

use crate::portal::{self, PortalStream};

const REPEAT_AFTER: Duration = Duration::from_millis(100);
const MIN_FRAME_GAP_US: i64 = 5_000;

/// Pixel format of a captured CPU-accessible frame (DMA-BUF zero-copy path will add a `DmaBuf`
/// variant once wired into VAAPI `AVHWFramesContext` in M7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra,
    Bgrx,
    Nv12,
}

/// One captured Linux video frame.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

/// A chunk of captured PipeWire audio: interleaved f32 PCM at `pts_us` (CLOCK_MONOTONIC).
pub struct PcmChunk {
    pub pts_us: i64,
    pub sample_rate: u32,
    pub channels: u8,
    pub samples: Vec<f32>,
}

pub struct CaptureConfig {
    pub max_width: usize,
    pub fps: i32,
    pub audio: bool,
    pub window_id: Option<u32>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { max_width: 1920, fps: 30, audio: true, window_id: None }
    }
}

pub fn monotonic_us() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as i64 * 1_000_000 + ts.tv_nsec as i64 / 1_000
}

struct Item {
    frame: Frame,
    pts_us: i64,
}

pub struct PwCapture {
    _portal: PortalStream,
    rx: Receiver<Item>,
    audio_rx: Option<Receiver<PcmChunk>>,
    pub width: usize,
    pub height: usize,
    last: Option<(Frame, i64, Instant)>,
    last_emitted_us: i64,
    dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
    dropped_audio: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pw_loop_quit: Option<pipewire::channel::Sender<()>>,
    pw_thread: Option<std::thread::JoinHandle<()>>,
}

impl PwCapture {
    /// Open an xdg-desktop-portal ScreenCast session and attach a PipeWire video stream.
    ///
    /// On first run on Linux, complete the SPA `EnumFormat` pod and `process` callback below
    /// (see LINUX_HANDOFF.md M6).
    pub fn start(cfg: &CaptureConfig) -> Result<Self, String> {
        let portal = portal::open_screencast()?;
        let width = portal.width.unwrap_or(1920).min(cfg.max_width as u32) as usize & !1;
        let height = portal.height.unwrap_or(1080) as usize & !1;

        let (_tx, rx) = mpsc::sync_channel::<Item>(2);
        let audio_rx = if cfg.audio {
            let (_atx, arx) = mpsc::sync_channel::<PcmChunk>(256);
            Some(arx)
        } else {
            None
        };
        let (quit_tx, _quit_rx) = pipewire::channel::channel::<()>();

        // M6 TODO (on Linux): spawn PipeWire MainLoop thread using `portal.pipewire_fd` +
        // `portal.node_id`, negotiate SPA_TYPE_OBJECT_Format (BGRx/BGRA/NV12, DMA-BUF + MemFd),
        // push `Item { frame, pts_us: monotonic_us() }` into `_tx` (try_send, increment `dropped`
        // when full), and in M8 attach a PipeWire sink-monitor audio stream pushing 48 kHz stereo
        // f32 `PcmChunk`s into `_atx`.
        let _ = cfg.window_id;

        Err(format!(
            "PipeWire stream wiring (M6) not yet completed on Linux (portal opened node_id={}, {}x{})",
            portal.node_id, width, height
        ))?;

        Ok(Self {
            _portal: portal,
            rx,
            audio_rx,
            width,
            height,
            last: None,
            last_emitted_us: i64::MIN,
            dropped: Default::default(),
            dropped_audio: Default::default(),
            pw_loop_quit: Some(quit_tx),
            pw_thread: None,
        })
    }

    pub fn flush_backlog(&mut self) {
        while self.rx.try_recv().is_ok() {}
        self.last = None;
        self.last_emitted_us = i64::MIN;
    }

    pub fn take_audio(&mut self) -> Option<Receiver<PcmChunk>> {
        self.audio_rx.take()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn dropped_audio(&self) -> u64 {
        self.dropped_audio.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Wait up to `wait` for a frame, repeating the last frame if the desktop is static.
    pub fn poll_frame(&mut self, wait: Duration) -> Result<Option<(Frame, i64)>, String> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left.min(REPEAT_AFTER)) {
                Ok(item) => {
                    let pts_us = item.pts_us.max(self.last_emitted_us.saturating_add(MIN_FRAME_GAP_US));
                    self.last = Some((item.frame.clone(), pts_us, Instant::now()));
                    self.last_emitted_us = pts_us;
                    return Ok(Some((item.frame, pts_us)));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some((frame, pts, at)) = &self.last {
                        let pts_us = (pts + at.elapsed().as_micros() as i64)
                            .max(self.last_emitted_us.saturating_add(MIN_FRAME_GAP_US));
                        self.last_emitted_us = pts_us;
                        return Ok(Some((frame.clone(), pts_us)));
                    }
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Err("PipeWire capture stream closed".into()),
            }
        }
    }
}

impl Capture for PwCapture {
    type Frame = Frame;

    fn next_frame(&mut self) -> Result<(Frame, i64), String> {
        loop {
            if let Some(f) = self.poll_frame(Duration::from_secs(1))? {
                return Ok(f);
            }
        }
    }
}

impl Drop for PwCapture {
    fn drop(&mut self) {
        if let Some(q) = self.pw_loop_quit.take() {
            let _ = q.send(());
        }
        if let Some(t) = self.pw_thread.take() {
            let _ = t.join();
        }
    }
}
