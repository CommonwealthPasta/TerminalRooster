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
/// The argument that makes this executable a console window showing a
/// terminal that keeps running: `attach <terminal id> [window title]`.
pub const ATTACH_ARG: &str = "attach";

/// Whether terminals can be shown in windows of their own here.
pub const WINDOWS_OF_THEIR_OWN: bool = cfg!(windows);

/// Shows a terminal that keeps running in a console window of its own,
/// which runs on its own too (closing Terminal Rooster leaves it open).
pub fn open_window(host: &HostCommand, pane: termroost_core::PaneId, title: &str) -> std::io::Result<()> {
    let mut command = std::process::Command::new(&host.program);
    command
        .args([ATTACH_ARG, &pane.0.to_string(), title])
        .envs(host.env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(dir) = host.program.parent() {
        command.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // A console of its own (a build without one opens it itself).
        command.creation_flags(CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if let Err(e) = command.spawn() {
            if e.raw_os_error() != Some(5) {
                return Err(e);
            }
            command.creation_flags(CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP);
            command.spawn()?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        command.spawn().map(drop)
    }
}

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
