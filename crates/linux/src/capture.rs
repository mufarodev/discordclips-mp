//! PipeWire video + system audio capture.
//!
//! Connects to the PipeWire remote returned by `portal::open_screencast`, negotiates DMA-BUF with
//! GPU DRM format modifiers (falling back automatically to MemFd/MemPtr), and delivers frames
//! with monotonic µs timestamps plus static-screen frame repeats.

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clipsd_core::Capture;
use pipewire::spa::buffer::{ChunkFlags, DataType};
use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pipewire::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pipewire::spa::param::video::{VideoFormat, VideoInfoRaw};
use pipewire::spa::param::{format_utils, ParamType};
use pipewire::spa::pod::deserialize::PodDeserializer;
use pipewire::spa::pod::serialize::PodSerializer;
use pipewire::spa::pod::{self, ChoiceValue, Object, Pod, Property, PropertyFlags, Value};
use pipewire::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Direction, Fraction, Rectangle, SpaTypes};
use pipewire::stream::{StreamFlags, StreamState};

use crate::gpu::{
    self, DmaBufFrame, DmaBufPlane, DRM_FORMAT_ARGB8888, DRM_FORMAT_MOD_INVALID, DRM_FORMAT_XRGB8888,
};
use crate::portal::{self, PortalStream};

const REPEAT_AFTER: Duration = Duration::from_millis(100);
const MIN_FRAME_GAP_US: i64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra,
    Bgrx,
    Nv12,
}

#[derive(Clone)]
pub struct CpuFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
}

pub enum Frame {
    Cpu(CpuFrame),
    DmaBuf(DmaBufFrame),
    RepeatLastGpu,
}

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

enum LastFrame {
    Cpu(CpuFrame),
    Gpu,
}

pub struct PwCapture {
    _portal: PortalStream,
    rx: Receiver<Item>,
    audio_rx: Option<Receiver<PcmChunk>>,
    pub width: usize,
    pub height: usize,
    dmabuf: bool,
    last: Option<(LastFrame, i64, Instant)>,
    last_emitted_us: i64,
    dropped: Arc<AtomicU64>,
    dropped_audio: Arc<AtomicU64>,
    pw_loop_quit: Option<pipewire::channel::Sender<()>>,
    pw_thread: Option<std::thread::JoinHandle<()>>,
}

impl PwCapture {
    /// Open an xdg-desktop-portal ScreenCast session and attach a PipeWire video stream.
    ///
    /// Blocks until the stream has negotiated a format (the real pixel size only arrives
    /// there; the portal's `size()` is the logical size) or fails.
    pub fn start(cfg: &CaptureConfig) -> Result<Self, String> {
        let portal = portal::open_screencast()?;
        let fd = portal.pipewire_fd.try_clone().map_err(|e| format!("dup pipewire fd: {e}"))?;
        let node_id = portal.node_id;
        let _ = cfg.window_id; // the portal picker owns source selection on Linux
        let modifiers = gpu::query_dmabuf_modifiers().unwrap_or_default();

        // Keep DMA-BUF queue shallow so KWin's buffer pool never stalls.
        let (tx, rx) = mpsc::sync_channel::<Item>(if modifiers.is_empty() { 8 } else { 3 });
        let (audio_tx, audio_rx) = if cfg.audio {
            let (atx, arx) = mpsc::sync_channel::<PcmChunk>(256);
            (Some(atx), Some(arx))
        } else {
            (None, None)
        };
        let (quit_tx, quit_rx) = pipewire::channel::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, u32, bool), String>>();
        let dropped: Arc<AtomicU64> = Default::default();
        let dropped_audio: Arc<AtomicU64> = Default::default();

        let args = PwThreadArgs {
            fd,
            node_id,
            fps: cfg.fps.max(1),
            modifiers,
            tx,
            audio_tx,
            ready_tx,
            quit_rx,
            dropped: dropped.clone(),
            dropped_audio: dropped_audio.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("clipsd-pipewire".into())
            .spawn(move || pw_thread(args))
            .map_err(|e| format!("spawn pipewire thread: {e}"))?;

        let (src_w, src_h, dmabuf) = match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let _ = quit_tx.send(());
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => {
                let _ = quit_tx.send(());
                let _ = thread.join();
                return Err(format!("PipeWire node {node_id} never negotiated a video format"));
            }
        };
        // Fit to max_width keeping the aspect ratio; encoders want even dimensions.
        let width = (src_w.min(cfg.max_width as u32).max(2)) & !1;
        let height = (((src_h as u64 * width as u64) / src_w.max(1) as u64) as u32).max(2) & !1;
        eprintln!(
            "clipsd-linux: capture {src_w}x{src_h} -> encode {width}x{height} ({})",
            if dmabuf { "DMA-BUF zero-copy" } else { "MemFd CPU" }
        );

