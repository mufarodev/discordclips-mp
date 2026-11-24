//! Capture system audio for N seconds and encode to AAC.
//! Play something while it runs. Usage: m4_audio [seconds]
#[cfg(target_os = "macos")]
fn main() {
    use clipsd_macos::aac::AacEncoder;
    use clipsd_macos::capture::{CaptureConfig, SckCapture};
    use std::time::{Duration, Instant};

    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let mut cap = SckCapture::start(&CaptureConfig::default()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(2);
    });
    let rx = cap.take_audio().expect("audio");
    let t0 = Instant::now();
    let (mut chunks, mut frames, mut peak) = (0u64, 0u64, 0f32);
    let (mut enc, mut pkts, mut bytes) = (None::<AacEncoder>, 0usize, 0usize);
    let (mut first_pts, mut last_pts) = (None::<i64>, 0i64);
    while t0.elapsed() < Duration::from_secs(secs) {
        let Ok(c) = rx.recv_timeout(Duration::from_millis(500)) else { continue };
        if chunks < 3 {
            println!("chunk: pts={} rate={} ch={} frames={}", c.pts_us, c.sample_rate, c.channels, c.samples.len() / c.channels as usize);
        }
        chunks += 1;
        frames += (c.samples.len() / c.channels as usize) as u64;
        peak = c.samples.iter().fold(peak, |m, s| m.max(s.abs()));
        let e = enc.get_or_insert_with(|| AacEncoder::new(c.sample_rate, c.channels, 128_000).expect("aac"));
        for p in e.push(c.pts_us, &c.samples).expect("push") {
            pkts += 1;
            bytes += p.data.len();
            first_pts.get_or_insert(p.pts_us);
            last_pts = p.pts_us;
        }
    }
    println!(
        "chunks={chunks} frames={frames} ({:.2}s of audio) peak={peak:.3} dropped={} | aac packets={pkts} bytes={bytes} (~{:.0} kbps) span={:.2}s",
        frames as f64 / 48000.0,
        cap.dropped_audio(),
        bytes as f64 * 8.0 / (pkts.max(1) as f64 * 1024.0 / 48000.0) / 1000.0,
        (last_pts - first_pts.unwrap_or(0)) as f64 / 1e6
    );
}
#[cfg(not(target_os = "macos"))]
fn main() {}
