# clipsd protocol

Spoken over a Unix socket (macOS/Linux). Vocabulary follows REFERENCE.md; the transport is our own.

**Framing:** 4-byte big-endian length + JSON object tagged by `"type"`. Max frame 16 MiB.
**Exchange:** every command frame gets exactly **one** event frame in reply, in order. A connection may carry many exchanges. Socket mode is `0600`.
**Source of truth:** `crates/core/src/proto.rs` (types + tests), `crates/core/src/ipc.rs` (server/client).

## Commands → reply
| Command | Fields | Reply |
|---|---|---|
| `StartRecording` | `fps`, `duration_ms` (retained window), `bitrate_kbps`, `startup_delay_ms?`, `width?`, `window_id?` | `RecordingEvent{state:"started"}` · `InitFailure{message}` (permission, encoder…) · `Error` if already recording |
| `StopRecording` | | `RecordingEvent{state:"stopped"}` |
| `SetDuration` | `duration_ms` | `Ack` |
| `SetBitrate` | `bitrate_kbps` (applied live, no restart) | `Ack` |
| `SaveClip` | `filepath`, `metadata?` (echoed), `trim_start_ms?`, `trim_end_ms?`, `thumbnail_ms?`, `saved_at?`, `last_ms?` (extension: only the newest N ms), `embed_thumbnail?` (when true, returns `data:image/jpeg;base64,…` and updates the MP4 `uuid` box instead of writing `<clip>.jpg`) | `ClipSaved{filepath,duration_ms,stats_json,thumbnail,metadata}` · `ClipSaveFailed{filepath,error}` |
| `UpdateClipMetadata` | `filepath`, `metadata` (JSON object replacing the MP4 `uuid` box) | `Ack` · `Error` |
| `TakeScreenshot` | `filepath`, `quality` (1–100), `metadata?` (JSON written into the JPEG COM comment), `window_id?` | `ScreenshotComplete{filepath}` · `Error` |
| `GetStats` | | `Stats{…}` |
| `Shutdown` | | `Ack`, then the server exits |

## Interpretations (the Windows semantics aren't fully known — change if a host needs otherwise)
- `trim_start_ms` / `trim_end_ms` are removed from the beginning / end of the **buffered window**. The clip still starts on the keyframe at-or-before the cut, so it can be up to one GOP (2 s) longer at the start.
- `thumbnail_ms` is an offset into the **saved clip**; the JPEG is written next to it as `<clip>.jpg` and returned in `ClipSaved.thumbnail`. Thumbnail failure never fails the clip.
- Clips are written to `<file>.mp4.part` then renamed, so a reader never sees a partial file.
- Extensions beyond the Windows vocabulary: `Ack`, `last_ms`, `TakeScreenshot.filepath`.

## CLI
`clipsd start [--idle] [--socket PATH] [--buffer S] [--fps N] [--bitrate KBPS]` · `save <secs> [-o F]` · `screenshot [-o F]` · `stats` · `stop` · `send '<json>'`.
`--idle` serves without recording so a host can drive it with `StartRecording`.
