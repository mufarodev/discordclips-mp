//! macOS platform layer: ScreenCaptureKit capture + VideoToolbox encode.
#![cfg(target_os = "macos")]

pub mod capture;
pub mod encode;
pub mod aac;
