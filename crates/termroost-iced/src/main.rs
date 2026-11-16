// Release builds on Windows should not open a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent_icon;
mod app;
mod color_picker;
#[cfg(target_os = "linux")]
mod desktop_entry;
mod dialogs;
mod keymap;
mod layout_conv;
mod logging;
mod notify;
mod panes;
mod reorder;
mod settings;
mod sidebar;
mod tasks;
mod terminal_view;
mod theme;
mod themes;
mod toasts;
mod worktree;

use std::sync::Mutex;

use iced::{Point, Size, window};
use termroost_core::{Config, Core};

use crate::app::{App, Prefs};

/// Desktop application id (also the .desktop and AppStream file names).
pub const APP_ID: &str = "io.github.Davenport_Physics.TerminalRooster";

fn main() -> iced::Result {
    let data_dir = termroost_core::paths::default_data_dir().ok();
    // Flushes the log file when main returns.
    let _log_guard = logging::init(data_dir.as_deref());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "TerminalRooster starting");
    // Lets desktops show the icon for a build run without installing a package.
    #[cfg(target_os = "linux")]
    desktop_entry::ensure();
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        tracing::error!("no display: neither WAYLAND_DISPLAY nor DISPLAY is set");
        eprintln!("terminalrooster: no graphical session found (neither WAYLAND_DISPLAY nor DISPLAY is set)");
        std::process::exit(1);
    }

    // Terminals started from here must not believe they run inside another
    // Claude Code session (e.g. when TerminalRooster itself was launched from one).
    // SAFETY: no other threads exist yet.
    unsafe {
        std::env::remove_var("CLAUDECODE");
        std::env::remove_var("CLAUDE_CODE_ENTRYPOINT");
    }

    let core = match Core::open(Config::default()) {
        Ok(core) => core,
        Err(e) => {
            tracing::error!(error = %e, "could not open the core");
            let _ = rfd::MessageDialog::new()
                .set_title("TerminalRooster")
                .set_description(e.to_string())
                .set_level(rfd::MessageLevel::Error)
                .show();
            std::process::exit(1);
        }
    };

    let prefs = Prefs::load(&core);
    let geom = prefs.window.unwrap_or_default();
    let size = if geom.width >= 640.0 && geom.height >= 400.0 {
        Size::new(geom.width, geom.height)
    } else {
        Size::new(1400.0, 860.0)
    };
    let position = match (geom.x, geom.y) {
        (Some(x), Some(y)) if x > -10_000.0 && y > -10_000.0 => window::Position::Specific(Point::new(x, y)),
        _ => window::Position::Default,
    };
    let window = window::Settings {
        size,
        position,
        min_size: Some(Size::new(640.0, 400.0)),
        exit_on_close_request: false,
        icon: window::icon::from_rgba(include_bytes!(concat!(env!("OUT_DIR"), "/icon-64.rgba")).to_vec(), 64, 64).ok(),
        #[cfg(target_os = "linux")]
        platform_specific: window::settings::PlatformSpecific {
            // Matches the .desktop file name, so desktops show the right icon.
            application_id: APP_ID.to_owned(),
            ..Default::default()
        },
        ..window::Settings::default()
    };

    // `boot` is a `Fn`; hand the already-open core over exactly once.
    let core = Mutex::new(Some(core));
    let mut application = iced::application(
        move || App::new(core.lock().unwrap().take().expect("boot runs once")),
        App::update,
        App::view,
    )
    .title(App::title)
    .subscription(App::subscription)
    .theme(App::theme)
    .window(window)
    .antialiasing(true);
    for font in theme::FONTS {
        application = application.font(font);
    }
    let result = application.run();
    tracing::info!("TerminalRooster exited");
    result
}
