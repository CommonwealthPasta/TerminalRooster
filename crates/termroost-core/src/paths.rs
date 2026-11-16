//! Data directory resolution, the move from TermRoost's data directory, and
//! the single-instance lock.

use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Environment variable overriding the data directory (used by tests).
pub const DATA_DIR_ENV: &str = "TERMINALROOSTER_DATA_DIR";
/// The app's folder name in the platform's data directories.
pub const APP_DIR: &str = "TerminalRooster";
/// The database, in the data directory.
pub const DB_FILE: &str = "terminalrooster.db";
const LOCK_FILE: &str = "terminalrooster.lock";
/// Prefix of the data files (`terminalrooster.db`, ...).
const FILE_PREFIX: &str = "terminalrooster.";
/// The app's folder and file name while it was called TermRoost.
const LEGACY_NAME: &str = "termroost";
const LEGACY_FILE_PREFIX: &str = "termroost.";

/// `~/.local/share/terminalrooster` on Linux, `%APPDATA%\TerminalRooster\data` on Windows.
pub fn default_data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    directories::ProjectDirs::from("", "", APP_DIR).map(|d| d.data_dir().to_path_buf()).ok_or(Error::NoDataDir)
}

/// The data directory used while the app was called TermRoost:
/// `~/.local/share/termroost` on Linux, `%APPDATA%\termroost\data` on Windows.
pub fn legacy_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", LEGACY_NAME).map(|d| d.data_dir().to_path_buf())
}

/// Moves everything in TermRoost's data directory (`legacy`) into `new_dir`,
/// renaming `termroost.*` files to `terminalrooster.*`, unless `new_dir` already
/// has a database. Refuses while TermRoost is running, since moving an open
/// database could corrupt it. Returns whether anything was moved.
pub fn migrate_legacy(legacy: &Path, new_dir: &Path) -> Result<bool> {
    let legacy_db = format!("{LEGACY_FILE_PREFIX}db");
    if new_dir.join(DB_FILE).exists() || !legacy.join(&legacy_db).is_file() {
        return Ok(false);
    }
    let lock_name = format!("{LEGACY_FILE_PREFIX}lock");
    let lock_path = legacy.join(&lock_name);
    let old_lock = File::options().create(true).truncate(false).write(true).open(&lock_path)?;
    match old_lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Err(Error::LegacyRunning(legacy.to_path_buf())),
        Err(TryLockError::Error(e)) => return Err(Error::Io(e)),
    }
    std::fs::create_dir_all(new_dir)?;

    let mut entries = std::fs::read_dir(legacy)?.collect::<io::Result<Vec<_>>>()?;
    entries.retain(|e| e.file_name() != lock_name.as_str());
    // The database (with its write-ahead log and index) goes last: once it is
    // in `new_dir`, the move counts as done.
    entries.sort_by_key(|e| e.file_name().to_string_lossy().starts_with(&legacy_db));
    for entry in entries {
        move_into(&entry.path(), new_dir)?;
    }

    drop(old_lock);
    let _ = std::fs::remove_file(&lock_path);
    // These only succeed on empty folders, so nothing left behind is lost.
    let _ = std::fs::remove_dir(legacy);
    if let Some(parent) = legacy.parent().filter(|p| p.file_name().is_some_and(|n| n == LEGACY_NAME)) {
        // Windows keeps the data in `termroost\data`.
        let _ = std::fs::remove_dir(parent);
    }
    tracing::info!(from = %legacy.display(), to = %new_dir.display(), "moved data from the old TermRoost folder");
    Ok(true)
}

