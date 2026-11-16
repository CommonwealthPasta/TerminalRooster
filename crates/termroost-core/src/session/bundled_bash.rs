//! TerminalRooster's own bash on Windows, for people who want bash without
//! installing anything system-wide.
//!
//! On request it downloads a pinned Git for Windows release (bash, coreutils,
//! vim, less, ssh and git) and unpacks it into the local data directory (see
//! [`super::bundle`]). The release is the `.tar.bz2` and not the smaller
//! PortableGit, because Windows' `tar.exe` cannot unpack PortableGit's 7-Zip
//! archive and its self-extractor would have to run.

use std::path::{Path, PathBuf};

pub use super::bundle::Progress;
use super::bundle::{self, Asset, Layout};
use crate::session::shell::ShellOption;

/// Whether TerminalRooster can install its own bash on this platform.
pub const SUPPORTED: bool = cfg!(windows);

/// Name of the bundled bash in the shell lists.
pub const NAME: &str = "Terminal Rooster bash";

/// The Git for Windows release the bash comes from.
pub const GIT_VERSION: &str = "2.56.1";

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
    bundle::local_data_subdir("bash")
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

/// Downloads and unpacks the bash, reporting each step to `progress`, and
/// returns the installed `bash.exe`. Blocks for as long as the download
/// takes, so run it on its own thread. Does nothing if already installed.
pub fn install(progress: impl Fn(Progress)) -> crate::Result<PathBuf> {
    if let Some(exe) = installed() {
        return Ok(exe);
    }
    let dir = install_dir().ok_or(crate::Error::NoDataDir)?;
    bundle::install(&ASSET, &dir, Layout::Flat, &progress)?;
    std::fs::write(dir.join(VERSION_FILE), GIT_VERSION)?;
    tracing::info!(dir = %dir.display(), version = GIT_VERSION, "installed TerminalRooster bash");
    installed().ok_or_else(|| crate::Error::Invalid("the download did not contain bin\\bash.exe".into()))
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
