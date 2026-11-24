# Handoff — continue on Linux (Phase 3: M6 → M8 + Discord integration)

You are picking this up on a Linux desktop (Wayland + PipeWire, or X11). Everything in `crates/core`, `crates/macos`, and `integration/` has been built and verified on macOS, including live Clips saving + clip editor playback inside Discord Canary. **`crates/linux` was scaffolded on macOS and has NOT been compiled on Linux yet.**

## What is already done & ready for Linux
- **`crates/core`** (100% portable, tested on macOS):
  - `Ring`, `AudioRing`, `mux::write_mp4_av` (including the `esds` `SLConfigDescriptor` `[0x06, 0x01, 0x02]` fix required by Discord's `mp4box.js` clip editor!), `mux::update_discord_metadata`, `proto`, `ipc`.
  - `h264::annex_b_to_avcc` + `h264::parse_extradata_sps_pps` — unit-tested Annex-B → AVCC converter for `libavcodec` (`h264_vaapi` / `h264_nvenc` / `libx264`).
  - `jpeg::{jpeg_with_comment, base64_encode}` and `metadata::with_defaults`.
- **`integration/common/` + `integration/linux/inject_discord.sh`**:
  - `clips_bridge.js` already supports `process.platform === "linux"` and uses `$XDG_RUNTIME_DIR` for the Unix socket.
  - `core_hook.js` and `discord_clips/` are OS-agnostic.
  - `integration/linux/inject_discord.sh` supports native `~/.config/discord{,ptb,canary,development}/` and `--flatpak`, with `--uninstall`.
- **`crates/linux`** (scaffolded, not yet compiled on Linux):
  - `portal.rs`: `ashpd` ScreenCast + restore-token persistence (`$XDG_CONFIG_HOME/clipsd/restore_token`) + portal screenshot.
  - `capture.rs`: `PwCapture` shell + static-screen repeat loop (needs PipeWire stream SPA format negotiation + frame callback + audio monitor stream wired up).
  - `encode.rs`: `FfmpegVideoEncoder` (`h264_nvenc` → `libx264` fallback; VAAPI `AVHWDeviceContext`/DMA-BUF next) + `FfmpegAacEncoder`.
  - `thumbnail.rs`: `jpeg-encoder` + `ffmpeg-next` frame extractor.
  - `recorder.rs` & `bin/clipsd.rs`: full `PROTOCOL.md` service + CLI mirroring macOS.

## System dependencies to install first
- **Debian / Ubuntu**:
  ```bash
  sudo apt install pkg-config libclang-dev libpipewire-0.3-dev \
    libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libavdevice-dev
  ```
- **Arch Linux**:
  ```bash
  sudo pacman -S pkgconf clang pipewire ffmpeg
  ```
- **Fedora**:
  ```bash
  sudo dnf install pkg-config clang-devel pipewire-devel ffmpeg-free-devel
  ```
- **Nix / NixOS** (`nix-shell`):
  ```bash
  nix-shell -p pkg-config clang libclang pipewire ffmpeg
  ```

## Target scenario: KDE Plasma / Hyprland + NVIDIA RTX 40-series + AV1 (`av1_nvenc`)
This stack is the primary Linux target. Key details already accounted for:

1. **AV1 MP4 muxing is built & verified against Discord's `mp4box.js`**:
   - `crates/core/src/av1.rs` parses AV1 OBUs, strips `OBU_TEMPORAL_DELIMITER` (forbidden in MP4 per AV1-ISOBMFF), and builds/validates `av1C`.
   - `crates/core/src/lib.rs` (`mux::write_mp4_av`) writes `av01` + `av1C` + fixed AAC `esds` (`06 01 02`) + Discord `uuid` metadata box.
   - `crates/linux/src/encode.rs` defaults to `av1_nvenc` (`preset=p3`, `tune=ll`, `rc=cbr`, `multipass=disabled`, `max_b_frames=0`) → `h264_nvenc` → `libx264`. Override anytime with `CLIPSD_CODEC=h264` or `CLIPSD_ENCODER=...`.
2. **NVIDIA DMA-BUF modifier gotcha (KDE KWin & Hyprland `xdph`)**:
   - On NVIDIA (driver 550+/560+), KWin and `xdg-desktop-portal-hyprland` export PipeWire DMA-BUFs with **NVIDIA block-linear tiled modifiers** (`DRM_FORMAT_MOD_NVIDIA_*`). Never `mmap()` those directly on CPU or the image will be scrambled/fail.
   - **Step 1 (bring-up, guaranteed clean image)**: in `capture.rs`, negotiate `SPA_DATA_MemFd` / `SPA_DATA_MemPtr` (linear `BGRx`/`BGRA`) first; both KWin and `xdph` cleanly deliver linear SHM buffers when requested, and `swscale` → NV12 → `av1_nvenc` will immediately produce crisp 60 fps AV1 clips.
   - **Step 2 (zero-copy GPU upgrade)**: negotiate `SPA_DATA_DmaBuf` with explicit NVIDIA modifiers, import via EGL (`eglCreateImageKHR` with `EGL_LINUX_DMA_BUF_EXT` + modifier attribs) or Vulkan (`VK_EXT_external_memory_dma_buf`), register into CUDA (`cuGraphicsEGLRegisterImage` / Vulkan-CUDA external memory), color-convert BGRA→NV12 on GPU, and feed `AV_PIX_FMT_CUDA` directly into `av1_nvenc` (`AVHWFramesContext`).
3. **KDE vs Hyprland portal differences**:
   - **KDE (`xdg-desktop-portal-kde`)**: supports `PersistMode::ExplicitlyRevoked` restore tokens (one-time screen picker) and `GlobalShortcuts` portal.
   - **Hyprland (`xdg-desktop-portal-hyprland`)**: opens `hyprland-share-picker` (supports monitor, window, or region); supports `GlobalShortcuts` in `xdph` 1.3.4+. Make sure `xdg-desktop-portal-hyprland` is active in your systemd user session (`systemctl --user status xdg-desktop-portal-hyprland`).
4. **Verify NVENC AV1 on the Linux box first**:
   ```bash
   ffmpeg -hide_banner -encoders | grep nvenc
   # Expect: av1_nvenc and h264_nvenc
   ```
   (On NixOS/Arch/Ubuntu, ensure `nvidia-driver` + `libnvidia-encode` / `cuda` runtime are installed so `libnvidia-encode.so.1` is loadable by `libavcodec`.)

## Linux-specific Discord quirks already discovered
1. **Default Clip keybind is disabled on Linux**: Discord's renderer explicitly removes managed `SAVE_CLIP` (`Alt+C`) and `SAVE_SCREENSHOT` (`F12`) keybinds when `isLinux()` is true. Discord *does* have native Wayland `GlobalShortcuts` portal support in `discord_utils` (`isWaylandGlobalShortcutsAvailable`), so bind **Save Clip** in **Settings → Keybinds** (and approve the KDE/Hyprland portal shortcut prompt).
2. **Wayland `windowHandle` & game detection**:
   - On XWayland games (Steam/Proton/etc.), `RunningGameStore.getVisibleGame().windowHandle` is often populated (X11 XID), triggering `setClipsSource` automatically ~30s after launch.
   - On pure Wayland games where `windowHandle` is null, use `window.__clipsdBridge.startDisplay()` in DevTools (or trigger `StartRecording` when `setClipsRecordingEnabled(true)` is active).
3. **No code-signing / `discord_krisp.node` headaches**: Unlike macOS ad-hoc re-signing, Linux in-place injection into `~/.config/discord*/<ver>/modules/` does not break any signatures.
4. **Portal screenshot format**: `xdg-desktop-portal`'s Screenshot portal writes a **PNG** file, whereas Discord expects a **JPEG** with a COM JSON comment (`TakeScreenshot` in `recorder.rs` currently passes portal bytes directly to `jpeg_with_comment` — decode PNG → encode JPEG via `encode_rgb_jpeg` or grab the latest buffered video frame!).

## First steps on Linux
1. Run `cargo test` (core tests) and `cargo check -p clipsd-linux` — fix any `ashpd` / `pipewire` / `ffmpeg-next` API version mismatches.
2. **M6**: Complete `PwCapture::start` in `crates/linux/src/capture.rs` (connect `pipewire::main_loop::MainLoop` to `portal.pipewire_fd`, attach `Stream` to `portal.node_id`, negotiate `SPA_TYPE_OBJECT_Format` for `BGRx`/`BGRA`/`NV12`, push frames on `process`). Verify `clipsd start` + `clipsd save 5` produces a playable MP4!
3. **M7**: Add VAAPI (`h264_vaapi` + `AVHWDeviceContext` / zero-copy DMA-BUF import) alongside `h264_nvenc` and `libx264` in `crates/linux/src/encode.rs`. Fix portal screenshot PNG→JPEG conversion in `recorder.rs`.
4. **M8**: Add PipeWire audio monitor stream (`media.class = Stream/Input/Audio`, `stream.capture.sink = true`, 48 kHz stereo f32) pushing `PcmChunk`s into `FfmpegAacEncoder`.
5. **Discord test**:
   ```bash
   ./integration/linux/inject_discord.sh --channel canary   # or --channel stable / --flatpak
   ```
   Restart Discord, enable **Settings → Clips**, bind a **Save Clip** keybind in **Settings → Keybinds**, and verify saving + opening in Discord's clip editor!
