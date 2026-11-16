//! Data directory resolution and the single-instance lock.

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Environment variable overriding the data directory (used by tests).
pub const DATA_DIR_ENV: &str = "TERMROOST_DATA_DIR";

/// `~/.local/share/termroost` on Linux, `%APPDATA%\termroost\data` on Windows.
pub fn default_data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    directories::ProjectDirs::from("", "", "termroost").map(|d| d.data_dir().to_path_buf()).ok_or(Error::NoDataDir)
}

/// The data directory used before the project was renamed from agentdeck.
pub fn legacy_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "agentdeck").map(|d| d.data_dir().to_path_buf())
}

/// Moves the database from the pre-rename data directory (`agentdeck`) into
/// `new_dir`, unless `new_dir` already has one. Refuses while the old app is
/// running, since moving an open database could corrupt it. Returns whether
/// anything was moved.
pub fn migrate_legacy(legacy: &Path, new_dir: &Path) -> Result<bool> {
    let old_db = legacy.join("agentdeck.db");
    if new_dir.join("termroost.db").exists() || !old_db.is_file() {
        return Ok(false);
    }
    let old_lock = File::options().create(true).truncate(false).write(true).open(legacy.join("agentdeck.lock"))?;
    match old_lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(Error::LegacyRunning(legacy.to_path_buf())),
        Err(TryLockError::Error(e)) => return Err(Error::Io(e)),
    }
    std::fs::create_dir_all(new_dir)?;
    // The write-ahead log and its index must move together with the database.
    for suffix in ["", "-wal", "-shm"] {
        let from = legacy.join(format!("agentdeck.db{suffix}"));
        let to = new_dir.join(format!("termroost.db{suffix}"));
        if from.exists() && std::fs::rename(&from, &to).is_err() {
            // Different file systems: copy, then remove.
            std::fs::copy(&from, &to)?;
            std::fs::remove_file(&from)?;
        }
    }
    tracing::info!(from = %legacy.display(), to = %new_dir.display(), "moved data from the old agentdeck folder");
    Ok(true)
}

/// Exclusive lock on `<data dir>/termroost.lock`, released when dropped
/// (or when the process dies, so a crash never leaves a stale lock).
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// Waits up to two seconds for the lock: an instance that is still
    /// shutting down (or a just-forked child process briefly holding a copy of
    /// the descriptor) releases it moments later.
    pub fn acquire(dir: &Path) -> Result<Self> {
        let path = dir.join("termroost.lock");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let file = File::options().create(true).truncate(false).write(true).open(&path)?;
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(TryLockError::WouldBlock) => return Err(Error::AlreadyRunning(path)),
                Err(TryLockError::Error(e)) => return Err(Error::Io(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_data_moves_once_and_waits_for_the_old_app() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = (tmp.path().join("agentdeck"), tmp.path().join("termroost"));
        assert!(!migrate_legacy(&old, &new).unwrap(), "nothing to move");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("agentdeck.db"), "db").unwrap();
        std::fs::write(old.join("agentdeck.db-wal"), "wal").unwrap();

        // The old app still holds its lock.
        let held = File::options().create(true).truncate(false).write(true).open(old.join("agentdeck.lock")).unwrap();
        held.lock().unwrap();
        assert!(matches!(migrate_legacy(&old, &new), Err(Error::LegacyRunning(_))));
        assert!(old.join("agentdeck.db").exists(), "nothing moved while it runs");
        drop(held);

        assert!(migrate_legacy(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("termroost.db")).unwrap(), "db");
        assert_eq!(std::fs::read_to_string(new.join("termroost.db-wal")).unwrap(), "wal");
        assert!(!old.join("agentdeck.db").exists());
        assert!(!migrate_legacy(&old, &new).unwrap(), "only once");
    }
}
