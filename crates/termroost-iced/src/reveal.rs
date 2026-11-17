//! Showing a file or folder in the system's file manager.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use iced::Task;

/// The menu entry: "Show in Explorer" for a file (its folder opens with it
/// selected), "Open in Explorer" for a folder.
pub fn label(is_dir: bool) -> &'static str {
    match (cfg!(windows), cfg!(target_os = "macos"), is_dir) {
        (true, _, true) => "Open in Explorer",
        (true, _, false) => "Show in Explorer",
        (_, true, true) => "Open in Finder",
        (_, true, false) => "Show in Finder",
        (_, _, true) => "Open in file manager",
        (_, _, false) => "Show in file manager",
    }
}

/// Opens the system's file manager on `path`: a folder shows what is in it,
/// a file is selected in its folder. Waits for the launcher, so call it on a
/// thread.
pub fn reveal(path: &Path, is_dir: bool) -> Result<(), String> {
    imp::reveal(path, is_dir)
}

/// [`reveal`] on a thread; the task ends with how it went.
pub fn in_background(path: PathBuf, is_dir: bool) -> Task<Result<(), String>> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(reveal(&path, is_dir));
    });
    Task::perform(async move { rx.recv().await.unwrap_or(Ok(())) }, |r| r)
}

fn run(cmd: &mut Command) -> Result<std::process::ExitStatus, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.status().map_err(|e| format!("could not run {program}: {e}"))
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    pub(super) fn reveal(path: &Path, is_dir: bool) -> Result<(), String> {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        // Explorer doesn't understand forward slashes, which project folders may have.
        let path = path.to_string_lossy().replace('/', "\\");
        let mut cmd = Command::new(root.join("explorer.exe"));
        if is_dir {
            cmd.arg(&path);
        } else {
            // Explorer parses its own command line: the quotes go around the path only.
            cmd.raw_arg(format!("/select,\"{path}\""));
        }
        // Explorer's exit status says nothing (it is 1 when it worked).
        super::run(&mut cmd).map(drop)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::Path;
    use std::process::Command;

    pub(super) fn reveal(path: &Path, is_dir: bool) -> Result<(), String> {
        let mut cmd = Command::new("open");
        if !is_dir {
            cmd.arg("-R");
        }
        match super::run(cmd.arg(path))? {
            s if s.success() => Ok(()),
            s => Err(format!("open failed ({s})")),
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::path::Path;
    use std::process::Command;

    pub(super) fn reveal(path: &Path, is_dir: bool) -> Result<(), String> {
        // File managers that implement org.freedesktop.FileManager1 (Dolphin,
        // Nautilus, Nemo, Thunar…) can open a folder with the file selected.
        if !is_dir {
            let shown = super::run(Command::new("dbus-send").args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{}", super::file_uri(path)),
                "string:",
            ]));
            if shown.is_ok_and(|s| s.success()) {
                return Ok(());
            }
        }
        // Otherwise the default file manager, on the folder.
        let folder = if is_dir { path } else { path.parent().unwrap_or(path) };
        match super::run(Command::new("xdg-open").arg(folder))? {
            s if s.success() => Ok(()),
            s => Err(format!("xdg-open failed ({s})")),
        }
    }
}

/// `path` as a `file://` URI. Everything but unreserved characters and `/`
/// is percent-encoded, commas included (dbus-send splits arrays on them).
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_become_file_uris() {
        assert_eq!(file_uri(Path::new("/home/a/notes.md")), "file:///home/a/notes.md");
        assert_eq!(file_uri(Path::new("/tmp/a b,c#é")), "file:///tmp/a%20b%2Cc%23%C3%A9");
    }

    #[test]
    fn files_are_shown_and_folders_opened() {
        assert!(label(false).starts_with("Show in"));
        assert!(label(true).starts_with("Open in"));
    }
}
