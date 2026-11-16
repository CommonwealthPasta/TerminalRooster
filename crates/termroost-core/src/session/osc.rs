//! Finds the escape sequences alacritty ignores but notifications need, in
//! the raw output of a terminal:
//!
//! * `OSC 133 ; D ; <exit code>` (and VS Code's `OSC 633 ; D`): a shell
//!   integration reports that a command finished; `OSC 633 ; E ; <command>`
//!   names the command.
//! * `OSC 9 ; 4 ; <state> ; <percent>`: a progress indicator (ConEmu, Windows
//!   Terminal). Agents such as Grok show one while they work.
//! * `OSC 7501 ; state=<state>:app=<name>[:kind=<why>]`: a program's
//!   lifecycle state (`working`, `done`, `error`, `blocked`, `idle`, `clear`).
//!   Codex sends it to every terminal.
//! * `OSC 9 ; <text>` (iTerm2), `OSC 777 ; notify ; <title> ; <body>`
//!   (urxvt, Ghostty, VTE) and `OSC 99` (kitty): a program asks for a desktop
//!   notification, e.g. Codex or Grok at the end of a turn.
//! * `OSC 7 ; file://<host>/<path>`, `OSC 9 ; 9 ; <path>` (ConEmu, Windows
//!   Terminal) and `OSC 633 ; P ; Cwd=<path>` (VS Code): the shell's working
//!   directory, so a restored terminal can start there again.

use std::path::PathBuf;

/// A sequence of interest found in the output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    /// A shell command finished, with its exit status when the shell sent one.
    CommandDone(Option<i32>),
    /// The command line of the command that is finishing (`OSC 633 ; E`).
    CommandLine(String),
    /// `OSC 9;4` progress: 0 clears it, 1–4 show it (normal, error, busy, paused).
    Progress(u8),
    /// `OSC 7501` program state, e.g. `working` or `done`, with the program's
    /// name and, for `blocked`, what it waits for (`permission`, `question`, `auth`).
    Status { state: String, app: Option<String>, kind: Option<String> },
    /// A program asked for a desktop notification.
    Notify { title: Option<String>, body: String },
    /// The shell's working directory, as an absolute path of this system.
    Cwd(PathBuf),
}

/// Longest OSC payload kept; longer ones (e.g. huge OSC 52 copies) are skipped.
const MAX_OSC: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Osc,
    /// ESC inside an OSC: `\` ends it, anything else aborts it.
    OscEscape,
}

/// Incremental scanner; sequences may be split across reads.
#[derive(Debug, Default)]
pub(crate) struct OscScanner {
    state: State,
    buf: Vec<u8>,
    overflow: bool,
    /// Kitty notification being assembled from chunks (`d=0`).
    kitty: Option<(Option<String>, String)>,
}

