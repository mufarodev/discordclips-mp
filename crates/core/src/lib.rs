//! Platform-independent core: the two traits platforms implement, plus the ring buffer.
use std::collections::VecDeque;

/// One encoded video packet (AVCC, as VideoToolbox emits; Linux encoders must match).
#[derive(Clone, Debug)]
pub struct Packet {
    pub pts_us: i64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

/// Platform seam #1: screen capture. Frames stay GPU-resident; `Frame` is platform-defined.
pub trait Capture {
    type Frame;
    fn next_frame(&mut self) -> Result<(Self::Frame, i64 /* pts_us */), String>;
}

/// Stream parameters needed to mux H.264 (the `avcC` box). Packets carry AVCC data with
/// 4-byte NAL length prefixes; SPS/PPS are raw NAL units without a start code or length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecConfig {
    pub width: u32,
    pub height: u32,
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
}

/// Audio stream parameters for the AAC-LC track. Packets are raw AAC frames (no ADTS), 1024
/// samples each, so a packet's duration is `1024 / sample_rate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u8,
    pub bitrate_bps: u32,
}

/// Samples per AAC-LC frame.
pub const AAC_FRAME_SAMPLES: u32 = 1024;

/// Rolling buffer of encoded audio packets (every AAC frame is independently decodable, so
/// unlike video there is no GOP alignment). Trimmed purely by time.
pub struct AudioRing {
    window_us: i64,
    pkts: VecDeque<Packet>,
}

impl AudioRing {
    pub fn new(window_secs: u32) -> Self {
        Self { window_us: window_secs as i64 * 1_000_000, pkts: VecDeque::new() }
    }
    pub fn set_window(&mut self, window_secs: u32) {
        self.window_us = window_secs as i64 * 1_000_000;
    }
    pub fn len(&self) -> usize {
        self.pkts.len()
    }
    pub fn push(&mut self, p: Packet) {
        self.pkts.push_back(p);
        let newest = self.pkts.back().unwrap().pts_us;
        while self.pkts.front().is_some_and(|f| newest - f.pts_us > self.window_us) {
            self.pkts.pop_front();
        }
    }
    /// All packets covering `[start_us, end_us]` (a packet starting just before `start_us` is
    /// kept if it overlaps it).
    pub fn range(&self, start_us: i64, end_us: i64) -> Vec<Packet> {
        let frame_us = 1_000_000 * AAC_FRAME_SAMPLES as i64 / 48_000;
        self.pkts.iter().filter(|p| p.pts_us + frame_us > start_us && p.pts_us <= end_us).cloned().collect()
    }
}

/// Platform seam #2: hardware encoder.
pub trait Encoder {
    type Frame;
    /// Submit a frame. Encoders may be asynchronous: this returns whatever packets have
    /// finished since the previous call, which may be none (or packets for earlier frames).
    fn encode(&mut self, frame: Self::Frame, pts_us: i64) -> Result<Vec<Packet>, String>;
    /// Force out and return all in-flight packets.
    fn flush(&mut self) -> Result<Vec<Packet>, String>;
    /// Available once the first keyframe has been produced.
    fn codec_config(&self) -> Option<CodecConfig>;
}

/// Rolling buffer of encoded packets, trimmed on GOP boundaries so it always starts on a keyframe.
pub struct Ring {
    window_us: i64,
    pkts: VecDeque<Packet>,
}

impl Ring {
    pub fn new(window_secs: u32) -> Self {
        Self { window_us: window_secs as i64 * 1_000_000, pkts: VecDeque::new() }
    }

    pub fn push(&mut self, p: Packet) {
        if self.pkts.is_empty() && !p.keyframe {
            return; // can't start a clip mid-GOP
        }
        self.pkts.push_back(p);
        let newest = self.pkts.back().unwrap().pts_us;
        // Drop whole GOPs while the *next* keyframe still leaves a full window.
        while let Some(k) = self.pkts.iter().skip(1).position(|p| p.keyframe).map(|i| i + 1) {
            if newest - self.pkts[k].pts_us >= self.window_us {
                self.pkts.drain(..k);
            } else {
                break;
            }
        }
    }

