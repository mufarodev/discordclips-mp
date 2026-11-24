# Hooking into the Discord desktop app (macOS)

Status: **research only**. Do all of this on a *copy* of the app, never the installed one, and never distribute patched bundles.

## What we know
- The shipped Windows `app.asar` main bundle (`bundle.js`) has **no** reference to `discord_clips`. The clips JS is loaded via the downloadable `discord_desktop_core` module and driven by the remotely served renderer. We have neither yet — M9 must fetch the macOS module manifest/core module and inspect it.
- The Windows `.node` is a PE DLL: it cannot be loaded on macOS. "Backport" = write our own tiny macOS `.node` (N-API, exports `getModulePath`) + ship `clipsd` as the worker, speaking the same pipe/socket vocabulary.
- Platform gating (Windows-only) is likely in the renderer/feature flags, i.e. outside the asar. Unknown how much of that we can influence. This may bound the whole integration goal.

## macOS obstacles to plan for
- Code signature / hardened runtime / library validation: editing `app.asar` or adding a `.node` invalidates the bundle signature. Dev workflow: copy `Discord Canary.app`, re-sign ad-hoc (`codesign --force --deep -s -`), strip quarantine on the copy. Expect Electron fuses (`EnableEmbeddedAsarIntegrityValidation`, `OnlyLoadAppFromAsar`) to need checking — read them with `npx @electron/fuses read`.
- TCC: Screen Recording permission is granted per signed binary; a re-signed app/worker is a new identity and will re-prompt.
- Distribution: modules normally arrive via Discord's module updater with hash checks (`delta_manifest.json` sha256 in the Windows distro). A local override path is needed so the updater doesn't replace ours.

## Risks
- Modifying the official client is against Discord's ToS and could affect the account used with it. Use a throwaway account, and prefer showing the standalone `clipsd` + protocol compatibility over a patched client when presenting to Discord.

## Tasks
See ROADMAP M9–M11.
