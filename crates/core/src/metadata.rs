//! Default clip/screenshot metadata helpers shared across platforms.

use std::sync::atomic::{AtomicU64, Ordering};

/// Unique-enough id for clip/screenshot metadata (no RNG dependency): time + pid + counter.
pub fn new_id() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{ns:x}-{:x}-{:x}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

/// Host metadata plus defaults: Discord's loader requires a string `id` in screenshot metadata
/// and clips carry `id` + `version`. Host-supplied keys always win.
pub fn with_defaults(meta: Option<serde_json::Value>) -> serde_json::Value {
    let mut m = match meta {
        Some(serde_json::Value::Object(o)) => o,
        _ => serde_json::Map::new(),
    };
    m.entry("id").or_insert_with(|| new_id().into());
    m.entry("version").or_insert(1.into());
    serde_json::Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_populate_id_and_version_without_overwriting_host_keys() {
        let a = with_defaults(None);
        assert!(a["id"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(a["version"], 1);

        let b = with_defaults(Some(serde_json::json!({"id": "custom", "version": 2, "game": "Celeste"})));
        assert_eq!(b["id"], "custom");
        assert_eq!(b["version"], 2);
        assert_eq!(b["game"], "Celeste");
    }
}
