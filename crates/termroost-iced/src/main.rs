// Release builds on Windows should not open a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod dialogs;
mod keymap;
mod layout_conv;
mod logging;
mod panes;
mod settings;
mod sidebar;
mod tasks;
mod terminal_view;
mod theme;
mod themes;

use std::sync::Mutex;

use iced::{Point, Size, window};
use termroost_core::{Config, Core};

use crate::app::{App, Prefs};

fn main() -> iced::Result {
    let data_dir = termroost_core::paths::default_data_dir().ok();
    // Flushes the log file when main returns.
    let _log_guard = logging::init(data_dir.as_deref());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "TermRoost starting");

    // Terminals started from here must not believe they run inside another
    // Claude Code session (e.g. when TermRoost itself was launched from one).
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
                .set_title("TermRoost")
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
    tracing::info!("TermRoost exited");
    result
}
