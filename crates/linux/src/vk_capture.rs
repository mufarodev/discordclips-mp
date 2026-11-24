//! Direct zero-copy Vulkan swapchain DMA-BUF capture receiver.
//!
//! Connects to the dormant implicit Vulkan layer (`libclipsd_vk_layer.so`) over a private Linux
//! abstract Unix socket (`\0clipsd-vk-<uid>-<pid>`), receives 2 ping-pong DMA-BUF staging image
//! fds via `SCM_RIGHTS` once per swapchain, and emits `Frame::DmaBuf` directly into `GpuCudaBridge`
//! with zero portal dialogs, zero Alt-Tab leaks, and zero CPU copies.

use std::collections::{HashMap, HashSet};
use std::mem::{self, MaybeUninit};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::capture::{
    monotonic_us, start_audio_only, AudioOnlyHandle, CaptureConfig, Frame, PcmChunk,
};
use crate::gpu::{self, DmaBufFrame, DmaBufPlane};

const CLVK_MAGIC: u32 = 0x434C_564B; // 'CLVK'
const CLVK_MSG_SETUP: u32 = 1;
const CLVK_MSG_FRAME: u32 = 2;
const NUM_SLOTS: usize = 2;

const REPEAT_AFTER: Duration = Duration::from_millis(100);
const MIN_FRAME_GAP_US: i64 = 5_000;

#[repr(C)]
#[derive(Clone, Copy)]
struct ClvkHello {
    magic: u32,
    target_fps: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ClvkMsg {
    magic: u32,
    kind: u32,
    width: u32,
    height: u32,
    fourcc: u32,
    slot: u32,
    modifier: u64,
    stride: [u32; NUM_SLOTS],
    offset: [u32; NUM_SLOTS],
    pts_us: i64,
}

struct Item {
    frame: Frame,
    pts_us: i64,
}

pub struct VkCapture {
    pub width: usize,
    pub height: usize,
    pub target_pid: u32,
    rx: Receiver<Item>,
    audio_rx: Option<Receiver<PcmChunk>>,
    _audio_handle: Option<AudioOnlyHandle>,
    dropped: Arc<AtomicU64>,
    dropped_audio: Arc<AtomicU64>,
    last: Option<(i64, Instant)>,
    last_emitted_us: i64,
    stop: Arc<AtomicBool>,
    sock_fd: RawFd,
    rx_thread: Option<JoinHandle<()>>,
}

impl VkCapture {
    /// Attempt to attach to a hooked Vulkan process matching `pid` (exact PID or descendant
    /// process, e.g. Proton/Wine game child of a Steam launcher).
    ///
    /// Returns `None` if no Vulkan layer socket matches (caller should fall back to PipeWire
    /// portal capture). Returns `Some(Err(..))` only if a matching Vulkan layer was found but
    /// failed to initialize.
    pub fn try_start(cfg: &CaptureConfig, pid: Option<u32>) -> Option<Result<Self, String>> {
        if std::env::var("CLIPSD_NO_VK").ok().as_deref() == Some("1") {
            return None;
        }
        // Vulkan layer delivers GPU DMA-BUFs; require NVIDIA EGL/CUDA zero-copy support.
        gpu::query_dmabuf_modifiers()?;

        let (sock_name, target_pid) = find_vk_socket(pid)?;
        Some(Self::connect(cfg, &sock_name, target_pid))
    }

    fn connect(cfg: &CaptureConfig, sock_name: &str, target_pid: u32) -> Result<Self, String> {
        let fd = connect_abstract(sock_name)?;
        let hello = ClvkHello { magic: CLVK_MAGIC, target_fps: cfg.fps.clamp(1, 240) as u32 };
        let hello_bytes = unsafe {
            std::slice::from_raw_parts(&hello as *const ClvkHello as *const u8, mem::size_of::<ClvkHello>())
        };
        let n = unsafe {
            libc::send(fd.as_raw_fd(), hello_bytes.as_ptr() as *const libc::c_void, hello_bytes.len(), libc::MSG_NOSIGNAL)
        };
        if n != hello_bytes.len() as isize {
            return Err(format!("vulkan layer hello send failed (pid {target_pid})"));
        }

        let raw_fd = fd.as_raw_fd();
        let (tx, rx) = mpsc::sync_channel::<Item>(3);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, u32, u64), String>>();
        let dropped: Arc<AtomicU64> = Default::default();
        let stop = Arc::new(AtomicBool::new(false));

