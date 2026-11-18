//! A terminal that keeps running, in a console window of its own:
//! `terminalrooster attach <pane>`. The window takes the terminal over from
//! TerminalRooster, which waits meanwhile and shows it again once the window
//! closes (or takes it back, which closes the window).

use std::path::Path;

use crate::model::PaneId;
#[cfg(windows)]
use {
    super::client::{self, Connected},
    super::proto::{self, Exited, FrameReader},
    std::io,
    std::net::TcpStream,
    std::sync::{Arc, Mutex},
    std::time::Duration,
};

/// Shows the terminal of `pane` (of the TerminalRooster whose data is in
/// `data_dir`) in this process's console, opening one if it has none, until
/// the window closes, TerminalRooster takes the terminal back, or its
/// program ends. Returns the process exit code.
pub fn attach_console(data_dir: &Path, pane: PaneId, title: Option<&str>) -> i32 {
    #[cfg(windows)]
    {
        let console = match win::Console::open() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "no console to show the terminal in");
                return 1;
            }
        };
        if let Some(title) = title {
            console.set_title(title);
        }
        match run(&console, data_dir, pane) {
            Ok(()) => 0,
            Err(e) => {
                tracing::warn!(%pane, error = %e, "could not show the terminal");
                console.write(format!("\r\n\x1b[0;33m{e}\x1b[0m · any key closes\r\n").as_bytes());
                console.wait_for_key();
                1
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (data_dir, title);
        tracing::error!(%pane, "terminals in windows of their own are not available on this system yet");
        eprintln!("terminalrooster: terminals in windows of their own are not available on this system yet");
        1
    }
}

#[cfg(windows)]
fn run(console: &win::Console, data_dir: &Path, pane: PaneId) -> io::Result<()> {
    let info = proto::read_conn(&proto::conn_file(data_dir, pane))
        .ok_or_else(|| io::Error::other("this terminal doesn't keep running"))?;
    let size = console.size();
    let conn = match client::connect(&info, true, false, size)? {
        Connected::Attached(conn) => conn,
        Connected::Refused(r) => return Err(io::Error::other(format!("the terminal's helper refused ({r:?})"))),
    };
    tracing::info!(%pane, helper = info.pid, "showing a terminal in a window of its own");
    let writer = Arc::new(Mutex::new(conn.writer));
    let send = {
        let writer = writer.clone();
        move |kind: u8, payload: &[u8]| {
            let _ = proto::write_frame(&mut *writer.lock().unwrap(), kind, payload);
        }
    };

    // The window's size, which only some consoles report as it changes.
    let output = console.output_handle();
    let watch = send.clone();
    std::thread::spawn(move || {
        let mut last = size;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let now = output.size();
            if now != last {
                last = now;
                watch(proto::RESIZE, &serde_json::to_vec(&now).unwrap_or_default());
            }
        }
    });

    // The program's output, until it ends or TerminalRooster takes over.
    let (ended_tx, ended_rx) = std::sync::mpsc::channel();
    let output = console.output_handle();
    std::thread::spawn(move || {
        let ending = show_output(conn.reader, &output);
        let _ = ended_tx.send(ending);
    });

    // Keys go to the program as the console sends them (VT input).
    let mut input = console.input_handle();
    let typing = send.clone();
    std::thread::spawn(move || {
        while let Some(text) = input.read_text() {
            if !text.is_empty() {
                typing(proto::INPUT, text.as_bytes());
            }
        }
    });

    match ended_rx.recv() {
        Ok(Ending::TakenBack) | Err(_) => Ok(()),
        Ok(Ending::Exited(code)) => {
            let code = code.map_or(String::new(), |c| format!(" ({c})"));
            console.write(format!("\r\n\x1b[0;2m─── exited{code} · any key closes ───\x1b[0m\r\n").as_bytes());
            console.wait_for_key();
            Ok(())
        }
        Ok(Ending::Lost) => Err(io::Error::other("lost the terminal's helper")),
    }
}

