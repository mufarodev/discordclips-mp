//! Diagnostic: feed ONE captured frame repeatedly (as the static-screen path does) and report
//! what the encoder emits. Usage: m2_static [repeat_interval_ms] [count]
#[cfg(target_os = "macos")]
fn main() {
    use clipsd_core::{Capture, Encoder};
    use clipsd_macos::capture::{CaptureConfig, Frame, SckCapture};
    use clipsd_macos::encode::{EncoderConfig, VtEncoder};
    let iv: i64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(100);
    let n: i64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(60);
    let mut cap = SckCapture::start(&CaptureConfig::default()).expect("capture");
    let (f, pts0) = cap.next_frame().expect("frame");
    let buf = f.0.clone();
    let mut enc = VtEncoder::new(&EncoderConfig::new(cap.width, cap.height)).expect("enc");
    let mut pkts = vec![];
    for i in 0..n {
        pkts.extend(enc.encode(Frame(buf.clone()), pts0 + i * iv * 1000).expect("encode"));
    }
    pkts.extend(enc.flush().expect("flush"));
    println!("submitted={n} emitted={} keyframes={}", pkts.len(), pkts.iter().filter(|p| p.keyframe).count());
    let missing: Vec<_> = (0..n).filter(|i| !pkts.iter().any(|p| p.pts_us == pts0 + i * iv * 1000)).collect();
    println!("missing frame indexes: {missing:?}");
}
#[cfg(not(target_os = "macos"))]
fn main() {}