        let rx_thread = {
            let dropped = dropped.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("clipsd-vk-rx".into())
                .spawn(move || vk_rx_thread(fd, tx, ready_tx, dropped, stop))
                .map_err(|e| format!("spawn clipsd-vk-rx: {e}"))?
        };

        let (src_w, src_h, modifier) = match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                stop.store(true, Ordering::Relaxed);
                unsafe { libc::shutdown(raw_fd, libc::SHUT_RDWR) };
                let _ = rx_thread.join();
                return Err(e);
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                unsafe { libc::shutdown(raw_fd, libc::SHUT_RDWR) };
                let _ = rx_thread.join();
                return Err(format!("timed out waiting for Vulkan SETUP from pid {target_pid}"));
            }
        };

        let width = (src_w.min(cfg.max_width as u32).max(2)) & !1;
        let height = (((src_h as u64 * width as u64) / src_w.max(1) as u64) as u32).max(2) & !1;
        eprintln!(
            "clipsd-linux: Vulkan layer attached to pid {target_pid} ({src_w}x{src_h} mod=0x{modifier:x} -> encode {width}x{height}, zero-copy)"
        );

        let (audio_rx, dropped_audio, audio_handle) = if cfg.audio {
            match start_audio_only() {
                Ok((arx, da, h)) => (Some(arx), da, Some(h)),
                Err(e) => {
                    eprintln!("clipsd-linux: audio capture disabled: {e}");
                    (None, Default::default(), None)
                }
            }
        } else {
            (None, Default::default(), None)
        };

        Ok(Self {
            width: width as usize,
            height: height as usize,
            target_pid,
            rx,
            audio_rx,
            _audio_handle: audio_handle,
            dropped,
            dropped_audio,
            last: None,
            last_emitted_us: i64::MIN,
            stop,
            sock_fd: raw_fd,
            rx_thread: Some(rx_thread),
        })
    }

    pub fn flush_backlog(&mut self) {
        while self.rx.try_recv().is_ok() {}
        self.dropped.store(0, Ordering::Relaxed);
        self.last = None;
        self.last_emitted_us = i64::MIN;
    }

    pub fn take_audio(&mut self) -> Option<Receiver<PcmChunk>> {
        self.audio_rx.take()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn dropped_audio(&self) -> u64 {
        self.dropped_audio.load(Ordering::Relaxed)
    }

    pub fn poll_frame(&mut self, wait: Duration) -> Result<Option<(Frame, i64)>, String> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left.min(REPEAT_AFTER)) {
                Ok(item) => {
                    let pts_us = item.pts_us.max(self.last_emitted_us.saturating_add(MIN_FRAME_GAP_US));
                    self.last = Some((pts_us, Instant::now()));
                    self.last_emitted_us = pts_us;
                    return Ok(Some((item.frame, pts_us)));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some((pts, at)) = self.last {
                        let pts_us = (pts + at.elapsed().as_micros() as i64)
                            .max(self.last_emitted_us.saturating_add(MIN_FRAME_GAP_US));
                        self.last_emitted_us = pts_us;
                        return Ok(Some((Frame::RepeatLastGpu, pts_us)));
                    }
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("Vulkan capture stream closed (pid {})", self.target_pid));
                }
            }
        }
    }
}

impl Drop for VkCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        unsafe { libc::shutdown(self.sock_fd, libc::SHUT_RDWR) };
        if let Some(t) = self.rx_thread.take() {
            let _ = t.join();
        }
    }
}