    /// Change the retention window; takes effect on the next `push`.
    pub fn set_window(&mut self, window_secs: u32) {
        self.window_us = window_secs as i64 * 1_000_000;
    }

    /// Time span currently buffered (newest pts - oldest pts).
    pub fn buffered_us(&self) -> i64 {
        match (self.pkts.front(), self.pkts.back()) {
            (Some(a), Some(b)) => b.pts_us - a.pts_us,
            _ => 0,
        }
    }

    /// Total encoded bytes held.
    pub fn bytes(&self) -> usize {
        self.pkts.iter().map(|p| p.data.len()).sum()
    }

    /// Last `secs` of video, starting at the keyframe at-or-before the cutoff (so >= `secs`).
    pub fn snapshot(&self, secs: u32) -> Vec<Packet> {
        self.snapshot_us(secs as i64 * 1_000_000)
    }

    /// Like [`snapshot`](Self::snapshot) with microsecond precision.
    pub fn snapshot_us(&self, keep_us: i64) -> Vec<Packet> {
        let Some(newest) = self.pkts.back() else { return vec![] };
        let cutoff = newest.pts_us - keep_us;
        let start = self.pkts.iter().rposition(|p| p.keyframe && p.pts_us <= cutoff).unwrap_or(0);
        self.pkts.iter().skip(start).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn feed(r: &mut Ring, secs: i64) {
        // 1 fps, keyframe every 2s
        for s in 0..secs {
            r.push(Packet { pts_us: s * 1_000_000, keyframe: s % 2 == 0, data: vec![] });
        }
    }
    #[test]
    fn starts_on_keyframe_and_covers_window() {
        let mut r = Ring::new(10);
        feed(&mut r, 100);
        assert!(r.pkts[0].keyframe);
        let span = r.pkts.back().unwrap().pts_us - r.pkts[0].pts_us;
        assert!(span >= 10_000_000 && span < 13_000_000, "span={span}");
    }
    #[test]
    fn snapshot_is_at_least_requested_and_keyframe_aligned() {
        let mut r = Ring::new(30);
        feed(&mut r, 100);
        let s = r.snapshot(5);
        assert!(s[0].keyframe);
        assert!(s.last().unwrap().pts_us - s[0].pts_us >= 5_000_000);
    }
    #[test]
    fn drops_leading_non_keyframes() {
        let mut r = Ring::new(10);
        r.push(Packet { pts_us: 0, keyframe: false, data: vec![] });
        assert!(r.snapshot(5).is_empty());
    }
}

pub mod proto;
#[cfg(unix)]
pub mod ipc;

pub mod mux {
    //! MP4 muxing (pure Rust, `mp4` crate). Runs at save time only.
    use super::{AudioConfig, CodecConfig, Packet, AAC_FRAME_SAMPLES};
    use std::io::{Seek, Write};

    /// Media timescale for the video track.
    const TIMESCALE: u32 = 90_000;

    /// Write `packets` (as returned by `Ring::snapshot`, so starting on a keyframe) as an MP4
    /// with one H.264 track. AVCC packet bytes pass through unchanged; SPS/PPS go in `avcC`.
    ///
    /// Timestamps are rebased so the first sample is at 0 (capture pts are host-clock values
    /// and the writer emits no edit list). Each sample's duration is the gap to the next one;
    /// the last sample reuses the previous gap.
    pub fn write_mp4<W: Write + Seek>(w: W, cfg: &CodecConfig, packets: &[Packet]) -> Result<(), String> {
        write_mp4_av(w, cfg, packets, None)
    }

