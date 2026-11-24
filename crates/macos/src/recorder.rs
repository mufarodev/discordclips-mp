//! The macOS clips worker: implements the command vocabulary (`clipsd_core::proto`) on top of
//! ScreenCaptureKit + VideoToolbox + AudioToolbox.
//!
//! Threads while recording:
//! * video thread: owns the `SckCapture` and `VtEncoder` (neither is `Send`-safe to share),
//!   pushes packets into the shared rings and applies live control messages;
//! * audio thread: PCM → AAC → audio ring.

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

use crate::aac::AacEncoder;
use crate::capture::{CaptureConfig, CaptureError, PcmChunk, SckCapture};
use crate::encode::{EncoderConfig, VtEncoder};
use crate::thumbnail;

const AUDIO_BITRATE_BPS: u32 = 128_000;
const THUMB_QUALITY: u8 = 80;

/// Shared between the capture threads and command handlers.
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
    ) -> Event {
        let mut slot = self.session.lock().unwrap();
        if slot.is_some() {
            return Event::Error { message: "already recording".into() };
        }
        if startup_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(startup_delay_ms as u64));
        }
        match Self::spawn(fps.clamp(1, 120), duration_ms, bitrate_kbps.max(100), width, window_id) {
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
        // The video thread reports (width, height, audio receiver) once capture + encoder are up.
        type Init = Result<(u32, u32, Option<Receiver<PcmChunk>>), String>;
        let (init_tx, init_rx) = mpsc::channel::<Init>();

        let video = {
            let (state, stop) = (state.clone(), stop.clone());
            std::thread::Builder::new()
                .name("clipsd-video".into())
                .spawn(move || video_thread(fps, bitrate_kbps, max_width, window_id, state, stop, ctl_rx, init_tx))
                .map_err(|e| e.to_string())?
        };
        let (width, height, audio_rx) = match init_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let _ = video.join();
                return Err(e);
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                return Err("timed out starting capture".into());
            }
        };
        let mut threads = vec![video];
        if let Some(rx) = audio_rx {
            let (state, stop) = (state.clone(), stop.clone());
            threads.push(std::thread::Builder::new().name("clipsd-audio".into()).spawn(move || audio_thread(rx, state, stop)).map_err(|e| e.to_string())?);
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

        // Copy the snapshot under the lock; mux and write outside it.
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
        // Write to a temp name and rename so a reader never sees a half-written clip.
        let tmp: PathBuf = path.with_extension("mp4.part");
        let file = match std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&tmp) {
            Ok(f) => f,
            Err(e) => return fail(format!("{}: {e}", tmp.display())),
        };
        let audio_ref = audio.as_ref().filter(|(_, a)| !a.is_empty()).map(|(c, a)| (c, a.as_slice()));
        let audio_packets = audio_ref.map_or(0, |(_, a)| a.len());
        let span_us = pkts.last().unwrap().pts_us - pkts[0].pts_us;
        let duration_ms = (span_us / 1000) as u64;
        // Every clip carries the Discord `uuid` box; hosts own the metadata schema (default `{}`).
        let mut meta_val = with_defaults(metadata);
        if embed_thumbnail {
            if let serde_json::Value::Object(ref mut o) = meta_val {
                o.insert("length".into(), serde_json::json!(duration_ms));
            }
        }
        let meta = meta_val.to_string();
        if let Err(e) = mux::write_mp4_av(file, &cfg, &pkts, audio_ref, Some(meta.as_bytes())) {
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
                Err(_) => None, // a missing thumbnail must not fail the clip
            }
        };
        let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        // First audio packet's start relative to the first video frame (ms). The muxer puts the
        // first kept audio packet at track time 0, so a clearly positive value means audio
        // would play early by that amount.
        let audio_lag_ms = audio_ref.and_then(|(_, a)| {
            let first_audio = a.iter().map(|p| p.pts_us).min()?;
            Some((first_audio - pkts[0].pts_us) as f64 / 1000.0)
        });
        let stats_json = serde_json::json!({
            "audio_start_after_video_ms": audio_lag_ms,
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
            Command::StartRecording { fps, duration_ms, bitrate_kbps, startup_delay_ms, width, window_id } => {
                self.start(fps, duration_ms, bitrate_kbps, startup_delay_ms, width, window_id)
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
            Command::TakeScreenshot { filepath, quality, metadata, window_id } => {
                let path = Path::new(&filepath);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let comment = with_defaults(metadata).to_string();
                let r = crate::capture::screenshot(window_id)
                    .map_err(|e: CaptureError| e.to_string())
                    .and_then(|img| thumbnail::write_jpeg_with_comment(&img, path, quality, Some(&comment)));
                match r {
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

#[allow(clippy::too_many_arguments)]
fn video_thread(
    fps: u32,
    bitrate_kbps: u32,
    max_width: Option<u32>,
    window_id: Option<u32>,
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
    let mut cap = match SckCapture::start(&ccfg) {
        Ok(c) => c,
        Err(e) => {
            let _ = init.send(Err(e.to_string()));
            return;
        }
    };
    if let Some(ms) = std::env::var("CLIPSD_TEST_INIT_DELAY_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
        std::thread::sleep(Duration::from_millis(ms)); // test hook: simulate a slow encoder start
    }
    let mut ecfg = EncoderConfig::new(cap.width, cap.height);
    ecfg.fps = fps as i32;
    ecfg.bitrate_bps = bitrate_kbps as i32 * 1000;
    let mut enc = match VtEncoder::new(&ecfg) {
        Ok(e) => e,
        Err(e) => {
            let _ = init.send(Err(e));
            return;
        }
    };
    cap.flush_backlog(); // drop frames that queued up during encoder init (see flush_backlog)
    let _ = init.send(Ok((cap.width as u32, cap.height as u32, cap.take_audio())));
    let trace = std::env::var_os("CLIPSD_TRACE").is_some();
    let t_start = std::time::Instant::now();
    let (mut last_poll, mut last_out) = (t_start, t_start);

    while !stop.load(Ordering::Relaxed) {
        while let Ok(c) = ctl.try_recv() {
            match c {
                Ctl::SetBitrate(bps) => {
                    if let Err(e) = enc.set_bitrate(bps) {
                        eprintln!("clipsd: SetBitrate: {e}");
                    }
                }
            }
        }
        if trace {
            let now = std::time::Instant::now();
            if now - last_poll > Duration::from_millis(150) {
                eprintln!("[trace] {:?}: video loop stalled {:?} between polls", now - t_start, now - last_poll);
            }
            last_poll = now;
        }
        let step = match cap.poll_frame(Duration::from_millis(100)) {
            Ok(Some((f, pts))) => enc.encode(f, pts).map(Some),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        };
        match step {
            Ok(Some(pkts)) => {
                if trace && !pkts.is_empty() {
                    let now = std::time::Instant::now();
                    if now - last_out > Duration::from_millis(150) {
                        eprintln!("[trace] {:?}: encoder produced nothing for {:?}", now - t_start, now - last_out);
                    }
                    last_out = now;
                }
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
                eprintln!("clipsd: capture/encode error: {e}");
                break;
            }
        }
    }
    // `cap` drops here: stops the SCK stream.
}

fn audio_thread(rx: Receiver<PcmChunk>, state: Arc<Mutex<State>>, stop: Arc<AtomicBool>) {
    let mut enc: Option<AacEncoder> = None;
    while !stop.load(Ordering::Relaxed) {
        let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) else { continue };
        if enc.is_none() {
            match AacEncoder::new(chunk.sample_rate, chunk.channels, AUDIO_BITRATE_BPS) {
                Ok(e) => {
                    state.lock().unwrap().audio_cfg = Some(e.cfg.clone());
                    enc = Some(e);
                }
                Err(e) => {
                    eprintln!("clipsd: audio disabled: {e}");
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
                eprintln!("clipsd: audio encode error: {e}");
                return;
            }
        }
    }
}
