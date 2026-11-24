//! Linux JPEG thumbnails & screenshot encoding (`jpeg-encoder` + `ffmpeg-next`).

use std::path::Path;

use ffmpeg_next as ffmpeg;

pub use clipsd_core::jpeg::{base64_encode, jpeg_with_comment};

/// Encode an RGB24 buffer into JPEG bytes, optionally splicing a Discord JSON COM comment.
pub fn encode_rgb_jpeg(rgb: &[u8], width: u16, height: u16, quality: u8, comment: Option<&str>) -> Result<Vec<u8>, String> {
    let mut raw = Vec::new();
    let enc = jpeg_encoder::Encoder::new(&mut raw, quality.clamp(1, 100));
    enc.encode(rgb, width, height, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| format!("jpeg encode: {e}"))?;
    match comment {
        Some(c) => jpeg_with_comment(&raw, c),
        None => Ok(raw),
    }
}

/// Extract a thumbnail frame at `at_ms` from an MP4 file as JPEG bytes (max 640px wide).
pub fn thumbnail_bytes_from_video(video: &Path, at_ms: u32, quality: u8) -> Result<Vec<u8>, String> {
    ffmpeg::init().map_err(|e| format!("ffmpeg init: {e}"))?;
    let mut ictx = ffmpeg::format::input(video).map_err(|e| format!("open {}: {e}", video.display()))?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or("no video stream")?;
    let idx = stream.index();
    let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|e| format!("decoder ctx: {e}"))?;
    let mut dec = ctx.decoder().video().map_err(|e| format!("video decoder: {e}"))?;

    if at_ms > 0 {
        let ts = at_ms as i64 * (ffmpeg::ffi::AV_TIME_BASE as i64) / 1000;
        let _ = ictx.seek(ts, ..ts);
    }

    let mut frame = ffmpeg::util::frame::Video::empty();
    for (s, pkt) in ictx.packets() {
        if s.index() != idx {
            continue;
        }
        if dec.send_packet(&pkt).is_err() {
            continue;
        }
        if dec.receive_frame(&mut frame).is_ok() {
            let sw = frame.width().max(1);
            let sh = frame.height().max(1);
            let dw = sw.min(640);
            let dh = (sh * dw / sw).max(1);
            let mut scaler = ffmpeg::software::scaling::Context::get(
                frame.format(),
                sw,
                sh,
                ffmpeg::format::Pixel::RGB24,
                dw,
                dh,
                ffmpeg::software::scaling::Flags::BILINEAR,
            )
            .map_err(|e| format!("thumb swscale: {e}"))?;
            let mut rgb = ffmpeg::util::frame::Video::empty();
            scaler.run(&frame, &mut rgb).map_err(|e| format!("thumb scale: {e}"))?;
            // Pack tightly in case stride > dw*3.
            let stride = rgb.stride(0);
            let row = (dw * 3) as usize;
            let mut packed = Vec::with_capacity(row * dh as usize);
            let src = rgb.data(0);
            for y in 0..dh as usize {
                packed.extend_from_slice(&src[y * stride..y * stride + row]);
            }
            return encode_rgb_jpeg(&packed, dw as u16, dh as u16, quality, None);
        }
    }
    Err("could not decode a video frame for thumbnail".into())
}

pub fn thumbnail_from_video(video: &Path, at_ms: u32, out: &Path, quality: u8) -> Result<(), String> {
    let bytes = thumbnail_bytes_from_video(video, at_ms, quality)?;
    std::fs::write(out, bytes).map_err(|e| format!("{}: {e}", out.display()))
}
