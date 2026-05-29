//! Shared on-disk persistence for an iroh secret key.
//!
//! Both the cli and the app keep a stable endpoint id across runs by
//! persisting a raw 32-byte secret key. This module owns that read/create
//! primitive; each binary picks its own path and layers on any
//! binary-specific behaviour (legacy directories, OpenSSH keypairs, or
//! hex/random overrides).

use std::path::Path;

use anyhow::{Context, Result};
use iroh::SecretKey;

/// Reads a raw 32-byte secret key from `path`. Returns `None` when the file
/// is missing or is not exactly 32 bytes.
#[must_use]
pub fn read_secret_key(path: &Path) -> Option<SecretKey> {
    let bytes = std::fs::read(path).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    Some(SecretKey::from_bytes(&arr))
}

/// Writes `key` as 32 raw bytes to `path`, creating parent directories.
///
/// # Errors
///
/// Returns an error if the parent directory cannot be created or the file
/// cannot be written.
pub fn persist_secret_key(path: &Path, key: &SecretKey) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(path, key.to_bytes()).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Loads the secret key at `path`, generating and persisting a fresh one when
/// the file is missing or malformed so the caller announces a stable endpoint
/// id across runs.
///
/// # Errors
///
/// Returns an error only if a freshly generated key cannot be persisted; a
/// missing or malformed file is handled by regenerating.
pub fn load_or_create_secret_key(path: &Path) -> Result<SecretKey> {
    if let Some(key) = read_secret_key(path) {
        return Ok(key);
    }
    if path.exists() {
        tracing::warn!("{} has wrong length, regenerating", path.display());
    }
    let key = SecretKey::generate();
    persist_secret_key(path, &key)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_a_temp_path() {
        let path = std::env::temp_dir().join(format!(
            "iroh-doctor-core-identity-test-{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        // Missing file -> creates and persists.
        let created = load_or_create_secret_key(&path).expect("create");
        // Second call reads the same key back.
        let reread = load_or_create_secret_key(&path).expect("reread");
        assert_eq!(created.public(), reread.public());
        // The raw reader sees the same bytes.
        assert_eq!(
            read_secret_key(&path).map(|k| k.public()),
            Some(created.public())
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn wrong_length_file_reads_as_none() {
        let path = std::env::temp_dir().join(format!(
            "iroh-doctor-core-identity-badlen-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, b"too short").expect("write");
        assert!(read_secret_key(&path).is_none());
        std::fs::remove_file(&path).ok();
    }
}
