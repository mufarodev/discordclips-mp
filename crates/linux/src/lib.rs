//! Linux platform layer: implicit Vulkan DMA-BUF layer + xdg-desktop-portal/PipeWire capture
//! + zero-copy EGL/GL→CUDA→NVENC encoding.
#![cfg(target_os = "linux")]

pub mod portal;
pub mod gpu;
pub mod capture;
pub mod vk_capture;
pub mod encode;
pub mod thumbnail;
pub mod recorder;
