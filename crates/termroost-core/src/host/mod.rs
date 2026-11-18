//! Terminals that keep running when TerminalRooster quits.
//!
//! Such a terminal's program runs under a helper process of its own (see
//! [`server`]), which owns its PTY. TerminalRooster shows the terminal as a
//! client of the helper; quitting it only disconnects. Started again, it
//! connects to the helper and finds the terminal as the program left it.

pub(crate) mod client;
mod console;
pub(crate) mod proto;
mod server;

pub use client::HostCommand;
pub use console::attach_console;
pub use server::serve;

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use super::client::{self, Connected, HostCommand};
    use super::proto::{self, Spec};
    use crate::model::PaneId;
    use crate::session::{GridSize, HostPlan, TerminalHandle};

    const HOST_ENV: &str = "TERMROOST_LIB_TEST_HOST";

    /// This test binary is the helper when started with `HOST_ENV` set.
    #[test]
    fn helper_entry() {
        if std::env::var_os(HOST_ENV).is_some() {
            std::process::exit(super::serve());
        }
    }

    fn start_helper(dir: &Path, pane: PaneId, program: &str, args: &[&str]) -> proto::ConnInfo {
        let command = HostCommand {
            program: std::env::current_exe().unwrap(),
            args: ["host::tests::helper_entry", "--exact", "--nocapture", "--test-threads=1"]
                .map(String::from)
                .to_vec(),
            env: vec![(HOST_ENV.to_owned(), "1".to_owned())],
        };
        let spec = Spec {
            pane,
            conn_file: proto::conn_file(dir, pane),
            token: proto::new_token(),
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            cwd: dir.to_path_buf(),
            env: Default::default(),
            size: GridSize::default(),
            scrollback_lines: 1000,
            preload: None,
        };
        client::start(&command, &spec, dir).expect("start a helper")
    }

    fn handle(pane: PaneId) -> TerminalHandle {
        let (events, _rx) = async_channel::unbounded();
        let palette = std::sync::Arc::new(std::sync::RwLock::new(Default::default()));
        TerminalHandle::new(pane, events, palette, 1000, GridSize::default(), Default::default())
    }

    fn wait(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    fn text(h: &TerminalHandle) -> String {
        h.with_content(|c| c.text())
    }

    /// A program that echoes typed lines.
    fn echo() -> (&'static str, &'static [&'static str]) {
        if cfg!(windows) { ("cmd", &["/c", "echo ready-1 & more"]) } else { ("sh", &["-c", "echo ready-1; exec cat"]) }
    }

    #[test]
    fn another_client_takes_over_and_gives_the_terminal_back() {
        let dir = tempfile::tempdir().unwrap();
        let pane = PaneId(9);
        let (program, args) = echo();
        let info = start_helper(dir.path(), pane, program, args);
        let file = proto::conn_file(dir.path(), pane);

        let here = handle(pane);
        here.attach_host(HostPlan { existing: Some((file.clone(), info.clone())), steal: false, start: None });
        wait("the program's output here", || text(&here).contains("ready-1"));
        assert!(!here.is_shown_elsewhere());
        // An idle connection stays (the helper's wait for a hello is over).
        std::thread::sleep(Duration::from_secs(6));
        assert!(here.is_running(), "still connected after a quiet while");
        here.write("typed-later\r");
        wait("typing after a quiet while", || text(&here).contains("typed-later"));

        // A console window takes over; the terminal here waits.
        let size = GridSize { cols: 60, rows: 20, cell_width: 8, cell_height: 16 };
        let Connected::Attached(mut other) = client::connect(&info, true, false, size).unwrap() else { panic!() };
        assert!(other.welcome.is_some());
        wait("the terminal here to pause", || here.is_shown_elsewhere());
        let screen = other.reader.next().unwrap().unwrap();
        assert_eq!(screen.kind, proto::OUTPUT);
        assert!(String::from_utf8_lossy(&screen.payload).contains("ready-1"), "it gets the whole screen");
        proto::write_frame(&mut other.writer, proto::INPUT, b"typed-elsewhere\r").unwrap();
        // What is typed here meanwhile goes nowhere.
        here.write("typed-here\r");
        let mut seen = String::new();
        wait("the echo in the other window", || {
            let frame = other.reader.next().unwrap().unwrap();
            seen.push_str(&String::from_utf8_lossy(&frame.payload));
            seen.contains("typed-elsewhere")
        });

        // The window closes: the terminal shows here again, with what
        // happened meanwhile.
        drop(other);
        wait("the terminal back here", || !here.is_shown_elsewhere() && text(&here).contains("typed-elsewhere"));
        assert!(!text(&here).contains("typed-here"));
        here.write("typed-back\r");
        wait("typing here again", || text(&here).contains("typed-back"));

        // Taking it back from a window closes the window.
        let Connected::Attached(mut other) = client::connect(&info, true, false, size).unwrap() else { panic!() };
        wait("the terminal here to pause", || here.is_shown_elsewhere());
        assert!(here.take_back());
        wait("the window to be let go", || match other.reader.next() {
            Ok(Some(f)) => f.kind == proto::KICKED,
            _ => true,
        });
        wait("the terminal back here", || !here.is_shown_elsewhere());

        // Closing the terminal ends the helper.
        here.shutdown();
        wait("the helper to end", || !file.exists());
    }
}
