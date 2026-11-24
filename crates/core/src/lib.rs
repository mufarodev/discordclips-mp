//! Platform-independent core: the two traits platforms implement, plus the ring buffer.
use std::collections::VecDeque;

/// One encoded video packet (AVCC, as VideoToolbox emits; Linux encoders must match).
#[derive(Clone, Debug)]
pub struct Packet {
    pub pts_us: i64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

/// Platform seam #1: screen capture. Frames stay GPU-resident; `Frame` is platform-defined.
pub trait Capture {
    type Frame;
    fn next_frame(&mut self) -> Result<(Self::Frame, i64 /* pts_us */), String>;
}

/// Platform seam #2: hardware encoder.
pub trait Encoder {
    type Frame;
    fn encode(&mut self, frame: Self::Frame, pts_us: i64) -> Result<Vec<Packet>, String>;
}

/// Rolling buffer of encoded packets, trimmed on GOP boundaries so it always starts on a keyframe.
pub struct Ring {
    window_us: i64,
    pkts: VecDeque<Packet>,
}

impl Ring {
    pub fn new(window_secs: u32) -> Self {
        Self { window_us: window_secs as i64 * 1_000_000, pkts: VecDeque::new() }
    }

    pub fn push(&mut self, p: Packet) {
        if self.pkts.is_empty() && !p.keyframe {
            return; // can't start a clip mid-GOP
        }
        self.pkts.push_back(p);
        let newest = self.pkts.back().unwrap().pts_us;
        // Drop whole GOPs while the *next* keyframe still leaves a full window.
        while let Some(k) = self.pkts.iter().skip(1).position(|p| p.keyframe).map(|i| i + 1) {
            if newest - self.pkts[k].pts_us >= self.window_us {
                self.pkts.drain(..k);
            } else {
                break;
            }
        }
    }

    /// Last `secs` of video, starting at the keyframe at-or-before the cutoff (so >= `secs`).
    pub fn snapshot(&self, secs: u32) -> Vec<Packet> {
        let Some(newest) = self.pkts.back() else { return vec![] };
        let cutoff = newest.pts_us - secs as i64 * 1_000_000;
        let start = self.pkts.iter().rposition(|p| p.keyframe && p.pts_us <= cutoff).unwrap_or(0);
        self.pkts.iter().skip(start).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn feed(r: &mut Ring, secs: i64) {
        // 1 fps, keyframe every 2s
        for s in 0..secs {
            r.push(Packet { pts_us: s * 1_000_000, keyframe: s % 2 == 0, data: vec![] });
        }
    }
    #[test]
    fn starts_on_keyframe_and_covers_window() {
        let mut r = Ring::new(10);
        feed(&mut r, 100);
        assert!(r.pkts[0].keyframe);
        let span = r.pkts.back().unwrap().pts_us - r.pkts[0].pts_us;
        assert!(span >= 10_000_000 && span < 13_000_000, "span={span}");
    }
    #[test]
    fn snapshot_is_at_least_requested_and_keyframe_aligned() {
        let mut r = Ring::new(30);
        feed(&mut r, 100);
        let s = r.snapshot(5);
        assert!(s[0].keyframe);
        assert!(s.last().unwrap().pts_us - s[0].pts_us >= 5_000_000);
    }
    #[test]
    fn drops_leading_non_keyframes() {
        let mut r = Ring::new(10);
        r.push(Packet { pts_us: 0, keyframe: false, data: vec![] });
        assert!(r.snapshot(5).is_empty());
    }
}
