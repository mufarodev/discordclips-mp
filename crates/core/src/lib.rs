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

/// Stream parameters needed to mux video: either H.264 (`avcC`: raw SPS/PPS NALs) or AV1
/// (`av1C`: AV1CodecConfigurationRecord bytes).
///
/// For H.264 (`av1c: None`), packets carry AVCC data with 4-byte NAL length prefixes.
/// For AV1 (`av1c: Some(...)`), packets carry raw OBU samples with temporal delimiters stripped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecConfig {
    pub width: u32,
    pub height: u32,
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub av1c: Option<Vec<u8>>,
}

impl CodecConfig {
    pub fn h264(width: u32, height: u32, sps: Vec<u8>, pps: Vec<u8>) -> Self {
        Self { width, height, sps, pps, av1c: None }
    }

    pub fn av1(width: u32, height: u32, av1c: Vec<u8>) -> Self {
        Self { width, height, sps: Vec::new(), pps: Vec::new(), av1c: Some(av1c) }
    }
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

pub mod av1;
pub mod h264;
pub mod jpeg;
pub mod metadata;
pub mod proto;
#[cfg(unix)]
pub mod ipc;

pub mod mux {
    //! MP4 muxing (pure Rust, `mp4` crate). Runs at save time only.
    use super::{AudioConfig, CodecConfig, Packet, AAC_FRAME_SAMPLES};
    use std::io::{BufWriter, Read, Seek, SeekFrom, Write};

    /// Media timescale for the video track.
    const TIMESCALE: u32 = 90_000;

    /// Write `packets` (as returned by `Ring::snapshot`, so starting on a keyframe) as an MP4
    /// with one H.264 track. AVCC packet bytes pass through unchanged; SPS/PPS go in `avcC`.
    ///
    /// Timestamps are rebased so the first sample is at 0 (capture pts are host-clock values
    /// and the writer emits no edit list). Each sample's duration is the gap to the next one;
    /// the last sample reuses the previous gap.
    pub fn write_mp4<W: Read + Write + Seek>(w: W, cfg: &CodecConfig, packets: &[Packet]) -> Result<(), String> {
        write_mp4_av(w, cfg, packets, None, None)
    }

    /// UUID identifying a Discord-format clip in its top-level `uuid` box (interface note, see
    /// REFERENCE.md). The client rejects `.mp4` files without this box.
    pub const DISCORD_UUID: [u8; 16] =
        [0xa1, 0xc8, 0x52, 0x99, 0x33, 0x46, 0x4d, 0xb8, 0x88, 0xf0, 0x83, 0xf5, 0x7a, 0x75, 0xa5, 0xef];

    /// A top-level `uuid` box: size(4) "uuid"(4) UUID(16) then the JSON metadata bytes.
    pub fn discord_uuid_box(metadata_json: &[u8]) -> Vec<u8> {
        let mut b = Vec::with_capacity(24 + metadata_json.len());
        b.extend_from_slice(&((24 + metadata_json.len()) as u32).to_be_bytes());
        b.extend_from_slice(b"uuid");
        b.extend_from_slice(&DISCORD_UUID);
        b.extend_from_slice(metadata_json);
        b
    }

