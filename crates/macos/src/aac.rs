//! AAC-LC encoder on top of AudioToolbox's `AudioConverter`.
//!
//! Input is interleaved f32 PCM with host-clock timestamps. Output packets are raw AAC frames
//! (1024 samples, no ADTS) on a gapless timeline: capture gaps are filled with silence so the
//! audio track never drifts against video.

use std::ffi::c_void;
use std::ptr::NonNull;

use clipsd_core::{AudioConfig, Packet, AAC_FRAME_SAMPLES};
use objc2_audio_toolbox::{
    kAudioConverterEncodeBitRate, AudioConverterDispose, AudioConverterFillComplexBuffer, AudioConverterNew,
    AudioConverterRef, AudioConverterSetProperty,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC, AudioBuffer,
    AudioBufferList, AudioStreamBasicDescription, AudioStreamPacketDescription,
};

/// Apple's AAC-LC encoder delay: decoded sample k corresponds to input sample k - 2112.
/// Without an MP4 edit list we compensate by shifting packet timestamps earlier.
const ENCODER_DELAY_FRAMES: i64 = 2112;
/// Gaps longer than this are filled with silence; anything longer than the cap re-anchors.
const GAP_TOLERANCE_US: i64 = 40_000;
const MAX_FILL_US: i64 = 10_000_000;
const NO_MORE_INPUT: i32 = 1; // sentinel returned from the input proc

pub struct AacEncoder {
    conv: AudioConverterRef,
    pub cfg: AudioConfig,
    /// Timeline origin: host-clock µs of input frame 0 (after the last re-anchor).
    origin_us: Option<i64>,
    frames_in: i64,
    packets_out: i64,
}

// AudioConverter is only touched through &mut self.
unsafe impl Send for AacEncoder {}

fn check(what: &str, st: i32) -> Result<(), String> {
    if st == 0 { Ok(()) } else { Err(format!("{what} failed: OSStatus {st}")) }
}

struct InputState {
    data: *mut f32,
    frames: u32,
    channels: u32,
    given: bool,
}

unsafe extern "C-unwind" fn input_proc(
    _conv: AudioConverterRef,
    n_packets: NonNull<u32>,
    abl: NonNull<AudioBufferList>,
    _desc: *mut *mut AudioStreamPacketDescription,
    user: *mut c_void,
) -> i32 {
    let st = &mut *(user as *mut InputState);
    if st.given || st.frames == 0 {
        *n_packets.as_ptr() = 0;
        return NO_MORE_INPUT;
    }
    st.given = true;
    *n_packets.as_ptr() = st.frames;
    let l = abl.as_ptr();
    (*l).mNumberBuffers = 1;
    (*l).mBuffers[0] = AudioBuffer {
        mNumberChannels: st.channels,
        mDataByteSize: st.frames * st.channels * 4,
        mData: st.data.cast(),
    };
    0
}