        Ok(Self {
            _portal: portal,
            rx,
            audio_rx,
            width: width as usize,
            height: height as usize,
            dmabuf,
            last: None,
            last_emitted_us: i64::MIN,
            dropped,
            dropped_audio,
            pw_loop_quit: Some(quit_tx),
            pw_thread: Some(thread),
        })
    }

    pub fn is_dmabuf(&self) -> bool {
        self.dmabuf
    }

    pub fn flush_backlog(&mut self) {
        while self.rx.try_recv().is_ok() {}
        // Frames shed while the encoder was initialising are stale by definition, not real drops.
        self.dropped.store(0, Ordering::Relaxed);
        self.last = None;
        self.last_emitted_us = i64::MIN;
    }

    pub fn take_audio(&mut self) -> Option<Receiver<PcmChunk>> {
        self.audio_rx.take()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn dropped_audio(&self) -> u64 {
        self.dropped_audio.load(Ordering::Relaxed)
    }

    /// Wait up to `wait` for a frame, repeating the last frame if the desktop is static.
    pub fn poll_frame(&mut self, wait: Duration) -> Result<Option<(Frame, i64)>, String> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left.min(REPEAT_AFTER)) {
                Ok(item) => {
                    let pts_us = item.pts_us.max(self.last_emitted_us.saturating_add(MIN_FRAME_GAP_US));
                    let marker = match &item.frame {
                        Frame::Cpu(c) => LastFrame::Cpu(c.clone()),
                        Frame::DmaBuf(_) | Frame::RepeatLastGpu => LastFrame::Gpu,
                    };
                    self.last = Some((marker, pts_us, Instant::now()));
                    self.last_emitted_us = pts_us;
                    return Ok(Some((item.frame, pts_us)));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some((marker, pts, at)) = &self.last {
                        let pts_us = (pts + at.elapsed().as_micros() as i64)
                            .max(self.last_emitted_us.saturating_add(MIN_FRAME_GAP_US));
                        self.last_emitted_us = pts_us;
                        let frame = match marker {
                            LastFrame::Cpu(c) => Frame::Cpu(c.clone()),
                            LastFrame::Gpu => Frame::RepeatLastGpu,
                        };
                        return Ok(Some((frame, pts_us)));
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

struct PwThreadArgs {
    fd: std::os::fd::OwnedFd,
    node_id: u32,
    fps: i32,
    modifiers: Vec<u64>,
    tx: SyncSender<Item>,
    audio_tx: Option<SyncSender<PcmChunk>>,
    ready_tx: mpsc::Sender<Result<(u32, u32, bool), String>>,
    quit_rx: pipewire::channel::Receiver<()>,
    dropped: Arc<AtomicU64>,
    dropped_audio: Arc<AtomicU64>,
}

struct VideoUser {
    info: VideoInfoRaw,
    fps: i32,
    use_dmabuf: bool,
    modifier: u64,
    tx: SyncSender<Item>,
    ready_tx: mpsc::Sender<Result<(u32, u32, bool), String>>,
    ready_sent: bool,
    dropped: Arc<AtomicU64>,
    frame_us: i64,
    next_due_us: i64,
    warned_dmabuf: bool,
}

fn serialize_pod(obj: Object) -> Vec<u8> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj))
        .expect("serialize pod")
        .0
        .into_inner()
}