fn vk_rx_thread(
    sock: OwnedFd,
    tx: SyncSender<Item>,
    ready_tx: mpsc::Sender<Result<(u32, u32, u64), String>>,
    dropped: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
) {
    let mut slot_fds: Vec<OwnedFd> = Vec::new();
    let mut width = 0u32;
    let mut height = 0u32;
    let mut fourcc = 0u32;
    let mut modifier = 0u64;
    let mut strides = [0u32; NUM_SLOTS];
    let mut offsets = [0u32; NUM_SLOTS];
    let mut ready_sent = false;

    while !stop.load(Ordering::Relaxed) {
        let (msg, fds) = match recv_msg_with_fds(sock.as_raw_fd()) {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(e) => {
                if !ready_sent {
                    let _ = ready_tx.send(Err(e));
                }
                break;
            }
        };
        if msg.magic != CLVK_MAGIC {
            continue;
        }
        match msg.kind {
            CLVK_MSG_SETUP => {
                if fds.len() < NUM_SLOTS || msg.width == 0 || msg.height == 0 {
                    continue;
                }
                slot_fds = fds;
                width = msg.width;
                height = msg.height;
                fourcc = msg.fourcc;
                modifier = msg.modifier;
                strides = msg.stride;
                offsets = msg.offset;
                if !ready_sent {
                    ready_sent = true;
                    let _ = ready_tx.send(Ok((width, height, modifier)));
                } else {
                    eprintln!("clipsd-linux: Vulkan swapchain resized to {width}x{height} (mod=0x{modifier:x})");
                }
            }
            CLVK_MSG_FRAME => {
                let s = msg.slot as usize;
                if s >= slot_fds.len() || width == 0 || height == 0 {
                    continue;
                }
                let dup_fd = unsafe { libc::dup(slot_fds[s].as_raw_fd()) };
                if dup_fd < 0 {
                    continue;
                }
                let frame = Frame::DmaBuf(DmaBufFrame {
                    width,
                    height,
                    fourcc,
                    modifier,
                    planes: vec![DmaBufPlane {
                        fd: unsafe { OwnedFd::from_raw_fd(dup_fd) },
                        offset: offsets[s],
                        stride: strides[s],
                    }],
                });
                let pts_us = if msg.pts_us > 0 { msg.pts_us } else { monotonic_us() };
                if tx.try_send(Item { frame, pts_us }).is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }
    if !ready_sent {
        let _ = ready_tx.send(Err("Vulkan layer disconnected before SETUP".into()));
    }
}

fn connect_abstract(name: &str) -> Result<OwnedFd, String> {
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if raw < 0 {
        return Err(format!("socket: {}", std::io::Error::last_os_error()));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };

    let mut addr: libc::sockaddr_un = unsafe { mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = name.as_bytes();
    if bytes.len() + 1 >= addr.sun_path.len() {
        return Err("abstract socket name too long".into());
    }
    // sun_path[0] = 0 marks an abstract Unix socket on Linux.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), addr.sun_path.as_mut_ptr().add(1) as *mut u8, bytes.len());
    }
    let len = (mem::offset_of!(libc::sockaddr_un, sun_path) + 1 + bytes.len()) as libc::socklen_t;

    let rc = unsafe {
        libc::connect(fd.as_raw_fd(), &addr as *const libc::sockaddr_un as *const libc::sockaddr, len)
    };
    if rc != 0 {
        return Err(format!("connect @{name}: {}", std::io::Error::last_os_error()));
    }
    Ok(fd)
}

fn recv_msg_with_fds(fd: RawFd) -> Result<Option<(ClvkMsg, Vec<OwnedFd>)>, String> {
    let mut msg = MaybeUninit::<ClvkMsg>::zeroed();
    let mut iov = libc::iovec {
        iov_base: msg.as_mut_ptr() as *mut libc::c_void,
        iov_len: mem::size_of::<ClvkMsg>(),
    };
    let mut cbuf = [0u8; 128];
    let mut hdr: libc::msghdr = unsafe { mem::zeroed() };
    hdr.msg_iov = &mut iov;
    hdr.msg_iovlen = 1;
    hdr.msg_control = cbuf.as_mut_ptr() as *mut libc::c_void;
    hdr.msg_controllen = cbuf.len();

    let n = unsafe { libc::recvmsg(fd, &mut hdr, libc::MSG_CMSG_CLOEXEC) };
    if n == 0 {
        return Ok(None);
    }
    if n < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            return recv_msg_with_fds(fd);
        }
        return Err(format!("recvmsg: {err}"));
    }
    if (n as usize) != mem::size_of::<ClvkMsg>() {
        return Ok(None);
    }

    let mut fds = Vec::new();
    unsafe {
        let mut cm = libc::CMSG_FIRSTHDR(&hdr);
        while !cm.is_null() {
            if (*cm).cmsg_level == libc::SOL_SOCKET && (*cm).cmsg_type == libc::SCM_RIGHTS {
                let data_len = (*cm).cmsg_len.saturating_sub(libc::CMSG_LEN(0) as usize);
                let count = data_len / mem::size_of::<RawFd>();
                let ptr = libc::CMSG_DATA(cm) as *const RawFd;
                for i in 0..count {
                    let rfd = ptr::read_unaligned(ptr.add(i));
                    if rfd >= 0 {
                        fds.push(OwnedFd::from_raw_fd(rfd));
                    }
                }
            }
            cm = libc::CMSG_NXTHDR(&hdr, cm);
        }
    }
    Ok(Some((unsafe { msg.assume_init() }, fds)))
}

