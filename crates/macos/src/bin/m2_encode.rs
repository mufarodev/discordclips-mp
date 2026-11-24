//! M2 demo: capture → VideoToolbox H.264, report packets, keyframe cadence and SPS/PPS.
//! Usage: m2_encode [seconds]  (default 8)
#[cfg(target_os = "macos")]
fn main() {
    use clipsd_core::{Capture, Encoder};
    use clipsd_macos::capture::{CaptureConfig, SckCapture};
    use clipsd_macos::encode::{EncoderConfig, VtEncoder};
    use std::time::{Duration, Instant};

    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(8);
    let mut cap = SckCapture::start(&CaptureConfig::default()).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(2);
    });
    let mut enc = VtEncoder::new(&EncoderConfig::new(cap.width, cap.height)).expect("encoder");
    eprintln!("encoding {}x{} for {secs}s", cap.width, cap.height);

    let mut pkts = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        let (f, pts) = cap.next_frame().expect("frame");
        pkts.extend(enc.encode(f, pts).expect("encode"));
    }
    pkts.extend(enc.flush().expect("flush"));

    let keys: Vec<_> = pkts.iter().filter(|p| p.keyframe).collect();
    let bytes: usize = pkts.iter().map(|p| p.data.len()).sum();
    println!("packets={} keyframes={} bytes={}", pkts.len(), keys.len(), bytes);
    println!(
        "keyframe pts gaps (s): {:?}",
        keys.windows(2).map(|w| (w[1].pts_us - w[0].pts_us) as f64 / 1e6).collect::<Vec<_>>()
    );
    println!("first packet keyframe={} (must be true)", pkts[0].keyframe);
    let mono = pkts.windows(2).all(|w| w[1].pts_us > w[0].pts_us);
    println!("pts strictly increasing in output order: {mono}");
    // AVCC sanity: the length prefixes must exactly tile each packet.
    let ok = pkts.iter().all(|p| {
        let (mut i, d) = (0usize, &p.data);
        while i + 4 <= d.len() {
            i += 4 + u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
        }
        i == d.len()
    });
    println!("AVCC length prefixes tile every packet: {ok}");
    let c = enc.codec_config().expect("codec config");
    println!(
        "config {}x{} sps={}B pps={}B sps[1..4]={:02x?} (profile/compat/level)",
        c.width, c.height, c.sps.len(), c.pps.len(), &c.sps[1..4]
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macOS only");
}
