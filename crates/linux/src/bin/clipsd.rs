//! clipsd (Linux) — retroactive screen-clip recorder.
//!
//!   clipsd start [--buffer SECS] [--fps N] [--bitrate KBPS] [--idle] [--socket PATH]
//!   clipsd save <secs> [-o FILE]
//!   clipsd screenshot [-o FILE]
//!   clipsd stats | stop
//!   clipsd send '<json command>'

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Linux only");
}

#[cfg(target_os = "linux")]
fn main() {
    std::process::exit(cli::run(std::env::args().skip(1).collect()));
}

#[cfg(target_os = "linux")]
mod cli {
    use clipsd_core::ipc::{serve, Client, Service};
    use clipsd_core::proto::{Command, Event};
    use clipsd_linux::recorder::Recorder;
    use std::path::PathBuf;
    use std::sync::Arc;

    const USAGE: &str = "usage: clipsd start [--buffer SECS] [--fps N] [--bitrate KBPS] [--idle] [--socket PATH]\n       \
        clipsd save <secs> [-o FILE] | screenshot [-o FILE] | stats | stop | send '<json>'";

    fn flag(args: &[String], name: &str) -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    }

    fn socket(args: &[String]) -> PathBuf {
        flag(args, "--socket").map(PathBuf::from).unwrap_or_else(|| {
            let base = std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            base.join("clipsd.sock")
        })
    }

    pub fn run(args: Vec<String>) -> i32 {
        let Some(cmd) = args.first().cloned() else {
            eprintln!("{USAGE}");
            return 2;
        };
        let rest = &args[1..];
        match cmd.as_str() {
            "start" => start(rest),
            "save" => {
                let Some(secs) = rest.first().and_then(|s| s.parse::<u32>().ok()) else {
                    eprintln!("{USAGE}");
                    return 2;
                };
                let out = flag(rest, "-o").map(PathBuf::from).unwrap_or_else(|| default_name("clip", "mp4"));
                call(rest, Command::SaveClip {
                    filepath: abs(out),
                    metadata: None,
                    trim_start_ms: 0,
                    trim_end_ms: 0,
                    thumbnail_ms: None,
                    saved_at: None,
                    last_ms: Some(secs * 1000),
                    embed_thumbnail: false,
                })
            }
            "screenshot" => {
                let out = flag(rest, "-o").map(PathBuf::from).unwrap_or_else(|| default_name("shot", "jpg"));
                call(rest, Command::TakeScreenshot { filepath: abs(out), quality: 85, metadata: None, window_id: None })
            }
            "stats" => call(rest, Command::GetStats),
            "stop" => call(rest, Command::Shutdown),
            "send" => match rest.first().map(|j| serde_json::from_str::<Command>(j)) {
                Some(Ok(c)) => call(rest, c),
                Some(Err(e)) => {
                    eprintln!("bad command json: {e}");
                    2
                }
                None => {
                    eprintln!("{USAGE}");
                    2
                }
            },
            _ => {
                eprintln!("{USAGE}");
                2
            }
        }
    }

    fn abs(p: PathBuf) -> String {
        std::path::absolute(p).unwrap().to_string_lossy().into_owned()
    }

    fn default_name(prefix: &str, ext: &str) -> PathBuf {
        let dir = std::env::var_os("CLIPSD_CLIPS_DIR").map(PathBuf::from).unwrap_or_default();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        dir.join(format!("{prefix}-{ts}.{ext}"))
    }

    fn call(args: &[String], cmd: Command) -> i32 {
        let sock = socket(args);
        let mut c = match Client::connect(&sock) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: cannot reach clipsd at {} ({e}); is `clipsd start` running?", sock.display());
                return 1;
            }
        };
        match c.call(&cmd) {
            Ok(ev) => {
                println!("{}", serde_json::to_string(&ev).unwrap());
                match ev {
                    Event::ClipSaveFailed { .. } | Event::Error { .. } | Event::InitFailure { .. } => 1,
                    _ => 0,
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        }
    }

    fn start(args: &[String]) -> i32 {
        let num = |name: &str, default: u32| flag(args, name).and_then(|s| s.parse().ok()).unwrap_or(default);
        let recorder = Arc::new(Recorder::new());
        if !args.iter().any(|a| a == "--idle") {
            let ev = recorder.handle(Command::StartRecording {
                fps: num("--fps", 30),
                duration_ms: num("--buffer", 60) * 1000,
                bitrate_kbps: num("--bitrate", 8000),
                startup_delay_ms: 0,
                width: None,
                window_id: None,
                pid: flag(args, "--pid").and_then(|s| s.parse::<u32>().ok()),
            });
            match ev {
                Event::RecordingEvent { .. } => {}
                Event::InitFailure { message } | Event::Error { message } => {
                    eprintln!("error: {message}");
                    return 2;
                }
                other => eprintln!("unexpected: {other:?}"),
            }
        }
        let sock = socket(args);
        eprintln!(
            "clipsd: serving {} ({})",
            sock.display(),
            if args.iter().any(|a| a == "--idle") { "idle" } else { "recording" }
        );
        match serve(&sock, recorder) {
            Ok(()) => {
                eprintln!("clipsd: stopped");
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        }
    }
}
