//! Desktop integration for builds that are not installed from a package.
//!
//! On Wayland the taskbar shows the icon of the desktop entry whose file name
//! matches the window's app id (`APP_ID`); the icon a window sets itself is
//! not used there. Packages install that entry system-wide. A build run
//! straight from `target/` has none, so desktops show a generic icon. For
//! such builds this writes a hidden (`NoDisplay`) entry and the icons into
//! `~/.local/share`. Once a package provides the entry, the user-level copy is
//! removed again, since it would shadow the packaged one.
//!
//! Set `TERMROOST_NO_DESKTOP_ENTRY=1` to turn this off.

use std::path::{Path, PathBuf};

use crate::APP_ID;

const MARKER: &str = "# Written by TermRoost for a build that is not installed";
const TEMPLATE: &str = include_str!("../../../packaging/linux/io.github.Davenport_Physics.TermRoost.desktop");
const ICONS: [(u32, &[u8]); 7] = [
    (16, include_bytes!("../../../packaging/icons/termroost-16.png")),
    (24, include_bytes!("../../../packaging/icons/termroost-24.png")),
    (32, include_bytes!("../../../packaging/icons/termroost-32.png")),
    (48, include_bytes!("../../../packaging/icons/termroost-48.png")),
    (64, include_bytes!("../../../packaging/icons/termroost-64.png")),
    (128, include_bytes!("../../../packaging/icons/termroost-128.png")),
    (256, include_bytes!("../../../packaging/icons/termroost-256.png")),
];

pub fn ensure() {
    if std::env::var_os("TERMROOST_NO_DESKTOP_ENTRY").is_some() {
        return;
    }
    let Some(data_home) = directories::BaseDirs::new().map(|d| d.data_dir().to_path_buf()) else { return };
    let file_name = format!("{APP_ID}.desktop");
    let user_entry = data_home.join("applications").join(&file_name);

    if installed_system_wide(&file_name, &data_home) {
        // A package provides the entry; drop our copy so it doesn't shadow it.
        if std::fs::read_to_string(&user_entry).is_ok_and(|t| t.starts_with(MARKER)) {
            let _ = std::fs::remove_file(&user_entry);
            for (size, _) in ICONS {
                let _ = std::fs::remove_file(icon_path(&data_home, size));
            }
            refresh_menus();
            tracing::info!("removed the desktop entry for the uninstalled build; the package provides it");
        }
        return;
    }

    // An entry the user (or another tool) wrote themselves is left alone.
    if std::fs::read_to_string(&user_entry).is_ok_and(|t| !t.starts_with(MARKER)) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let entry = format!(
        "{MARKER}, so it gets its icon. Safe to delete.\n{}NoDisplay=true\n",
        TEMPLATE.replace("Exec=termroost", &format!("Exec={}", exec_quote(&exe)))
    );
    let mut changed = write_if_changed(&user_entry, entry.as_bytes());
    for (size, png) in ICONS {
        changed |= write_if_changed(&icon_path(&data_home, size), png);
    }
    if changed {
        tracing::info!(entry = %user_entry.display(), "installed a desktop entry so the window shows its icon");
        refresh_menus();
    }
}

fn icon_path(data_home: &Path, size: u32) -> PathBuf {
    data_home.join(format!("icons/hicolor/{size}x{size}/apps/{APP_ID}.png"))
}

/// Whether a desktop entry exists in a system data directory (not ours).
fn installed_system_wide(file_name: &str, data_home: &Path) -> bool {
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    std::env::split_paths(&dirs).filter(|d| d != data_home).any(|d| d.join("applications").join(file_name).is_file())
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
        assert_eq!(exec_quote(Path::new("/opt/term-roost/termroost")), "/opt/term-roost/termroost");
        assert_eq!(exec_quote(Path::new("/home/me/My Apps/termroost")), "\"/home/me/My Apps/termroost\"");
        assert_eq!(exec_quote(Path::new("/x/a$b")), "\"/x/a\\\\$b\"");
    }

    #[test]
    fn template_has_the_exec_line_to_replace() {
        assert!(TEMPLATE.contains("\nExec=termroost\n"));
        assert!(TEMPLATE.contains(&format!("Icon={APP_ID}")));
    }
}