#[cfg(windows)]
enum Ending {
    /// TerminalRooster shows the terminal again.
    TakenBack,
    Exited(Option<i32>),
    Lost,
}

/// Writes the program's output to the console until it ends.
#[cfg(windows)]
fn show_output(mut reader: FrameReader<TcpStream>, output: &win::Output) -> Ending {
    // The screen comes first; it is laid out for an empty one.
    output.write(b"\x1b[0m\x1b[2J\x1b[3J\x1b[H");
    let mut utf8 = Utf8Joiner::default();
    loop {
        let frame = match reader.next() {
            Ok(Some(frame)) => frame,
            _ => return Ending::Lost,
        };
        match frame.kind {
            proto::OUTPUT => output.write(&utf8.complete(&frame.payload)),
            proto::EXITED => {
                return Ending::Exited(proto::parse::<Exited>(&frame.payload).ok().and_then(|e| e.code));
            }
            proto::KICKED => return Ending::TakenBack,
            _ => {}
        }
    }
}

/// Holds back a character split between two writes until it is whole.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Default)]
struct Utf8Joiner {
    partial: Vec<u8>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Utf8Joiner {
    fn complete(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut all = std::mem::take(&mut self.partial);
        all.extend_from_slice(bytes);
        // A split character is at most 3 bytes from the end.
        let keep = (1..=3.min(all.len()))
            .find(|&n| {
                let start = all.len() - n;
                let lead = all[start];
                let len = match lead {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf7 => 4,
                    _ => return false,
                };
                len > n
            })
            .unwrap_or(0);
        self.partial = all.split_off(all.len() - keep);
        all
    }
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::ptr::null;

    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WriteFile,
    };
    use windows_sys::Win32::System::Console::{
        AllocConsole, CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN, ENABLE_PROCESSED_OUTPUT,
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, ENABLE_WINDOW_INPUT,
        ENABLE_WRAP_AT_EOL_OUTPUT, GetConsoleScreenBufferInfo, GetConsoleWindow, INPUT_RECORD, KEY_EVENT,
        ReadConsoleInputW, SetConsoleCP, SetConsoleCtrlHandler, SetConsoleMode, SetConsoleOutputCP, SetConsoleTitleW,
    };

    use crate::session::GridSize;

