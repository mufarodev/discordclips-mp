//! Portable JPEG COM metadata splicing and RFC 4648 base64 encoding.

/// Insert a JPEG COM (comment) segment right after SOI. Discord-format screenshots carry their
/// metadata as JSON in this segment (interface note, see REFERENCE.md).
pub fn jpeg_with_comment(jpeg: &[u8], comment: &str) -> Result<Vec<u8>, String> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err("not a JPEG".into());
    }
    let len = comment.len() + 2;
    if len > u16::MAX as usize {
        return Err("JPEG comment too long".into());
    }
    let mut out = Vec::with_capacity(jpeg.len() + comment.len() + 4);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xFF, 0xFE]);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(comment.as_bytes());
    out.extend_from_slice(&jpeg[2..]);
    Ok(out)
}

/// Standard base64 encoding (RFC 4648) with no external dependency.
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 0x3f) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 0x3f) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_segment_follows_soi() {
        let j = [0xFF, 0xD8, 0xFF, 0xE0, 0, 2];
        let o = jpeg_with_comment(&j, "{\"id\":\"a\"}").unwrap();
        assert_eq!(&o[..4], &[0xFF, 0xD8, 0xFF, 0xFE]);
        assert_eq!(u16::from_be_bytes([o[4], o[5]]) as usize, 2 + 10);
        assert_eq!(&o[6..16], b"{\"id\":\"a\"}");
        assert_eq!(&o[16..], &j[2..]);
        assert!(jpeg_with_comment(b"nope", "x").is_err());
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
