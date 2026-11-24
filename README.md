# clipsd

Open reimplementation of Discord's "Clips" (retroactive screen-recording buffer) for macOS and Linux.
Written in Rust. Clean-room: built from observed behavior and public APIs, not decompiled code.

- [ARCHITECTURE.md](ARCHITECTURE.md) — pipeline design, traits, zero-copy paths
- [PROTOCOL.md](PROTOCOL.md) — Unix-socket IPC command/event vocabulary
- [INTEGRATION.md](INTEGRATION.md) — hooking into the Discord desktop client (macOS & Linux)
- [ROADMAP.md](ROADMAP.md) — milestones and remaining work
- [REFERENCE.md](REFERENCE.md) — observed Windows module interface notes

## Quick start

```bash
# Portable core tests (runs anywhere)
cargo test -p clipsd-core

# Standalone Linux CLI (requires pipewire, ffmpeg, clang, pkg-config)
cargo build --release -p clipsd-linux --bin clipsd
./target/release/clipsd start --fps 60 --buffer 30
./target/release/clipsd save 15 -o clip.mp4

# Hook into Discord Canary (Linux) — builds clipsd + implicit Vulkan DMA-BUF layer
./integration/linux/inject_discord.sh --channel canary
# Uninstall cleanly:
./integration/linux/inject_discord.sh --channel canary --uninstall
```
