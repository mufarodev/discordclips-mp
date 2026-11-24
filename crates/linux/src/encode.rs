//! M7 & M8: Linux hardware video (`av1_nvenc` on RTX 40+ → `h264_nvenc` → `libx264`) and
//! AAC-LC audio encoding via `libavcodec`.
//!
//! - For AV1 (`av1_nvenc`), strips temporal delimiters and extracts `av1C` via
//!   `clipsd_core::av1::{prepare_av1_sample, build_av1c}` so `clipsd_core::mux` writes a
//!   spec-compliant `av01` + `av1C` MP4 accepted by Discord's `mp4box.js` clip editor.
//! - For H.264 (`h264_nvenc`, `h264_vaapi`, `libx264`), converts Annex-B packets to 4-byte
//!   length-prefixed AVCC + SPS/PPS via `clipsd_core::h264`.
//!
//! Override encoder selection with `CLIPSD_ENCODER=av1_nvenc|h264_nvenc|libx264` or
//! `CLIPSD_CODEC=av1|h264`.
//!
//! NOTE: Scaffolded on macOS against `ffmpeg-next 7`; verify `av1_nvenc` option names and
//! zero-copy CUDA/VAAPI `AVHWFramesContext` wiring on Linux (see LINUX_HANDOFF.md).

use clipsd_core::av1::{build_av1c, prepare_av1_sample};
use clipsd_core::h264::{annex_b_to_avcc, parse_extradata_sps_pps};
use clipsd_core::{AudioConfig, CodecConfig, Encoder, Packet, AAC_FRAME_SAMPLES};
use ffmpeg_next as ffmpeg;

use crate::capture::{Frame, PixelFormat};

pub struct EncoderConfig {
    pub width: usize,
    pub height: usize,
    pub fps: i32,
    pub bitrate_bps: i32,
    pub keyframe_secs: i32,
}

impl EncoderConfig {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, fps: 30, bitrate_bps: 8_000_000, keyframe_secs: 2 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoCodecKind {
    Av1,
    H264,
}

pub struct FfmpegVideoEncoder {
    encoder: ffmpeg::codec::encoder::Video,
    kind: VideoCodecKind,
    scaler: Option<ffmpeg::software::scaling::Context>,
    scaler_src_fmt: Option<ffmpeg::format::Pixel>,
    width: u32,
    height: u32,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    av1c: Option<Vec<u8>>,
    dropped: u64,
}

fn encoder_candidates() -> Vec<String> {
    if let Ok(explicit) = std::env::var("CLIPSD_ENCODER") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return vec![trimmed.to_string()];
        }
    }
    match std::env::var("CLIPSD_CODEC").ok().as_deref() {
        Some("h264") | Some("avc") => vec!["h264_nvenc".into(), "libx264".into()],
        Some("av1") => vec!["av1_nvenc".into()],
        _ => vec!["av1_nvenc".into(), "h264_nvenc".into(), "libx264".into()],
    }
}

impl FfmpegVideoEncoder {
    /// Open a video encoder, preferring RTX 40-series hardware AV1 (`av1_nvenc`), then
    /// `h264_nvenc`, then `libx264` (`ultrafast` + `zerolatency`).
    pub fn new(cfg: &EncoderConfig) -> Result<Self, String> {
        ffmpeg::init().map_err(|e| format!("ffmpeg init: {e}"))?;

        let candidates = encoder_candidates();
        let mut last_err = String::new();

        for name in &candidates {
            let Some(codec) = ffmpeg::encoder::find_by_name(name) else {
                last_err = format!("{name}: codec not compiled into libavcodec");
                continue;
            };
            let kind = if name.starts_with("av1") {
                VideoCodecKind::Av1
            } else {
                VideoCodecKind::H264
            };
            let ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
            let mut video = match ctx.encoder().video() {
                Ok(v) => v,
                Err(e) => {
                    last_err = format!("{name}: {e}");
                    continue;
                }
            };
            video.set_width(cfg.width as u32);
            video.set_height(cfg.height as u32);
            video.set_format(ffmpeg::format::Pixel::NV12);
            video.set_time_base(ffmpeg::Rational(1, 1_000_000));
            video.set_frame_rate(Some(ffmpeg::Rational(cfg.fps.max(1), 1)));
            video.set_bit_rate(cfg.bitrate_bps as usize);
            video.set_gop((cfg.fps * cfg.keyframe_secs).max(1) as u32);
            video.set_max_b_frames(0); // no B-frames: pts == dts, matching macOS

            let mut opts = ffmpeg::Dictionary::new();
            match name.as_str() {
                "av1_nvenc" => {
                    // Ada Lovelace (RTX 40-series) low-latency rolling buffer config.
                    opts.set("preset", "p3");
                    opts.set("tune", "ll");
                    opts.set("rc", "cbr");
                    opts.set("multipass", "disabled");
                    opts.set("b_ref_mode", "disabled");
                    opts.set("zerolatency", "1");
                }
                "h264_nvenc" => {
                    opts.set("preset", "p3");
                    opts.set("tune", "ll");
                    opts.set("rc", "cbr");
                    opts.set("zerolatency", "1");
                }
                "libx264" => {
                    opts.set("preset", "ultrafast");
                    opts.set("tune", "zerolatency");
                }
                _ => {}
            }

            match video.open_as_with(codec, opts) {
                Ok(opened) => {
                    eprintln!(
                        "clipsd-linux: opened video encoder '{name}' ({:?}, {}x{} @ {}fps)",
                        kind, cfg.width, cfg.height, cfg.fps
                    );
                    let (sps, pps, av1c) = extract_extradata(&opened, kind);
                    return Ok(Self {
                        encoder: opened,
                        kind,
                        scaler: None,
                        scaler_src_fmt: None,
                        width: cfg.width as u32,
                        height: cfg.height as u32,
                        sps,
                        pps,
                        av1c,
                        dropped: 0,
                    });
                }
                Err(e) => last_err = format!("{name}: {e}"),
            }
        }
        Err(format!("no working video encoder found from {candidates:?} ({last_err})"))
    }