impl OscScanner {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Signal>) {
        for &b in bytes {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Escape;
                    }
                }
                State::Escape => self.escape(b),
                State::Osc => match b {
                    0x07 => {
                        self.dispatch(out);
                        self.state = State::Ground;
                    }
                    0x1b => self.state = State::OscEscape,
                    // CAN and SUB cancel the sequence.
                    0x18 | 0x1a => self.state = State::Ground,
                    _ if self.buf.len() < MAX_OSC => self.buf.push(b),
                    _ => self.overflow = true,
                },
                State::OscEscape => {
                    if b == b'\\' {
                        self.dispatch(out);
                        self.state = State::Ground;
                    } else {
                        self.escape(b);
                    }
                }
            }
        }
    }

    fn escape(&mut self, b: u8) {
        self.state = match b {
            b']' => {
                self.buf.clear();
                self.overflow = false;
                State::Osc
            }
            0x1b => State::Escape,
            _ => State::Ground,
        };
    }

    fn dispatch(&mut self, out: &mut Vec<Signal>) {
        if self.overflow {
            return;
        }
        let buf = std::mem::take(&mut self.buf);
        let text = String::from_utf8_lossy(&buf);
        let (code, rest) = text.split_once(';').unwrap_or((&text, ""));
        match code {
            "133" | "633" => {
                let (kind, args) = rest.split_once(';').unwrap_or((rest, ""));
                match kind {
                    "D" => out.push(Signal::CommandDone(args.split(';').next().and_then(|c| c.trim().parse().ok()))),
                    "E" if code == "633" => {
                        let line = unescape(args.split(';').next().unwrap_or_default());
                        out.push(Signal::CommandLine(clean(&line)));
                    }
                    "P" if code == "633" => {
                        if let Some(dir) = args.strip_prefix("Cwd=").and_then(|d| cwd(&unescape(d))) {
                            out.push(Signal::Cwd(dir));
                        }
                    }
                    _ => {}
                }
            }
            "7" => {
                if let Some(dir) = file_url_path(rest).and_then(|p| cwd(&p)) {
                    out.push(Signal::Cwd(dir));
                }
            }
            "9" => {
                let (sub, args) = rest.split_once(';').unwrap_or((rest, ""));
                if sub == "4" {
                    let state = args.split(';').next().and_then(|s| s.trim().parse::<u8>().ok()).unwrap_or(0);
                    out.push(Signal::Progress(state.min(4)));
                } else if sub == "9" {
                    let dir = args.trim();
                    let dir = dir.strip_prefix('"').and_then(|d| d.strip_suffix('"')).unwrap_or(dir);
                    if let Some(dir) = cwd(dir) {
                        out.push(Signal::Cwd(dir));
                    }
                } else if !sub.is_empty() && sub.bytes().all(|b| b.is_ascii_digit()) {
                    // Other ConEmu commands, not notifications.
                } else if !rest.trim().is_empty() {
                    out.push(Signal::Notify { title: None, body: clean(rest) });
                }
            }
            "777" => {
                let mut parts = rest.splitn(3, ';');
                if parts.next() == Some("notify") {
                    let title = parts.next().map(clean).filter(|t| !t.is_empty());
                    let body = parts.next().map(clean).unwrap_or_default();
                    if title.is_some() || !body.is_empty() {
                        out.push(Signal::Notify { title, body });
                    }
                }
            }
            "99" => self.kitty(rest, out),
            "7501" => {
                let (mut state, mut app, mut kind) = (None, None, None);
                for kv in rest.split(':') {
                    match kv.split_once('=') {
                        Some(("state", v)) => state = Some(v.trim().to_owned()),
                        Some(("app", v)) => app = Some(v.trim().to_owned()),
                        Some(("kind", v)) => kind = Some(v.trim().to_owned()),
                        _ => {}
                    }
                }
                if let Some(state) = state.filter(|s| !s.is_empty()) {
                    out.push(Signal::Status { state, app, kind });
                }
            }
            _ => {}
        }
    }

    /// Kitty's protocol: `99 ; key=value:key=value ; payload`. The payload is
    /// the title unless `p=body`; `d=0` means more chunks follow.
    fn kitty(&mut self, rest: &str, out: &mut Vec<Signal>) {
        let (meta, payload) = rest.split_once(';').unwrap_or((rest, ""));
        let mut done = true;
        let mut part = "title";
        let mut base64 = false;
        for kv in meta.split(':') {
            match kv.split_once('=') {
                Some(("d", v)) => done = v != "0",
                Some(("p", v)) => part = v,
                Some(("e", v)) => base64 = v == "1",
                _ => {}
            }
        }
        if base64 || !matches!(part, "title" | "body") {
            // Queries, icons and the like; and encoded text is rare enough to skip.
            if done {
                self.kitty = None;
            }
            return;
        }
        let (title, body) = self.kitty.get_or_insert_with(Default::default);
        if part == "title" {
            title.get_or_insert_with(String::new).push_str(payload);
        } else {
            body.push_str(payload);
        }
        if done && let Some((title, body)) = self.kitty.take() {
            let title = title.map(|t| clean(&t)).filter(|t| !t.is_empty());
            let body = clean(&body);
            if title.is_some() || !body.is_empty() {
                out.push(Signal::Notify { title, body });
            }
        }
    }
}

