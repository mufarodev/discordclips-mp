//! VideoToolbox H.264 hardware encoder.
//!
//! Real-time, no frame reordering (so pts == dts and `Packet` needs no ctts), keyframe at
//! least every 2 s. Output is AVCC (4-byte length prefixes); SPS/PPS are pulled from the
//! format description of each keyframe.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use clipsd_core::{CodecConfig, Encoder, Packet};
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFBoolean, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    kCMSampleAttachmentKey_NotSync, kCMVideoCodecType_H264, CMSampleBuffer, CMTime, CMTimeFlags,
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
};
use objc2_foundation::{NSArray, NSDictionary};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, kVTCompressionPropertyKey_ProfileLevel,
    kVTCompressionPropertyKey_RealTime, kVTProfileLevel_H264_High_AutoLevel, VTCompressionSession,
    VTEncodeInfoFlags, VTSessionSetProperty,
};

use crate::capture::Frame;

pub struct EncoderConfig {
    pub width: usize,
    pub height: usize,
    pub fps: i32,
    pub bitrate_bps: i32,
    pub keyframe_secs: i32,
}

impl EncoderConfig {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, fps: 30, bitrate_bps: 8_000_000, keyframe_secs: 2 }
    }
}

/// State shared with the VT output callback (which runs on a VideoToolbox thread).
#[derive(Default)]
struct Shared {
    ready: Mutex<Vec<Packet>>,
    config: Mutex<Option<CodecConfig>>,
    error: Mutex<Option<String>>,
    /// Frames VideoToolbox reported as dropped (NULL sample buffer in the callback).
    dropped: std::sync::atomic::AtomicU64,
}

pub struct VtEncoder {
    session: CFRetained<VTCompressionSession>,
    shared: Arc<Shared>,
    /// Reference handed to VT as the callback refcon; reclaimed in `Drop`.
    refcon: *const Shared,
    width: usize,
    height: usize,
}

// The session is only driven from one thread at a time (&mut self); VT is internally thread-safe.
unsafe impl Send for VtEncoder {}

fn check(what: &str, status: i32) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("{what} failed: OSStatus {status}"))
    }
}

fn set_prop(s: &VTCompressionSession, key: &CFString, value: &CFType) -> Result<(), String> {
    check(&format!("VTSessionSetProperty({key})"), unsafe { VTSessionSetProperty(s, key, Some(value)) })
}

unsafe extern "C-unwind" fn output_cb(
    refcon: *mut c_void,
    _src_refcon: *mut c_void,
    status: i32,
    _flags: VTEncodeInfoFlags,
    sb: *mut CMSampleBuffer,
) {
    let shared = &*(refcon as *const Shared);
    if status != 0 {
        *shared.error.lock().unwrap() = Some(format!("encode callback OSStatus {status}"));
        return;
    }
    // NULL sample buffer == frame dropped by the encoder.
    let Some(sb) = sb.as_ref() else {
        shared.dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return;
    };
    if let Err(e) = handle_sample(shared, sb) {
        *shared.error.lock().unwrap() = Some(e);
    }
}

unsafe fn is_keyframe(sb: &CMSampleBuffer) -> bool {
    // A frame is a sync sample unless it carries kCMSampleAttachmentKey_NotSync = true.
    let Some(arr) = sb.sample_attachments_array(false) else { return true };
    let arr: &NSArray<NSDictionary<AnyObject, AnyObject>> =
        &*(&*arr as *const _ as *const NSArray<NSDictionary<AnyObject, AnyObject>>);
    let Some(dict) = arr.firstObject() else { return true };
    let key: &AnyObject = &*(kCMSampleAttachmentKey_NotSync as *const _ as *const AnyObject);
    !dict.objectForKey(key).is_some()
}

unsafe fn handle_sample(shared: &Shared, sb: &CMSampleBuffer) -> Result<(), String> {
    let keyframe = is_keyframe(sb);
    let pts = sb.presentation_time_stamp();
    let pts_us = (pts.value as i128 * 1_000_000 / (pts.timescale.max(1)) as i128) as i64;

    if keyframe {
        let fd = sb.format_description().ok_or("keyframe without format description")?;
        let mut cfg_ptrs = [(std::ptr::null::<u8>(), 0usize); 2];
        let mut count = 0usize;
        let mut nal_len = 0i32;
        for (i, slot) in cfg_ptrs.iter_mut().enumerate() {
            check(
                "GetH264ParameterSetAtIndex",
                CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    &fd, i, &mut slot.0, &mut slot.1, &mut count, &mut nal_len,
                ),
            )?;
        }
        if nal_len != 4 {
            return Err(format!("unexpected NAL length-prefix size {nal_len} (want 4)"));
        }
        let sps = std::slice::from_raw_parts(cfg_ptrs[0].0, cfg_ptrs[0].1).to_vec();
        let pps = std::slice::from_raw_parts(cfg_ptrs[1].0, cfg_ptrs[1].1).to_vec();
        let dims = objc2_core_media::CMVideoFormatDescriptionGetDimensions(&fd);
        let mut cfg = shared.config.lock().unwrap();
        // Keep the first config; a mid-stream change would need a new track (not supported yet).
        if cfg.is_none() {
            *cfg = Some(CodecConfig::h264(dims.width as u32, dims.height as u32, sps, pps));
        }
    }

    // The block buffer may be non-contiguous, so copy rather than borrow.
    let bb = sb.data_buffer().ok_or("sample without data buffer")?;
    let len = bb.data_length();
    let mut data = Vec::<u8>::with_capacity(len);
    check(
        "CMBlockBufferCopyDataBytes",
        bb.copy_data_bytes(0, len, NonNull::new_unchecked(data.as_mut_ptr().cast())),
    )?;
    data.set_len(len);

    shared.ready.lock().unwrap().push(Packet { pts_us, keyframe, data });
    Ok(())
}