    pub fn set_bitrate(&mut self, bitrate_bps: i32) -> Result<(), String> {
        unsafe {
            (*self.encoder.as_mut_ptr()).bit_rate = bitrate_bps as i64;
        }
        Ok(())
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    fn drain(&mut self) -> Result<Vec<Packet>, String> {
        let mut out = Vec::new();
        let mut pkt = ffmpeg::Packet::empty();
        while self.encoder.receive_packet(&mut pkt).is_ok() {
            let Some(raw) = pkt.data() else { continue };
            match self.kind {
                VideoCodecKind::Av1 => {
                    let s = prepare_av1_sample(raw);
                    if self.av1c.is_none() {
                        if let Some(seq) = s.seq_header.as_deref() {
                            self.av1c = build_av1c(seq);
                        }
                    }
                    if !s.data.is_empty() {
                        out.push(Packet {
                            pts_us: pkt.pts().unwrap_or(0),
                            keyframe: s.keyframe || pkt.is_key(),
                            data: s.data,
                        });
                    }
                }
                VideoCodecKind::H264 => {
                    let avcc = annex_b_to_avcc(raw);
                    if self.sps.is_none() {
                        self.sps = avcc.sps;
                    }
                    if self.pps.is_none() {
                        self.pps = avcc.pps;
                    }
                    if !avcc.data.is_empty() {
                        out.push(Packet {
                            pts_us: pkt.pts().unwrap_or(0),
                            keyframe: avcc.keyframe || pkt.is_key(),
                            data: avcc.data,
                        });
                    }
                }
            }
        }
        Ok(out)
    }
}

fn extract_extradata(
    enc: &ffmpeg::codec::encoder::Video,
    kind: VideoCodecKind,
) -> (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>) {
    unsafe {
        let ptr = enc.as_ptr();
        let size = (*ptr).extradata_size;
        let data = (*ptr).extradata;
        if size > 0 && !data.is_null() {
            let slice = std::slice::from_raw_parts(data, size as usize);
            match kind {
                VideoCodecKind::Av1 => return (None, None, build_av1c(slice)),
                VideoCodecKind::H264 => {
                    if let Some((s, p)) = parse_extradata_sps_pps(slice) {
                        return (Some(s), Some(p), None);
                    }
                }
            }
        }
    }
    (None, None, None)
}

impl Encoder for FfmpegVideoEncoder {
    type Frame = Frame;

    fn encode(&mut self, frame: Frame, pts_us: i64) -> Result<Vec<Packet>, String> {
        let src_pix = match frame.format {
            PixelFormat::Bgra | PixelFormat::Bgrx => ffmpeg::format::Pixel::BGRA,
            PixelFormat::Nv12 => ffmpeg::format::Pixel::NV12,
        };
        let mut src = ffmpeg::util::frame::Video::new(src_pix, frame.width, frame.height);
        if src_pix == ffmpeg::format::Pixel::BGRA {
            let dst_stride = src.stride(0);
            let src_stride = frame.stride as usize;
            let row_bytes = (frame.width as usize * 4).min(dst_stride).min(src_stride);
            let dst = src.data_mut(0);
            for y in 0..frame.height as usize {
                let s0 = y * src_stride;
                let d0 = y * dst_stride;
                if s0 + row_bytes <= frame.data.len() && d0 + row_bytes <= dst.len() {
                    dst[d0..d0 + row_bytes].copy_from_slice(&frame.data[s0..s0 + row_bytes]);
                }
            }
        } else {
            let y_len = (frame.width * frame.height) as usize;
            if frame.data.len() >= y_len + y_len / 2 {
                src.data_mut(0)[..y_len].copy_from_slice(&frame.data[..y_len]);
                src.data_mut(1)[..y_len / 2].copy_from_slice(&frame.data[y_len..y_len + y_len / 2]);
            }
        }

        let mut nv12 = if src_pix == ffmpeg::format::Pixel::NV12 && frame.width == self.width && frame.height == self.height {
            src
        } else {
            if self.scaler.is_none() || self.scaler_src_fmt != Some(src_pix) {
                self.scaler = Some(
                    ffmpeg::software::scaling::Context::get(
                        src_pix,
                        frame.width,
                        frame.height,
                        ffmpeg::format::Pixel::NV12,
                        self.width,
                        self.height,
                        ffmpeg::software::scaling::Flags::FAST_BILINEAR,
                    )
                    .map_err(|e| format!("swscale: {e}"))?,
                );
                self.scaler_src_fmt = Some(src_pix);
            }
            let mut dst = ffmpeg::util::frame::Video::empty();
            self.scaler.as_mut().unwrap().run(&src, &mut dst).map_err(|e| format!("swscale run: {e}"))?;
            dst
        };

        nv12.set_pts(Some(pts_us));
        self.encoder.send_frame(&nv12).map_err(|e| format!("encode send_frame: {e}"))?;
        self.drain()
    }

