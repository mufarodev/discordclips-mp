//! Linux clips worker: implements `clipsd_core::ipc::Service` on top of
//! xdg-desktop-portal + PipeWire + libavcodec, mirroring `crates/macos/src/recorder.rs`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use clipsd_core::ipc::Service;
use clipsd_core::metadata::with_defaults;
use clipsd_core::proto::{Command, Event, Stats};
use clipsd_core::{mux, AudioConfig, AudioRing, CodecConfig, Encoder, Ring};

use crate::capture::{CaptureConfig, Frame, PcmChunk, PwCapture};
use crate::encode::{EncoderConfig, FfmpegAacEncoder, FfmpegVideoEncoder};
use crate::thumbnail;
use crate::vk_capture::VkCapture;

const AUDIO_BITRATE_BPS: u32 = 128_000;
const THUMB_QUALITY: u8 = 80;

struct State {
    ring: Ring,
    audio: AudioRing,
    config: Option<CodecConfig>,
    audio_cfg: Option<AudioConfig>,
    frames: u64,
    dropped: u64,
    audio_packets: u64,
    audio_dropped: u64,
}

enum Ctl {
    SetBitrate(i32),
}

struct Session {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    ctl: Sender<Ctl>,
    threads: Vec<JoinHandle<()>>,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
    window_ms: u32,
}

