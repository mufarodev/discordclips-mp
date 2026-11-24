//! M6: xdg-desktop-portal ScreenCast session & restore-token persistence.
//!
//! Opens a Wayland/X11-agnostic ScreenCast portal session via `ashpd`, persists the portal
//! restore token under `$XDG_CONFIG_HOME/clipsd/restore_token` (so subsequent launches avoid
//! re-prompting), and returns the PipeWire file descriptor + video node ID.

use std::os::fd::OwnedFd;
use std::path::PathBuf;

use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;

/// Active portal ScreenCast handle. Keep `_session` alive for as long as PipeWire streams run.
pub struct PortalStream {
    pub pipewire_fd: OwnedFd,
    pub node_id: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    _session: ashpd::desktop::Session<'static, Screencast<'static>>,
    /// Keeps the D-Bus tasks (zbus runs on tokio here) alive for the whole session. Declared
    /// after `_session` so the session is dropped first.
    _rt: tokio::runtime::Runtime,
}

fn token_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg).join("clipsd").join("restore_token"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("clipsd").join("restore_token"))
}

fn load_restore_token() -> Option<String> {
    let p = token_path()?;
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn save_restore_token(token: &str) {
    let Some(p) = token_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(p, token);
}

/// Request a ScreenCast stream via xdg-desktop-portal (blocking wrapper around a one-shot tokio rt).
pub fn open_screencast() -> Result<PortalStream, String> {
    // Multi-thread (1 worker): the runtime must keep driving zbus after `block_on` returns,
    // otherwise the portal session's D-Bus connection stalls and the stream dies.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;

    let portal = rt.block_on(async {
        // Leak the proxy for the lifetime of the session (one per recording; tiny allocation)
        // so `PortalStream` can own the `'static` Session handle cleanly.
        let proxy: &'static Screencast<'static> = Box::leak(Box::new(
            Screencast::new().await.map_err(|e| format!("ScreenCast portal unavailable: {e}"))?,
        ));
        let session = proxy
            .create_session()
            .await
            .map_err(|e| format!("create_session: {e}"))?;

        let token = load_restore_token();
        // Default to Monitor so restore tokens persist across game restarts and switches;
        // set CLIPSD_SOURCE=window|any to opt into per-window portal selection.
        let source_types = match std::env::var("CLIPSD_SOURCE").ok().as_deref() {
            Some("window") => SourceType::Window.into(),
            Some("any") => SourceType::Monitor | SourceType::Window,
            _ => SourceType::Monitor.into(),
        };
        proxy
            .select_sources(
                &session,
                CursorMode::Embedded,
                source_types,
                false,
                token.as_deref(),
                PersistMode::ExplicitlyRevoked,
            )
            .await
            .map_err(|e| format!("select_sources: {e}"))?;

        let response = proxy
            .start(&session, None)
            .await
            .map_err(|e| format!("portal start: {e}"))?
            .response()
            .map_err(|e| format!("portal cancelled or failed: {e}"))?;

        if let Some(new_token) = response.restore_token() {
            save_restore_token(new_token);
        }

        let stream = response
            .streams()
            .first()
            .ok_or_else(|| "portal returned no video streams".to_string())?;
        let node_id = stream.pipe_wire_node_id();
        let (width, height) = stream
            .size()
            .map(|(w, h)| (Some(w as u32), Some(h as u32)))
            .unwrap_or((None, None));

        let pipewire_fd = proxy
            .open_pipe_wire_remote(&session)
            .await
            .map_err(|e| format!("open_pipe_wire_remote: {e}"))?;

        Ok::<_, String>((pipewire_fd, node_id, width, height, session))
    })
    .map(|(pipewire_fd, node_id, width, height, session)| PortalStream { pipewire_fd, node_id, width, height, _session: session, _rt: rt })?;
    Ok(portal)
}

/// Take a non-interactive screenshot via xdg-desktop-portal (fallback when no frame is buffered).
/// Returns the screenshot file path (a PNG owned by the caller, who must delete it).
pub fn portal_screenshot_path() -> Result<std::path::PathBuf, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;

    rt.block_on(async {
        let uri = ashpd::desktop::screenshot::Screenshot::request()
            .interactive(false)
            .modal(false)
            .send()
            .await
            .map_err(|e| format!("screenshot portal: {e}"))?
            .response()
            .map_err(|e| format!("screenshot response: {e}"))?;
        let path = uri
            .uri()
            .to_file_path()
            .map_err(|_| format!("non-file screenshot URI: {}", uri.uri()))?;
        Ok(path)
    })
}
