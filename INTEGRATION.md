# Hooking into the Discord desktop app (macOS)

Status: **research only**. Do all of this on a *copy* of the app, never the installed one, and never distribute patched bundles.

## What we know
- The shipped Windows `app.asar` main bundle (`bundle.js`) has **no** reference to `discord_clips`. The clips JS is loaded via the downloadable `discord_desktop_core` module and driven by the remotely served renderer. We have neither yet — M9 must fetch the macOS module manifest/core module and inspect it.
- The Windows `.node` is a PE DLL: it cannot be loaded on macOS. "Backport" = write our own tiny macOS `.node` (N-API, exports `getModulePath`) + ship `clipsd` as the worker, speaking the same pipe/socket vocabulary.
- Platform gating (Windows-only) is likely in the renderer/feature flags, i.e. outside the asar. Unknown how much of that we can influence. This may bound the whole integration goal.

## M9 findings (2026-11-24, Canary 0.0.1591)
See REFERENCE.md ("macOS client"). In short:
1. **File side is already native to the macOS client.** Any clip we write that satisfies the `uuid`-box check is loadable/deletable/movable by the existing main-process code. `clipsd` now writes that box on every clip (metadata = whatever the host passes in `SaveClip.metadata`, default `{}`).
2. **Recording control is not.** The clips API on `discord_voice.node` exists but reports "clips not supported in this build", and the worker/pipe client the Windows build uses is absent from the macOS binary. The thing that would *call* `clipsd` does not exist on macOS, and it lives in closed, signed native code plus a remotely-served renderer.
3. **Consequence:** a `discord_clips.node` shim + `clipsd` (M10) cannot by itself make the client record: nothing on the macOS side would launch or talk to it. Making it work means modifying/replacing the closed `discord_voice.node` clips code, which is outside this project's clean-room rules, would break the code signature, and is against Discord's ToS.
4. **What integrates cleanly:** the standalone `clipsd` (PROTOCOL.md), producing Discord-format clips into a directory the client already reads, and a protocol a Discord engineer can map onto their worker interface. M10/M11 are therefore **blocked pending a decision** (see ROADMAP).

## macOS obstacles to plan for
- Code signature / hardened runtime / library validation: editing `app.asar` or adding a `.node` invalidates the bundle signature. Dev workflow: copy `Discord Canary.app`, re-sign ad-hoc (`codesign --force --deep -s -`), strip quarantine on the copy. Expect Electron fuses (`EnableEmbeddedAsarIntegrityValidation`, `OnlyLoadAppFromAsar`) to need checking — read them with `npx @electron/fuses read`.
- TCC: Screen Recording permission is granted per signed binary; a re-signed app/worker is a new identity and will re-prompt.
- Distribution: modules normally arrive via Discord's module updater with hash checks (`delta_manifest.json` sha256 in the Windows distro). A local override path is needed so the updater doesn't replace ours.

## Risks
- Modifying the official client is against Discord's ToS and could affect the account used with it. Use a throwaway account, and prefer showing the standalone `clipsd` + protocol compatibility over a patched client when presenting to Discord.

## Tasks
See ROADMAP M9–M11.
