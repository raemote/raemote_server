//! The persistent iroh identity and the `~/.raemote` data directory.

use std::path::PathBuf;

use anyhow::{Context, Result};
use iroh::SecretKey;

/// Load the persistent iroh identity, creating and storing a fresh one on first run.
///
/// The key lives at `~/.raemote/secret.key` as 32 raw bytes so that the endpoint's
/// node ID stays stable across restarts.
pub fn load_or_create_secret_key() -> Result<SecretKey> {
    let path = key_path()?;
    if path.exists() {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("failed to read secret key at {}", path.display()))?;
        let bytes: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("secret key at {} must be 32 bytes", path.display()))?;
        Ok(SecretKey::from_bytes(&bytes))
    } else {
        let key = SecretKey::generate();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
            restrict(parent, 0o700);
        }
        std::fs::write(&path, key.to_bytes())
            .with_context(|| format!("failed to write secret key to {}", path.display()))?;
        restrict(&path, 0o600);
        Ok(key)
    }
}

/// The `~/.raemote` directory holding all persistent raemote state.
pub fn data_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".raemote"))
}

fn key_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("secret.key"))
}

/// Create `~/.raemote` if missing and restrict it to the current user.
pub fn ensure_data_dir() -> Result<std::path::PathBuf> {
    let dir = data_dir()?;
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    restrict(&dir, 0o700);
    Ok(dir)
}

/// Restrict a path's permissions (no-op off unix).
#[cfg(unix)]
pub fn restrict(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
pub fn restrict(_path: &std::path::Path, _mode: u32) {}
