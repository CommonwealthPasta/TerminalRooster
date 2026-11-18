//! How terminals that keep running when TerminalRooster quits get their
//! helper process: this executable, run as `terminalrooster host`.
//!
//! On Windows the helpers run a copy kept in the data directory. They run on
//! after TerminalRooster quits, and a running executable can't be replaced,
//! so running the original would block updating or rebuilding it.

use std::path::Path;

use termroost_core::HostCommand;

/// The argument that makes this executable a terminal's helper.
pub const HOST_ARG: &str = "host";
/// How to start a helper, or `None` when this executable can't be found.
pub fn command(data_dir: Option<&Path>) -> Option<HostCommand> {
    let exe = std::env::current_exe().map_err(|e| tracing::warn!(error = %e, "can't find this executable")).ok()?;
    #[cfg(windows)]
    let exe = match data_dir.map(|dir| copy_for_helpers(&exe, dir)) {
        Some(Ok(copy)) => copy,
        Some(Err(e)) => {
            tracing::warn!(error = %e, "can't copy this executable for terminal helpers; they run the original");
            exe
        }
        None => exe,
    };
    #[cfg(not(windows))]
    let _ = data_dir;
    Some(HostCommand { program: exe, args: vec![HOST_ARG.to_owned()], env: Vec::new() })
}

/// Copies `exe` into `<data dir>/bin` unless this build is there already,
/// and deletes the copies of older builds that no helper runs any more.
#[cfg(windows)]
fn copy_for_helpers(exe: &Path, data_dir: &Path) -> std::io::Result<std::path::PathBuf> {
    let meta = std::fs::metadata(exe)?;
    let stamp = meta.modified()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let dir = data_dir.join("bin");
    std::fs::create_dir_all(&dir)?;
    let name = format!("terminalrooster-host-{}-{stamp}.exe", meta.len());
    let target = dir.join(&name);
    if !target.exists() {
        tracing::info!(copy = %target.display(), "copying this executable for terminal helpers");
        let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
        std::fs::copy(exe, &tmp)?;
        std::fs::rename(&tmp, &target)?;
    }
    for entry in std::fs::read_dir(&dir)?.flatten() {
        // A copy a helper still runs can't be deleted; it goes next time.
        if entry.path() != target && std::fs::remove_file(entry.path()).is_ok() {
            tracing::debug!(copy = %entry.path().display(), "deleted an older helper executable");
        }
    }
    Ok(target)
}
