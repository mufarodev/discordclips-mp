# Architecture

```
 platform capture ──Frame(GPU)──▶ platform encoder ──Packet(AVCC)──▶ Ring ──snapshot──▶ mp4 muxer ──▶ clip.mp4
 (SCK | PipeWire)                 (VideoToolbox | VAAPI/NVENC)        (core)             (core)
                                                      ▲
                                  Unix-socket commands/events (core)
```

- Only `Capture` and `Encoder` are platform code (`clipsd-core` traits). Ring, mux, IPC are shared.
- Frames stay GPU-resident (IOSurface on macOS, DMA-BUF on Linux). No CPU copies on the hot path.
- Ring stores *encoded* packets in memory, trimmed on GOP boundaries, so it always starts on a keyframe. Snapshot starts at the keyframe at/before the cutoff (clip is ≥ requested length).
- Mux at save time only. Container: MP4, H.264 + AAC-LC, SPS/PPS in `avcC`.
- Timestamps: microseconds, monotonic capture clock; audio and video share one clock.

## Decisions
| Decision | Why |
|---|---|
| Rust, workspace | Matches where Discord's media stack is heading; safe FFI boundary per platform |
| Pure-Rust `mp4` muxer | No ffmpeg link on macOS; same code on Linux |
| libavcodec only for Linux encode | Mature VAAPI/NVENC; not used for capture |
| Not the ffmpeg CLI | No Wayland portal capture, no ScreenCaptureKit, CPU copies kill HW accel |
