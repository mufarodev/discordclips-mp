# Handoff — continue on macOS

You are picking this up on the user's Mac (macOS 27, Rust installed). Previous work was done on Linux, so **none of the macOS-specific plan has been compiled or run yet.**

## Goal
Reimplement Discord "Clips" (retroactive screen-recording buffer) in Rust: macOS first, Linux later. Clean-room: use only the interface vocabulary in REFERENCE.md, never decompiled code. Long-term: show it to Discord engineers; optionally hook into a *copy* of the Discord macOS app (INTEGRATION.md, research-gated).

## Read first
README.md → ROADMAP.md (tracker; update checkboxes as you go) → ARCHITECTURE.md → REFERENCE.md → INTEGRATION.md.

## Current state
- Done: workspace + `crates/core` (`Capture`/`Encoder` traits, `Ring` GOP-aligned buffer, 3 passing tests). `cargo test` should pass on macOS unchanged.
- Not started: everything in ROADMAP Phase 1+.

## Your first task: M1 → M3 (video only, standalone)
1. Add `crates/macos`: ScreenCaptureKit display capture → log frame count + pts. Handle missing Screen Recording permission (TCC) with a clear error message.
2. `VTCompressionSession` H.264, real-time, 2 s keyframe interval; emit `clipsd_core::Packet` (AVCC, correct `keyframe` flag; keep SPS/PPS from the format description).
3. Wire into `Ring`; `save(last N s)` → playable `.mp4` using the pure-Rust `mp4` crate. Verify in QuickTime and `ffprobe`.
4. Tiny CLI: `clipsd start | save <secs> | stop`.
Stop after M3 and report; audio/IPC come next.

## Decisions already made (don't relitigate without a reason)
- Rust workspace; only capture + encoder are platform code; ring/mux/IPC live in `core`.
- Do **not** use the ffmpeg CLI for capture (no ScreenCaptureKit, CPU copies). Pure-Rust `mp4` muxer, no ffmpeg link on macOS.
- Frames stay GPU-resident (IOSurface `CVPixelBuffer`) from SCK straight into VideoToolbox.
- Ring stores encoded packets in memory; snapshot starts at the keyframe at/before the cutoff.

## Unverified — check before relying on
- Current API/maintenance state of the `screencapturekit`, `objc2-*` (ScreenCaptureKit, VideoToolbox, CoreMedia, CoreVideo) crates and the `mp4` crate. Look at current docs (context7 or docs.rs) before writing code; fall back to a small Swift shim only if the Rust bindings are inadequate.
- macOS 27 SCK behavior/permissions may differ from what older docs say.
- Whether `mp4` crate's AVC track setup accepts VT's SPS/PPS directly.

## Constraints
- Don't touch or patch the installed Discord app. Integration work (Phase 4) happens on a copy only, ideally with a throwaway account; modifying the client is against Discord's ToS.
- Don't commit Discord binaries, models, or extracted bundles. Repo contains only our code and notes.
- Commit small, update ROADMAP.md checkboxes, keep REFERENCE.md as interface notes only.

## Background (from the Linux session)
- Windows Canary clips = `discord_clips.node` (N-API shim exporting only `getModulePath`) + `discord_clips.exe` worker (Rust+C++) over a named pipe. Details in REFERENCE.md.
- The shipped Windows `bundle.js` has no reference to clips; JS wiring is in the downloadable `discord_desktop_core` module / remote renderer (not yet fetched) — that is milestone M9.
