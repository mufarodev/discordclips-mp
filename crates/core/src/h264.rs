//! Portable H.264 Annex-B ↔ AVCC conversion and SPS/PPS extraction.
//!
//! Hardware encoders on macOS (VideoToolbox) already emit AVCC (4-byte length-prefixed NALs)
//! plus out-of-band SPS/PPS. Linux encoders via `libavcodec` (`h264_vaapi`, `h264_nvenc`,
//! `libx264`) emit Annex-B (`00 00 00 01` / `00 00 01` start codes) with inline or extradata
//! SPS/PPS. This module converts Annex-B packets into the AVCC `Packet` + `CodecConfig` shape
//! expected by `clipsd_core::mux`.

/// Result of converting one Annex-B H.264 access unit into AVCC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvccFrame {
    /// 4-byte big-endian length-prefixed NAL units (excluding AUD/SPS/PPS parameter sets).
    pub data: Vec<u8>,
    /// True if the access unit contains an IDR slice (NAL unit type 5).
    pub keyframe: bool,
    /// Inline SPS (NAL type 7), if present in this access unit.
    pub sps: Option<Vec<u8>>,
    /// Inline PPS (NAL type 8), if present in this access unit.
    pub pps: Option<Vec<u8>>,
}

/// Split an Annex-B H.264 byte stream into raw NAL units (start codes stripped).
pub fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0usize;
    let mut nal_start: Option<usize> = None;

    while i < data.len() {
        let sc_len = if i + 3 < data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1 {
            Some(4)
        } else if i + 2 < data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            Some(3)
        } else {
            None
        };

        if let Some(len) = sc_len {
            if let Some(start) = nal_start {
                let slice = trim_trailing_zeros(&data[start..i]);
                if !slice.is_empty() {
                    nals.push(slice);
                }
            }
            i += len;
            nal_start = Some(i);
        } else {
            i += 1;
        }
    }

    if let Some(start) = nal_start {
        let slice = trim_trailing_zeros(&data[start..]);
        if !slice.is_empty() {
            nals.push(slice);
        }
    }
    nals
}

fn trim_trailing_zeros(mut s: &[u8]) -> &[u8] {
    while s.last() == Some(&0) {
        s = &s[..s.len() - 1];
    }
    s
}

/// Convert one Annex-B H.264 packet into 4-byte length-prefixed AVCC plus extracted SPS/PPS
/// and IDR keyframe flag.
pub fn annex_b_to_avcc(data: &[u8]) -> AvccFrame {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut keyframe = false;
    let mut sps = None;
    let mut pps = None;

    for nal in split_annex_b(data) {
        let nal_type = nal[0] & 0x1F;
        match nal_type {
            7 => sps = Some(nal.to_vec()),
            8 => pps = Some(nal.to_vec()),
            9 => {} // Access Unit Delimiter — omitted in MP4 AVCC samples
            _ => {
                if nal_type == 5 {
                    keyframe = true;
                }
                out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                out.extend_from_slice(nal);
            }
        }
    }

    AvccFrame { data: out, keyframe, sps, pps }
}

/// Extract `(sps, pps)` from `libavcodec` extradata, supporting both Annex-B and `avcC` layouts.
pub fn parse_extradata_sps_pps(extra: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    if extra.is_empty() {
        return None;
    }
    // AVCDecoderConfigurationRecord (`avcC`) starts with configurationVersion == 1.
    if extra.len() >= 11 && extra[0] == 1 {
        let num_sps = (extra[5] & 0x1F) as usize;
        let mut pos = 6usize;
        let mut sps = None;
        for _ in 0..num_sps {
            if pos + 2 > extra.len() {
                return None;
            }
            let len = u16::from_be_bytes([extra[pos], extra[pos + 1]]) as usize;
            pos += 2;
            if pos + len > extra.len() {
                return None;
            }
            if sps.is_none() && len > 0 {
                sps = Some(extra[pos..pos + len].to_vec());
            }
            pos += len;
        }
        if pos >= extra.len() {
            return None;
        }
        let num_pps = extra[pos] as usize;
        pos += 1;
        let mut pps = None;
        for _ in 0..num_pps {
            if pos + 2 > extra.len() {
                return None;
            }
            let len = u16::from_be_bytes([extra[pos], extra[pos + 1]]) as usize;
            pos += 2;
            if pos + len > extra.len() {
                return None;
            }
            if pps.is_none() && len > 0 {
                pps = Some(extra[pos..pos + len].to_vec());
            }
            pos += len;
        }
        return Some((sps?, pps?));
    }

    // Otherwise parse as Annex-B.
    let f = annex_b_to_avcc(extra);
    Some((f.sps?, f.pps?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_mixed_start_codes_and_extracts_sps_pps_idr() {
        let stream = [
            0, 0, 0, 1, 0x09, 0x10, // AUD (dropped)
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xac, // SPS (type 7)
            0, 0, 1, 0x68, 0xee, 0x3c, 0x80, // PPS (type 8)
            0, 0, 0, 1, 0x65, 0x88, 0x84, 0x21, // IDR (type 5)
        ];
        let f = annex_b_to_avcc(&stream);
        assert!(f.keyframe);
        assert_eq!(f.sps.as_deref(), Some(&[0x67, 0x64, 0x00, 0x28, 0xac][..]));
        assert_eq!(f.pps.as_deref(), Some(&[0x68, 0xee, 0x3c, 0x80][..]));
        assert_eq!(f.data, &[0, 0, 0, 4, 0x65, 0x88, 0x84, 0x21]);
    }

    #[test]
    fn non_idr_p_frame_is_not_keyframe() {
        let stream = [0, 0, 1, 0x41, 0x9a, 0x01, 0x02];
        let f = annex_b_to_avcc(&stream);
        assert!(!f.keyframe);
        assert!(f.sps.is_none() && f.pps.is_none());
        assert_eq!(f.data, &[0, 0, 0, 4, 0x41, 0x9a, 0x01, 0x02]);
    }

    #[test]
    fn parses_avcc_and_annex_b_extradata() {
        let avcc = [
            0x01, 0x64, 0x00, 0x2a, 0xff, 0xe1, 0x00, 0x04, 0x67, 0x64, 0x00, 0x2a,
            0x01, 0x00, 0x03, 0x68, 0xee, 0x3c,
        ];
        let (sps, pps) = parse_extradata_sps_pps(&avcc).unwrap();
        assert_eq!(sps, &[0x67, 0x64, 0x00, 0x2a]);
        assert_eq!(pps, &[0x68, 0xee, 0x3c]);

        let annex_b = [0, 0, 0, 1, 0x67, 0x64, 0x00, 0x2a, 0, 0, 0, 1, 0x68, 0xee, 0x3c];
        assert_eq!(parse_extradata_sps_pps(&annex_b).unwrap(), (sps, pps));
    }
}