impl AacEncoder {
    pub fn new(sample_rate: u32, channels: u8, bitrate_bps: u32) -> Result<Self, String> {
        let ch = channels as u32;
        let mut src = AudioStreamBasicDescription {
            mSampleRate: sample_rate as f64,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 4 * ch,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4 * ch,
            mChannelsPerFrame: ch,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        let mut dst = AudioStreamBasicDescription {
            mSampleRate: sample_rate as f64,
            mFormatID: kAudioFormatMPEG4AAC,
            mFormatFlags: 2, // kMPEG4Object_AAC_LC
            mBytesPerPacket: 0,
            mFramesPerPacket: AAC_FRAME_SAMPLES,
            mBytesPerFrame: 0,
            mChannelsPerFrame: ch,
            mBitsPerChannel: 0,
            mReserved: 0,
        };
        let mut conv: AudioConverterRef = std::ptr::null_mut();
        check("AudioConverterNew", unsafe {
            AudioConverterNew(NonNull::from(&mut src), NonNull::from(&mut dst), NonNull::from(&mut conv))
        })?;
        let mut br = bitrate_bps;
        check("set EncodeBitRate", unsafe {
            AudioConverterSetProperty(conv, kAudioConverterEncodeBitRate, 4, NonNull::from(&mut br).cast())
        })?;
        Ok(Self {
            conv,
            cfg: AudioConfig { sample_rate, channels, bitrate_bps },
            origin_us: None,
            frames_in: 0,
            packets_out: 0,
        })
    }

    fn frame_us(&self, frames: i64) -> i64 {
        frames * 1_000_000 / self.cfg.sample_rate as i64
    }

    /// Feed interleaved f32 PCM whose first frame is at `pts_us`; returns finished AAC packets.
    pub fn push(&mut self, pts_us: i64, pcm: &[f32]) -> Result<Vec<Packet>, String> {
        let ch = self.cfg.channels as usize;
        let frames = (pcm.len() / ch) as i64;
        let mut buf: Vec<f32>;
        let mut input: &[f32] = pcm;

        match self.origin_us {
            None => self.origin_us = Some(pts_us),
            Some(origin) => {
                let expected = origin + self.frame_us(self.frames_in);
                let gap = pts_us - expected;
                if gap > MAX_FILL_US {
                    // Long outage: flush what we have, then start a new timeline.
                    let mut out = self.encode(&[])?;
                    self.origin_us = Some(pts_us);
                    self.frames_in = 0;
                    self.packets_out = 0;
                    out.extend(self.push(pts_us, pcm)?);
                    return Ok(out);
                } else if gap > GAP_TOLERANCE_US {
                    let fill = (gap * self.cfg.sample_rate as i64 / 1_000_000) as usize;
                    buf = vec![0.0; (fill + frames as usize) * ch];
                    buf[fill * ch..].copy_from_slice(pcm);
                    input = &buf;
                }
            }
        }
        let n = (input.len() / ch) as i64;
        self.frames_in += n;
        let mut owned = input.to_vec();
        self.encode(&mut owned)
    }

    fn encode(&mut self, pcm: &[f32]) -> Result<Vec<Packet>, String> {
        let ch = self.cfg.channels as u32;
        let frames = (pcm.len() / ch as usize) as u32;
        // Upper bound on packets this call can produce (+2 for priming/lookahead).
        let cap = (frames / AAC_FRAME_SAMPLES + 2) as usize;
        const MAX_PACKET: usize = 1536 * 2;
        let mut out = vec![0u8; cap * MAX_PACKET];
        let mut descs = vec![AudioStreamPacketDescription { mStartOffset: 0, mVariableFramesInPacket: 0, mDataByteSize: 0 }; cap];
        let mut abl = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [AudioBuffer { mNumberChannels: ch, mDataByteSize: out.len() as u32, mData: out.as_mut_ptr().cast() }],
        };
        let mut n_out = cap as u32;
        let mut st = InputState { data: pcm.as_ptr() as *mut f32, frames, channels: ch, given: false };
        let r = unsafe {
            AudioConverterFillComplexBuffer(
                self.conv,
                Some(input_proc),
                (&mut st as *mut InputState).cast(),
                NonNull::from(&mut n_out),
                NonNull::from(&mut abl),
                descs.as_mut_ptr(),
            )
        };
        if r != 0 && r != NO_MORE_INPUT {
            return Err(format!("AudioConverterFillComplexBuffer failed: OSStatus {r}"));
        }
        let origin = self.origin_us.unwrap_or(0);
        let mut pkts = Vec::with_capacity(n_out as usize);
        for d in &descs[..n_out as usize] {
            let (o, l) = (d.mStartOffset as usize, d.mDataByteSize as usize);
            let idx = self.packets_out * AAC_FRAME_SAMPLES as i64 - ENCODER_DELAY_FRAMES;
            self.packets_out += 1;
            pkts.push(Packet { pts_us: origin + self.frame_us(idx), keyframe: true, data: out[o..o + l].to_vec() });
        }
        Ok(pkts)
    }
}

impl Drop for AacEncoder {
    fn drop(&mut self) {
        unsafe { AudioConverterDispose(self.conv) };
    }
}