fn extract_pod_modifiers(param: &Pod) -> Vec<u64> {
    let bytes = param.as_bytes();
    let Ok((_, Value::Object(obj))) = PodDeserializer::deserialize_any_from(bytes) else {
        return Vec::new();
    };
    let Some(prop) = obj
        .properties
        .into_iter()
        .find(|p| p.key == pipewire::spa::sys::SPA_FORMAT_VIDEO_modifier)
    else {
        return Vec::new();
    };
    match prop.value {
        Value::Long(m) => vec![m as u64],
        Value::Choice(ChoiceValue::Long(Choice(_, ChoiceEnum::Enum { default, alternatives }))) => {
            let mut out = Vec::with_capacity(alternatives.len() + 1);
            out.push(default as u64);
            for a in alternatives {
                let u = a as u64;
                if !out.contains(&u) {
                    out.push(u);
                }
            }
            out
        }
        Value::Choice(ChoiceValue::Long(Choice(_, ChoiceEnum::None(m)))) => vec![m as u64],
        _ => Vec::new(),
    }
}

fn video_enum_format(
    fps: i32,
    fixed_format: Option<VideoFormat>,
    modifiers: Option<&[u64]>,
    fixate_modifier: Option<u64>,
) -> Vec<u8> {
    let mut obj = pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 16384, height: 16384 }
        ),
        pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: fps as u32, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 1000, denom: 1 }
        ),
    );
    if let Some(vf) = fixed_format {
        obj.properties.push(pod::property!(FormatProperties::VideoFormat, Id, vf));
    } else {
        obj.properties.push(pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA
        ));
    }
    if let Some(m) = fixate_modifier {
        let mut p = Property::new(pipewire::spa::sys::SPA_FORMAT_VIDEO_modifier, Value::Long(m as i64));
        p.flags = PropertyFlags::from_bits_retain(pipewire::spa::sys::SPA_POD_PROP_FLAG_MANDATORY);
        obj.properties.push(p);
    } else if let Some(mods) = modifiers.filter(|m| !m.is_empty()) {
        let alts: Vec<i64> = mods.iter().map(|&m| m as i64).collect();
        let val = Value::Choice(ChoiceValue::Long(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Enum { default: alts[0], alternatives: alts },
        )));
        let mut p = Property::new(pipewire::spa::sys::SPA_FORMAT_VIDEO_modifier, val);
        p.flags = PropertyFlags::from_bits_retain(
            pipewire::spa::sys::SPA_POD_PROP_FLAG_MANDATORY
                | pipewire::spa::sys::SPA_POD_PROP_FLAG_DONT_FIXATE,
        );
        obj.properties.push(p);
    }
    serialize_pod(obj)
}

fn buffers_param(allow_dmabuf: bool) -> Vec<u8> {
    let mut mask = (1i32 << DataType::MemFd.as_raw()) | (1i32 << DataType::MemPtr.as_raw());
    if allow_dmabuf {
        mask |= 1i32 << DataType::DmaBuf.as_raw();
    }
    serialize_pod(Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![Property::new(pipewire::spa::sys::SPA_PARAM_BUFFERS_dataType, Value::Int(mask))],
    })
}

