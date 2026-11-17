//! Desktop integration for builds that are not installed from a package.
//!
//! On Wayland the taskbar shows the icon of the desktop entry whose file name
//! matches the window's app id; the icon a window sets itself is not used
//! there. Packages install that entry (`APP_ID`) system-wide. A build run
//! straight from `target/` has none, so desktops would show a generic icon.
//!
//! Such builds therefore run under their own id, `DEV_APP_ID`, and write a
//! hidden (`NoDisplay`) entry plus icons for it into `~/.local/share`. Using a
//! separate id matters: an entry in `~/.local/share` takes precedence over the
//! system one with the same name, so a hidden copy named `APP_ID` would hide
//! the installed app from the menu.
//!
//! Set `TERMINALROOSTER_NO_DESKTOP_ENTRY=1` to not write the entry.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::APP_ID;

/// The id a build that wasn't installed from a package runs under.
pub const DEV_APP_ID: &str = "io.github.Davenport_Physics.TerminalRooster.Devel";
const MARKER: &str = "# Written by TerminalRooster for a build that is not installed";
/// Desktop id and marker used while the app was called TermRoost.
const LEGACY_APP_ID: &str = "io.github.Davenport_Physics.TermRoost";
const LEGACY_MARKER: &str = "# Written by TermRoost for a build that is not installed";
const TEMPLATE: &str = include_str!("../../../packaging/linux/io.github.Davenport_Physics.TerminalRooster.desktop");
const ICONS: [(u32, &[u8]); 7] = [
    (16, include_bytes!("../../../packaging/icons/terminalrooster-16.png")),
    (24, include_bytes!("../../../packaging/icons/terminalrooster-24.png")),
    (32, include_bytes!("../../../packaging/icons/terminalrooster-32.png")),
    (48, include_bytes!("../../../packaging/icons/terminalrooster-48.png")),
    (64, include_bytes!("../../../packaging/icons/terminalrooster-64.png")),
    (128, include_bytes!("../../../packaging/icons/terminalrooster-128.png")),
    (256, include_bytes!("../../../packaging/icons/terminalrooster-256.png")),
];

/// The desktop id of this build: `APP_ID` when an installed (system) desktop
/// entry launches this very executable, as with a package; `DEV_APP_ID`
/// otherwise.
pub fn app_id() -> &'static str {
    static ID: OnceLock<&'static str> = OnceLock::new();
    ID.get_or_init(|| {
        let path = std::env::var_os("PATH");
        let packaged = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .is_ok_and(|exe| launched_by_installed_entry(&exe, &data_dirs(), path.as_deref()));
        if packaged { APP_ID } else { DEV_APP_ID }
    })
}

pub fn ensure() {
    let Some(data_home) = directories::BaseDirs::new().map(|d| d.data_dir().to_path_buf()) else { return };
    // Older builds wrote their hidden entry under the packaged id, where it
    // hides the installed app from the menu.
    remove_written(&data_home, APP_ID, MARKER);
    remove_written(&data_home, LEGACY_APP_ID, LEGACY_MARKER);

    if app_id() == APP_ID || std::env::var_os("TERMINALROOSTER_NO_DESKTOP_ENTRY").is_some() {
        return;
    }
    let user_entry = entry_path(&data_home, DEV_APP_ID);
    // An entry the user (or another tool) wrote themselves is left alone.
    if std::fs::read_to_string(&user_entry).is_ok_and(|t| !t.starts_with(MARKER)) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let mut changed = write_if_changed(&user_entry, dev_entry(&exe).as_bytes());
    for (size, png) in ICONS {
        changed |= write_if_changed(&icon_path(&data_home, DEV_APP_ID, size), png);
    }
    if changed {
        tracing::info!(entry = %user_entry.display(), "installed a desktop entry so the window shows its icon");
        refresh_menus();
    }
}

