# Hooking into the Discord desktop app (macOS)

Status: **research only**. Do all of this on a *copy* of the app, never the installed one, and never distribute patched bundles.

## What we know
- The shipped Windows `app.asar` main bundle (`bundle.js`) has **no** reference to `discord_clips`. The clips JS is loaded via the downloadable `discord_desktop_core` module and driven by the remotely served renderer. We have neither yet — M9 must fetch the macOS module manifest/core module and inspect it.
- The Windows `.node` is a PE DLL: it cannot be loaded on macOS. "Backport" = write our own tiny macOS `.node` (N-API, exports `getModulePath`) + ship `clipsd` as the worker, speaking the same pipe/socket vocabulary.
- Platform gating (Windows-only) is likely in the renderer/feature flags, i.e. outside the asar. Unknown how much of that we can influence. This may bound the whole integration goal.

## M9 findings & M10 architecture (2026-11-24, Canary 0.0.1591)
See REFERENCE.md ("macOS client"). Key findings from static inspection of the installed Canary bundle:

1. **File side is already native to the macOS client.** `discord_desktop_core` already handles `loadClip`, `loadClipsDirectory`, `deleteClip`, `moveClip`, `updateClipMetadata`, and the custom clip protocol on macOS. `clipsd` writes the Discord `uuid` box on every MP4 and the JSON COM comment on every JPEG screenshot.
2. **The renderer does *not* hardcode Windows for Clips.** Renderer eligibility checks `isDesktop() && mediaEngine.supports("clips") && mediaEngine.hasClipsV3Support()`. `isDesktop()` is already `true` on `darwin`; `supports("clips")` comes from `features.declareSupported("clips")` in `discord_voice/index.js`; and `hasClipsV3Support()` checks for `setClipsModulePath`, `setClipsRecordingEnabled`, and `exportClipToFile` on `VoiceEngine`.
3. **No closed native binary patching is required.** Every Clips recording/save call from the renderer passes through the plain-JS `VoiceEngine` module (`discord_voice/index.js`). Overriding only the Clips methods on `VoiceEngine` in JS (`integration/macos/clips_bridge.js`) leaves `discord_voice.node` untouched while routing all Clips control to our `clipsd` worker over its Unix socket.
4. **Local module loading is built into `app.asar`.** Setting `"disableUpdater": true` and `"standaloneModules": true` in a *copy*'s `Contents/Resources/build_info.json` disables `updater.node`, skips the splash update loop, and loads all modules from `Contents/Resources/standalone_modules/`. Zero edits to `app.asar`, `core.asar`, or `discord_voice.node` are needed.

## Two ways to run it (M10)

### Option A: In-place injection (Vencord-style, recommended)
```bash
./integration/macos/inject_discord.sh --channel canary
# Uninstall cleanly:
./integration/macos/inject_discord.sh --channel canary --uninstall
```
Patches **only** two plain-JS files inside `~/Library/Application Support/discordcanary/app-<ver>/modules/` (`discord_desktop_core/index.js` and `discord_voice/index.js`) and installs `clipsd` under `~/Library/Application Support/discordcanary/clipsd_modules/discord_clips/`.
- **Does not touch `Discord Canary.app`**: keeps Discord's genuine Apple-notarized signature intact, so `discord_krisp.node` works normally without crashing and existing macOS Screen Recording permissions keep working.
- Re-run after a Discord module update that bumps `discord_desktop_core` or `discord_voice`.

### Option B: Standalone patched copy of `Discord Canary.app`
```bash
./integration/macos/patch_discord_copy.sh
"$HOME/Applications/Discord Canary Clips.app/Contents/MacOS/Discord Canary" \
  --multi-instance --enable-logging=stderr
```
Uses APFS copy-on-write cloning (`cp -cR`) to create `~/Applications/Discord Canary Clips.app` with `disableUpdater: true` + `standaloneModules: true` in `Contents/Resources/build_info.json` and a separate `CFBundleIdentifier` (`com.hnc.DiscordCanary.clipsd`).
- **Caveat**: because the copied bundle is ad-hoc signed, `discord_krisp.node` crashes with `SIGSEGV` inside `CFArrayGetValueAtIndex` (it expects an Apple Developer ID certificate chain in `kSecCodeInfoCertificates`). `patch_discord_copy.sh` stubs `standalone_modules/discord_krisp/index.js` to avoid that crash (disabling Krisp noise suppression in the copy).

## macOS obstacles & notes
- Electron fuses on Canary 0.0.1591: `EnableEmbeddedAsarIntegrityValidation=0`, `OnlyLoadAppFromAsar=0`.
- TCC: Screen Recording permission is granted per signed app identity; the ad-hoc copy prompts separately as **Discord Canary Clips**.
- macOS game detection: Discord's `fireClipsInitEvent` checks `RunningGameStore.getVisibleGame()?.windowHandle`. On macOS, if no game with a window handle is active, you can test capture immediately from DevTools via `window.__clipsdBridge.startDisplay()`.

## Risks
- Modifying the official client is against Discord's ToS and could affect the account used with it. Always test on a *copy* with an isolated profile and a throwaway account, and never distribute patched bundles.

## Tasks
See ROADMAP M9–M11.
