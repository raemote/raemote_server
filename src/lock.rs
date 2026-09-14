//! A single-instance guard for the daemon.
//!
//! [`InstanceLock`] holds an OS advisory lock for the daemon's lifetime, so a
//! second `raemoted` refuses to start.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::identity;

/// An exclusive advisory lock held for a `raemoted` process's lifetime.
///
/// Two things make this reliable:
/// - it is an OS advisory lock (`flock`), owned by the open file description,
///   so the kernel releases it automatically when the process exits or crashes;
/// - it lives at a fixed path (`~/.raemote/daemon.lock`), independent of the IPC
///   socket, so a second daemon cannot steal the socket out from under the first.
///
/// A leftover `daemon.lock` file from a crash is harmless; the next process
/// simply acquires the (unlocked) file.
pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

/// Acquire the lock for the default data directory.
pub fn acquire() -> Result<Option<InstanceLock>> {
    let dir = identity::ensure_data_dir()?;
    acquire_at(&dir.join("daemon.lock"))
}

/// Acquire the lock at an explicit path (used by tests).
pub fn acquire_at(path: &Path) -> Result<Option<InstanceLock>> {
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("failed to open lock file {}", path.display()))?;
    identity::restrict(path, 0o600);

    match file.try_lock() {
        Ok(()) => {
            // Record the owner for diagnostics; the lock itself is the guard.
            let _ = file.set_len(0);
            let _ = file.seek(SeekFrom::Start(0));
            let _ = write!(file, "{}", std::process::id());
            let _ = file.flush();
            Ok(Some(InstanceLock {
                _file: file,
                path: path.to_path_buf(),
            }))
        }
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(anyhow::Error::from(err)
            .context(format!("failed to lock {}", path.display()))),
    }
}

/// Whether another process currently holds the lock.
///
/// Acquires and immediately releases, so it must not be relied on to *reserve*
/// the instance — only as a cheap, best-effort check (e.g. in the CLI).
pub fn is_held() -> bool {
    match acquire() {
        Ok(Some(_lock)) => false,
        Ok(None) => true,
        Err(_) => true,
    }
}

/// Best-effort read of the pid recorded by the current holder.
pub fn owner_pid() -> Option<u32> {
    let path = identity::data_dir().ok()?.join("daemon.lock");
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

impl InstanceLock {
    /// The lock file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_second_instance_and_releases_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.lock");

        let first = acquire_at(&path).unwrap();
        assert!(first.is_some(), "first acquire should succeed");

        let second = acquire_at(&path).unwrap();
        assert!(second.is_none(), "second acquire should be refused");

        drop(first);
        let third = acquire_at(&path).unwrap();
        assert!(third.is_some(), "lock should be acquirable after release");
    }
}