/// The packaged entry, rewritten to start this executable under `DEV_APP_ID`.
fn dev_entry(exe: &Path) -> String {
    let mut out = format!("{MARKER}, so it gets its icon. Safe to delete.\n");
    for line in TEMPLATE.lines() {
        let line = match line.split_once('=') {
            Some(("Name", name)) => format!("Name={name} (dev build)"),
            Some(("Exec", _)) => format!("Exec={}", exec_quote(exe)),
            Some((key @ ("Icon" | "StartupWMClass"), _)) => format!("{key}={DEV_APP_ID}"),
            _ => line.to_owned(),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("NoDisplay=true\n");
    out
}

/// The system data directories (`XDG_DATA_DIRS`).
fn data_dirs() -> Vec<PathBuf> {
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::env::split_paths(&dirs).collect()
}

/// Whether the `APP_ID` entry in one of `data_dirs` starts `exe` (a
/// canonical path), resolving its `Exec` program through `path` like a
/// launcher would.
fn launched_by_installed_entry(exe: &Path, data_dirs: &[PathBuf], path: Option<&OsStr>) -> bool {
    data_dirs.iter().any(|dir| {
        std::fs::read_to_string(entry_path(dir, APP_ID))
            .ok()
            .and_then(|text| exec_program(&text))
            .and_then(|program| resolve_program(&program, path))
            .and_then(|found| std::fs::canonicalize(found).ok())
            .is_some_and(|found| found == exe)
    })
}

/// The program of an entry's `Exec` key: its first argument, unquoted.
fn exec_program(entry: &str) -> Option<String> {
    // Undo the string-value escaping (`\\` is one `\`), then the Exec quoting.
    let exec = entry.lines().find_map(|l| l.strip_prefix("Exec="))?.replace("\\\\", "\\");
    let exec = exec.trim_start();
    if let Some(quoted) = exec.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = quoted.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => out.push(chars.next()?),
                c => out.push(c),
            }
        }
        None
    } else {
        exec.split_whitespace().next().map(str::to_owned)
    }
}

/// `program` itself when it is a path, otherwise its first match in `path`.
fn resolve_program(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    std::env::split_paths(path?).map(|dir| dir.join(program)).find(|p| p.is_file())
}

fn entry_path(data_dir: &Path, app_id: &str) -> PathBuf {
    data_dir.join("applications").join(format!("{app_id}.desktop"))
}

fn icon_path(data_home: &Path, app_id: &str, size: u32) -> PathBuf {
    data_home.join(format!("icons/hicolor/{size}x{size}/apps/{app_id}.png"))
}

/// Removes a hidden entry (and its icons) that a build wrote under `app_id`.
fn remove_written(data_home: &Path, app_id: &str, marker: &str) {
    let entry = entry_path(data_home, app_id);
    if std::fs::read_to_string(&entry).is_ok_and(|t| t.starts_with(marker)) {
        let _ = std::fs::remove_file(&entry);
        for (size, _) in ICONS {
            let _ = std::fs::remove_file(icon_path(data_home, app_id, size));
        }
        refresh_menus();
        tracing::info!(entry = %entry.display(), "removed a desktop entry an older build wrote");
    }
}

fn write_if_changed(path: &Path, content: &[u8]) -> bool {
    if std::fs::read(path).is_ok_and(|c| c == content) {
        return false;
    }
    let ok = path.parent().is_some_and(|p| std::fs::create_dir_all(p).is_ok()) && std::fs::write(path, content).is_ok();
    if !ok {
        tracing::warn!(path = %path.display(), "could not write desktop integration file");
    }
    ok
}

/// Quotes a path for the `Exec` key (desktop entry spec: double quotes, with
/// `"`, `` ` ``, `$` and `\` escaped).
fn exec_quote(path: &Path) -> String {
    let s = path.to_string_lossy();
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        return s.into_owned();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // `\` itself must be escaped once more in a desktop file string value.
    out.replace('\\', "\\\\")
}

