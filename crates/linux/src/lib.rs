//! Linux platform layer: xdg-desktop-portal ScreenCast + PipeWire capture + libavcodec encode.
//!
//! Scaffolded from macOS; compile and exercise M6-M8 on a Linux desktop (see LINUX_HANDOFF.md).
#![cfg(target_os = "linux")]

pub mod portal;
pub mod capture;
pub mod encode;
pub mod thumbnail;
pub mod recorder;
