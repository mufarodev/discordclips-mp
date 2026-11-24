//! Command/event vocabulary spoken between a host (e.g. a Discord client shim) and the clips
//! worker. Names follow the interface notes in REFERENCE.md. Frames on the wire are a 4-byte
//! big-endian length followed by a JSON object tagged with `"type"`.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Largest frame we will accept (defends against a corrupt length prefix).
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum Command {
    /// Begin the rolling capture. `duration_ms` is the retained window.
    StartRecording {
        fps: u32,
        duration_ms: u32,
        bitrate_kbps: u32,
        #[serde(default)]
        startup_delay_ms: u32,
        /// Optional maximum capture width in pixels (default 1920).
        #[serde(default)]
        width: Option<u32>,
        /// Optional macOS CGWindowID to capture; falls back to main display if absent/not found.
        #[serde(default)]
        window_id: Option<u32>,
    },
    StopRecording,
    SetDuration { duration_ms: u32 },
    /// Takes effect live, without restarting the capture session.
    SetBitrate { bitrate_kbps: u32 },
    /// Save the buffered window to `filepath`. `trim_start_ms`/`trim_end_ms` are removed from
    /// the beginning/end of the buffered window. `thumbnail_ms` is a time offset into the saved
    /// clip. `metadata` is opaque and echoed back in `ClipSaved`.
    SaveClip {
        filepath: String,
        #[serde(default)]
        metadata: Option<serde_json::Value>,
        #[serde(default)]
        trim_start_ms: u32,
        #[serde(default)]
        trim_end_ms: u32,
        #[serde(default)]
        thumbnail_ms: Option<u32>,
        #[serde(default)]
        saved_at: Option<String>,
        /// Extension: keep only the most recent `last_ms` of the buffer (CLI convenience).
        #[serde(default)]
        last_ms: Option<u32>,
        /// When true, embed the generated thumbnail as a JPEG data URL in metadata + `ClipSaved`
        /// instead of leaving a sidecar `.jpg` file on disk.
        #[serde(default)]
        embed_thumbnail: bool,
    },
    /// Replace the Discord `uuid` metadata box in an existing clip MP4.
    UpdateClipMetadata {
        filepath: String,
        metadata: serde_json::Value,
    },
    /// JPEG of the current display (or `window_id` if provided). `quality` is 1..=100.
    /// `metadata` is merged into the JPEG comment (Discord-format screenshots carry JSON with a
    /// string `id`; one is generated if absent).
    TakeScreenshot {
        filepath: String,
        quality: u8,
        #[serde(default)]
        metadata: Option<serde_json::Value>,
        #[serde(default)]
        window_id: Option<u32>,
    },
    GetStats,
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Stats {
    pub recording: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub window_ms: u32,
    pub buffered_ms: u64,
    pub buffered_bytes: u64,
    pub frames_captured: u64,
    pub frames_dropped: u64,
    pub audio_packets: u64,
    pub audio_chunks_dropped: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum Event {
    /// Extension: reply to commands with no natural event (`SetDuration`, `SetBitrate`, `Shutdown`).
    Ack,
    /// Reply to `StartRecording`/`StopRecording`: `state` is `"started"` or `"stopped"`.
    RecordingEvent { state: String },
    ClipSaved {
        filepath: String,
        duration_ms: u64,
        stats_json: String,
        thumbnail: Option<String>,
        metadata: Option<serde_json::Value>,
    },
    ClipSaveFailed { filepath: String, error: String },
    ScreenshotComplete { filepath: String },
    /// Reply to `GetStats`.
    Stats(Stats),
    /// The recorder could not start (permissions, encoder, …).
    InitFailure { message: String },
    Error { message: String },
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    if body.len() > MAX_FRAME {
        return Err(io::Error::other("frame too large"));
    }
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

/// Read one frame; `Ok(None)` on a clean EOF at a frame boundary.
pub fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip_and_eof() {
        let cmds = vec![
            Command::StartRecording {
                fps: 30,
                duration_ms: 60_000,
                bitrate_kbps: 8000,
                startup_delay_ms: 0,
                width: None,
                window_id: None,
            },
            Command::SaveClip {
                filepath: "/x.mp4".into(),
                metadata: Some(serde_json::json!({"game": "x"})),
                trim_start_ms: 5,
                trim_end_ms: 0,
                thumbnail_ms: Some(1000),
                saved_at: None,
                last_ms: None,
                embed_thumbnail: false,
            },
            Command::Shutdown,
        ];
        let mut buf = Vec::new();
        for c in &cmds {
            write_frame(&mut buf, c).unwrap();
        }
        let mut r = &buf[..];
        for c in &cmds {
            assert_eq!(&read_frame::<_, Command>(&mut r).unwrap().unwrap(), c);
        }
        assert!(read_frame::<_, Command>(&mut r).unwrap().is_none());
    }

    #[test]
    fn optional_fields_default_and_wire_shape_is_tagged() {
        let c: Command = serde_json::from_str(r#"{"type":"SaveClip","filepath":"/a.mp4"}"#).unwrap();
        assert!(matches!(c, Command::SaveClip { trim_start_ms: 0, thumbnail_ms: None, .. }));
        let e = serde_json::to_string(&Event::RecordingEvent { state: "started".into() }).unwrap();
        assert_eq!(e, r#"{"type":"RecordingEvent","state":"started"}"#);
    }

    #[test]
    fn rejects_oversized_and_garbage_frames() {
        let mut huge = (MAX_FRAME as u32 + 1).to_be_bytes().to_vec();
        huge.extend_from_slice(b"{}");
        assert!(read_frame::<_, Command>(&mut &huge[..]).is_err());
        let mut bad = 3u32.to_be_bytes().to_vec();
        bad.extend_from_slice(b"{{{");
        assert!(read_frame::<_, Command>(&mut &bad[..]).is_err());
    }
}
