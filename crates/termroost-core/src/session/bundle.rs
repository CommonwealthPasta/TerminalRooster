//! Programs TerminalRooster downloads for itself on Windows (its own bash,
//! Neovim), so people needn't install anything system-wide.
//!
//! A pinned release file is downloaded with Windows' own `curl.exe`, checked
//! against its SHA-256 and unpacked with Windows' own `tar.exe` into the local
//! data directory. Nothing downloaded runs during the install.

use std::path::{Path, PathBuf};

/// A pinned release file. Hashes are the ones the release publishes.
pub(crate) struct Asset {
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// Install steps, reported while an install runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Downloading { done: u64, total: u64 },
    Verifying,
    Unpacking,
}

impl std::fmt::Display for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const MB: u64 = 1_000_000;
        match self {
            Progress::Downloading { done, total } => write!(f, "Downloading… {} of {} MB", done / MB, total / MB),
            Progress::Verifying => f.write_str("Checking the download…"),
            Progress::Unpacking => f.write_str("Unpacking… (this takes a few seconds)"),
        }
    }
}

/// How an archive holds its files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    /// Its top level is what goes into the install folder (Git for Windows).
    Flat,
    /// It holds one folder, which becomes the install folder (Neovim's zips).
    SingleFolder,
}

/// `name` in the local (not roaming: these are large) data directory,
/// `%LOCALAPPDATA%\TerminalRooster\data\<name>`, or in `TERMINALROOSTER_DATA_DIR`.
pub(crate) fn local_data_subdir(name: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(crate::paths::DATA_DIR_ENV) {
        return Some(PathBuf::from(dir).join(name));
    }
    directories::ProjectDirs::from("", "", crate::paths::APP_DIR).map(|d| d.data_local_dir().join(name))
}

/// Downloads `asset`, checks it and unpacks it into `dir`, replacing what
/// was there. Blocks for as long as the download takes, so run it on its
/// own thread.
pub(crate) fn install(asset: &Asset, dir: &Path, layout: Layout, progress: &dyn Fn(Progress)) -> crate::Result<()> {
    imp::install(asset, dir, layout, progress)
}

/// The archive's file name ending, which also says how to unpack it.
fn archive_ext(url: &str) -> Option<&'static str> {
    [".tar.bz2", ".zip"].into_iter().find(|ext| url.ends_with(ext))
}

/// Moves what was unpacked into `staging` to `dir`, replacing `dir`.
fn promote(staging: &Path, dir: &Path, layout: Layout) -> crate::Result<()> {
    let from = match layout {
        Layout::Flat => staging.to_path_buf(),
        Layout::SingleFolder => {
            let entries: Vec<std::fs::DirEntry> = std::fs::read_dir(staging)?.collect::<Result<_, _>>()?;
            match entries.as_slice() {
                [only] if only.file_type()?.is_dir() => only.path(),
                _ => return Err(crate::Error::Invalid("the download is not laid out as expected".into())),
            }
        }
    };
    // Leftovers of an install that stopped half way, or an older version.
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::rename(&from, dir)?;
    Ok(())
}

#[cfg(not(windows))]
mod imp {
    use super::{Asset, Layout, Progress};

    pub(super) fn install(_: &Asset, _: &std::path::Path, _: Layout, _: &dyn Fn(Progress)) -> crate::Result<()> {
        Err(crate::Error::Invalid("TerminalRooster only installs programs for itself on Windows".into()))
    }
}

#[cfg(windows)]
mod imp {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use sha2::{Digest, Sha256};

    use super::{Asset, Layout, Progress, archive_ext, promote};
    use crate::Error;
    use crate::session::shell::CREATE_NO_WINDOW;

    pub(super) fn install(asset: &Asset, dir: &Path, layout: Layout, progress: &dyn Fn(Progress)) -> crate::Result<()> {
        let parent = dir.parent().ok_or(Error::NoDataDir)?;
        let name = dir.file_name().ok_or(Error::NoDataDir)?.to_string_lossy();
        let ext = archive_ext(asset.url).ok_or_else(|| Error::Invalid(format!("unknown archive: {}", asset.url)))?;
        std::fs::create_dir_all(parent)?;
        let archive = parent.join(format!("{name}-download{ext}"));
        let staging = parent.join(format!("{name}-unpacking"));
        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_dir_all(&staging);

        let result = download(asset, &archive, progress)
            .and_then(|()| {
                progress(Progress::Verifying);
                verify(asset, &archive)
            })
            .and_then(|()| {
                progress(Progress::Unpacking);
                std::fs::create_dir_all(&staging)?;
                unpack(&archive, ext, &staging)
            })
            .and_then(|()| promote(&staging, dir, layout));
        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_dir_all(&staging);
        result
    }