impl VtEncoder {
    pub fn new(cfg: &EncoderConfig) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let refcon = Arc::into_raw(shared.clone());
        let mut session: *mut VTCompressionSession = std::ptr::null_mut();
        let status = unsafe {
            VTCompressionSession::create(
                None,
                cfg.width as i32,
                cfg.height as i32,
                kCMVideoCodecType_H264,
                None,
                None,
                None,
                Some(output_cb),
                refcon as *mut c_void,
                NonNull::from(&mut session),
            )
        };
        if let Err(e) = check("VTCompressionSessionCreate", status) {
            unsafe { drop(Arc::from_raw(refcon)) };
            return Err(format!("{e} (size {}x{})", cfg.width, cfg.height));
        }
        let session = unsafe { CFRetained::from_raw(NonNull::new(session).ok_or("null session")?) };

        let int = |v: i32| CFNumber::new_i32(v);
        unsafe {
            set_prop(&session, kVTCompressionPropertyKey_RealTime, CFBoolean::new(true))?;
            // B-frames would need DTS/ctts; Packet only has a pts.
            set_prop(&session, kVTCompressionPropertyKey_AllowFrameReordering, CFBoolean::new(false))?;
            set_prop(&session, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel)?;
            set_prop(&session, kVTCompressionPropertyKey_AverageBitRate, &int(cfg.bitrate_bps))?;
            set_prop(&session, kVTCompressionPropertyKey_ExpectedFrameRate, &int(cfg.fps))?;
            // Whichever limit hits first wins, which matters with a variable frame rate.
            set_prop(
                &session,
                kVTCompressionPropertyKey_MaxKeyFrameInterval,
                &int(cfg.fps * cfg.keyframe_secs),
            )?;
            set_prop(
                &session,
                kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
                &int(cfg.keyframe_secs),
            )?;
        }
        check("PrepareToEncodeFrames", unsafe { session.prepare_to_encode_frames() })?;

        Ok(Self { session, shared, refcon, width: cfg.width, height: cfg.height })
    }

    /// Change the target bitrate on the running session (no restart, no keyframe).
    pub fn set_bitrate(&mut self, bitrate_bps: i32) -> Result<(), String> {
        set_prop(&self.session, unsafe { kVTCompressionPropertyKey_AverageBitRate }, &CFNumber::new_i32(bitrate_bps))
    }

    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn drain(&self) -> Result<Vec<Packet>, String> {
        if let Some(e) = self.shared.error.lock().unwrap().take() {
            return Err(e);
        }
        Ok(std::mem::take(&mut *self.shared.ready.lock().unwrap()))
    }
}

impl Encoder for VtEncoder {
    type Frame = Frame;

    fn encode(&mut self, frame: Frame, pts_us: i64) -> Result<Vec<Packet>, String> {
        let pts = CMTime { value: pts_us, timescale: 1_000_000, flags: CMTimeFlags::Valid, epoch: 0 };
        // kCMTimeInvalid: we don't know the frame's duration up front.
        let invalid = CMTime { value: 0, timescale: 0, flags: CMTimeFlags(0), epoch: 0 };
        let mut info = VTEncodeInfoFlags::empty();
        check("VTCompressionSessionEncodeFrame", unsafe {
            self.session.encode_frame(&frame.0, pts, invalid, None, std::ptr::null_mut(), &mut info)
        })?;
        self.drain()
    }

    fn flush(&mut self) -> Result<Vec<Packet>, String> {
        let invalid = CMTime { value: 0, timescale: 0, flags: CMTimeFlags(0), epoch: 0 };
        check("VTCompressionSessionCompleteFrames", unsafe { self.session.complete_frames(invalid) })?;
        self.drain()
    }

    fn codec_config(&self) -> Option<CodecConfig> {
        self.shared.config.lock().unwrap().clone()
    }
}

impl Drop for VtEncoder {
    fn drop(&mut self) {
        // Invalidate first so no callback can run after the refcon is freed.
        unsafe {
            self.session.invalidate();
            drop(Arc::from_raw(self.refcon));
        }
    }
}
