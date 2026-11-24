//! Capture the main display for N seconds and log frame count + pts.
//! Usage: m1_capture [seconds]   (default 5). Move something on screen while it runs.
#[cfg(target_os = "macos")]
fn main() {
    use clipsd_core::Capture;
    use clipsd_macos::capture::{CaptureConfig, SckCapture};
    use std::time::{Duration, Instant};

    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let mut cap = match SckCapture::start(&CaptureConfig::default()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    eprintln!("capturing {}x{} for {secs}s", cap.width, cap.height);
    let t0 = Instant::now();
    let (mut n, mut first, mut last, mut max_gap) = (0u64, None::<i64>, 0i64, 0i64);
    while t0.elapsed() < Duration::from_secs(secs) {
        let (_frame, pts) = cap.next_frame().expect("frame");
        if n < 5 || n % 30 == 0 {
            println!("frame {n:5} pts_us={pts}");
        }
        if first.is_some() {
            max_gap = max_gap.max(pts - last);
        }
        first.get_or_insert(pts);
        last = pts;
        n += 1;
    }
    let span = (last - first.unwrap_or(0)) as f64 / 1e6;
    println!(
        "frames={n} pts_span={span:.2}s fps={:.1} max_gap={:.0}ms dropped={}",
        n as f64 / span.max(1e-9),
        max_gap as f64 / 1e3,
        cap.dropped()
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macOS only");
}
