//! Persistent list of endpoints we have connected to before.
//!
//! Stored at `dirs::config_dir()/iroh-doctor-app/endpoints.json`. Loaded
//! once at app start, written through after every mutation. The file
//! format is a small JSON array of [`Endpoint`] records; missing or
//! malformed files load as an empty list so a corrupted file is not fatal.
//!
//! Earlier versions stored this list under the `iroh-pong` directory,
//! first as `devices.json` and later as `endpoints.json`. [`load`] falls
//! back to both legacy locations so existing entries survive the rename;
//! the next mutation rewrites them to the current path.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// An endpoint the user has connected to at least once.
///
/// Unknown JSON fields are ignored and every field except `id` defaults when
/// absent, so a future schema bump that adds fields can still be opened by an
/// older binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// 64-hex iroh endpoint id.
    pub id: String,
    /// User-chosen label. Empty string means "unnamed"; the UI shows the
    /// short id in that case.
    #[serde(default)]
    pub name: String,
    /// Unix seconds when we first saved this endpoint.
    #[serde(default)]
    pub first_seen: u64,
    /// Unix seconds of the most recent successful connect.
    #[serde(default)]
    pub last_seen: u64,
}

fn endpoints_path() -> Result<PathBuf> {
    Ok(crate::identity::config_dir()?.join("endpoints.json"))
}

/// Pre-rename storage locations, newest first, read by [`load`] when the
/// current `endpoints.json` is absent so an upgrade does not drop the
/// user's saved list. Both live under the old `iroh-pong` directory:
/// `endpoints.json` from before the app rename, and `devices.json` from
/// before the devices -> endpoints rename.
fn legacy_paths() -> Vec<PathBuf> {
    let Ok(old) = crate::identity::legacy_config_dir() else {
        return Vec::new();
    };
    vec![old.join("endpoints.json"), old.join("devices.json")]
}

/// Current time as Unix seconds, saturating to 0 on a pre-epoch clock.
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Loads the list from disk. Returns an empty list when the file is
/// missing or unreadable so first-run users do not see an error.
pub fn load() -> Vec<Endpoint> {
    // Prefer the current file; fall back to the pre-rename locations so
    // existing entries are not lost on upgrade.
    let mut text = endpoints_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok());
    if text.is_none() {
        for legacy in legacy_paths() {
            if let Ok(contents) = std::fs::read_to_string(&legacy) {
                text = Some(contents);
                break;
            }
        }
    }
    let Some(text) = text else {
        return Vec::new();
    };
    match parse(&text) {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(err = %e, "endpoints.json malformed, ignoring");
            Vec::new()
        }
    }
}

/// Writes the list to disk. Errors propagate so the caller can surface
/// them in the UI.
pub fn save(endpoints: &[Endpoint]) -> Result<()> {
    let path = endpoints_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = serialize(endpoints);
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Records a successful connection. Inserts a new entry if `id` is not
/// already known, or updates `last_seen` on an existing one. Returns the
/// updated list so the caller can persist it.
pub fn record_connection(mut endpoints: Vec<Endpoint>, id: &str) -> Vec<Endpoint> {
    let now = now_secs();
    if let Some(existing) = endpoints.iter_mut().find(|d| d.id == id) {
        existing.last_seen = now;
    } else {
        endpoints.push(Endpoint {
            id: id.to_string(),
            name: String::new(),
            first_seen: now,
            last_seen: now,
        });
    }
    endpoints
}

/// Renames a endpoint by id. Returns the updated list. Empty name clears
/// the user label so the UI falls back to the short id.
pub fn rename(mut endpoints: Vec<Endpoint>, id: &str, name: String) -> Vec<Endpoint> {
    if let Some(d) = endpoints.iter_mut().find(|d| d.id == id) {
        d.name = name.trim().to_string();
    }
    endpoints
}

/// Removes a endpoint by id. Returns the updated list.
pub fn remove(mut endpoints: Vec<Endpoint>, id: &str) -> Vec<Endpoint> {
    endpoints.retain(|d| d.id != id);
    endpoints
}

/// Writes the in-memory list as pretty-printed JSON.
pub(crate) fn serialize(endpoints: &[Endpoint]) -> String {
    // An array of plain structs cannot fail to encode.
    serde_json::to_string_pretty(endpoints).expect("endpoints encode as JSON")
}

fn parse(text: &str) -> Result<Vec<Endpoint>> {
    serde_json::from_str(text).context("parsing endpoints JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: &str, name: &str, first: u64, last: u64) -> Endpoint {
        Endpoint {
            id: id.into(),
            name: name.into(),
            first_seen: first,
            last_seen: last,
        }
    }

    #[test]
    fn empty_list_roundtrips() {
        let s = serialize(&[]);
        let parsed = parse(&s).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn one_endpoint_roundtrips() {
        let list = vec![d("abc", "Phone", 100, 200)];
        let parsed = parse(&serialize(&list)).unwrap();
        assert_eq!(parsed, list);
    }

    #[test]
    fn many_endpoints_roundtrip() {
        let list = vec![
            d("abc", "Phone", 100, 200),
            d("def", "", 50, 60),
            d("ghi", "Laptop \"work\"", 1, 2),
        ];
        let parsed = parse(&serialize(&list)).unwrap();
        assert_eq!(parsed, list);
    }

    #[test]
    fn record_connection_inserts_when_new() {
        let list = record_connection(Vec::new(), "abc");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "abc");
        assert_eq!(list[0].first_seen, list[0].last_seen);
    }

    #[test]
    fn record_connection_updates_last_seen_on_revisit() {
        let mut list = vec![d("abc", "Phone", 100, 100)];
        // Bypass the SystemTime call by directly testing the path.
        list = record_connection(list, "abc");
        assert_eq!(list.len(), 1);
        assert!(list[0].last_seen >= 100);
        assert_eq!(list[0].first_seen, 100);
        assert_eq!(list[0].name, "Phone");
    }

    #[test]
    fn rename_keeps_other_fields() {
        let list = rename(vec![d("abc", "Phone", 100, 200)], "abc", "Tablet".into());
        assert_eq!(list[0].name, "Tablet");
        assert_eq!(list[0].first_seen, 100);
    }

    #[test]
    fn rename_trims_whitespace() {
        let list = rename(
            vec![d("abc", "Phone", 100, 200)],
            "abc",
            "  Laptop  ".into(),
        );
        assert_eq!(list[0].name, "Laptop");
    }

    #[test]
    fn rename_to_empty_clears_label() {
        let list = rename(vec![d("abc", "Phone", 100, 200)], "abc", String::new());
        assert_eq!(list[0].name, "");
    }

    #[test]
    fn remove_drops_matching_id() {
        let list = remove(
            vec![d("a", "", 0, 0), d("b", "", 0, 0), d("c", "", 0, 0)],
            "b",
        );
        let ids: Vec<&str> = list.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
    }

    #[test]
    fn remove_noop_when_id_absent() {
        let list = remove(vec![d("a", "", 0, 0)], "missing");
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn parse_ignores_unknown_fields() {
        let text = r#"[{"id":"abc","name":"P","first_seen":1,"last_seen":2,"extra":"ignore"}]"#;
        let list = parse(text).unwrap();
        assert_eq!(list[0].id, "abc");
    }

    #[test]
    fn parse_rejects_missing_id() {
        let text = r#"[{"name":"P","first_seen":1,"last_seen":2}]"#;
        assert!(parse(text).is_err());
    }
}