fn pw_thread(a: PwThreadArgs) {
    let PwThreadArgs { fd, node_id, fps, modifiers, tx, audio_tx, ready_tx, quit_rx, dropped, dropped_audio } = a;
    let fail = |ready_tx: &mpsc::Sender<Result<(u32, u32, bool), String>>, msg: String| {
        let _ = ready_tx.send(Err(msg));
    };
    pipewire::init();
    let mainloop = match pipewire::main_loop::MainLoopRc::new(None) {
        Ok(m) => m,
        Err(e) => return fail(&ready_tx, format!("pipewire main loop: {e}")),
    };
    let context = match pipewire::context::ContextRc::new(&mainloop, None) {
        Ok(c) => c,
        Err(e) => return fail(&ready_tx, format!("pipewire context: {e}")),
    };
    let core = match context.connect_fd_rc(fd, None) {
        Ok(c) => c,
        Err(e) => return fail(&ready_tx, format!("pipewire connect (portal fd): {e}")),
    };

    let _quit = quit_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });

    let stream = match pipewire::stream::StreamBox::new(
        &core,
        "clipsd-video",
        pipewire::properties::properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
    ) {
        Ok(s) => s,
        Err(e) => return fail(&ready_tx, format!("pipewire stream: {e}")),
    };

    let frame_us = 1_000_000 / fps as i64;
    let user = VideoUser {
        info: Default::default(),
        fps,
        use_dmabuf: false,
        modifier: DRM_FORMAT_MOD_INVALID,
        tx,
        ready_tx: ready_tx.clone(),
        ready_sent: false,
        dropped,
        frame_us,
        next_due_us: 0,
        warned_dmabuf: false,
    };
    let _listener = stream
        .add_local_listener_with_user_data(user)
        .state_changed(|_, u, _old, new| {
            if let StreamState::Error(msg) = new {
                let _ = u.ready_tx.send(Err(format!("PipeWire stream error: {msg}")));
            }
        })
        .param_changed(|stream, u, id, param| {
            let Some(param) = param else { return };
            if id != ParamType::Format.as_raw() {
                return;
            }
            let Ok((mt, st)) = format_utils::parse_format(param) else { return };
            if mt != MediaType::Video || st != MediaSubtype::Raw {
                return;
            }
            if u.info.parse(param).is_err() {
                return;
            }
            let raw_flags = u.info.as_raw().flags;
            let has_mod = (raw_flags & pipewire::spa::sys::SPA_VIDEO_FLAG_MODIFIER) != 0;
            let need_fixate =
                (raw_flags & pipewire::spa::sys::SPA_VIDEO_FLAG_MODIFIER_FIXATION_REQUIRED) != 0;
            let chosen_mod = u.info.modifier();
            if has_mod && need_fixate {
                let candidates = extract_pod_modifiers(param);
                if let Some(&picked) = candidates.first() {
                    eprintln!(
                        "clipsd-linux: fixating DMA-BUF modifier 0x{picked:x} (from {} candidates)",
                        candidates.len()
                    );
                    let fixed_fmt = video_enum_format(u.fps, Some(u.info.format()), None, Some(picked));
                    let fallback_fmt = video_enum_format(u.fps, None, None, None);
                    if let (Some(p0), Some(p1)) = (Pod::from_bytes(&fixed_fmt), Pod::from_bytes(&fallback_fmt)) {
                        let _ = stream.update_params(&mut [p0, p1]);
                    }
                    return;
                }
            }
            let size = u.info.size();
            u.use_dmabuf = has_mod;
            u.modifier = if has_mod { chosen_mod } else { DRM_FORMAT_MOD_INVALID };
            eprintln!(
                "clipsd-linux: negotiated {:?} {}x{} @ {}/{} (mod={})",
                u.info.format(),
                size.width,
                size.height,
                u.info.framerate().num,
                u.info.framerate().denom,
                if has_mod { format!("0x{chosen_mod:x}") } else { "none/shm".into() }
            );
            let buffers = buffers_param(u.use_dmabuf);
            if let Some(pod) = Pod::from_bytes(&buffers) {
                let _ = stream.update_params(&mut [pod]);
            }
            if !u.ready_sent {
                u.ready_sent = true;
                let _ = u.ready_tx.send(Ok((size.width, size.height, u.use_dmabuf)));
            }
        })
        .process(|stream, u| {
            // Always drain to the newest buffer; older ones are requeued on drop.
            let mut latest = None;
            while let Some(b) = stream.dequeue_buffer() {
                latest = Some(b);
            }
            let Some(mut buf) = latest else { return };

            let now = monotonic_us();
            // Pace to the requested fps on a fixed grid (compositors deliver at refresh rate).
            let tol = u.frame_us / 8;
            if now + tol < u.next_due_us {
                return;
            }
            u.next_due_us = if u.next_due_us == 0 || now > u.next_due_us + 2 * u.frame_us {
                now + u.frame_us
            } else {
                u.next_due_us + u.frame_us
            };

            let size = u.info.size();
            let (format, fourcc) = match u.info.format() {
                VideoFormat::BGRx => (PixelFormat::Bgrx, DRM_FORMAT_XRGB8888),
                VideoFormat::BGRA => (PixelFormat::Bgra, DRM_FORMAT_ARGB8888),
                _ => return,
            };
            let datas = buf.datas_mut();
            let Some(first) = datas.first() else { return };
            let ty = first.type_();

            let frame = if ty == DataType::DmaBuf {
                let mut planes = Vec::with_capacity(datas.len());
                for d in datas.iter() {
                    let c = d.chunk();
                    if c.flags().contains(ChunkFlags::CORRUPTED) {
                        return;
                    }
                    let dup_fd = unsafe { libc::dup(d.fd()) };
                    if dup_fd < 0 {
                        return;
                    }
                    let stride = if c.stride() > 0 { c.stride() as u32 } else { size.width * 4 };
                    planes.push(DmaBufPlane {
                        fd: unsafe { OwnedFd::from_raw_fd(dup_fd) },
                        offset: c.offset(),
                        stride,
                    });
                }
                if planes.is_empty() {
                    return;
                }
                Frame::DmaBuf(DmaBufFrame {
                    width: size.width,
                    height: size.height,
                    fourcc,
                    modifier: u.modifier,
                    planes,
                })
            } else if ty == DataType::MemFd || ty == DataType::MemPtr {
                let d = &mut datas[0];
                let (off, csize, cstride, corrupted) = {
                    let c = d.chunk();
                    (c.offset() as usize, c.size() as usize, c.stride(), c.flags().contains(ChunkFlags::CORRUPTED))
                };
                if csize == 0 || corrupted {
                    return;
                }
                let stride = if cstride > 0 { cstride as usize } else { size.width as usize * 4 };
                let Some(bytes) = d.data() else { return };
                let need = stride * size.height as usize;
                if off + need > bytes.len() || need == 0 {
                    return;
                }
                Frame::Cpu(CpuFrame {
                    width: size.width,
                    height: size.height,
                    stride: stride as u32,
                    format,
                    data: bytes[off..off + need].to_vec(),
                })
            } else {
                if !u.warned_dmabuf {
                    u.warned_dmabuf = true;
                    eprintln!("clipsd-linux: compositor sent unsupported {ty:?} buffers");
                }
                return;
            };
            if u.tx.try_send(Item { frame, pts_us: now }).is_err() {
                u.dropped.fetch_add(1, Ordering::Relaxed);
            }
        })
        .register();
    let _listener = match _listener {
        Ok(l) => l,
        Err(e) => return fail(&ready_tx, format!("pipewire listener: {e}")),
    };

    let shm_fmt = video_enum_format(fps, None, None, None);
    let dma_fmt = if !modifiers.is_empty() {
        Some(video_enum_format(fps, None, Some(&modifiers), None))
    } else {
        None
    };
    let mut params: Vec<&Pod> = Vec::with_capacity(2);
    if let Some(ref df) = dma_fmt {
        params.push(Pod::from_bytes(df).expect("dmabuf format pod"));
    }
    params.push(Pod::from_bytes(&shm_fmt).expect("shm format pod"));
    if let Err(e) = stream.connect(
        Direction::Input,
        Some(node_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut params,
    ) {
        return fail(&ready_tx, format!("pipewire stream connect: {e}"));
    }

    // System audio: a second connection to the default PipeWire daemon (the portal fd only
    // exposes the screencast node). Failure disables audio but never video.
    let audio = audio_tx.and_then(|atx| match setup_audio(&context, atx, dropped_audio) {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("clipsd-linux: audio capture disabled: {e}");
            None
        }
    });

    mainloop.run();
    let _ = stream.disconnect();
    if let Some(a) = audio {
        let _ = a.stream.disconnect();
    }
}