/// Asks KDE to pick up the change now instead of at its next scan.
fn refresh_menus() {
    for tool in ["kbuildsycoca6", "kbuildsycoca5"] {
        if let Ok(mut child) = std::process::Command::new(tool)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            // Reap it in the background so it doesn't linger as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_quoting() {
        assert_eq!(
            exec_quote(Path::new("/opt/terminal-rooster/terminalrooster")),
            "/opt/terminal-rooster/terminalrooster"
        );
        assert_eq!(exec_quote(Path::new("/home/me/My Apps/terminalrooster")), "\"/home/me/My Apps/terminalrooster\"");
        assert_eq!(exec_quote(Path::new("/x/a$b")), "\"/x/a\\\\$b\"");
    }

    #[test]
    fn dev_entry_never_claims_the_packaged_id() {
        assert!(TEMPLATE.contains("\nExec=terminalrooster\n"));
        assert!(TEMPLATE.contains(&format!("\nIcon={APP_ID}\n")));
        let entry = dev_entry(Path::new("/src/target/release/terminalrooster"));
        assert!(entry.starts_with(MARKER));
        assert!(entry.contains("\nName=Terminal Rooster (dev build)\n"));
        assert!(entry.contains("\nExec=/src/target/release/terminalrooster\n"));
        assert!(entry.contains(&format!("\nIcon={DEV_APP_ID}\n")));
        assert!(entry.contains(&format!("\nStartupWMClass={DEV_APP_ID}\n")));
        assert!(entry.ends_with("\nNoDisplay=true\n"));
        // Only the dev id may appear; the bare packaged id would mean a key was missed.
        assert_eq!(entry.matches(APP_ID).count(), entry.matches(DEV_APP_ID).count());
    }

    #[test]
    fn exec_program_parsing() {
        assert_eq!(exec_program(TEMPLATE).as_deref(), Some("terminalrooster"));
        assert_eq!(exec_program("Exec=/opt/tr/terminalrooster %U\n").as_deref(), Some("/opt/tr/terminalrooster"));
        for path in ["/home/me/My Apps/terminalrooster", "/x/a$b"] {
            let line = format!("Exec={}\n", exec_quote(Path::new(path)));
            assert_eq!(exec_program(&line).as_deref(), Some(path), "{line}");
        }
        assert_eq!(exec_program("Name=x\n"), None);
    }

    #[test]
    fn packaged_only_when_the_installed_entry_starts_this_exe() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let file = |p: &str| {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "").unwrap();
            p
        };
        let usr_bin = file("usr/bin/terminalrooster");
        let local_bin = file("usr/local/bin/terminalrooster");
        let dev = file("src/target/release/terminalrooster");
        let entry = file(&format!("usr/share/applications/{APP_ID}.desktop"));
        std::fs::write(&entry, TEMPLATE).unwrap();
        let dirs = [root.join("usr/local/share"), root.join("usr/share")];
        let path = std::env::join_paths([root.join("usr/bin")]).unwrap();
        let path = Some(path.as_os_str());

        assert!(launched_by_installed_entry(&usr_bin, &dirs, path));
        assert!(!launched_by_installed_entry(&dev, &dirs, path));
        assert!(!launched_by_installed_entry(&local_bin, &dirs, path), "not what Exec finds on PATH");

        // A package that keeps the binary elsewhere and links it into PATH.
        let real = file("usr/lib/terminalrooster/terminalrooster");
        std::fs::remove_file(&usr_bin).unwrap();
        std::os::unix::fs::symlink(&real, &usr_bin).unwrap();
        assert!(launched_by_installed_entry(&real, &dirs, path));

        // Without the installed entry nothing counts as packaged.
        std::fs::remove_file(&entry).unwrap();
        assert!(!launched_by_installed_entry(&real, &dirs, path));
    }

    #[test]
    fn removes_only_entries_it_wrote() {
        let home = tempfile::tempdir().unwrap();
        let entry = entry_path(home.path(), APP_ID);
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "[Desktop Entry]\nName=Mine\n").unwrap();
        remove_written(home.path(), APP_ID, MARKER);
        assert!(entry.exists(), "an entry the user wrote stays");

        std::fs::write(&entry, dev_entry(Path::new("/x/terminalrooster"))).unwrap();
        let icon = icon_path(home.path(), APP_ID, 64);
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        std::fs::write(&icon, ICONS[4].1).unwrap();
        remove_written(home.path(), APP_ID, MARKER);
        assert!(!entry.exists() && !icon.exists());
    }
}