/// Moves `from` into the directory `dir`, renaming a `termroost.` prefix to
/// `terminalrooster.`. A folder that already exists there is merged. A file
/// whose new name is taken keeps its old name: the new app may already have
/// started today's log, `terminalrooster.<date>.log`.
fn move_into(from: &Path, dir: &Path) -> io::Result<()> {
    let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let new_name = match name.strip_prefix(LEGACY_FILE_PREFIX) {
        Some(rest) => format!("{FILE_PREFIX}{rest}"),
        None => name.clone(),
    };
    let mut to = dir.join(&new_name);
    if from.is_dir() && to.is_dir() {
        for entry in std::fs::read_dir(from)? {
            move_into(&entry?.path(), &to)?;
        }
        return std::fs::remove_dir(from);
    }
    if to.exists() {
        // Never pair the moved database with another database's leftover files.
        let keep = dir.join(&name);
        if new_name.starts_with(DB_FILE) || keep.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", to.display())));
        }
        to = keep;
    }
    if std::fs::rename(from, &to).is_err() {
        // Different file systems: copy, then remove.
        std::fs::copy(from, &to)?;
        std::fs::remove_file(from)?;
    }
    Ok(())
}

/// Exclusive lock on `<data dir>/terminalrooster.lock`, released when dropped
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
        let path = dir.join(LOCK_FILE);
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

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn legacy_data_moves_once_and_waits_for_the_old_app() {
        let tmp = tempfile::tempdir().unwrap();
        // Laid out like Windows: `termroost\data` becomes `TerminalRooster\data`.
        let (old, new) = (tmp.path().join("termroost/data"), tmp.path().join("TerminalRooster/data"));
        assert!(!migrate_legacy(&old, &new).unwrap(), "nothing to move");
        write(&old.join("termroost.db"), "db");
        write(&old.join("termroost.db-wal"), "wal");
        write(&old.join("termroost.db-shm"), "shm");
        write(&old.join("logs/termroost.2026-11-15.log"), "old log 15");
        write(&old.join("logs/termroost.2026-11-16.log"), "old log 16");
        write(&old.join("codex-hooks/.running"), "");
        // The new app has already started today's log before moving data.
        write(&new.join("logs/terminalrooster.2026-11-16.log"), "new log 16");

        // TermRoost is still running and holds its lock.
        let held = File::options().create(true).truncate(false).write(true).open(old.join("termroost.lock")).unwrap();
        held.lock().unwrap();
        assert!(matches!(migrate_legacy(&old, &new), Err(Error::LegacyRunning(_))));
        assert!(old.join("termroost.db").exists(), "nothing moved while it runs");
        assert!(old.join("logs/termroost.2026-11-15.log").exists(), "nothing moved while it runs");
        drop(held);

        assert!(migrate_legacy(&old, &new).unwrap());
        assert_eq!(read(&new.join("terminalrooster.db")), "db");
        assert_eq!(read(&new.join("terminalrooster.db-wal")), "wal");
        assert_eq!(read(&new.join("terminalrooster.db-shm")), "shm");
        assert_eq!(read(&new.join("logs/terminalrooster.2026-11-15.log")), "old log 15");
        assert_eq!(read(&new.join("logs/terminalrooster.2026-11-16.log")), "new log 16");
        assert_eq!(read(&new.join("logs/termroost.2026-11-16.log")), "old log 16");
        assert!(new.join("codex-hooks/.running").is_file());
        assert!(!new.join("termroost.lock").exists() && !new.join("terminalrooster.lock").exists());
        assert!(!tmp.path().join("termroost").exists(), "the emptied old folder is removed");
        assert!(!migrate_legacy(&old, &new).unwrap(), "only once");
    }

    #[test]
    fn leftover_files_in_the_old_folder_are_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = (tmp.path().join("termroost"), tmp.path().join("terminalrooster"));
        write(&old.join("termroost.db"), "db");
        write(&new.join("notes.txt"), "mine");
        write(&old.join("notes.txt"), "also mine");
        assert!(migrate_legacy(&old, &new).is_err(), "a name taken under both names stops the move");
        assert_eq!(read(&old.join("termroost.db")), "db", "the database stays until everything else moved");
        assert_eq!(read(&new.join("notes.txt")), "mine");
    }
}