fn list_active_vk_sockets() -> Vec<(String, u32)> {
    let uid = unsafe { libc::getuid() };
    let prefix = format!("@clipsd-vk-{uid}-");
    let Ok(txt) = std::fs::read_to_string("/proc/net/unix") else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in txt.lines().skip(1) {
        let Some(path) = line.split_whitespace().last() else { continue };
        let Some(pid_str) = path.strip_prefix(&prefix) else { continue };
        let Ok(pid) = pid_str.parse::<u32>() else { continue };
        let name = path.trim_start_matches('@').to_string();
        if seen.insert(pid) {
            out.push((name, pid));
        }
    }
    out
}

fn read_ppid(pid: u32) -> Option<u32> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = s.rsplit_once(')')?.1;
    after.split_whitespace().nth(1)?.parse().ok()
}

fn is_descendant(child: u32, ancestor: u32) -> bool {
    let mut cur = child;
    for _ in 0..16 {
        let Some(p) = read_ppid(cur) else { return false };
        if p == ancestor {
            return true;
        }
        if p <= 1 || p == cur {
            return false;
        }
        cur = p;
    }
    false
}

fn find_vk_socket(target_pid: Option<u32>) -> Option<(String, u32)> {
    let active = list_active_vk_sockets();
    if active.is_empty() {
        return None;
    }
    if let Some(pid) = target_pid {
        // 1. Exact PID match
        if let Some(hit) = active.iter().find(|(_, p)| *p == pid) {
            return Some(hit.clone());
        }
        // 2. Descendant match (e.g. Steam/Proton wrapper -> game .exe)
        if let Some(hit) = active.iter().find(|(_, p)| is_descendant(*p, pid) || is_descendant(pid, *p)) {
            return Some(hit.clone());
        }
        return None;
    }
    // Standalone CLI convenience: attach to any running Vulkan app if CLIPSD_VK_ANY=1.
    if std::env::var("CLIPSD_VK_ANY").ok().as_deref() == Some("1") {
        let mut by_pid: HashMap<u32, String> = active.into_iter().map(|(n, p)| (p, n)).collect();
        let max_pid = *by_pid.keys().max()?;
        return by_pid.remove(&max_pid).map(|n| (n, max_pid));
    }
    None
}
