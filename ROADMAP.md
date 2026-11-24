# Roadmap

Legend: `[ ]` todo · `[~]` in progress · `[x]` done

## Phase 0 — Core
- [x] Workspace + `clipsd-core` (`Capture`/`Encoder` traits, GOP-aligned `Ring`, `AudioRing`, AV1/H.264 MP4 muxer with Discord `uuid` box, JPEG COM metadata, IPC protocol & tests)

## Phase 1 & 2 — macOS standalone
- [x] **M1 Capture**: ScreenCaptureKit stream (display + `CGWindowID` filtering) with monotonic timestamps and TCC error handling.
- [x] **M2 Encode**: `VTCompressionSession` H.264, real-time, 2 s keyframes, AVCC + SPS/PPS.
- [x] **M3 Save & Thumbnails**: GOP-aligned `Ring` → pure-Rust MP4 + `AVAssetImageGenerator` JPEG thumbnail + CLI (`start | save | screenshot | stats | stop | send`).
- [x] **M4 Audio**: ScreenCaptureKit system audio PCM → AAC-LC (AudioToolbox) → muxed second MP4 track.
- [x] **M5 IPC**: Unix-socket server with the full command/event vocabulary and live bitrate updates.

## Phase 3 — Linux
- [x] **M6 Capture**: Hybrid zero-copy capture:
  - Primary: dormant implicit Vulkan DMA-BUF layer (`crates/linux/vk_layer/clipsd_vk_layer.c` + `vk_capture.rs`, abstract Unix socket `\0clipsd-vk-<uid>-<pid>`, 2-slot ping-pong `vkCmdBlitImage` → `SCM_RIGHTS` DMA-BUF, zero prompts, game-only, handles swapchain resize).
  - Fallback: `ashpd` `xdg-desktop-portal` ScreenCast (Monitor default with persistent restore token) + PipeWire DMA-BUF (NVIDIA tiled modifiers, MemFd fallback).
- [x] **M7 Encode**: `ffmpeg-next 9` (`av1_nvenc` → `h264_nvenc` → `libx264`). Zero-copy NVIDIA path (`crates/linux/src/gpu.rs`): DMA-BUF → headless EGL/GL scale → `cuGraphicsGLRegisterImage` + `cuMemcpy2DAsync_v2` → FFmpeg `AV_PIX_FMT_CUDA` (`RGB0`) → `av1_nvenc`. Cuts steady-state CPU 16.1× (112.2% → 7.0% of a core at 1440p→1080p60; 8.0% at 1440p60; 10.4% at 1080p120) with 0 dropped frames (verified over a 31-min 110,978-frame live soak).
- [x] **M8 Audio**: PipeWire default-sink monitor → 48 kHz stereo f32 → AAC-LC with silence-padded idle gaps.

## Phase 4 — Discord client integration
- [x] **M9 Research**: Documented renderer Clips V3 gating (`isDesktop() && supports("clips") && hasClipsV3Support()`) and `VoiceEngine` interface in `INTEGRATION.md`.
- [x] **M10 macOS integration**: `discord_clips` shim + `clips_bridge.js` + `core_hook.js` + `inject_discord.sh` / `patch_discord_copy.sh`.
- [x] **M11 End-to-end verification**: Proven live inside Discord Canary on both **macOS** (Celeste via `CGWindowID`) and **Linux** (Hollow Knight: Silksong via both PipeWire DMA-BUF and implicit Vulkan layer inside Steam `srt-bwrap`, saved via native Discord hotkey directly into the Clips gallery and playable in the clip editor).

## Phase 5 — Remaining improvements
- [ ] VAAPI zero-copy DMA-BUF encode path for AMD/Intel GPUs
- [ ] Per-app PipeWire audio capture (filter by game PID instead of default-sink monitor)
- [ ] 32-bit x86 Vulkan layer build alongside 64-bit (for legacy 32-bit DX9/Vulkan games)
- [ ] Auto-exit `clipsd` on parent process death (stdin EOF / `PR_SET_PDEATHSIG`)
- [ ] `ExportVideoComposition` (recipe JSON → ffmpeg filter graph)
- [ ] ML highlight detection (ONNX Runtime) → `MlDetection` events

## Notable fixes
- **`esds` `SLConfigDescriptor` in `mp4` v0.14.0**: patched `[0x06, 0x00, 0x00]` → `[0x06, 0x01, 0x02]` in `clipsd_core::mux::patch_esds_sl_config` so Discord's `mp4box.js` clip editor demuxes AAC cleanly.
- **Startup video hole**: `flush_backlog()` drains stale capture frames accumulated during encoder init.
- **Linux `windowHandle` gap**: `discord_utils.node` omits `windowHandle` on Linux; `integration/linux/utils_hook.js` synthesizes deterministic handles mapped back via `getPidFromWindowHandle`.