struct AudioUser {
    info: AudioInfoRaw,
    tx: SyncSender<PcmChunk>,
    dropped: Arc<AtomicU64>,
}

struct AudioHandles {
    _core: pipewire::core::CoreRc,
    stream: pipewire::stream::StreamRc,
    _listener: pipewire::stream::StreamListener<AudioUser>,
}

/// Standalone PipeWire audio monitor handle for capture backends that don't use PipeWire video
/// (such as the direct Vulkan DMA-BUF layer).
pub struct AudioOnlyHandle {
    quit: Option<pipewire::channel::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AudioOnlyHandle {
    fn drop(&mut self) {
        if let Some(q) = self.quit.take() {
            let _ = q.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn start_audio_only() -> Result<(Receiver<PcmChunk>, Arc<AtomicU64>, AudioOnlyHandle), String> {
    let (tx, rx) = mpsc::sync_channel::<PcmChunk>(256);
    let dropped: Arc<AtomicU64> = Default::default();
    let (quit_tx, quit_rx) = pipewire::channel::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

    let dropped_bg = dropped.clone();
    let thread = std::thread::Builder::new()
        .name("clipsd-pw-audio".into())
        .spawn(move || {
            pipewire::init();
            let mainloop = match pipewire::main_loop::MainLoopRc::new(None) {
                Ok(m) => m,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("pipewire main loop: {e}")));
                    return;
                }
            };
            let context = match pipewire::context::ContextRc::new(&mainloop, None) {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("pipewire context: {e}")));
                    return;
                }
            };
            let _quit = quit_rx.attach(mainloop.loop_(), {
                let mainloop = mainloop.clone();
                move |_| mainloop.quit()
            });
            let audio = match setup_audio(&context, tx, dropped_bg) {
                Ok(h) => {
                    let _ = ready_tx.send(Ok(()));
                    h
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            mainloop.run();
            let _ = audio.stream.disconnect();
        })
        .map_err(|e| format!("spawn audio thread: {e}"))?;

    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok((rx, dropped, AudioOnlyHandle { quit: Some(quit_tx), thread: Some(thread) })),
        Ok(Err(e)) => {
            let _ = quit_tx.send(());
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = quit_tx.send(());
            let _ = thread.join();
            Err("timed out initializing PipeWire audio".into())
        }
    }
}

