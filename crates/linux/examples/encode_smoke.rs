//! Encoder smoke test: synthetic moving BGRA frames -> FfmpegVideoEncoder -> MP4.
//! Usage: cargo run -p clipsd-linux --example encode_smoke -- out.mp4 [WIDTH HEIGHT SECONDS]
use clipsd_core::{mux, Encoder};
use clipsd_linux::capture::{CpuFrame, Frame, PixelFormat};
use clipsd_linux::encode::{EncoderConfig, FfmpegVideoEncoder};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let out = a.get(1).expect("out.mp4");
    let w: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1920);
    let h: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1080);
    let secs: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(4);
    let fps = 60usize;
    let mut cfg = EncoderConfig::new(w, h);
    cfg.fps = fps as i32;
    let mut enc = FfmpegVideoEncoder::new(&cfg).expect("encoder");
    let mut pkts = Vec::new();
    for i in 0..fps * secs {
        // Left half red, right half blue, moving white bar: catches R/B swaps and stride bugs.
        let mut data = vec![0u8; w * h * 4];
        let bar = (i * 16) % w;
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                let (b, g, r) = if x >= bar && x < bar + 40 { (255, 255, 255) } else if x < w / 2 { (0, 0, 255) } else { (255, 0, 0) };
                data[o] = b; data[o + 1] = g; data[o + 2] = r; data[o + 3] = 255;
            }
        }
        let f = Frame::Cpu(CpuFrame { width: w as u32, height: h as u32, stride: (w * 4) as u32, format: PixelFormat::Bgrx, data });
        pkts.extend(enc.encode(f, i as i64 * 1_000_000 / fps as i64).expect("encode"));
    }
    pkts.extend(enc.flush().expect("flush"));
    let cc = enc.codec_config().expect("codec config (extradata/av1C)");
    eprintln!("{} packets, {} keyframes", pkts.len(), pkts.iter().filter(|p| p.keyframe).count());
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(out).unwrap();
    mux::write_mp4_av(f, &cc, &pkts, None, None).expect("mux");
}
