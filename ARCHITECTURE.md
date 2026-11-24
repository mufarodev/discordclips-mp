# Architecture

```
 platform capture ────Frame(GPU)───▶ platform encoder ──Packet──▶ Ring ──snapshot──▶ mp4 muxer ──▶ clip.mp4
 (SCK | Vulkan layer / PipeWire)     (VideoToolbox | NVENC/x264)  (core)             (core)
                                                      ▲
                                  Unix-socket commands/events (core)
```

- Only capture and encode are platform code (`clipsd-core` traits). Ring buffer, MP4 muxer, JPEG comment writer, and IPC are shared.
- Frames stay GPU-resident on both platforms:
  - **macOS**: `ScreenCaptureKit` IOSurface (`CVPixelBuffer`) → `VideoToolbox` H.264.
  - **Linux**: Implicit Vulkan capture layer (`crates/linux/vk_layer/clipsd_vk_layer.c`) exports ping-pong swapchain DMA-BUFs over an abstract Unix socket (`SCM_RIGHTS`), with automatic `xdg-desktop-portal` + PipeWire DMA-BUF fallback → headless NVIDIA EGL/GL scale → `cuGraphicsGLRegisterImage` + `cuMemcpy2DAsync_v2` → FFmpeg `AV_PIX_FMT_CUDA` (`av1_nvenc` / `h264_nvenc`, falling back to MemFd + `swscale` + `libx264`).
- Ring stores *encoded* packets in memory, trimmed on GOP boundaries, so snapshots always start on a keyframe and cover at least the requested duration.
- Mux happens at save time only: MP4 (`av01`/`av1C` or `avc1`/`avcC`) + AAC-LC (`esds`) + Discord `uuid` metadata box.
- Timestamps use a single monotonic microsecond clock shared between video and audio.

## Decisions
| Decision | Why |
|---|---|
| Rust workspace | Matches Discord's media stack; clean FFI boundary per platform |
| Pure-Rust `mp4` muxer | No ffmpeg link on macOS; identical muxer on Linux |
| `libavcodec` only for Linux encode/thumb | Mature NVENC/AV1/AAC support; capture stays native (Vulkan/PipeWire) |
| Dormant implicit Vulkan layer | Zero prompts & Alt-Tab-proof capture across Proton/DXVK/VKD3D and native Vulkan games without root or launch options |