const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u32 = 2;

fn setup_audio(
    context: &pipewire::context::ContextRc,
    tx: SyncSender<PcmChunk>,
    dropped: Arc<AtomicU64>,
) -> Result<AudioHandles, String> {
    let core = context.connect_rc(None).map_err(|e| format!("connect: {e}"))?;
    let mut props = pipewire::properties::properties! {
        *pipewire::keys::MEDIA_TYPE => "Audio",
        *pipewire::keys::MEDIA_CATEGORY => "Capture",
        *pipewire::keys::MEDIA_ROLE => "Music",
        *pipewire::keys::NODE_NAME => "clipsd-audio",
    };
    // Capture what is *played* (the default sink's monitor), not the microphone.
    props.insert(*pipewire::keys::STREAM_CAPTURE_SINK, "true");
    let stream = pipewire::stream::StreamRc::new(core.clone(), "clipsd-audio", props)
        .map_err(|e| format!("stream: {e}"))?;

    let user = AudioUser { info: Default::default(), tx, dropped };
    let listener = stream
        .add_local_listener_with_user_data(user)
        .param_changed(|_, u, id, param| {
            let Some(param) = param else { return };
            if id != ParamType::Format.as_raw() {
                return;
            }
            let Ok((mt, st)) = format_utils::parse_format(param) else { return };
            if mt != MediaType::Audio || st != MediaSubtype::Raw {
                return;
            }
            if u.info.parse(param).is_ok() {
                eprintln!("clipsd-linux: audio {} Hz x{}", u.info.rate(), u.info.channels());
            }
        })
        .process(|stream, u| {
            let Some(mut buf) = stream.dequeue_buffer() else { return };
            let Some(d) = buf.datas_mut().first_mut() else { return };
            let (off, size) = (d.chunk().offset() as usize, d.chunk().size() as usize);
            let (rate, ch) = (u.info.rate(), u.info.channels() as usize);
            if size == 0 || rate == 0 || ch == 0 {
                return;
            }
            let Some(bytes) = d.data() else { return };
            if off + size > bytes.len() {
                return;
            }
            let samples: Vec<f32> = bytes[off..off + size]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            // The callback fires when the chunk has just been captured: stamp its *start*.
            let dur_us = (samples.len() / ch) as i64 * 1_000_000 / rate as i64;
            let chunk = PcmChunk {
                pts_us: monotonic_us() - dur_us,
                sample_rate: rate,
                channels: ch as u8,
                samples,
            };
            if u.tx.try_send(chunk).is_err() {
                u.dropped.fetch_add(1, Ordering::Relaxed);
            }
        })
        .register()
        .map_err(|e| format!("listener: {e}"))?;

    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32LE);
    info.set_rate(AUDIO_RATE);
    info.set_channels(AUDIO_CHANNELS);
    let mut pos = [0u32; pipewire::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    pos[0] = pipewire::spa::sys::SPA_AUDIO_CHANNEL_FL;
    pos[1] = pipewire::spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(pos);
    let bytes = serialize_pod(Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    });
    let mut params = [Pod::from_bytes(&bytes).ok_or("audio format pod")?];
    stream
        .connect(Direction::Input, None, StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS, &mut params)
        .map_err(|e| format!("connect stream: {e}"))?;
    Ok(AudioHandles { _core: core, stream, _listener: listener })
}
