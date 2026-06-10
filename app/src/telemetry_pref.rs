//! Persistence for the user's telemetry opt-out.
//!
//! Stored as a marker file `telemetry_disabled` in the app config directory,
//! alongside `secret_key.bin`, `api_secret.txt`, and `trust_note_dismissed`.
//! Telemetry is on by default, so the file's presence means the user turned it
//! off. Its contents are ignored, so a future version can add metadata without
//! breaking older readers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::identity;

const MARKER_FILE: &str = "telemetry_disabled";

fn config_dir() -> Option<PathBuf> {
    identity::config_dir().ok()
}

/// Returns true when the user has turned telemetry off.
///
/// Telemetry is on by default, so a missing marker (including platforms
/// without a config dir) reads as enabled.
pub fn telemetry_disabled() -> bool {
    config_dir().is_some_and(|dir| telemetry_disabled_in(&dir))
}

/// Persists the user's telemetry on/off choice: writing the marker turns
/// telemetry off, removing it turns it back on.
///
/// # Errors
///
/// Returns an error if the config directory cannot be created or the marker
/// file cannot be written or removed.
pub fn set_telemetry_disabled(disabled: bool) -> Result<()> {
    let dir = config_dir().context("no config dir on this platform")?;
    set_telemetry_disabled_in(&dir, disabled)
}

fn telemetry_disabled_in(dir: &Path) -> bool {
    dir.join(MARKER_FILE).exists()
}

fn set_telemetry_disabled_in(dir: &Path, disabled: bool) -> Result<()> {
    let path = dir.join(MARKER_FILE);
    if disabled {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(&path, b"1").with_context(|| format!("writing {}", path.display()))?;
    } else if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh per-test directory under the system temp dir. Left to the test to
    /// remove; a unique name keeps parallel tests apart.
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "iroh-doctor-app-telemetry-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_marker_reads_as_enabled() {
        let dir = scratch_dir("missing");
        assert!(!telemetry_disabled_in(&dir));
    }

    #[test]
    fn toggle_roundtrips_and_is_idempotent() {
        let dir = scratch_dir("roundtrip");
        // Default with no marker: telemetry on.
        assert!(!telemetry_disabled_in(&dir));
        // Turn off, then off again: the second write must not fail.
        set_telemetry_disabled_in(&dir, true).unwrap();
        assert!(telemetry_disabled_in(&dir));
        set_telemetry_disabled_in(&dir, true).unwrap();
        assert!(telemetry_disabled_in(&dir));
        // Turn back on, then on again: removing an absent marker must not fail.
        set_telemetry_disabled_in(&dir, false).unwrap();
        assert!(!telemetry_disabled_in(&dir));
        set_telemetry_disabled_in(&dir, false).unwrap();
        assert!(!telemetry_disabled_in(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
