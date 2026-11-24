# Roadmap

Legend: [ ] todo · [~] in progress · [x] done

## Phase 0 — Scaffold
- [x] Workspace + `clipsd-core` (traits `Capture`/`Encoder`, GOP-aligned `Ring`, tests)

## Phase 1 — macOS standalone (video + audio, protocol, Discord-format clips)
- [x] **M1 Capture**: ScreenCaptureKit stream (display first, window/app filter later) logs frame count + pts. Handles Screen Recording permission (TCC) failure with a clear error.
- [x] **M2 Encode**: `VTCompressionSession` H.264, real-time, fixed keyframe interval (2s), outputs AVCC `Packet`s with correct `keyframe` flag + SPS/PPS from the format description.
- [x] **M3 Save**: `Ring` + `save(last N s)` → playable `.mp4` (pure-Rust `mp4` crate). First demo. Verify in QuickTime and `ffprobe`.
- [x] **M3b** Thumbnail (AVAssetImageGenerator → JPEG) at `thumbnail_ms`.
- [x] CLI: `clipsd start | save <secs> | stop | stats | screenshot | send` (see PROTOCOL.md)

## Phase 2 — Audio + protocol
- [x] **M4 Audio** (A/V sync by ear/visual check still TODO): ScreenCaptureKit system/app audio PCM → AAC-LC (AudioToolbox) → second mp4 track; A/V sync check.
- [x] **M5 IPC** (PROTOCOL.md): Unix-socket server with the Windows command/event vocabulary (see REFERENCE.md): StartRecording, StopRecording, SetDuration, SetBitrate, SaveClip, TakeScreenshot, GetStats, Shutdown ↔ ClipSaved, ClipSaveFailed, Stats, Error.
- [x] Live bitrate change without restarting the session.

## Phase 3 — Linux (see LINUX_HANDOFF.md)
- [~] **M6 Capture**: `crates/linux` scaffolded (`ashpd` portal + restore token in `portal.rs`, `PwCapture` shell in `capture.rs`); PipeWire stream SPA format negotiation + DMA-BUF frames to be wired on Linux.
- [~] **M7 Encode**: `clipsd_core::h264` (Annex-B→AVCC) + `clipsd_core::av1` (OBU/av1C) + `av01`/`av1C` MP4 muxing implemented & unit-tested; `ffmpeg-next` (`av1_nvenc` → `h264_nvenc` → `libx264`) scaffolded in `encode.rs`; zero-copy CUDA/VAAPI DMA-BUF import + live compile on Linux remaining.
- [~] **M8 Audio**: `FfmpegAacEncoder` scaffolded in `encode.rs`; PipeWire sink-monitor audio capture to be wired on Linux.
- [x] Shared Discord JS integration (`integration/common/` + `integration/linux/inject_discord.sh`) ready for Linux native & Flatpak installs.
- [ ] Same `core` + protocol verified end-to-end on Linux. Any change to `core` here is a design bug to log.

## Phase 4 — Discord integration (research-gated, see INTEGRATION.md)
- [x] **M9** Research done — see INTEGRATION.md. Renderer Clips V3 gate is `isDesktop() && supports("clips") && hasClipsV3Support()`; all recording control passes through the plain-JS `discord_voice/index.js` wrapper, and both Vencord-style in-place injection (`inject_discord.sh`) and standalone-copy patching (`patch_discord_copy.sh`) work cleanly.
- [x] **M10** macOS `discord_clips` module shim (`getModulePath`) + `clips_bridge.js` VoiceEngine wrapper + `core_hook.js` + `inject_discord.sh` / `patch_discord_copy.sh` (`integration/macos/`).
- [x] **M11** Proven end-to-end inside Discord Canary on macOS: game auto-detected (Celeste via `CGWindowID`), 60 fps ScreenCaptureKit + VideoToolbox + AAC rolling buffer, saved via native Discord Clips hotkey directly into the Discord Clips gallery.

## Phase 5 (optional)
- [ ] `ExportVideoComposition` (recipe JSON → ffmpeg filter graph)
- [ ] ML detection (ONNX Runtime, open model) → `MlDetection` events
- [ ] Write-up / demo for Discord engineers

## Fixed
- **`esds` `SLConfigDescriptor` bug in `mp4` v0.14.0**: `mp4` wrote `[0x06, 0x00, 0x00]` instead of ISO/IEC 14496-1 `[0x06, 0x01, 0x02]`, causing Discord's clip-editor `mp4box.js` demuxer worker to throw `RangeError: Offset is outside the bounds of the DataView` and spin forever. Patched in `clipsd_core::mux::patch_esds_sl_config` right after `write_end()`.
- **Startup hole in video** (first 1–2 s of a recording had a 0.7–2.1 s frame gap in ~3 of ~16 runs). Cause: encoder init takes hundreds of ms while ScreenCaptureKit is already streaming, so the channel filled with stale frames and the next fresh frame was an init-duration later (and `MaxKeyFrameIntervalDuration` forced a keyframe after the hole). Fix: `SckCapture::flush_backlog()` once the encoder is ready. Proven with `CLIPSD_TEST_INIT_DELAY_MS` (1 s and 2.5 s injected delays → max gap 50 ms; before the fix the gap equalled the delay).

## Open issues / not verified
- A/V sync checked by timestamps only (audio starts 6–17 ms before video, within one AAC frame), not by eye/ear.
- TCC-denied error path untested; QuickTime not opened by eye (AVFoundation `isPlayable` only).
- Discord UI: clips pass a mirror of the main-process `uuid`-box check, but nobody has loaded one in the real client; the renderer's metadata schema is unknown (default `{}`).
- `SetBitrate` is acknowledged but the resulting bitrate change was not measured.
- Static-screen repeat path is exercised by `m2_static` (encoder) but not end-to-end on a truly idle display.
