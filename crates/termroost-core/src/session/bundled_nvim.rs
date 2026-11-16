//! Neovim for TerminalRooster on Windows, for editing files in Neovim
//! without installing it system-wide.
//!
//! On request it downloads a pinned Neovim release (the official Windows
//! zip, which also holds `win32yank.exe` for the clipboard) and unpacks it
//! into the local data directory (see [`super::bundle`]). Elsewhere Neovim
//! comes from the system's package manager.

use std::path::PathBuf;

pub use super::bundle::Progress;
use super::bundle::{self, Asset, Layout};

/// Whether TerminalRooster can install Neovim on this platform.
pub const SUPPORTED: bool = cfg!(windows);

/// The Neovim release it installs.
pub const NVIM_VERSION: &str = "0.12.6";

const X64: Asset = Asset {
    url: "https://github.com/neovim/neovim/releases/download/v0.12.6/nvim-win64.zip",
    sha256: "4e12b7103b601ffc8f25954b4da100f339faa73dccb69a5b135e897fa3d893ed",
    size: 12_624_741,
};

const ARM64: Asset = Asset {
    url: "https://github.com/neovim/neovim/releases/download/v0.12.6/nvim-win-arm64.zip",
    sha256: "2e58d259ffcc6145e5112ea02330126c6b57da09087bb9265fb2fa9aa10b485c",
    size: 13_188_877,
};

const ASSET: &Asset = if cfg!(target_arch = "aarch64") { &ARM64 } else { &X64 };

/// Download size in bytes.
pub fn download_size() -> u64 {
    ASSET.size
}

/// Where Neovim lives: `%LOCALAPPDATA%\TerminalRooster\data\nvim`, or `nvim`
/// in `TERMINALROOSTER_DATA_DIR`.
pub fn install_dir() -> Option<PathBuf> {
    bundle::local_data_subdir("nvim")
}

/// Written last, so a half-finished install is never mistaken for a full one.
const VERSION_FILE: &str = ".terminalrooster-nvim-version";

/// The installed `nvim.exe`, if the install finished.
pub fn installed() -> Option<PathBuf> {
    let dir = install_dir()?;
    let exe = dir.join("bin").join("nvim.exe");
    (exe.is_file() && dir.join(VERSION_FILE).is_file()).then_some(exe)
}

/// Downloads and unpacks Neovim, reporting each step to `progress`, and
/// returns the installed `nvim.exe`. Blocks for as long as the download
/// takes, so run it on its own thread. Does nothing if already installed.
pub fn install(progress: impl Fn(Progress)) -> crate::Result<PathBuf> {
    if let Some(exe) = installed() {
        return Ok(exe);
    }
    let dir = install_dir().ok_or(crate::Error::NoDataDir)?;
    bundle::install(ASSET, &dir, Layout::SingleFolder, &progress)?;
    std::fs::write(dir.join(VERSION_FILE), NVIM_VERSION)?;
    tracing::info!(dir = %dir.display(), version = NVIM_VERSION, "installed Neovim for TerminalRooster");
    installed().ok_or_else(|| crate::Error::Invalid("the download did not contain bin\\nvim.exe".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_assets_are_consistent() {
        for asset in [&X64, &ARM64] {
            assert!(asset.url.contains(&format!("/v{NVIM_VERSION}/")), "{}", asset.url);
            assert!(asset.url.ends_with(".zip"));
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        }
    }
}
