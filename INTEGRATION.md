# Hooking into the Discord desktop client

> **Disclaimer:** Modifying the official Discord client is against Discord's ToS. Use for research/testing only and never distribute patched binaries or bundles.

## How it works

1. **File handling is already built into `discord_desktop_core`.** Both macOS and Linux clients already handle `loadClip`, `loadClipsDirectory`, `deleteClip`, `moveClip`, `updateClipMetadata`, and the clip protocol. `clipsd` writes the Discord `uuid` metadata box on every MP4 (with spec-compliant `av1C`/`avcC` and AAC `esds`) and JSON COM comments on JPEG screenshots.
2. **Renderer eligibility is not Windows-hardcoded.** The renderer checks `isDesktop() && mediaEngine.supports("clips") && mediaEngine.hasClipsV3Support()`. `discord_voice/index.js` declares those capabilities and routes all Clips V3 calls (`setClipsSource`, `saveClipEx`, `saveScreenshot`, `updateClipMetadata`, etc.) through [clips_bridge.js](integration/common/clips_bridge.js) to our `clipsd` worker over a private Unix socket, leaving normal voice/video calls on `discord_voice.node` untouched.
3. **No binary patching.** In-place injection patches only plain-JS module entrypoints inside Discord's user config directory (`--uninstall` restores `.clipsd.bak` backups cleanly):
   - `discord_desktop_core/index.js` → [core_hook.js](integration/common/core_hook.js): registers our local `discord_clips` module folder with Discord's module loader/updater.
   - `discord_voice/index.js` → [clips_bridge.js](integration/common/clips_bridge.js): attaches the Clips V3 bridge to `VoiceEngine`.
   - `discord_utils/index.js` *(Linux only)* → [utils_hook.js](integration/linux/utils_hook.js): synthesizes a deterministic `windowHandle` per detected game PID (mapping it back in `getPidFromWindowHandle`), working around `discord_utils.node` returning empty handles on Linux so `ClipsManager` dispatches `CLIPS_INIT` and enables saving.

## Linux

```bash
./integration/linux/inject_discord.sh --channel canary   # or stable / ptb / --flatpak
# Uninstall cleanly:
./integration/linux/inject_discord.sh --channel canary --uninstall
```

- Hooks every installed `app-*` / `0.*` version directory under `~/.config/discord*/` (handles staged updates).
- Also compiles and installs the dormant implicit Vulkan DMA-BUF capture layer (`~/.local/share/vulkan/implicit_layer.d/VK_LAYER_CLIPSD_capture.json`).
- **Keybind note:** Discord disables the default `Alt+C` clip binding on Linux. Bind **Save Clip** in **Settings → Keybinds** (Discord uses the native Wayland `GlobalShortcuts` portal on KDE/Hyprland).
- **Capture priority:** For any Vulkan/Proton (DXVK/VKD3D) game, `clipsd` attaches directly to the game's Vulkan swapchain with zero portal prompts. For OpenGL games, it falls back to `xdg-desktop-portal` Monitor capture (approve your monitor once; set `CLIPSD_SOURCE=window` to pick single windows instead).

## macOS

### Option A: In-place injection (recommended)
```bash
./integration/macos/inject_discord.sh --channel canary
# Uninstall cleanly:
./integration/macos/inject_discord.sh --channel canary --uninstall
```
Patches only `discord_desktop_core/index.js` and `discord_voice/index.js` inside `~/Library/Application Support/discordcanary/app-<ver>/modules/`, preserving `Discord Canary.app`'s Apple-notarized signature (so `discord_krisp.node` and TCC permissions stay intact).

### Option B: Standalone patched copy
```bash
./integration/macos/patch_discord_copy.sh
"$HOME/Applications/Discord Canary Clips.app/Contents/MacOS/Discord Canary" \
  --multi-instance --enable-logging=stderr
```
Clones `Discord Canary.app` with `disableUpdater: true` and `standaloneModules: true` (stubs `discord_krisp` to avoid ad-hoc signature crash).
