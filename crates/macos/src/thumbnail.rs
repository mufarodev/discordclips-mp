//! JPEG output: clip thumbnails (AVAssetImageGenerator) and screenshots.

use std::path::Path;

use objc2_av_foundation::{AVAssetImageGenerator, AVURLAsset};
use objc2_core_foundation::{CFDictionary, CFMutableData, CFNumber, CFRetained, CFString};
use objc2_core_graphics::CGImage;
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_foundation::{NSString, NSURL};
use objc2_image_io::{kCGImageDestinationLossyCompressionQuality, CGImageDestination};

/// Encode `img` as JPEG (`quality` 1..=100) and write it to `path`.
pub fn write_jpeg(img: &CGImage, path: &Path, quality: u8) -> Result<(), String> {
    let data = CFMutableData::new(None, 0).ok_or("CFMutableData alloc failed")?;
    let uti = CFString::from_str("public.jpeg");
    let dest = unsafe { CGImageDestination::with_data(&data, &uti, 1, None) }.ok_or("JPEG destination unavailable")?;
    let q = CFNumber::new_f64(quality.clamp(1, 100) as f64 / 100.0);
    let key = unsafe { kCGImageDestinationLossyCompressionQuality };
    let props = CFDictionary::<CFString, CFNumber>::from_slices(&[key], &[&*q]);
    unsafe {
        dest.add_image(img, Some(props.as_opaque()));
        if !dest.finalize() {
            return Err("JPEG encode failed".into());
        }
    }
    let bytes = unsafe { data.as_bytes_unchecked() };
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Extract the frame at `at_ms` of the video at `video` into a JPEG at `out`.
pub fn thumbnail_from_video(video: &Path, at_ms: u32, out: &Path, quality: u8) -> Result<(), String> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&video.to_string_lossy()));
    let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };
    let generator = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset) };
    unsafe {
        generator.setAppliesPreferredTrackTransform(true);
        // Nearest-keyframe is fine for a thumbnail and much cheaper than frame-accurate seeks.
        let inf = CMTime { value: 0, timescale: 1, flags: CMTimeFlags::PositiveInfinity, epoch: 0 };
        generator.setRequestedTimeToleranceBefore(inf);
        generator.setRequestedTimeToleranceAfter(inf);
        generator.setMaximumSize(objc2_core_foundation::CGSize { width: 640.0, height: 640.0 });
    }
    let t = CMTime { value: at_ms as i64, timescale: 1000, flags: CMTimeFlags::Valid, epoch: 0 };
    #[allow(deprecated)]
    let img = unsafe { generator.copyCGImageAtTime_actualTime_error(t, std::ptr::null_mut()) }
        .map_err(|e| format!("thumbnail: {}", e.localizedDescription()))?;
    write_jpeg(&img, out, quality)
}

#[allow(dead_code)]
fn _assert_types(_: &CFRetained<CGImage>) {}