/// Undoes VS Code's escaping of command lines: `\\` for a backslash and
/// `\xHH` for `;` and control characters.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            if b.get(i + 1) == Some(&b'\\') {
                out.push(b'\\');
                i += 2;
                continue;
            }
            if b.get(i + 1) == Some(&b'x')
                && let Some(v) = s.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The path of an `OSC 7` URL: `file://<host>/<path>` (or kitty's
/// `kitty-shell-cwd://`), percent-decoded. The host is not checked: a shell
/// on another machine (ssh) names a directory there, which then just does
/// not exist here.
fn file_url_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://").or_else(|| url.strip_prefix("kitty-shell-cwd://"))?;
    let path = &rest[rest.find('/')?..];
    let b = path.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = path.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// A reported directory as an absolute path of this system, if it is one.
fn cwd(reported: &str) -> Option<PathBuf> {
    if reported.chars().any(char::is_control) {
        return None;
    }
    let path = PathBuf::from(if cfg!(windows) { windows_path(reported) } else { reported.to_owned() });
    path.is_absolute().then_some(path)
}

/// A directory as Windows names it. Shells that know only Unix-style paths
/// report drives as `/c/...` (Git Bash, MSYS2), `/cygdrive/c/...` (Cygwin) or
/// `/mnt/c/...` (WSL), and `file://` URLs give `/C:/...`.
fn windows_path(path: &str) -> String {
    let drive = |rest: &str| -> Option<String> {
        let mut chars = rest.chars();
        let letter = chars.next().filter(char::is_ascii_alphabetic)?;
        let tail = chars.as_str();
        let tail = match tail.strip_prefix(':') {
            Some(t) => t,
            None => tail,
        };
        (tail.is_empty() || tail.starts_with('/')).then(|| format!("{}:{}", letter.to_ascii_uppercase(), tail))
    };
    let converted = path
        .strip_prefix("/cygdrive/")
        .or_else(|| path.strip_prefix("/mnt/"))
        .or_else(|| path.strip_prefix('/'))
        .and_then(drive)
        .unwrap_or_else(|| path.to_owned());
    let converted = converted.replace('/', "\\");
    // `C:` alone is the current directory on drive C, not its root.
    if converted.len() == 2 && converted.ends_with(':') { converted + "\\" } else { converted }
}

/// Notification text without control characters, trimmed and kept short.
fn clean(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    let s = s.trim();
    match s.char_indices().nth(300) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<Signal> {
        let mut s = OscScanner::default();
        let mut out = Vec::new();
        for c in chunks {
            s.feed(c, &mut out);
        }
        out
    }

    #[test]
    fn command_status() {
        assert_eq!(scan(&[b"\x1b]133;D;1\x07"]), vec![Signal::CommandDone(Some(1))]);
        assert_eq!(scan(&[b"\x1b]133;D;0\x1b\\"]), vec![Signal::CommandDone(Some(0))]);
        assert_eq!(scan(&[b"\x1b]133;D\x07"]), vec![Signal::CommandDone(None)]);
        assert_eq!(scan(&[b"\x1b]633;D;127\x07"]), vec![Signal::CommandDone(Some(127))]);
        assert!(scan(&[b"\x1b]133;A\x07\x1b]133;C\x07"]).is_empty());
        assert_eq!(scan(&[b"\x1b]633;E;echo a\\x3bb \\\\ c\x07"]), vec![Signal::CommandLine(r"echo a;b \ c".into())]);
    }

    #[test]
    fn split_across_reads() {
        assert_eq!(scan(&[b"out\x1b", b"]13", b"3;D;", b"2\x1b", b"\\more"]), vec![Signal::CommandDone(Some(2))]);
    }

    #[test]
    fn progress_and_notifications() {
        assert_eq!(scan(&[b"\x1b]9;4;3;0\x07\x1b]9;4;0\x07"]), vec![Signal::Progress(3), Signal::Progress(0)]);
        assert_eq!(
            scan(&[b"\x1b]9;Agent turn complete\x07"]),
            vec![Signal::Notify { title: None, body: "Agent turn complete".into() }]
        );
        assert!(scan(&[b"\x1b]9;9;C:\\Users\x07"]).iter().all(|s| matches!(s, Signal::Cwd(_))), "ConEmu cwd report");
        assert_eq!(
            scan(&[b"\x1b]777;notify;Codex;Done; all good\x1b\\"]),
            vec![Signal::Notify { title: Some("Codex".into()), body: "Done; all good".into() }]
        );
        assert_eq!(
            scan(&[b"\x1b]99;i=1:d=0;Grok\x1b\\\x1b]99;i=1:p=body;Turn finished\x1b\\"]),
            vec![Signal::Notify { title: Some("Grok".into()), body: "Turn finished".into() }]
        );
    }

    #[test]
    fn program_status() {
        // What Codex writes.
        assert_eq!(
            scan(&[b"\x1b]7501;state=working:app=codex\x1b\\"]),
            vec![Signal::Status { state: "working".into(), app: Some("codex".into()), kind: None }]
        );
        assert_eq!(
            scan(&[b"\x1b]7501;state=blocked:app=codex:kind=permission\x1b\\\x1b]7501;state=clear\x1b\\"]),
            vec![
                Signal::Status { state: "blocked".into(), app: Some("codex".into()), kind: Some("permission".into()) },
                Signal::Status { state: "clear".into(), app: None, kind: None },
            ]
        );
    }

    #[test]
    fn titles_and_aborted_sequences_are_ignored() {
        assert!(scan(&[b"\x1b]0;\xe2\x9c\xb3 Claude Code\x07"]).is_empty());
        // ESC aborts the first OSC; the second one still counts.
        assert_eq!(scan(&[b"\x1b]133;D;1\x1b[0m\x1b]133;D;3\x07"]), vec![Signal::CommandDone(Some(3))]);
        let mut big = b"\x1b]52;c;".to_vec();
        big.extend(std::iter::repeat_n(b'A', MAX_OSC * 2));
        big.extend(b"\x07\x1b]133;D;4\x07");
        assert_eq!(scan(&[&big]), vec![Signal::CommandDone(Some(4))]);
    }

    fn cwds(chunks: &[&[u8]]) -> Vec<String> {
        scan(chunks)
            .into_iter()
            .map(|s| match s {
                Signal::Cwd(d) => d.display().to_string(),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn working_directory_reports() {
        assert_eq!(cwds(&[b"\x1b]7;file://box/home/me/my%20dir\x07"]), ["/home/me/my dir"]);
        assert_eq!(cwds(&[b"\x1b]7;kitty-shell-cwd://box/srv\x1b\\"]), ["/srv"]);
        assert_eq!(cwds(&[b"\x1b]633;P;Cwd=/home/me/a\\x3bb\x07"]), ["/home/me/a;b"]);
        assert_eq!(cwds(&[b"\x1b]9;9;\"/opt/x\"\x07"]), ["/opt/x"]);
        // Not absolute, not a file URL, or another VS Code property.
        assert!(cwds(&[b"\x1b]7;relative\x07\x1b]7;http://x/y\x07\x1b]633;P;IsWindows=False\x07"]).is_empty());
        assert!(cwds(&[b"\x1b]9;9;C:\\Users\x07"]).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn working_directory_reports() {
        assert_eq!(cwds(&[b"\x1b]9;9;\"C:\\Users\\me\"\x07"]), [r"C:\Users\me"]);
        assert_eq!(cwds(&[b"\x1b]633;P;Cwd=D:/projects/a\\x3bb\x07"]), [r"D:\projects\a;b"]);
        assert_eq!(cwds(&[b"\x1b]7;file://box/C:/Program%20Files\x07"]), [r"C:\Program Files"]);
        assert_eq!(cwds(&[b"\x1b]7;file://box/d/projects\x07"]), [r"D:\projects"]);
        // A path inside MSYS2 or WSL that has no Windows name.
        assert!(cwds(&[b"\x1b]7;file://box/home/me\x07"]).is_empty());
    }

    #[test]
    fn unix_style_paths_become_windows_paths() {
        assert_eq!(windows_path("/c/Users/me"), r"C:\Users\me");
        assert_eq!(windows_path("/cygdrive/d/x"), r"D:\x");
        assert_eq!(windows_path("/mnt/e/src"), r"E:\src");
        assert_eq!(windows_path("/C:/Windows"), r"C:\Windows");
        assert_eq!(windows_path("C:/projects/app"), r"C:\projects\app");
        assert_eq!(windows_path("/d"), r"D:\");
        assert_eq!(windows_path(r"\\server\share"), r"\\server\share");
        // Not a drive: left alone (and not absolute on Windows).
        assert_eq!(windows_path("/home/me"), r"\home\me");
        assert_eq!(windows_path("/mnt/wsl"), r"\mnt\wsl");
    }
}
