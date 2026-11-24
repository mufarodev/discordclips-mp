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

## Phase 3 — Linux
- [ ] **M6 Capture**: xdg-desktop-portal ScreenCast → PipeWire (`ashpd` + `pipewire-rs`), DMA-BUF frames, restore token.
- [ ] **M7 Encode**: VAAPI / NVENC via libavcodec, zero-copy DMA-BUF import; `libx264` fallback.
- [ ] **M8 Audio**: PipeWire per-app capture → AAC.
- [ ] Same `core` + protocol, no changes expected. Any change to `core` here is a design bug to log.

## Phase 4 — Discord integration (research-gated, see INTEGRATION.md)
- [x] **M9** Research done — see INTEGRATION.md. The macOS client has the clip *file* plumbing; the recording-control path is stubbed in closed native code ("clips not supported in this build").
- [ ] (blocked — needs a decision, see INTEGRATION.md) **M10** macOS `discord_clips.node` shim exposing `getModulePath` + `clipsd` binary as the worker, in a *copy* of the app.
- [ ] (blocked) **M11** Prove one clip saved from inside the patched client.

## Phase 5 (optional)
- [ ] `ExportVideoComposition` (recipe JSON → ffmpeg filter graph)
- [ ] ML detection (ONNX Runtime, open model) → `MlDetection` events
- [ ] Write-up / demo for Discord engineers
