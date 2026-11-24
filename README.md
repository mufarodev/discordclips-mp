# clipsd

Open reimplementation of Discord's "Clips" (retroactive screen-recording buffer) for macOS first, Linux second.
Written in Rust. Clean-room: built from observed behavior and public APIs, not decompiled code.

- [ROADMAP.md](ROADMAP.md) — milestones and status (the tracker)
- [ARCHITECTURE.md](ARCHITECTURE.md) — design, traits, protocol
- [INTEGRATION.md](INTEGRATION.md) — hooking into the Discord desktop app (research, risks)
- [REFERENCE.md](REFERENCE.md) — what the Windows module looks like (interface notes only)

Dev: macOS 27, Rust. `cargo test` runs the portable core anywhere.
