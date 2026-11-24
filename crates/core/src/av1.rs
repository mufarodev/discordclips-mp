//! AV1 OBU parsing, temporal-delimiter stripping, and `av1C` box construction.

pub const OBU_SEQUENCE_HEADER: u8 = 1;
pub const OBU_TEMPORAL_DELIMITER: u8 = 2;
pub const OBU_FRAME_HEADER: u8 = 3;
pub const OBU_FRAME: u8 = 6;
pub const OBU_PADDING: u8 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Obu<'a> {
    pub obu_type: u8,
    pub raw: &'a [u8],
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Av1Sample {
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub seq_header: Option<Vec<u8>>,
}

fn read_leb128(buf: &[u8]) -> Option<(usize, usize)> {
    let mut val = 0usize;
    for (i, &b) in buf.iter().take(8).enumerate() {
        val |= ((b & 0x7F) as usize) << (i * 7);
        if b & 0x80 == 0 {
            return Some((val, i + 1));
        }
    }
    None
}

pub fn split_obus(data: &[u8]) -> Vec<Obu<'_>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let start = i;
        let hdr = data[i];
        if hdr & 0x80 != 0 {
            break; // forbidden bit must be 0
        }
        let obu_type = (hdr >> 3) & 0x0F;
        let has_ext = (hdr & 0x04) != 0;
        let has_size = (hdr & 0x02) != 0;
        i += 1;
        if has_ext {
            if i >= data.len() {
                break;
            }
            i += 1;
        }
        let (payload_len, end) = if has_size {
            let Some((sz, n)) = read_leb128(&data[i..]) else { break };
            i += n;
            let end = i.saturating_add(sz).min(data.len());
            (end - i, end)
        } else {
            (data.len() - i, data.len())
        };
        let payload = &data[i..i + payload_len];
        out.push(Obu { obu_type, raw: &data[start..end], payload });
        i = end;
    }
    out
}

/// Prepare one AV1 access unit (from `av1_nvenc` / `libsvtav1` / `libaom`) for MP4 muxing:
/// strips `OBU_TEMPORAL_DELIMITER` and `OBU_PADDING`, captures any inline `OBU_SEQUENCE_HEADER`,
/// and detects keyframes.
pub fn prepare_av1_sample(data: &[u8]) -> Av1Sample {
    let mut out = Vec::with_capacity(data.len());
    let mut keyframe = false;
    let mut seq_header = None;

    for obu in split_obus(data) {
        match obu.obu_type {
            OBU_TEMPORAL_DELIMITER | OBU_PADDING => continue,
            OBU_SEQUENCE_HEADER => {
                keyframe = true;
                if seq_header.is_none() {
                    seq_header = Some(obu.raw.to_vec());
                }
                out.extend_from_slice(obu.raw);
            }
            OBU_FRAME_HEADER | OBU_FRAME => {
                // Uncompressed frame header: show_existing_frame (1b); if 0, frame_type (2b, 0 == KEY_FRAME).
                if let Some(&b0) = obu.payload.first() {
                    let show_existing = (b0 >> 7) & 1;
                    let frame_type = (b0 >> 5) & 3;
                    if show_existing == 0 && frame_type == 0 {
                        keyframe = true;
                    }
                }
                out.extend_from_slice(obu.raw);
            }
            _ => out.extend_from_slice(obu.raw),
        }
    }

    Av1Sample { data: out, keyframe, seq_header }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }
    fn read_bits(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte_idx = self.bit_pos / 8;
            if byte_idx >= self.data.len() {
                return None;
            }
            let shift = 7 - (self.bit_pos % 8);
            v = (v << 1) | (((self.data[byte_idx] >> shift) & 1) as u32);
            self.bit_pos += 1;
        }
        Some(v)
    }
    fn read_bit(&mut self) -> Option<bool> {
        Some(self.read_bits(1)? != 0)
    }
    fn read_uvlc(&mut self) -> Option<u32> {
        let mut zeros = 0usize;
        while !self.read_bit()? {
            zeros += 1;
            if zeros >= 32 {
                return Some(u32::MAX);
            }
        }
        if zeros == 0 {
            return Some(0);
        }
        let val = self.read_bits(zeros)?;
        Some(val + (1u32 << zeros) - 1)
    }
}