    /// Like [`write_mp4`], plus an optional AAC-LC audio track. Audio packets are placed on the
    /// same (rebased) timeline as the video; those ending before the first video sample are
    /// dropped. Samples are written interleaved by time.
    pub fn write_mp4_av<W: Write + Seek>(
        w: W,
        cfg: &CodecConfig,
        packets: &[Packet],
        audio: Option<(&AudioConfig, &[Packet])>,
    ) -> Result<(), String> {
        use mp4::{
            AacConfig, AudioObjectType, AvcConfig, ChannelConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample,
            Mp4Writer, SampleFreqIndex, TrackConfig, TrackType,
        };

        let first = packets.first().ok_or("no packets to write")?;
        if !first.keyframe {
            return Err("first packet must be a keyframe".into());
        }
        if cfg.sps.len() < 4 || cfg.pps.is_empty() {
            return Err("codec config missing SPS/PPS".into());
        }
        let e = |e: mp4::Error| e.to_string();

        let mut mw = Mp4Writer::write_start(
            w,
            &Mp4Config {
                major_brand: FourCC { value: *b"isom" },
                minor_version: 512,
                compatible_brands: [*b"isom", *b"iso2", *b"avc1", *b"mp41"].map(|value| FourCC { value }).to_vec(),
                // Must equal the track timescale: mp4 truncates each sample's duration when
                // converting to the movie timescale, which shortens the movie (~0.3 ms/frame
                // at 1000) and makes players cut off the end of the clip.
                timescale: TIMESCALE,
            },
        )
        .map_err(e)?;
        mw.add_track(&TrackConfig {
            track_type: TrackType::Video,
            timescale: TIMESCALE,
            language: "und".into(),
            media_conf: MediaConfig::AvcConfig(AvcConfig {
                width: cfg.width as u16,
                height: cfg.height as u16,
                seq_param_set: cfg.sps.clone(),
                pic_param_set: cfg.pps.clone(),
            }),
        })
        .map_err(e)?;

        let audio_track = if let Some((ac, _)) = audio {
            let freq_index = match ac.sample_rate {
                48_000 => SampleFreqIndex::Freq48000,
                44_100 => SampleFreqIndex::Freq44100,
                n => return Err(format!("unsupported AAC sample rate {n}")),
            };
            let chan_conf = match ac.channels {
                1 => ChannelConfig::Mono,
                2 => ChannelConfig::Stereo,
                n => return Err(format!("unsupported AAC channel count {n}")),
            };
            mw.add_track(&TrackConfig {
                track_type: TrackType::Audio,
                timescale: ac.sample_rate,
                language: "und".into(),
                media_conf: MediaConfig::AacConfig(AacConfig {
                    bitrate: ac.bitrate_bps,
                    profile: AudioObjectType::AacLowComplexity,
                    freq_index,
                    chan_conf,
                }),
            })
            .map_err(e)?;
            Some(2u32)
        } else {
            None
        };

        // Video samples: (time_us, track, Mp4Sample).
        let ticks = |p: &Packet| ((p.pts_us - first.pts_us) as i128 * TIMESCALE as i128 / 1_000_000) as u64;
        let mut prev_dur = (TIMESCALE / 30) as u32;
        let mut items: Vec<(i64, u32, Mp4Sample)> = Vec::with_capacity(packets.len());
        for (i, p) in packets.iter().enumerate() {
            let start = ticks(p);
            let duration = match packets.get(i + 1) {
                Some(n) => (ticks(n).saturating_sub(start)).max(1) as u32,
                None => prev_dur,
            };
            prev_dur = duration;
            items.push((
                p.pts_us,
                1,
                Mp4Sample {
                    start_time: start,
                    duration,
                    rendering_offset: 0,
                    is_sync: p.keyframe,
                    bytes: bytes::Bytes::copy_from_slice(&p.data),
                },
            ));
        }
        if let (Some(_), Some((ac, apkts))) = (audio_track, audio) {
            let frame_us = 1_000_000 * AAC_FRAME_SAMPLES as i64 / ac.sample_rate as i64;
            let end_us = packets.last().map(|p| p.pts_us).unwrap_or(first.pts_us);
            // Timeline origin for audio: first kept packet, clamped to the video start.
            let kept: Vec<&Packet> =
                apkts.iter().filter(|p| p.pts_us + frame_us > first.pts_us && p.pts_us <= end_us).collect();
            if !kept.is_empty() {
                for (i, p) in kept.iter().enumerate() {
                    // Constant 1024-sample duration; start time derived from the packet index so
                    // the track stays gapless (the encoder already filled any capture gaps).
                    items.push((
                        p.pts_us,
                        2,
                        Mp4Sample {
                            start_time: i as u64 * AAC_FRAME_SAMPLES as u64,
                            duration: AAC_FRAME_SAMPLES,
                            rendering_offset: 0,
                            is_sync: true,
                            bytes: bytes::Bytes::copy_from_slice(&p.data),
                        },
                    ));
                }
            }
        }
        items.sort_by_key(|(t, trk, _)| (*t, *trk));
        for (_, track, sample) in &items {
            mw.write_sample(*track, sample).map_err(e)?;
        }
        mw.write_end().map_err(e)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn roundtrip_samples_sync_flags_and_rebased_times() {
            let cfg = CodecConfig { width: 640, height: 480, sps: vec![0x67, 0x64, 0x00, 0x28, 0xac], pps: vec![0x68, 0xee, 0x3c, 0x80] };
            let base = 499_140_000_000i64; // host-clock-sized pts
            let pk: Vec<Packet> = (0..90)
                .map(|i| Packet { pts_us: base + i * 33_333, keyframe: i % 60 == 0, data: vec![0, 0, 0, 2, 0x65, i as u8] })
                .collect();
            let mut buf = std::io::Cursor::new(Vec::new());
            write_mp4(&mut buf, &cfg, &pk).unwrap();
            let len = buf.get_ref().len() as u64;
            buf.set_position(0);
            let mut r = mp4::Mp4Reader::read_header(buf, len).unwrap();
            let (&id, track) = r.tracks().iter().next().unwrap();
            assert_eq!(track.width(), 640);
            assert_eq!(track.sample_count(), 90);
            let s0 = r.read_sample(id, 1).unwrap().unwrap();
            assert!(s0.is_sync && s0.start_time == 0);
            assert!(!r.read_sample(id, 2).unwrap().unwrap().is_sync);
            assert!(r.read_sample(id, 61).unwrap().unwrap().is_sync);
            let d = r.tracks()[&id].duration().as_secs_f64();
            assert!((d - 90.0 * 0.033333).abs() < 0.05, "duration {d}");
            // Movie-level duration (what players honour) must equal the media duration.
            let movie = r.duration().as_secs_f64();
            assert!((movie - d).abs() < 0.001, "movie {movie} vs media {d}");
        }

        #[test]
        fn av_roundtrip_two_tracks_and_audio_aligned() {
            let cfg = CodecConfig { width: 640, height: 480, sps: vec![0x67, 0x64, 0x00, 0x28, 0xac], pps: vec![0x68, 0xee, 0x3c, 0x80] };
            let ac = AudioConfig { sample_rate: 48_000, channels: 2, bitrate_bps: 128_000 };
            let base = 1_000_000_000i64;
            let vid: Vec<Packet> = (0..60)
                .map(|i| Packet { pts_us: base + i * 33_333, keyframe: i % 30 == 0, data: vec![0, 0, 0, 2, 0x65, i as u8] })
                .collect();
            // audio starts 100ms *before* video and runs past its end
            let frame_us = 1_000_000 * 1024 / 48_000;
            let aud: Vec<Packet> = (0..120)
                .map(|i| Packet { pts_us: base - 100_000 + i * frame_us, keyframe: true, data: vec![0x21, i as u8, 0, 0] })
                .collect();
            let mut buf = std::io::Cursor::new(Vec::new());
            write_mp4_av(&mut buf, &cfg, &vid, Some((&ac, &aud))).unwrap();
            let len = buf.get_ref().len() as u64;
            buf.set_position(0);
            let r = mp4::Mp4Reader::read_header(buf, len).unwrap();
            assert_eq!(r.tracks().len(), 2);
            let a = r.tracks().values().find(|t| t.track_type().unwrap() == mp4::TrackType::Audio).unwrap();
            assert!(a.sample_count() > 80 && a.sample_count() < 100, "audio samples {}", a.sample_count());
            let v = r.tracks().values().find(|t| t.track_type().unwrap() == mp4::TrackType::Video).unwrap();
            assert_eq!(v.sample_count(), 60);
        }

        #[test]
        fn rejects_non_keyframe_start() {
            let cfg = CodecConfig { width: 2, height: 2, sps: vec![0x67, 0x64, 0, 0x28], pps: vec![0x68] };
            let pk = [Packet { pts_us: 0, keyframe: false, data: vec![] }];
            assert!(write_mp4(std::io::Cursor::new(Vec::new()), &cfg, &pk).is_err());
        }
    }
}