    /// Like [`write_mp4`], plus an optional AAC-LC audio track. Audio packets are placed on the
    /// same (rebased) timeline as the video; those ending before the first video sample are
    /// dropped. Samples are written interleaved by time. If `discord_metadata` (JSON bytes) is
    /// given, a Discord `uuid` box carrying it is appended as the last top-level box.
    pub fn write_mp4_av<W: Read + Write + Seek>(
        mut w: W,
        cfg: &CodecConfig,
        packets: &[Packet],
        audio: Option<(&AudioConfig, &[Packet])>,
        discord_metadata: Option<&[u8]>,
    ) -> Result<(), String> {
        use mp4::{
            AacConfig, AudioObjectType, AvcConfig, ChannelConfig, FourCC, MediaConfig, Mp4Config, Mp4Sample,
            Mp4Writer, SampleFreqIndex, TrackConfig, TrackType,
        };

        let first = packets.first().ok_or("no packets to write")?;
        if !first.keyframe {
            return Err("first packet must be a keyframe".into());
        }
        let is_av1 = cfg.av1c.as_ref().is_some_and(|v| v.len() >= 4);
        if !is_av1 && (cfg.sps.len() < 4 || cfg.pps.is_empty()) {
            return Err("codec config missing SPS/PPS (or av1C)".into());
        }
        let e = |e: mp4::Error| e.to_string();

        let compat = if is_av1 {
            [*b"isom", *b"iso2", *b"av01", *b"mp41"]
        } else {
            [*b"isom", *b"iso2", *b"avc1", *b"mp41"]
        };

        let mut mw = Mp4Writer::write_start(
            BufWriter::new(&mut w),
            &Mp4Config {
                major_brand: FourCC { value: *b"isom" },
                minor_version: 512,
                compatible_brands: compat.map(|value| FourCC { value }).to_vec(),
                // Must equal the track timescale: mp4 truncates each sample's duration when
                // converting to the movie timescale, which shortens the movie (~0.3 ms/frame
                // at 1000) and makes players cut off the end of the clip.
                timescale: TIMESCALE,
            },
        )
        .map_err(e)?;
        // `mp4` crate v0.14.0 has no `av01`/`av1C` writer; for AV1 we write a placeholder `avc1`
        // entry and replace it with a spec-compliant `av01`+`av1C` box inside `moov` after
        // `write_end()` (since `moov` sits after `mdat`, sample offsets in `stco` stay identical).
        let (sps, pps) = if is_av1 {
            (vec![0x67, 0x64, 0x00, 0x28], vec![0x68])
        } else {
            (cfg.sps.clone(), cfg.pps.clone())
        };
        mw.add_track(&TrackConfig {
            track_type: TrackType::Video,
            timescale: TIMESCALE,
            language: "und".into(),
            media_conf: MediaConfig::AvcConfig(AvcConfig {
                width: cfg.width as u16,
                height: cfg.height as u16,
                seq_param_set: sps,
                pic_param_set: pps,
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
        mw.write_end().map_err(e)?;
        let mut bw = mw.into_writer();
        bw.flush().map_err(|e| e.to_string())?;
        drop(bw);
        if audio_track.is_some() || is_av1 {
            patch_moov_boxes(&mut w, audio_track.is_some(), cfg)?;
        }
        if let Some(json) = discord_metadata {
            w.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
            w.write_all(&discord_uuid_box(json)).map_err(|e| e.to_string())?;
            w.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Post-process the trailing `moov` box written by `mp4` v0.14.0:
    /// 1. Fix `esds`'s `SLConfigDescriptor` (`[0x06, 0x00, 0x00]` -> `[0x06, 0x01, 0x02]`) so
    ///    strict ISO 14496-1 parsers like Discord's `mp4box.js` don't throw `RangeError`.
    /// 2. For AV1 clips, replace the placeholder `avc1` sample entry in `stsd` with a proper
    ///    `av01` + `av1C` `VisualSampleEntry` and update ancestor container box sizes.
    fn patch_moov_boxes<S: Read + Write + Seek>(s: &mut S, fix_esds: bool, cfg: &CodecConfig) -> Result<(), String> {
        let total = s.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        let mut pos = 0u64;
        let mut hdr = [0u8; 8];
        while pos + 8 <= total {
            s.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
            s.read_exact(&mut hdr).map_err(|e| e.to_string())?;
            let size = u32::from_be_bytes(hdr[0..4].try_into().unwrap()) as u64;
            if size < 8 || pos + size > total {
                break;
            }
            if &hdr[4..8] == b"moov" {
                let mut moov = vec![0u8; size as usize];
                s.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
                s.read_exact(&mut moov).map_err(|e| e.to_string())?;
                let mut changed = false;
                if fix_esds {
                    changed |= fix_esds_in_container(&mut moov, 8, size as usize);
                }
                if let Some(av1c) = cfg.av1c.as_deref() {
                    let av01 = build_av01_sample_entry(cfg.width as u16, cfg.height as u16, av1c);
                    if let Some(rebuilt) = rewrite_avc1_to_av01(&moov, &av01) {
                        moov = rebuilt;
                        changed = true;
                    }
                }
                if changed {
                    // `moov` is at the end of the file (before `uuid` is appended), so rewriting
                    // it with a slightly different size never affects `mdat`/`stco` offsets.
                    let tail = if pos + size < total {
                        let mut rest = vec![0u8; (total - (pos + size)) as usize];
                        s.seek(SeekFrom::Start(pos + size)).map_err(|e| e.to_string())?;
                        s.read_exact(&mut rest).map_err(|e| e.to_string())?;
                        rest
                    } else {
                        Vec::new()
                    };
                    s.seek(SeekFrom::Start(pos)).map_err(|e| e.to_string())?;
                    s.write_all(&moov).map_err(|e| e.to_string())?;
                    if !tail.is_empty() {
                        s.write_all(&tail).map_err(|e| e.to_string())?;
                    }
                    // If `moov` shrank, pad any leftover trailing bytes with a `free` box so no
                    // stale bytes remain past `moov` (needed because `Seek` trait cannot truncate).
                    let written_end = pos + moov.len() as u64 + tail.len() as u64;
                    if written_end < total {
                        let pad = (total - written_end) as usize;
                        if pad >= 8 {
                            let mut free = vec![0u8; pad];
                            free[0..4].copy_from_slice(&(pad as u32).to_be_bytes());
                            free[4..8].copy_from_slice(b"free");
                            s.write_all(&free).map_err(|e| e.to_string())?;
                        } else {
                            // Append an 8-byte `free` box inside `moov`? Easier: grow `av01` padding
                            // is avoided because `av01`+`av1C` is checked against placeholder size;
                            // if pad < 8, emit an 8-byte `free` box (extends file by 8-pad bytes).
                            let mut free = vec![0u8; 8];
                            free[0..4].copy_from_slice(&8u32.to_be_bytes());
                            free[4..8].copy_from_slice(b"free");
                            s.write_all(&free).map_err(|e| e.to_string())?;
                        }
                    }
                }
                return Ok(());
            }
            pos += size;
        }
        Ok(())
    }

    /// Build a spec-compliant ISO/IEC 14496-12 + AV1-ISOBMFF `av01` VisualSampleEntry containing
    /// an `av1C` child box.
    fn build_av01_sample_entry(width: u16, height: u16, av1c: &[u8]) -> Vec<u8> {
        let av1c_box_len = 8 + av1c.len();
        let total = 86 + av1c_box_len;
        let mut out = vec![0u8; total];
        out[0..4].copy_from_slice(&(total as u32).to_be_bytes());
        out[4..8].copy_from_slice(b"av01");
        // data_reference_index = 1
        out[14..16].copy_from_slice(&1u16.to_be_bytes());
        out[32..34].copy_from_slice(&width.to_be_bytes());
        out[34..36].copy_from_slice(&height.to_be_bytes());
        // horiz/vert resolution = 72 dpi (0x00480000)
        out[36..40].copy_from_slice(&0x0048_0000u32.to_be_bytes());
        out[40..44].copy_from_slice(&0x0048_0000u32.to_be_bytes());
        // frame_count = 1
        out[48..50].copy_from_slice(&1u16.to_be_bytes());
        // depth = 24, pre_defined = -1
        out[82..84].copy_from_slice(&0x0018u16.to_be_bytes());
        out[84..86].copy_from_slice(&0xFFFFu16.to_be_bytes());
        // child `av1C` box
        out[86..90].copy_from_slice(&(av1c_box_len as u32).to_be_bytes());
        out[90..94].copy_from_slice(b"av1C");
        out[94..].copy_from_slice(av1c);
        out
    }

    /// Recursively rebuild a container box (`moov` -> `trak` -> `mdia` -> `minf` -> `stbl` -> `stsd`),
    /// replacing any `avc1` sample entry with `av01_entry` and updating all ancestor box sizes.
    fn rewrite_avc1_to_av01(box_bytes: &[u8], av01_entry: &[u8]) -> Option<Vec<u8>> {
        if box_bytes.len() < 8 {
            return None;
        }
        let btype = &box_bytes[4..8];
        if btype == b"stsd" {
            if box_bytes.len() < 16 {
                return None;
            }
            let mut body = Vec::with_capacity(box_bytes.len() + av01_entry.len());
            body.extend_from_slice(&box_bytes[8..16]); // version/flags + entry_count
            let mut i = 16usize;
            let mut replaced = false;
            while i + 8 <= box_bytes.len() {
                let sz = u32::from_be_bytes(box_bytes[i..i + 4].try_into().unwrap()) as usize;
                if sz < 8 || i + sz > box_bytes.len() {
                    break;
                }
                if &box_bytes[i + 4..i + 8] == b"avc1" {
                    body.extend_from_slice(av01_entry);
                    replaced = true;
                } else {
                    body.extend_from_slice(&box_bytes[i..i + sz]);
                }
                i += sz;
            }
            if !replaced {
                return None;
            }
            let mut out = Vec::with_capacity(8 + body.len());
            out.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
            out.extend_from_slice(b"stsd");
            out.extend_from_slice(&body);
            return Some(out);
        }
        if !matches!(btype, b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl") {
            return None;
        }
        let mut body = Vec::with_capacity(box_bytes.len());
        let mut i = 8usize;
        let mut changed = false;
        while i + 8 <= box_bytes.len() {
            let sz = u32::from_be_bytes(box_bytes[i..i + 4].try_into().unwrap()) as usize;
            if sz < 8 || i + sz > box_bytes.len() {
                body.extend_from_slice(&box_bytes[i..]);
                break;
            }
            let child = &box_bytes[i..i + sz];
            if let Some(rebuilt) = rewrite_avc1_to_av01(child, av01_entry) {
                body.extend_from_slice(&rebuilt);
                changed = true;
            } else {
                body.extend_from_slice(child);
            }
            i += sz;
        }
        if !changed {
            return None;
        }
        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
        out.extend_from_slice(btype);
        out.extend_from_slice(&body);
        Some(out)
    }

    fn fix_esds_in_container(buf: &mut [u8], mut i: usize, end: usize) -> bool {
        let mut changed = false;
        while i + 8 <= end {
            let size = u32::from_be_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
            if size < 8 || i + size > end {
                break;
            }
            let btype = &buf[i + 4..i + 8];
            let box_end = i + size;
            if matches!(btype, b"trak" | b"mdia" | b"minf" | b"stbl") {
                changed |= fix_esds_in_container(buf, i + 8, box_end);
            } else if btype == b"stsd" && i + 16 <= box_end {
                let mut e = i + 16;
                while e + 8 <= box_end {
                    let esize = u32::from_be_bytes(buf[e..e + 4].try_into().unwrap()) as usize;
                    if esize < 8 || e + esize > box_end {
                        break;
                    }
                    if &buf[e + 4..e + 8] == b"mp4a" && e + 36 <= e + esize {
                        changed |= fix_esds_in_container(buf, e + 36, e + esize);
                    }
                    e += esize;
                }
            } else if btype == b"esds" && size >= 15 && &buf[box_end - 3..box_end] == [0x06, 0x00, 0x00] {
                buf[box_end - 2] = 0x01;
                buf[box_end - 1] = 0x02;
                changed = true;
            }
            i = box_end;
        }
        changed
    }

    /// Mirror of the client's clip check (interface notes): `ftypisom` signature, then walk the
    /// top-level boxes (32-bit sizes, >= 8) until the Discord `uuid` box; returns its JSON body.
    /// Each box position must have 24 readable bytes, as the client reads a 24-byte header there.
    pub fn read_discord_metadata(file: &[u8]) -> Result<Vec<u8>, String> {
        if file.len() < 24 || &file[4..12] != b"ftypisom" {
            return Err("not-mp4-signature".into());
        }
        let mut i = u32::from_be_bytes(file[0..4].try_into().unwrap()) as usize;
        while i < file.len() {
            if i + 24 > file.len() {
                return Err("short read".into());
            }
            let size = u32::from_be_bytes(file[i..i + 4].try_into().unwrap()) as usize;
            if size < 8 {
                return Err("malformed-box".into());
            }
            if &file[i + 4..i + 8] == b"uuid" && file[i + 8..i + 24] == DISCORD_UUID {
                return Ok(file[i + 24..(i + size).min(file.len())].to_vec());
            }
            i += size;
        }
        Err("no-discord-uuid-box".into())
    }

    /// Replace (or append) the top-level Discord `uuid` metadata box in an existing MP4 file,
    /// writing atomically via a `.part` temp file and rename.
    pub fn update_discord_metadata(path: &std::path::Path, metadata_json: &[u8]) -> Result<(), String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if data.len() < 24 || &data[4..12] != b"ftypisom" {
            return Err("not-mp4-signature".into());
        }
        let mut i = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
        let mut found: Option<(usize, usize)> = None;
        while i < data.len() {
            if i + 8 > data.len() {
                return Err("malformed-box".into());
            }
            let size = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
            if size < 8 || i + size > data.len() {
                return Err("malformed-box".into());
            }
            if i + 24 <= data.len() && &data[i + 4..i + 8] == b"uuid" && data[i + 8..i + 24] == DISCORD_UUID {
                found = Some((i, size));
                break;
            }
            i += size;
        }
        let new_box = discord_uuid_box(metadata_json);
        let tmp = path.with_extension("mp4.meta.part");
        {
            let mut f = std::io::BufWriter::new(
                std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?,
            );
            match found {
                Some((pos, size)) => {
                    f.write_all(&data[..pos]).map_err(|e| e.to_string())?;
                    f.write_all(&new_box).map_err(|e| e.to_string())?;
                    f.write_all(&data[pos + size..]).map_err(|e| e.to_string())?;
                }
                None => {
                    f.write_all(&data).map_err(|e| e.to_string())?;
                    f.write_all(&new_box).map_err(|e| e.to_string())?;
                }
            }
            f.flush().map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("{}: {e}", path.display())
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn roundtrip_samples_sync_flags_and_rebased_times() {
            let cfg = CodecConfig::h264(640, 480, vec![0x67, 0x64, 0x00, 0x28, 0xac], vec![0x68, 0xee, 0x3c, 0x80]);
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
            let cfg = CodecConfig::h264(640, 480, vec![0x67, 0x64, 0x00, 0x28, 0xac], vec![0x68, 0xee, 0x3c, 0x80]);
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
            write_mp4_av(&mut buf, &cfg, &vid, Some((&ac, &aud)), Some(br#"{"game":"x"}"#)).unwrap();
            let bytes = buf.get_ref().clone();
            assert_eq!(read_discord_metadata(&bytes).unwrap(), br#"{"game":"x"}"#);
            // Verify the esds SLConfigDescriptor workaround produced spec-compliant [06, 01, 02]
            let esds_pos = bytes.windows(4).position(|w| w == b"esds").expect("esds box present");
            let esds_size = u32::from_be_bytes(bytes[esds_pos - 4..esds_pos].try_into().unwrap()) as usize;
            let esds_end = esds_pos - 4 + esds_size;
            assert_eq!(&bytes[esds_end - 3..esds_end], &[0x06, 0x01, 0x02]);
            // still a valid MP4 for the standard reader with the trailing box present
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
        fn plain_mp4_has_no_discord_box() {
            let cfg = CodecConfig::h264(64, 64, vec![0x67, 0x64, 0, 0x28], vec![0x68]);
            let pk = [Packet { pts_us: 0, keyframe: true, data: vec![0, 0, 0, 1, 0x65] }];
            let mut buf = std::io::Cursor::new(Vec::new());
            write_mp4(&mut buf, &cfg, &pk).unwrap();
            assert_eq!(read_discord_metadata(buf.get_ref()).unwrap_err(), "no-discord-uuid-box");
        }

        #[test]
        fn rejects_non_keyframe_start() {
            let cfg = CodecConfig::h264(2, 2, vec![0x67, 0x64, 0, 0x28], vec![0x68]);
            let pk = [Packet { pts_us: 0, keyframe: false, data: vec![] }];
            assert!(write_mp4(std::io::Cursor::new(Vec::new()), &cfg, &pk).is_err());
        }

        #[test]
        fn update_discord_metadata_replaces_uuid_box() {
            let cfg = CodecConfig::h264(64, 64, vec![0x67, 0x64, 0, 0x28], vec![0x68]);
            let pk = [Packet { pts_us: 0, keyframe: true, data: vec![0, 0, 0, 1, 0x65] }];
            let tmp = std::env::temp_dir().join(format!("clipsd-meta-test-{}.mp4", std::process::id()));
            {
                let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&tmp).unwrap();
                write_mp4_av(f, &cfg, &pk, None, Some(br#"{"id":"1","name":"old"}"#)).unwrap();
            }
            update_discord_metadata(&tmp, br#"{"id":"1","name":"updated","length":5000}"#).unwrap();
            let bytes = std::fs::read(&tmp).unwrap();
            let _ = std::fs::remove_file(&tmp);
            assert_eq!(read_discord_metadata(&bytes).unwrap(), br#"{"id":"1","name":"updated","length":5000}"#);
        }

        #[test]
        fn av1_av_roundtrip_writes_av01_av1c_esds_and_discord_uuid() {
            let av1c = vec![0x81, 0x0D, 0x0C, 0x00, 0x0A, 0x04, 0x0A, 0x00, 0x00, 0x00];
            let cfg = CodecConfig::av1(1920, 1080, av1c.clone());
            let ac = AudioConfig { sample_rate: 48_000, channels: 2, bitrate_bps: 128_000 };
            let vid = vec![
                Packet { pts_us: 0, keyframe: true, data: vec![0x0A, 0x04, 0x0A, 0x00, 0x00, 0x00, 0x32, 0x02, 0x00, 0xAA] },
                Packet { pts_us: 33_333, keyframe: false, data: vec![0x32, 0x02, 0x20, 0xBB] },
            ];
            let aud = vec![
                Packet { pts_us: 0, keyframe: true, data: vec![0x21, 0x00, 0x01, 0x02] },
            ];
            let mut buf = std::io::Cursor::new(Vec::new());
            write_mp4_av(&mut buf, &cfg, &vid, Some((&ac, &aud)), Some(br#"{"id":"av1-test"}"#)).unwrap();
            let bytes = buf.into_inner();
            if let Ok(p) = std::env::var("CLIPSD_TEST_DUMP_AV1") {
                std::fs::write(p, &bytes).unwrap();
            }
            assert_eq!(read_discord_metadata(&bytes).unwrap(), br#"{"id":"av1-test"}"#);
            assert!(bytes.windows(4).any(|w| w == b"av01"), "av01 sample entry present");
            assert!(bytes.windows(4).any(|w| w == b"av1C"), "av1C config box present");
            assert!(!bytes.windows(4).any(|w| w == b"avc1"), "placeholder avc1 must be gone");
            let esds_pos = bytes.windows(4).position(|w| w == b"esds").expect("esds present");
            let esds_sz = u32::from_be_bytes(bytes[esds_pos - 4..esds_pos].try_into().unwrap()) as usize;
            assert_eq!(&bytes[esds_pos - 4 + esds_sz - 3..esds_pos - 4 + esds_sz], &[0x06, 0x01, 0x02]);
        }
    }
}