    /// Windows' own tools, never a same-named program earlier on PATH.
    fn system_tool(exe: &str) -> crate::Result<PathBuf> {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let path = root.join("System32").join(exe);
        if path.is_file() {
            Ok(path)
        } else {
            Err(Error::Invalid(format!("{} not found (it comes with Windows 10 1803 and newer)", path.display())))
        }
    }

    fn download(asset: &Asset, to: &Path, progress: &dyn Fn(Progress)) -> crate::Result<()> {
        let total = asset.size;
        progress(Progress::Downloading { done: 0, total });
        let mut child = Command::new(system_tool("curl.exe")?)
            .args(["--fail", "--location", "--silent", "--show-error", "--retry", "3", "--output"])
            .arg(to)
            .arg(asset.url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        // curl only writes a line on error, so stderr cannot fill up meanwhile.
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            let done = std::fs::metadata(to).map_or(0, |m| m.len());
            progress(Progress::Downloading { done, total });
            std::thread::sleep(Duration::from_millis(250));
        };
        if status.success() {
            return Ok(());
        }
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut err);
        }
        Err(Error::Invalid(format!("download failed: {}", err.trim().trim_start_matches("curl: "))))
    }

    fn verify(asset: &Asset, file: &Path) -> crate::Result<()> {
        let mut f = std::fs::File::open(file)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if got == asset.sha256 {
            Ok(())
        } else {
            tracing::warn!(url = asset.url, expected = asset.sha256, got, "download has the wrong SHA-256");
            Err(Error::Invalid("the download is damaged or was changed (its SHA-256 does not match)".into()))
        }
    }

    fn unpack(archive: &Path, ext: &str, into: &Path) -> crate::Result<()> {
        // Windows' tar (bsdtar) also reads zip files.
        let mode = if ext == ".tar.bz2" { "-xjf" } else { "-xf" };
        let out = Command::new(system_tool("tar.exe")?)
            .arg(mode)
            .arg(archive)
            .arg("-C")
            .arg(into)
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            let first = err.lines().next().unwrap_or("").trim();
            Err(Error::Invalid(format!("unpacking failed: {first}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_are_known_by_their_ending() {
        assert_eq!(archive_ext("https://x/Git-2.56.1-64-bit.tar.bz2"), Some(".tar.bz2"));
        assert_eq!(archive_ext("https://x/nvim-win64.zip"), Some(".zip"));
        assert_eq!(archive_ext("https://x/setup.exe"), None);
    }

    #[test]
    fn a_flat_archive_becomes_the_folder() {
        let root = tempfile::tempdir().unwrap();
        let (staging, dir) = (root.path().join("x-unpacking"), root.path().join("x"));
        std::fs::create_dir_all(staging.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("old")).unwrap();
        promote(&staging, &dir, Layout::Flat).unwrap();
        assert!(dir.join("bin").is_dir());
        assert!(!dir.join("old").exists(), "what was there is replaced");
        assert!(!staging.exists());
    }

    #[test]
    fn an_archive_with_one_folder_has_that_folder_moved() {
        let root = tempfile::tempdir().unwrap();
        let (staging, dir) = (root.path().join("nvim-unpacking"), root.path().join("nvim"));
        std::fs::create_dir_all(staging.join("nvim-win64").join("bin")).unwrap();
        std::fs::write(staging.join("nvim-win64").join("bin").join("nvim.exe"), "").unwrap();
        promote(&staging, &dir, Layout::SingleFolder).unwrap();
        assert!(dir.join("bin").join("nvim.exe").is_file());

        // Anything else is refused, and nothing is replaced.
        std::fs::create_dir_all(staging.join("a")).unwrap();
        std::fs::create_dir_all(staging.join("b")).unwrap();
        assert!(promote(&staging, &dir, Layout::SingleFolder).is_err(), "two folders");
        assert!(dir.join("bin").join("nvim.exe").is_file());
        let lone_file = root.path().join("f-unpacking");
        std::fs::create_dir_all(&lone_file).unwrap();
        std::fs::write(lone_file.join("readme.txt"), "").unwrap();
        assert!(promote(&lone_file, &root.path().join("f"), Layout::SingleFolder).is_err(), "a file");
    }
}
