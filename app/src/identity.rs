//! On-disk persistence for the iroh secret key + the optional API secret override.
//!
//! Stored under `dirs::config_dir()/iroh-doctor-app/`:
//! - `secret_key.bin` - 32 bytes, generated on first run.
//! - `api_secret.txt` - optional override for the services key; missing = use bundled default.
//!
//! The app was previously named iroh-pong and stored these under
//! `iroh-pong/`. Both readers fall back to that legacy directory so an
//! upgrade keeps the same endpoint id and saved API secret; the next
//! write lands in the new directory.

use std::path::PathBuf;

use anyhow::{Context, Result};
use iroh::SecretKey;
use iroh_doctor_core::identity::{persist_secret_key, read_secret_key};

fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no config dir on this platform")?;
    Ok(base.join("iroh-doctor-app"))
}

/// Pre-rename config directory (`iroh-pong`). Read as a fallback so an
/// upgrade preserves the existing identity and settings.
fn legacy_config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no config dir on this platform")?;
    Ok(base.join("iroh-pong"))
}

fn secret_key_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("secret_key.bin"))
}

fn api_secret_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("api_secret.txt"))
}

pub fn load_or_create_secret_key() -> Result<SecretKey> {
    let path = secret_key_path()?;
    if let Some(key) = read_secret_key(&path) {
        return Ok(key);
    }
    // Migrate the pre-rename key so the endpoint id stays stable across
    // the iroh-pong -> iroh-doctor-app rename.
    if let Some(key) = legacy_config_dir()
        .ok()
        .and_then(|d| read_secret_key(&d.join("secret_key.bin")))
    {
        persist_secret_key(&path, &key)?;
        return Ok(key);
    }
    if path.exists() {
        tracing::warn!("secret_key.bin has wrong length, regenerating");
    }
    let key = SecretKey::generate();
    persist_secret_key(&path, &key)?;
    Ok(key)
}

pub fn load_api_secret_override() -> String {
    if let Ok(path) = api_secret_path() {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            return contents.trim().to_string();
        }
    }
    // Fall back to the pre-rename location.
    if let Some(legacy) = legacy_config_dir().ok().map(|d| d.join("api_secret.txt")) {
        if let Ok(contents) = std::fs::read_to_string(&legacy) {
            return contents.trim().to_string();
        }
    }
    String::new()
}

pub fn save_api_secret_override(value: &str) -> Result<()> {
    let trimmed = value.trim();
    let path = api_secret_path()?;
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(&path, trimmed).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