    const CP_UTF8: u32 = 65001;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn open(name: &str) -> io::Result<HANDLE> {
        let name = wide(name);
        // SAFETY: a valid, nul-terminated name; no security attributes.
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(h) }
    }

    /// This process's console, set up for a terminal: keys come as VT
    /// sequences, output is UTF-8 with VT sequences, Ctrl+C is a key.
    pub struct Console {
        input: HANDLE,
        output: HANDLE,
    }

    impl Console {
        pub fn open() -> io::Result<Console> {
            // SAFETY: plain console calls.
            unsafe {
                if GetConsoleWindow().is_null() && AllocConsole() == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            let input = open("CONIN$")?;
            let output = open("CONOUT$")?;
            // SAFETY: handles to this process's console.
            unsafe {
                SetConsoleMode(input, ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_WINDOW_INPUT);
                SetConsoleMode(
                    output,
                    ENABLE_PROCESSED_OUTPUT
                        | ENABLE_WRAP_AT_EOL_OUTPUT
                        | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                        | DISABLE_NEWLINE_AUTO_RETURN,
                );
                SetConsoleCP(CP_UTF8);
                SetConsoleOutputCP(CP_UTF8);
                // Ctrl+C is the program's (it arrives as a key anyway).
                SetConsoleCtrlHandler(None, 1);
            }
            Ok(Console { input, output })
        }

        pub fn set_title(&self, title: &str) {
            let title = wide(title);
            // SAFETY: a nul-terminated string.
            unsafe { SetConsoleTitleW(title.as_ptr()) };
        }

        pub fn size(&self) -> GridSize {
            self.output_handle().size()
        }

        pub fn write(&self, bytes: &[u8]) {
            self.output_handle().write(bytes);
        }

        pub fn output_handle(&self) -> Output {
            Output(self.output)
        }

        pub fn input_handle(&self) -> Input {
            Input { handle: self.input, high: None }
        }

        /// Waits for a key press.
        pub fn wait_for_key(&self) {
            let mut input = self.input_handle();
            while let Some(text) = input.read_text() {
                if !text.is_empty() {
                    return;
                }
            }
        }
    }

    /// The console's output, for any thread.
    #[derive(Clone, Copy)]
    pub struct Output(HANDLE);

    // SAFETY: console handles may be used from any thread.
    unsafe impl Send for Output {}

    impl Output {
        pub fn write(&self, mut bytes: &[u8]) {
            while !bytes.is_empty() {
                let mut written = 0;
                // SAFETY: a valid buffer and handle.
                let ok = unsafe {
                    WriteFile(self.0, bytes.as_ptr(), bytes.len() as u32, &mut written, std::ptr::null_mut())
                };
                if ok == 0 || written == 0 {
                    return;
                }
                bytes = &bytes[written as usize..];
            }
        }

        /// The size of the window (not of the buffer behind it).
        pub fn size(&self) -> GridSize {
            // SAFETY: a plain struct filled by the call.
            let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
            // SAFETY: a valid handle and struct.
            if unsafe { GetConsoleScreenBufferInfo(self.0, &mut info) } == 0 {
                return GridSize::default();
            }
            let w = info.srWindow;
            GridSize {
                cols: (w.Right - w.Left + 1).max(2) as u16,
                rows: (w.Bottom - w.Top + 1).max(1) as u16,
                ..GridSize::default()
            }
        }
    }

    /// The console's input.
    pub struct Input {
        handle: HANDLE,
        /// The first half of a character split between two reads.
        high: Option<u16>,
    }

    // SAFETY: console handles may be used from any thread.
    unsafe impl Send for Input {}

    impl Input {
        /// Waits for input and returns the text typed (empty for other
        /// events, a resize say); `None` once the console is gone.
        pub fn read_text(&mut self) -> Option<String> {
            // SAFETY: plain structs, filled by the call.
            let mut records: [INPUT_RECORD; 64] = unsafe { std::mem::zeroed() };
            let mut n = 0;
            // SAFETY: a valid handle and buffer of the given length.
            if unsafe { ReadConsoleInputW(self.handle, records.as_mut_ptr(), records.len() as u32, &mut n) } == 0 {
                return None;
            }
            let mut units: Vec<u16> = self.high.take().into_iter().collect();
            for r in &records[..n as usize] {
                if u32::from(r.EventType) != KEY_EVENT {
                    continue;
                }
                // SAFETY: a key event's record holds a key event.
                let key = unsafe { r.Event.KeyEvent };
                // SAFETY: Unicode input, so the character is UTF-16.
                let c = unsafe { key.uChar.UnicodeChar };
                if key.bKeyDown != 0 && c != 0 {
                    units.extend(std::iter::repeat_n(c, usize::from(key.wRepeatCount.max(1))));
                }
            }
            if units.last().is_some_and(|u| (0xd800..0xdc00).contains(u)) {
                self.high = units.pop();
            }
            Some(String::from_utf16_lossy(&units))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Utf8Joiner;

    #[test]
    fn characters_split_between_writes_are_joined() {
        let text = "a漢🐓b".as_bytes();
        for split in 0..=text.len() {
            let mut j = Utf8Joiner::default();
            let mut out = j.complete(&text[..split]);
            assert!(std::str::from_utf8(&out).is_ok(), "split at {split}");
            out.extend(j.complete(&text[split..]));
            assert_eq!(out, text);
        }
    }
}
