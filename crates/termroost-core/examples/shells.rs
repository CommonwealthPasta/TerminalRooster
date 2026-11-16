//! Prints the shells TerminalRooster offers in its shell pickers on this machine.
//!
//! Usage: `cargo run -p termroost-core --example shells [-- --install]`
//!
//! `--install` first installs TerminalRooster's own bash (Windows), like Settings
//! does. Set `TERMINALROOSTER_DATA_DIR` to try it without touching the real data.

use termroost_core::session::{bundled_bash, shell};

fn main() {
    if std::env::args().any(|a| a == "--install") {
        let started = std::time::Instant::now();
        let last = std::cell::Cell::new(None);
        let result = bundled_bash::install(|p| {
            // Downloading reports every 250 ms; print each step once.
            let step = std::mem::discriminant(&p);
            if last.replace(Some(step)) != Some(step) {
                println!("{p}");
            }
        });
        match result {
            Ok(exe) => println!("installed {} in {:.1?}\n", exe.display(), started.elapsed()),
            Err(e) => {
                eprintln!("install failed: {e}");
                std::process::exit(1);
            }
        }
    }
    for option in shell::detect_shells() {
        println!("{:<20} {}", option.name, option.command);
    }
    if bundled_bash::SUPPORTED {
        let dir = bundled_bash::install_dir().map(|d| d.display().to_string()).unwrap_or_default();
        match bundled_bash::installed() {
            Some(exe) => println!("\nTerminal Rooster bash: installed ({})", exe.display()),
            None => println!("\nTerminal Rooster bash: not installed (would go to {dir})"),
        }
    }
}