/// Build an ISO/IEC AV1 `av1C` box payload from either:
/// - `libavcodec` extradata that is already an `av1C` record (byte 0 == `0x81`, len >= 4), or
/// - a raw `OBU_SEQUENCE_HEADER` OBU (extracting profile/level/tier and embedding the OBU in `configOBUs`).
pub fn build_av1c(input: &[u8]) -> Option<Vec<u8>> {
    // Already an AV1CodecConfigurationRecord (marker=1, version=1 => 0x81)?
    if input.len() >= 4 && input[0] == 0x81 {
        return Some(input.to_vec());
    }
    let seq_obu = split_obus(input).into_iter().find(|o| o.obu_type == OBU_SEQUENCE_HEADER)?;
    let mut br = BitReader::new(seq_obu.payload);

    let seq_profile = br.read_bits(3)? as u8;
    let _still_picture = br.read_bit()?;
    let reduced_still = br.read_bit()?;

    let (seq_level_idx_0, seq_tier_0) = if reduced_still {
        (br.read_bits(5)? as u8, 0u8)
    } else {
        let timing_info_present = br.read_bit()?;
        let mut decoder_model_info_present = false;
        let mut buffer_delay_length_minus_1 = 0usize;
        if timing_info_present {
            let _num_units = br.read_bits(32)?;
            let _time_scale = br.read_bits(32)?;
            let equal_pic_interval = br.read_bit()?;
            if equal_pic_interval {
                let _ = br.read_uvlc()?;
            }
            decoder_model_info_present = br.read_bit()?;
            if decoder_model_info_present {
                buffer_delay_length_minus_1 = br.read_bits(5)? as usize;
                let _num_units_in_tick = br.read_bits(32)?;
                let _buffer_removal_len = br.read_bits(5)?;
                let _frame_pres_len = br.read_bits(5)?;
            }
        }
        let initial_display_delay_present = br.read_bit()?;
        let op_cnt = br.read_bits(5)? as usize;
        let mut lvl0 = 0u8;
        let mut tier0 = 0u8;
        for idx in 0..=op_cnt {
            let _idc = br.read_bits(12)?;
            let lvl = br.read_bits(5)? as u8;
            let tier = if lvl > 7 { br.read_bits(1)? as u8 } else { 0 };
            if idx == 0 {
                lvl0 = lvl;
                tier0 = tier;
            }
            if decoder_model_info_present && br.read_bit()? {
                let n = buffer_delay_length_minus_1 + 1;
                let _ = br.read_bits(n)?;
                let _ = br.read_bits(n)?;
                let _ = br.read_bit()?;
            }
            if initial_display_delay_present && br.read_bit()? {
                let _ = br.read_bits(4)?;
            }
        }
        (lvl0, tier0)
    };

    // Default 8-bit 4:2:0 flags (standard Main profile NV12 output from av1_nvenc);
    // high_bitdepth=0, twelve_bit=0, monochrome=0, subsampling_x=1, subsampling_y=1, chroma_pos=0.
    let byte0 = 0x81u8; // marker(1)=1, version(7)=1
    let byte1 = ((seq_profile & 0x07) << 5) | (seq_level_idx_0 & 0x1F);
    let byte2 = ((seq_tier_0 & 0x01) << 7) | 0b0000_1100; // subsampling_x=1, subsampling_y=1
    let byte3 = 0x00u8; // initial_presentation_delay_present=0

    let mut av1c = Vec::with_capacity(4 + seq_obu.raw.len());
    av1c.extend_from_slice(&[byte0, byte1, byte2, byte3]);
    av1c.extend_from_slice(seq_obu.raw);
    Some(av1c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_temporal_delimiter_and_detects_keyframe() {
        // OBU_TEMPORAL_DELIMITER (type 2, has_size=1, size=0): [0x12, 0x00]
        // OBU_SEQUENCE_HEADER (type 1, has_size=1, size=4): reduced_still=1, profile=0, level=8 (0b000_0_1_01000 = 0x0A, 0x00...)
        // OBU_FRAME (type 6, has_size=1, size=3): show_existing=0, frame_type=0 (keyframe)
        let stream = [
            0x12, 0x00, // temporal delimiter
            0x0A, 0x04, 0x0A, 0x00, 0x00, 0x00, // sequence header (profile 0, reduced_still=1, level=8)
            0x32, 0x03, 0x00, 0x11, 0x22, // key frame
        ];
        let s = prepare_av1_sample(&stream);
        assert!(s.keyframe);
        assert_eq!(s.seq_header.as_deref(), Some(&[0x0A, 0x04, 0x0A, 0x00, 0x00, 0x00][..]));
        assert_eq!(s.data, &[0x0A, 0x04, 0x0A, 0x00, 0x00, 0x00, 0x32, 0x03, 0x00, 0x11, 0x22]);

        let av1c = build_av1c(&s.seq_header.unwrap()).unwrap();
        assert_eq!(av1c[0], 0x81);
        assert_eq!(av1c[1], 8); // profile 0, level 8
        assert_eq!(&av1c[4..], &[0x0A, 0x04, 0x0A, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn passes_through_existing_av1c_extradata() {
        let existing = [0x81, 0x0D, 0x0C, 0x00, 0x0A, 0x02, 0x00, 0x00];
        assert_eq!(build_av1c(&existing).unwrap(), existing);
    }

    #[test]
    fn detects_non_keyframe_inter_frame() {
        // OBU_FRAME (type 6, has_size=1, size=2): show_existing=0, frame_type=1 (INTER_FRAME => 0b0010_0000 = 0x20)
        let stream = [0x12, 0x00, 0x32, 0x02, 0x20, 0xFF];
        let s = prepare_av1_sample(&stream);
        assert!(!s.keyframe);
        assert!(s.seq_header.is_none());
        assert_eq!(s.data, &[0x32, 0x02, 0x20, 0xFF]);
    }
}
