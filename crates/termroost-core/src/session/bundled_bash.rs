//! TerminalRooster's own bash on Windows, for people who want bash without
//! installing anything system-wide.
//!
//! On request it downloads a pinned Git for Windows release (bash, coreutils,
//! vim, less, ssh and git), checks its SHA-256 and unpacks it into the local
//! data directory with Windows' own `curl.exe` and `tar.exe`. Nothing
//! downloaded runs during the install. The release is the `.tar.bz2` and not
//! the smaller PortableGit, because Windows' `tar.exe` cannot unpack
//! PortableGit's 7-Zip archive and its self-extractor would have to run.

use std::path::{Path, PathBuf};

use crate::session::shell::ShellOption;

/// Whether TerminalRooster can install its own bash on this platform.
pub const SUPPORTED: bool = cfg!(windows);

/// Name of the bundled bash in the shell lists.
pub const NAME: &str = "Terminal Rooster bash";

/// The Git for Windows release the bash comes from.
pub const GIT_VERSION: &str = "2.56.1";

/// A pinned release file. Hashes are the ones the release publishes.
struct Asset {
    url: &'static str,
    sha256: &'static str,
    size: u64,
}

#[cfg(target_arch = "aarch64")]
const ASSET: Asset = Asset {
    url: "https://github.com/git-for-windows/git/releases/download/v2.56.1.windows.1/Git-2.56.1-arm64.tar.bz2",
    sha256: "4a5e1555ac8453033d504930857ec8ba4eef20275d784b4e71df84a3da071afb",
    size: 172_843_751,
};

#[cfg(not(target_arch = "aarch64"))]
const ASSET: Asset = Asset {
    url: "https://github.com/git-for-windows/git/releases/download/v2.56.1.windows.1/Git-2.56.1-64-bit.tar.bz2",
    sha256: "d7d89e69855f00e73d957b513c49538a420c465c84b9d212cf1add400eecdf54",
    size: 124_091_126,
};

/// Download size in bytes.
pub fn download_size() -> u64 {
    ASSET.size
}

/// Where the bash lives: `%LOCALAPPDATA%\TerminalRooster\data\bash` (local,
/// not roaming: it is a few hundred MB), or `bash` in `TERMINALROOSTER_DATA_DIR`.
pub fn install_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(crate::paths::DATA_DIR_ENV) {
        return Some(PathBuf::from(dir).join("bash"));
    }
    directories::ProjectDirs::from("", "", crate::paths::APP_DIR).map(|d| d.data_local_dir().join("bash"))
}

/// The installed `bash.exe`, if the install finished.
pub fn installed() -> Option<PathBuf> {
    let dir = install_dir()?;
    let exe = dir.join("bin").join("bash.exe");
    (exe.is_file() && dir.join(VERSION_FILE).is_file()).then_some(exe)
}

/// The shell list entry for the installed bash: an interactive login shell,
/// like Git Bash.
pub fn shell_option(exe: &Path) -> ShellOption {
    let exe = exe.display().to_string();
    ShellOption::new(NAME, &[exe.as_str(), "-i", "-l"])
}

/// Written last, so a half-finished install is never mistaken for a full one.
const VERSION_FILE: &str = ".terminalrooster-git-version";

/// Install steps, reported while [`install`] runs.
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

/// Downloads and unpacks the bash, reporting each step to `progress`, and
/// returns the installed `bash.exe`. Blocks for as long as the download
/// takes, so run it on its own thread. Does nothing if already installed.
pub fn install(progress: impl Fn(Progress)) -> crate::Result<PathBuf> {
    if let Some(exe) = installed() {
        return Ok(exe);
    }
    let dir = install_dir().ok_or(crate::Error::NoDataDir)?;
    imp::install(&dir, &progress)?;
    std::fs::write(dir.join(VERSION_FILE), GIT_VERSION)?;
    tracing::info!(dir = %dir.display(), version = GIT_VERSION, "installed TerminalRooster bash");
    installed().ok_or_else(|| crate::Error::Invalid("the download did not contain bin\\bash.exe".into()))
}

#[cfg(not(windows))]
mod imp {
    use super::Progress;

    pub(super) fn install(_dir: &std::path::Path, _progress: &dyn Fn(Progress)) -> crate::Result<()> {
        Err(crate::Error::Invalid("Terminal Rooster only installs its own bash on Windows".into()))
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

    use super::{ASSET, Progress};
    use crate::Error;
    use crate::session::shell::CREATE_NO_WINDOW;

    pub(super) fn install(dir: &Path, progress: &dyn Fn(Progress)) -> crate::Result<()> {
        let parent = dir.parent().ok_or(Error::NoDataDir)?;
        std::fs::create_dir_all(parent)?;
        let archive = parent.join("bash-download.tar.bz2");
        let staging = parent.join("bash-unpacking");
        let _ = std::fs::remove_file(&archive);
        let _ = std::fs::remove_dir_all(&staging);

        let result = download(&archive, progress)
            .and_then(|()| {
                progress(Progress::Verifying);
                verify(&archive)
            })
            .and_then(|()| {
                progress(Progress::Unpacking);
                std::fs::create_dir_all(&staging)?;
                unpack(&archive, &staging)
            })
            .and_then(|()| {
                // Leftovers of an install that stopped half way.
                if dir.exists() {
                    std::fs::remove_dir_all(dir)?;
                }
                Ok(std::fs::rename(&staging, dir)?)
            });
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

    fn download(to: &Path, progress: &dyn Fn(Progress)) -> crate::Result<()> {
        let total = ASSET.size;
        progress(Progress::Downloading { done: 0, total });
        let mut child = Command::new(system_tool("curl.exe")?)
            .args(["--fail", "--location", "--silent", "--show-error", "--retry", "3", "--output"])
            .arg(to)
            .arg(ASSET.url)
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

    fn verify(file: &Path) -> crate::Result<()> {
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
        if got == ASSET.sha256 {
            Ok(())
        } else {
            tracing::warn!(expected = ASSET.sha256, got, "bash download has the wrong SHA-256");
            Err(Error::Invalid("the download is damaged or was changed (its SHA-256 does not match)".into()))
        }
    }

    fn unpack(archive: &Path, into: &Path) -> crate::Result<()> {
        let out = Command::new(system_tool("tar.exe")?)
            .arg("-xjf")
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
    fn pinned_asset_is_consistent() {
        assert!(ASSET.url.contains(GIT_VERSION));
        assert_eq!(ASSET.sha256.len(), 64);
        assert!(ASSET.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }

    #[test]
    fn shell_option_quotes_the_path() {
        let o = shell_option(Path::new(r"C:\Users\A B\termroost\bash\bin\bash.exe"));
        assert_eq!(o.name, NAME);
        assert_eq!(o.command, r#""C:\Users\A B\termroost\bash\bin\bash.exe" -i -l"#);
    }
}
