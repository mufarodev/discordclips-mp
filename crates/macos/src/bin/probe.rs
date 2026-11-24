//! Prints whether the *responsible process* has Screen Recording permission.
//! TCC grants this to the host app that launched us (terminal / T3 Code), not to this binary.
#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

fn main() {
    #[cfg(target_os = "macos")]
    println!("screen_recording_granted={}", unsafe { CGPreflightScreenCaptureAccess() });
    #[cfg(not(target_os = "macos"))]
    println!("macOS only");
}