    fn flush(&mut self) -> Result<Vec<Packet>, String> {
        self.encoder.send_eof().map_err(|e| format!("encode eof: {e}"))?;
        self.drain()
    }

    fn codec_config(&self) -> Option<CodecConfig> {
        match self.kind {
            VideoCodecKind::Av1 => Some(CodecConfig::av1(self.width, self.height, self.av1c.clone()?)),
            VideoCodecKind::H264 => Some(CodecConfig::h264(self.width, self.height, self.sps.clone()?, self.pps.clone()?)),
        }
    }
}

/// AAC-LC encoder (`libavcodec` native `aac`) accepting interleaved f32 PCM and emitting
/// raw 1024-sample AAC frames matching `clipsd_core::AudioRing`.
pub struct FfmpegAacEncoder {
    pub cfg: AudioConfig,
    encoder: ffmpeg::codec::encoder::Audio,
    pcm_buf: Vec<f32>,
    next_pts_us: Option<i64>,
}

impl FfmpegAacEncoder {
    pub fn new(sample_rate: u32, channels: u8, bitrate_bps: u32) -> Result<Self, String> {
        ffmpeg::init().map_err(|e| format!("ffmpeg init: {e}"))?;
        let codec = ffmpeg::encoder::find(ffmpeg::codec::Id::AAC).ok_or("AAC encoder not found")?;
        let mut audio = ffmpeg::codec::context::Context::new_with_codec(codec)
            .encoder()
            .audio()
            .map_err(|e| format!("aac context: {e}"))?;
        audio.set_rate(sample_rate as i32);
        audio.set_channel_layout(if channels == 1 {
            ffmpeg::ChannelLayout::MONO
        } else {
            ffmpeg::ChannelLayout::STEREO
        });
        audio.set_format(ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar));
        audio.set_bit_rate(bitrate_bps as usize);
        audio.set_time_base(ffmpeg::Rational(1, sample_rate as i32));
        let opened = audio.open_as(codec).map_err(|e| format!("aac open: {e}"))?;

        Ok(Self {
            cfg: AudioConfig { sample_rate, channels, bitrate_bps },
            encoder: opened,
            pcm_buf: Vec::new(),
            next_pts_us: None,
        })
    }

    pub fn push(&mut self, pts_us: i64, interleaved_f32: &[f32]) -> Result<Vec<Packet>, String> {
        let ch = self.cfg.channels as usize;
        let frame_samples = AAC_FRAME_SAMPLES as usize;
        if self.next_pts_us.is_none() {
            self.next_pts_us = Some(pts_us);
        }
        self.pcm_buf.extend_from_slice(interleaved_f32);

        let mut out = Vec::new();
        let need = frame_samples * ch;
        let frame_us = 1_000_000 * frame_samples as i64 / self.cfg.sample_rate as i64;

        while self.pcm_buf.len() >= need {
            let chunk: Vec<f32> = self.pcm_buf.drain(..need).collect();
            let mut af = ffmpeg::util::frame::Audio::new(
                ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar),
                frame_samples,
                if ch == 1 { ffmpeg::ChannelLayout::MONO } else { ffmpeg::ChannelLayout::STEREO },
            );
            af.set_rate(self.cfg.sample_rate);
            for c in 0..ch {
                let plane = af.plane_mut::<f32>(c);
                for i in 0..frame_samples {
                    plane[i] = chunk[i * ch + c];
                }
            }
            let cur_pts = self.next_pts_us.unwrap_or(pts_us);
            af.set_pts(Some(cur_pts * self.cfg.sample_rate as i64 / 1_000_000));
            self.next_pts_us = Some(cur_pts + frame_us);

            self.encoder.send_frame(&af).map_err(|e| format!("aac send: {e}"))?;
            let mut pkt = ffmpeg::Packet::empty();
            while self.encoder.receive_packet(&mut pkt).is_ok() {
                if let Some(d) = pkt.data() {
                    out.push(Packet { pts_us: cur_pts, keyframe: true, data: d.to_vec() });
                }
            }
        }
        Ok(out)
    }
}
