//! `clipsd start [--buffer SECS] | save <secs> [-o FILE] | stop`
//!
//! `start` runs in the foreground: capture → encode → ring, plus a tiny line protocol on a
//! Unix socket (the full command/event vocabulary comes in M5). `save` and `stop` are clients.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macOS only");
}

#[cfg(target_os = "macos")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("start") => daemon::run(&args[1..]),
        Some("save") => client::save(&args[1..]),
        Some("stop") => client::stop(),
        _ => {
            eprintln!("usage: clipsd start [--buffer SECS]\n       clipsd save <secs> [-o FILE]\n       clipsd stop");
            2
        }
    };
    std::process::exit(code);
}

#[cfg(target_os = "macos")]
fn socket_path() -> std::path::PathBuf {
    // $TMPDIR is per-user on macOS, so this is not world-reachable.
    std::env::temp_dir().join("clipsd.sock")
}

#[cfg(target_os = "macos")]
mod client {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    fn request(line: &str) -> Result<String, String> {
        let mut s = UnixStream::connect(super::socket_path())
            .map_err(|e| format!("cannot reach clipsd ({e}); is `clipsd start` running?"))?;
        writeln!(s, "{line}").map_err(|e| e.to_string())?;
        let mut resp = String::new();
        BufReader::new(s).read_line(&mut resp).map_err(|e| e.to_string())?;
        Ok(resp.trim().to_string())
    }

    fn report(r: Result<String, String>) -> i32 {
        match r {
            Ok(r) if r.starts_with("ok") => {
                println!("{r}");
                0
            }
            Ok(r) => {
                eprintln!("{r}");
                1
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        }
    }

    pub fn save(args: &[String]) -> i32 {
        let Some(secs) = args.first().and_then(|s| s.parse::<u32>().ok()) else {
            eprintln!("usage: clipsd save <secs> [-o FILE]");
            return 2;
        };
        let out = match args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)) {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format!("clip-{ts}.mp4").into()
            }
        };
        let out = std::path::absolute(out).unwrap();
        report(request(&format!("save {secs} {}", out.display())))
    }

    pub fn stop() -> i32 {
        report(request("stop"))
    }
}

#[cfg(target_os = "macos")]
mod daemon {
    use clipsd_core::{mux, AudioConfig, AudioRing, Capture, CodecConfig, Encoder, Ring};
    use clipsd_macos::aac::AacEncoder;
    use clipsd_macos::capture::{CaptureConfig, SckCapture};
    use clipsd_macos::encode::{EncoderConfig, VtEncoder};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct State {
        ring: Ring,
        config: Option<CodecConfig>,
        audio: AudioRing,
        audio_cfg: Option<AudioConfig>,
        frames: u64,
    }

    pub fn run(args: &[String]) -> i32 {
        let buffer: u32 = args
            .iter()
            .position(|a| a == "--buffer")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);

        let sock = super::socket_path();
        if UnixStream::connect(&sock).is_ok() {
            eprintln!("error: clipsd is already running ({})", sock.display());
            return 1;
        }
        let _ = std::fs::remove_file(&sock); // stale socket from a crashed run

        let mut cap = match SckCapture::start(&CaptureConfig::default()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let mut enc = match VtEncoder::new(&EncoderConfig::new(cap.width, cap.height)) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let state = Arc::new(Mutex::new(State { ring: Ring::new(buffer), config: None, audio: AudioRing::new(buffer + 5), audio_cfg: None, frames: 0 }));
        let stop = Arc::new(AtomicBool::new(false));

        let listener = match UnixListener::bind(&sock) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: cannot bind {}: {e}", sock.display());
                return 1;
            }
        };
        {
            let (state, stop) = (state.clone(), stop.clone());
            std::thread::spawn(move || {
                for conn in listener.incoming().flatten() {
                    handle(conn, &state, &stop);
                }
            });
        }
        if let Some(rx) = cap.take_audio() {
            let state = state.clone();
            std::thread::spawn(move || {
                let mut enc: Option<AacEncoder> = None;
                for chunk in rx {
                    if enc.is_none() {
                        match AacEncoder::new(chunk.sample_rate, chunk.channels, 128_000) {
                            Ok(e) => {
                                state.lock().unwrap().audio_cfg = Some(e.cfg.clone());
                                enc = Some(e);
                            }
                            Err(e) => {
                                eprintln!("audio disabled: {e}");
                                return;
                            }
                        }
                    }
                    match enc.as_mut().unwrap().push(chunk.pts_us, &chunk.samples) {
                        Ok(pkts) => {
                            let mut st = state.lock().unwrap();
                            for p in pkts {
                                st.audio.push(p);
                            }
                        }
                        Err(e) => {
                            eprintln!("audio encode error: {e}");
                            return;
                        }
                    }
                }
            });
        }
        eprintln!("clipsd: recording {}x{}, keeping last {buffer}s; socket {}", cap.width, cap.height, sock.display());

        let mut result = 0;
        while !stop.load(Ordering::Relaxed) {
            let step = cap.next_frame().and_then(|(f, pts)| enc.encode(f, pts));
            match step {
                Ok(pkts) => {
                    let mut st = state.lock().unwrap();
                    if st.config.is_none() {
                        st.config = enc.codec_config();
                    }
                    st.frames += 1;
                    for p in pkts {
                        st.ring.push(p);
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    result = 1;
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(&sock);
        eprintln!("clipsd: stopped");
        result
    }

    fn handle(conn: UnixStream, state: &Mutex<State>, stop: &AtomicBool) {
        let mut line = String::new();
        if BufReader::new(&conn).read_line(&mut line).is_err() {
            return;
        }
        let reply = respond(line.trim(), state, stop);
        let _ = writeln!(&conn, "{reply}");
    }

    fn respond(line: &str, state: &Mutex<State>, stop: &AtomicBool) -> String {
        let mut it = line.splitn(3, ' ');
        match (it.next(), it.next(), it.next()) {
            (Some("stop"), _, _) => {
                stop.store(true, Ordering::Relaxed);
                "ok stopping".into()
            }
            (Some("save"), Some(secs), Some(path)) => match secs.parse::<u32>() {
                Ok(secs) => save(state, secs, path).unwrap_or_else(|e| format!("err {e}")),
                Err(_) => "err bad seconds".into(),
            },
            _ => "err unknown command".into(),
        }
    }

    fn save(state: &Mutex<State>, secs: u32, path: &str) -> Result<String, String> {
        // Hold the lock only to copy the snapshot; muxing/IO happens outside it.
        let (pkts, cfg, audio) = {
            let st = state.lock().unwrap();
            let pkts = st.ring.snapshot(secs);
            let audio = match (&st.audio_cfg, pkts.first(), pkts.last()) {
                (Some(ac), Some(f), Some(l)) => Some((ac.clone(), st.audio.range(f.pts_us, l.pts_us))),
                _ => None,
            };
            (pkts, st.config.clone(), audio)
        };
        let cfg = cfg.ok_or("no video buffered yet")?;
        if pkts.is_empty() {
            return Err("nothing buffered yet".into());
        }
        let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
        let audio_ref = audio.as_ref().filter(|(_, a)| !a.is_empty()).map(|(c, a)| (c, a.as_slice()));
        let n_audio = audio_ref.map_or(0, |(_, a)| a.len());
        mux::write_mp4_av(std::io::BufWriter::new(file), &cfg, &pkts, audio_ref)?;
        let span = (pkts.last().unwrap().pts_us - pkts[0].pts_us) as f64 / 1e6;
        Ok(format!("ok {path} frames={} audio_packets={n_audio} span={span:.2}s", pkts.len()))
    }
}