impl Session {
    fn shutdown(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

#[derive(Default)]
pub struct Recorder {
    session: Mutex<Option<Session>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self::default()
    }

    fn start(
        &self,
        fps: u32,
        duration_ms: u32,
        bitrate_kbps: u32,
        startup_delay_ms: u32,
        width: Option<u32>,
        window_id: Option<u32>,
        pid: Option<u32>,
    ) -> Event {
        let mut slot = self.session.lock().unwrap();
        if slot.is_some() {
            return Event::Error { message: "already recording".into() };
        }
        if startup_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(startup_delay_ms as u64));
        }
        // Also decode synthetic Linux window handles (`0x70000000 + pid`) if explicit `pid` is unset.
        let target_pid = pid.or_else(|| {
            window_id.and_then(|w| ((w & 0xf000_0000) == 0x7000_0000).then_some(w & 0x0fff_ffff))
        });
        match Self::spawn(fps.clamp(1, 120), duration_ms, bitrate_kbps.max(100), width, window_id, target_pid) {
            Ok(s) => {
                *slot = Some(s);
                Event::RecordingEvent { state: "started".into() }
            }
            Err(message) => Event::InitFailure { message },
        }
    }

    fn spawn(
        fps: u32,
        duration_ms: u32,
        bitrate_kbps: u32,
        max_width: Option<u32>,
        window_id: Option<u32>,
        pid: Option<u32>,
    ) -> Result<Session, String> {
        let window_secs = duration_ms.div_ceil(1000).max(1);
        let state = Arc::new(Mutex::new(State {
            ring: Ring::new(window_secs),
            audio: AudioRing::new(window_secs + 5),
            config: None,
            audio_cfg: None,
            frames: 0,
            dropped: 0,
            audio_packets: 0,
            audio_dropped: 0,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (ctl_tx, ctl_rx) = mpsc::channel::<Ctl>();
        type Init = Result<(u32, u32, Option<Receiver<PcmChunk>>), String>;
        let (init_tx, init_rx) = mpsc::channel::<Init>();

        let video = {
            let (state, stop) = (state.clone(), stop.clone());
            std::thread::Builder::new()
                .name("clipsd-video".into())
                .spawn(move || video_thread(fps, bitrate_kbps, max_width, window_id, pid, state, stop, ctl_rx, init_tx))
                .map_err(|e| e.to_string())?
        };
        let (width, height, audio_rx) = match init_rx.recv_timeout(Duration::from_secs(180)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let _ = video.join();
                return Err(e);
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                return Err("timed out starting capture (is a portal dialog pending?)".into());
            }
        };
        let mut threads = vec![video];
        if let Some(rx) = audio_rx {
            let (state, stop) = (state.clone(), stop.clone());
            threads.push(
                std::thread::Builder::new()
                    .name("clipsd-audio".into())
                    .spawn(move || audio_thread(rx, state, stop))
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(Session { state, stop, ctl: ctl_tx, threads, width, height, fps, bitrate_kbps, window_ms: window_secs * 1000 })
    }

    fn stop(&self) -> Event {
        match self.session.lock().unwrap().take() {
            Some(s) => {
                s.shutdown();
                Event::RecordingEvent { state: "stopped".into() }
            }
            None => Event::Error { message: "not recording".into() },
        }
    }

    fn stats(&self) -> Stats {
        let g = self.session.lock().unwrap();
        let Some(s) = g.as_ref() else { return Stats::default() };
        let st = s.state.lock().unwrap();
        Stats {
            recording: true,
            width: s.width,
            height: s.height,
            fps: s.fps,
            bitrate_kbps: s.bitrate_kbps,
            window_ms: s.window_ms,
            buffered_ms: (st.ring.buffered_us() / 1000) as u64,
            buffered_bytes: st.ring.bytes() as u64,
            frames_captured: st.frames,
            frames_dropped: st.dropped,
            audio_packets: st.audio_packets,
            audio_chunks_dropped: st.audio_dropped,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn save(
        &self,
        filepath: &str,
        metadata: Option<serde_json::Value>,
        trim_start_ms: u32,
        trim_end_ms: u32,
        thumbnail_ms: Option<u32>,
        last_ms: Option<u32>,
        embed_thumbnail: bool,
    ) -> Event {
        let fail = |error: String| Event::ClipSaveFailed { filepath: filepath.to_string(), error };

        let (pkts, cfg, audio) = {
            let g = self.session.lock().unwrap();
            let Some(s) = g.as_ref() else { return fail("not recording".into()) };
            let st = s.state.lock().unwrap();
            let buffered = st.ring.buffered_us();
            let mut keep_us = buffered - trim_start_ms as i64 * 1000;
            if let Some(l) = last_ms {
                keep_us = keep_us.min(l as i64 * 1000);
            }
            if keep_us <= 0 {
                return fail("nothing left after trimming".into());
            }
            let mut pkts = st.ring.snapshot_us(keep_us);
            if let Some(newest) = pkts.last().map(|p| p.pts_us) {
                let end = newest - trim_end_ms as i64 * 1000;
                pkts.retain(|p| p.pts_us <= end);
            }
            let audio = match (&st.audio_cfg, pkts.first(), pkts.last()) {
                (Some(ac), Some(f), Some(l)) => Some((ac.clone(), st.audio.range(f.pts_us, l.pts_us))),
                _ => None,
            };
            (pkts, st.config.clone(), audio)
        };
        let Some(cfg) = cfg else { return fail("no video buffered yet".into()) };
        if pkts.is_empty() {
            return fail("nothing buffered yet".into());
        }

        let path = Path::new(filepath);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp: PathBuf = path.with_extension("mp4.part");
        let file = match std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&tmp) {
            Ok(f) => f,
            Err(e) => return fail(format!("{}: {e}", tmp.display())),
        };
        let audio_ref = audio.as_ref().filter(|(_, a)| !a.is_empty()).map(|(c, a)| (c, a.as_slice()));
        let audio_packets = audio_ref.map_or(0, |(_, a)| a.len());
        let span_us = pkts.last().unwrap().pts_us - pkts[0].pts_us;
        let duration_ms = (span_us / 1000) as u64;

        let mut meta_val = with_defaults(metadata);
        if embed_thumbnail {
            if let serde_json::Value::Object(ref mut o) = meta_val {
                o.insert("length".into(), serde_json::json!(duration_ms));
            }
        }
        if let Err(e) = mux::write_mp4_av(file, &cfg, &pkts, audio_ref, Some(meta_val.to_string().as_bytes())) {
            let _ = std::fs::remove_file(&tmp);
            return fail(e);
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return fail(format!("{}: {e}", path.display()));
        }

        let thumbnail = if embed_thumbnail {
            match thumbnail::thumbnail_bytes_from_video(path, thumbnail_ms.unwrap_or(0), THUMB_QUALITY) {
                Ok(jpeg) => {
                    let data_url = format!("data:image/jpeg;base64,{}", thumbnail::base64_encode(&jpeg));
                    if let serde_json::Value::Object(ref mut o) = meta_val {
                        o.insert("thumbnail".into(), serde_json::Value::String(data_url.clone()));
                    }
                    let _ = mux::update_discord_metadata(path, meta_val.to_string().as_bytes());
                    Some(data_url)
                }
                Err(_) => None,
            }
        } else {
            let thumb_path = path.with_extension("jpg");
            match thumbnail::thumbnail_from_video(path, thumbnail_ms.unwrap_or(0), &thumb_path, THUMB_QUALITY) {
                Ok(()) => Some(thumb_path.to_string_lossy().into_owned()),
                Err(_) => None,
            }
        };

        let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let stats_json = serde_json::json!({
            "video_packets": pkts.len(),
            "audio_packets": audio_packets,
            "width": cfg.width,
            "height": cfg.height,
            "bytes": file_bytes,
            "clipSizeBytes": file_bytes,
        })
        .to_string();

        Event::ClipSaved {
            filepath: filepath.to_string(),
            duration_ms,
            stats_json,
            thumbnail,
            metadata: Some(meta_val),
        }
    }
}

impl Service for Recorder {
    fn handle(&self, cmd: Command) -> Event {
        match cmd {
            Command::StartRecording { fps, duration_ms, bitrate_kbps, startup_delay_ms, width, window_id, pid } => {
                self.start(fps, duration_ms, bitrate_kbps, startup_delay_ms, width, window_id, pid)
            }
            Command::StopRecording => self.stop(),
            Command::SetDuration { duration_ms } => match self.session.lock().unwrap().as_mut() {
                Some(s) => {
                    let secs = duration_ms.div_ceil(1000).max(1);
                    let mut st = s.state.lock().unwrap();
                    st.ring.set_window(secs);
                    st.audio.set_window(secs + 5);
                    s.window_ms = secs * 1000;
                    Event::Ack
                }
                None => Event::Error { message: "not recording".into() },
            },
            Command::SetBitrate { bitrate_kbps } => match self.session.lock().unwrap().as_mut() {
                Some(s) => {
                    let _ = s.ctl.send(Ctl::SetBitrate(bitrate_kbps.max(100) as i32 * 1000));
                    s.bitrate_kbps = bitrate_kbps.max(100);
                    Event::Ack
                }
                None => Event::Error { message: "not recording".into() },
            },
            Command::SaveClip { filepath, metadata, trim_start_ms, trim_end_ms, thumbnail_ms, last_ms, embed_thumbnail, .. } => {
                self.save(&filepath, metadata, trim_start_ms, trim_end_ms, thumbnail_ms, last_ms, embed_thumbnail)
            }
            Command::UpdateClipMetadata { filepath, metadata } => {
                match mux::update_discord_metadata(Path::new(&filepath), metadata.to_string().as_bytes()) {
                    Ok(()) => Event::Ack,
                    Err(message) => Event::Error { message },
                }
            }
            Command::TakeScreenshot { filepath, quality, metadata, .. } => {
                let path = Path::new(&filepath);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let comment = with_defaults(metadata).to_string();
                let q = quality.clamp(1, 100) as u8;
                match crate::portal::portal_screenshot_path()
                    .and_then(|png| {
                        let r = thumbnail::jpeg_from_image_file(&png, q, &comment);
                        let _ = std::fs::remove_file(&png);
                        r
                    })
                    .and_then(|bytes| std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display())))
                {
                    Ok(()) => Event::ScreenshotComplete { filepath },
                    Err(message) => Event::Error { message },
                }
            }
            Command::GetStats => Event::Stats(self.stats()),
            Command::Shutdown => {
                let _ = self.stop();
                Event::Ack
            }
        }
    }
}

enum CaptureBackend {
    Vulkan(VkCapture),
    PipeWire(PwCapture),
}

impl CaptureBackend {
    fn width(&self) -> usize {
        match self {
            Self::Vulkan(v) => v.width,
            Self::PipeWire(p) => p.width,
        }
    }
    fn height(&self) -> usize {
        match self {
            Self::Vulkan(v) => v.height,
            Self::PipeWire(p) => p.height,
        }
    }
    fn is_dmabuf(&self) -> bool {
        match self {
            Self::Vulkan(_) => true,
            Self::PipeWire(p) => p.is_dmabuf(),
        }
    }
    fn flush_backlog(&mut self) {
        match self {
            Self::Vulkan(v) => v.flush_backlog(),
            Self::PipeWire(p) => p.flush_backlog(),
        }
    }
    fn take_audio(&mut self) -> Option<Receiver<PcmChunk>> {
        match self {
            Self::Vulkan(v) => v.take_audio(),
            Self::PipeWire(p) => p.take_audio(),
        }
    }
    fn poll_frame(&mut self, wait: Duration) -> Result<Option<(Frame, i64)>, String> {
        match self {
            Self::Vulkan(v) => v.poll_frame(wait),
            Self::PipeWire(p) => p.poll_frame(wait),
        }
    }
    fn dropped(&self) -> u64 {
        match self {
            Self::Vulkan(v) => v.dropped(),
            Self::PipeWire(p) => p.dropped(),
        }
    }
    fn dropped_audio(&self) -> u64 {
        match self {
            Self::Vulkan(v) => v.dropped_audio(),
            Self::PipeWire(p) => p.dropped_audio(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn video_thread(
    fps: u32,
    bitrate_kbps: u32,
    max_width: Option<u32>,
    window_id: Option<u32>,
    pid: Option<u32>,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    ctl: Receiver<Ctl>,
    init: Sender<Result<(u32, u32, Option<Receiver<PcmChunk>>), String>>,
) {
    let ccfg = CaptureConfig {
        fps: fps as i32,
        max_width: max_width.map(|w| w.clamp(320, 3840) as usize).unwrap_or(1920),
        window_id,
        ..CaptureConfig::default()
    };
    let mut cap = match VkCapture::try_start(&ccfg, pid) {
        Some(Ok(vk)) => CaptureBackend::Vulkan(vk),
        Some(Err(e)) => {
            eprintln!("clipsd-linux: Vulkan layer attach failed ({e}); falling back to portal PipeWire");
            match PwCapture::start(&ccfg) {
                Ok(c) => CaptureBackend::PipeWire(c),
                Err(pe) => {
                    let _ = init.send(Err(pe));
                    return;
                }
            }
        }
        None => match PwCapture::start(&ccfg) {
            Ok(c) => CaptureBackend::PipeWire(c),
            Err(e) => {
                let _ = init.send(Err(e));
                return;
            }
        },
    };
    let mut ecfg = EncoderConfig::new(cap.width(), cap.height());
    ecfg.fps = fps as i32;
    ecfg.bitrate_bps = bitrate_kbps as i32 * 1000;
    ecfg.prefer_cuda = cap.is_dmabuf();
    let mut enc = match FfmpegVideoEncoder::new(&ecfg) {
        Ok(e) => e,
        Err(e) => {
            let _ = init.send(Err(e));
            return;
        }
    };
    cap.flush_backlog();
    let _ = init.send(Ok((cap.width() as u32, cap.height() as u32, cap.take_audio())));

    while !stop.load(Ordering::Relaxed) {
        while let Ok(Ctl::SetBitrate(bps)) = ctl.try_recv() {
            if let Err(e) = enc.set_bitrate(bps) {
                eprintln!("clipsd-linux: SetBitrate: {e}");
            }
        }
        let step = match cap.poll_frame(Duration::from_millis(100)) {
            Ok(Some((f, pts))) => enc.encode(f, pts).map(Some),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        };
        match step {
            Ok(Some(pkts)) => {
                let mut st = state.lock().unwrap();
                if st.config.is_none() {
                    st.config = enc.codec_config();
                }
                st.frames += 1;
                st.dropped = cap.dropped() + enc.dropped();
                st.audio_dropped = cap.dropped_audio();
                for p in pkts {
                    st.ring.push(p);
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("clipsd-linux: capture/encode error: {e}");
                break;
            }
        }
    }
}

fn audio_thread(rx: Receiver<PcmChunk>, state: Arc<Mutex<State>>, stop: Arc<AtomicBool>) {
    let mut enc: Option<FfmpegAacEncoder> = None;
    while !stop.load(Ordering::Relaxed) {
        let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) else { continue };
        if enc.is_none() {
            match FfmpegAacEncoder::new(chunk.sample_rate, chunk.channels, AUDIO_BITRATE_BPS) {
                Ok(e) => {
                    state.lock().unwrap().audio_cfg = Some(e.cfg.clone());
                    enc = Some(e);
                }
                Err(e) => {
                    eprintln!("clipsd-linux: audio disabled: {e}");
                    return;
                }
            }
        }
        match enc.as_mut().unwrap().push(chunk.pts_us, &chunk.samples) {
            Ok(pkts) => {
                let mut st = state.lock().unwrap();
                st.audio_packets += pkts.len() as u64;
                for p in pkts {
                    st.audio.push(p);
                }
            }
            Err(e) => {
                eprintln!("clipsd-linux: audio encode error: {e}");
                return;
            }
        }
    }
}
