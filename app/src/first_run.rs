//! Persistence for the first-run trust note's dismissed flag.
//!
//! Stored as a marker file at
//! `dirs::config_dir()/iroh-doctor-app/trust_note_dismissed`, next to the
//! other small state files (`identity.rs`, `endpoints.rs`). The file's
//! presence means the user dismissed the note; its contents are ignored
//! so a future version can add metadata without breaking older readers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const MARKER_FILE: &str = "trust_note_dismissed";

fn config_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("iroh-doctor-app"))
}

/// Returns true once the user has dismissed the first-run trust note.
/// Platforms without a config dir always report false, so the note shows
/// on every launch there; that is the safe default for a trust warning.
pub fn trust_note_dismissed() -> bool {
    config_dir().is_some_and(|dir| trust_note_dismissed_in(&dir))
}

/// Persists the dismissal so the note stays hidden on future launches.
pub fn dismiss_trust_note() -> Result<()> {
    let dir = config_dir().context("no config dir on this platform")?;
    dismiss_trust_note_in(&dir)
}

fn trust_note_dismissed_in(dir: &Path) -> bool {
    dir.join(MARKER_FILE).exists()
}

fn dismiss_trust_note_in(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(MARKER_FILE);
    std::fs::write(&path, b"1").with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh per-test directory under the system temp dir. Left to the
    /// test to remove; a unique name keeps parallel tests apart.
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "iroh-doctor-app-first-run-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_dir_reads_as_not_dismissed() {
        let dir = scratch_dir("missing");
        assert!(!trust_note_dismissed_in(&dir));
    }

    #[test]
    fn dismissal_roundtrips_and_is_idempotent() {
        let dir = scratch_dir("roundtrip");
        assert!(!trust_note_dismissed_in(&dir));
        dismiss_trust_note_in(&dir).unwrap();
        assert!(trust_note_dismissed_in(&dir));
        // A second dismissal must not fail (the marker already exists).
        dismiss_trust_note_in(&dir).unwrap();
        assert!(trust_note_dismissed_in(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
