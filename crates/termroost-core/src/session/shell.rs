//! Shell selection and command-line splitting, and finding programs on PATH.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A program plus its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// Splits a command line on whitespace, honouring single and double quotes.
pub fn split_command_line(s: &str) -> Option<ShellCommand> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_token = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_token {
                    parts.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_token = true;
            }
        }
    }
    if in_token {
        parts.push(cur);
    }
    let mut it = parts.into_iter();
    Some(ShellCommand { program: it.next()?, args: it.collect() })
}

/// The platform default interactive shell.
pub fn default_shell() -> ShellCommand {
    #[cfg(windows)]
    {
        for exe in ["pwsh.exe", "powershell.exe"] {
            if find_in_path(exe) {
                return ShellCommand { program: exe.into(), args: vec!["-NoLogo".into()] };
            }
        }
        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
        ShellCommand { program: comspec, args: Vec::new() }
    }
    #[cfg(not(windows))]
    {
        let program = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/bash".into());
        ShellCommand { program, args: Vec::new() }
    }
}

#[cfg(windows)]
pub(crate) fn find_in_path(exe: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file())).unwrap_or(false)
}

/// The full path of program `name` in the folders of `path` (a `PATH`
/// value). On Windows `name` may leave out `.exe` (or `.com`). Batch files
/// are not taken: they can't be started without a shell. Folders on `path`
/// that aren't absolute are skipped, as they depend on the working directory.
pub fn find_program(name: &str, path: &OsStr) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) && Path::new(name).extension().is_none() {
        vec![format!("{name}.exe"), format!("{name}.com")]
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(path)
        .filter(|d| d.is_absolute())
        .flat_map(|d| names.iter().map(move |n| d.join(n)).collect::<Vec<_>>())
        .find(|p| p.is_file())
}

/// A terminal editor for files, and what to add to its environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorProgram {
    pub program: PathBuf,
    pub env: Vec<(String, String)>,
}

impl EditorProgram {
    /// `program`, with what it needs to run here: a Vim built for MSYS (Git
    /// for Windows has one in `usr\bin`) reads `TERM`, which Windows doesn't
    /// set, and TerminalRooster's own Neovim gets its folder on `PATH`, for
    /// the tools that come with it (`win32yank.exe` for the clipboard).
    pub fn new(program: PathBuf) -> EditorProgram {
        let mut env = Vec::new();
        if cfg!(windows) && program.parent().is_some_and(|d| d.ends_with(Path::new("usr").join("bin"))) {
            env.push(("TERM".to_owned(), "xterm-256color".to_owned()));
        }
        if let Some(bin) = program.parent().filter(|_| super::bundled_nvim::installed().as_ref() == Some(&program)) {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let dirs = std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(&path));
            if let Ok(joined) = std::env::join_paths(dirs) {
                env.push(("PATH".to_owned(), joined.to_string_lossy().into_owned()));
            }
        }
        EditorProgram { program, env }
    }
}

/// The editor files open in when they are edited in a terminal editor and
/// no command is set: Neovim on PATH, else TerminalRooster's own Neovim
/// (Windows), else Vim on PATH; on Windows then the Vim of Git for Windows,
/// else the one of TerminalRooster's own bash.
pub fn find_terminal_editor() -> Option<EditorProgram> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let ours: Vec<PathBuf> = super::bundled_nvim::installed().into_iter().collect();
    pick_editor(&path, &ours, &fallback_editors()).map(EditorProgram::new)
}

fn pick_editor(path: &OsStr, ours: &[PathBuf], fallbacks: &[PathBuf]) -> Option<PathBuf> {
    let existing = |list: &[PathBuf]| list.iter().find(|p| p.is_file()).cloned();
    find_program("nvim", path)
        .or_else(|| existing(ours))
        .or_else(|| find_program("vim", path))
        .or_else(|| existing(fallbacks))
}

#[cfg(windows)]
fn fallback_editors() -> Vec<PathBuf> {
    let git = windows::git_root().map(|root| root.join(r"usr\bin\vim.exe"));
    let ours = super::bundled_bash::install_dir().map(|dir| dir.join(r"usr\bin\vim.exe"));
    git.into_iter().chain(ours).collect()
}

#[cfg(not(windows))]
fn fallback_editors() -> Vec<PathBuf> {
    Vec::new()
}

/// Resolves the shell for a pane: project override, then global default, then platform.
pub fn resolve(project: Option<&str>, global: Option<&str>) -> ShellCommand {
    project.and_then(split_command_line).or_else(|| global.and_then(split_command_line)).unwrap_or_else(default_shell)
}

/// Joins a program and its arguments into a command line that
/// [`split_command_line`] splits back into the same parts.
pub fn join_command_line<S: AsRef<str>>(parts: &[S]) -> String {
    let quote = |p: &str| {
        if !p.is_empty() && !p.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
            p.to_owned()
        } else {
            // A `"` inside: close the quotes, add it in single quotes, reopen.
            format!("\"{}\"", p.replace('"', "\"'\"'\""))
        }
    };
    parts.iter().map(|p| quote(p.as_ref())).collect::<Vec<_>>().join(" ")
}

/// A shell found on this machine, offered in the shell pickers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellOption {
    /// Display name, e.g. `Git Bash` or `WSL: Ubuntu`.
    pub name: String,
    /// The command line to store as a shell setting.
    pub command: String,
}

impl ShellOption {
    pub(crate) fn new<S: AsRef<str>>(name: impl Into<String>, parts: &[S]) -> Self {
        Self { name: name.into(), command: join_command_line(parts) }
    }
}

impl std::fmt::Display for ShellOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// The shells installed on this machine, best first. On Windows this lists
/// TerminalRooster's own bash (once downloaded), Git Bash, MSYS2, WSL distributions,
/// PowerShell and cmd; elsewhere the login shells in `/etc/shells`.
///
/// Can take a moment (it asks `wsl.exe` for its distributions), so call it
/// off the UI thread.
pub fn detect_shells() -> Vec<ShellOption> {
    #[cfg(windows)]
    {
        windows::detect()
    }
    #[cfg(not(windows))]
    {
        std::fs::read_to_string("/etc/shells")
            .map(|text| parse_etc_shells(&text, |p| std::path::Path::new(p).is_file()))
            .unwrap_or_default()
    }
}

/// Shells listed in `/etc/shells` that exist, one per name (the first listed
/// path wins, so `/bin/bash` hides `/usr/bin/bash`).
#[cfg_attr(windows, allow(dead_code))]
fn parse_etc_shells(text: &str, exists: impl Fn(&str) -> bool) -> Vec<ShellOption> {
    // Not interactive shells, or restricted ones.
    const SKIP: [&str; 6] = ["nologin", "false", "rbash", "git-shell", "tmux", "screen"];
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with('/'))
        .filter_map(|path| {
            let name = path.rsplit('/').next().filter(|n| !n.is_empty())?;
            let keep = !SKIP.contains(&name) && exists(path) && seen.insert(name.to_owned());
            keep.then(|| ShellOption::new(name, &[path]))
        })
        .collect()
}

/// Distribution names from `wsl.exe --list --quiet`, which writes UTF-16
/// unless `WSL_UTF8` is set. Docker Desktop's internal distributions are
/// not shells, so they are left out.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_wsl_list(out: &[u8]) -> Vec<String> {
    let text = if out.contains(&0) {
        let units: Vec<u16> = out.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(out).into_owned()
    };
    text.lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0'))
        .filter(|l| !l.is_empty() && !l.starts_with("docker-desktop"))
        .map(str::to_owned)
        .collect()
}

#[cfg(windows)]
mod windows {
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::{ShellOption, find_in_path, parse_wsl_list};
    use crate::session::bundled_bash;

    /// Keeps console programs started from the GUI from flashing a window.
    pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub(super) fn detect() -> Vec<ShellOption> {
        let mut found = Vec::new();
        if let Some(exe) = bundled_bash::installed() {
            found.push(bundled_bash::shell_option(&exe));
        }
        if let Some(root) = git_root() {
            let bash = display(&root.join(r"bin\bash.exe"));
            found.push(ShellOption::new("Git Bash", &[bash.as_str(), "-i", "-l"]));
        }
        if let Some(root) = msys2_root() {
            // The launcher keeps the working directory (`-here`) and runs in
            // this terminal (`-defterm -no-start`).
            let launcher = display(&root.join("msys2_shell.cmd"));
            found.push(ShellOption::new(
                "MSYS2 (UCRT64)",
                &[launcher.as_str(), "-defterm", "-here", "-no-start", "-ucrt64"],
            ));
        }
        for distro in wsl_distros() {
            // wsl.exe starts in the Windows working directory, under /mnt.
            found.push(ShellOption::new(format!("WSL: {distro}"), &["wsl.exe", "-d", distro.as_str()]));
        }
        for (name, exe) in [("PowerShell 7", "pwsh.exe"), ("Windows PowerShell", "powershell.exe")] {
            if find_in_path(exe) {
                found.push(ShellOption::new(name, &[exe, "-NoLogo"]));
            }
        }
        found.push(ShellOption::new("Command Prompt", &["cmd.exe"]));
        found
    }

    fn display(p: &Path) -> String {
        p.display().to_string()
    }

    /// A Git for Windows installation: next to `git.exe` on PATH, or in one of
    /// the usual places (installer, per-user installer, scoop).
    pub(super) fn git_root() -> Option<PathBuf> {
        let is_root = |d: &Path| d.join(r"bin\bash.exe").is_file() && d.join("git-bash.exe").is_file();
        // git.exe lives in <root>\cmd, <root>\bin or <root>\mingw64\bin.
        let near_git = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).filter(|d| d.join("git.exe").is_file()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .flat_map(|d| d.ancestors().skip(1).take(2).map(Path::to_path_buf).collect::<Vec<_>>());
        let under = |var: &str, rel: &str| std::env::var_os(var).map(|v| PathBuf::from(v).join(rel));
        let usual = [
            under("ProgramW6432", "Git"),
            under("ProgramFiles", "Git"),
            under("ProgramFiles(x86)", "Git"),
            under("LOCALAPPDATA", r"Programs\Git"),
            under("USERPROFILE", r"scoop\apps\git\current"),
        ];
        near_git.chain(usual.into_iter().flatten()).find(|d| is_root(d))
    }

    fn msys2_root() -> Option<PathBuf> {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        [PathBuf::from(format!(r"{drive}\msys64")), PathBuf::from(r"C:\msys64")]
            .into_iter()
            .chain(std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(r"scoop\apps\msys2\current")))
            .find(|d| d.join("msys2_shell.cmd").is_file() && d.join(r"usr\bin\bash.exe").is_file())
    }

    fn wsl_distros() -> Vec<String> {
        if !find_in_path("wsl.exe") {
            return Vec::new();
        }
        let child = Command::new("wsl.exe")
            .args(["--list", "--quiet"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        let Ok(mut child) = child else { return Vec::new() };
        // A broken WSL can hang; the list is not worth waiting long for.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::warn!("wsl.exe --list did not answer; leaving WSL out of the shell list");
                    return Vec::new();
                }
                // Exits with an error when no distribution is installed.
                _ => return Vec::new(),
            }
        }
        // The output is a few names, so it fit in the pipe while we waited.
        let mut out = Vec::new();
        if let Some(mut stdout) = child.stdout.take() {
            let _ = std::io::Read::read_to_end(&mut stdout, &mut out);
        }
        parse_wsl_list(&out)
    }
}

#[cfg(windows)]
pub(crate) use windows::CREATE_NO_WINDOW;

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty program file named as `name` would be on this platform.
    fn program(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() });
        std::fs::write(&path, "").unwrap();
        path
    }

    #[test]
    fn programs_are_found_on_the_path() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let vim = program(a.path(), "vim");
        let nvim = program(b.path(), "nvim");
        let both = std::env::join_paths([a.path(), b.path()]).unwrap();
        assert_eq!(find_program("vim", &both), Some(vim.clone()));
        assert_eq!(find_program("emacs", &both), None);
        let ours = program(a.path(), "our-nvim");
        assert_eq!(pick_editor(&both, std::slice::from_ref(&ours), &[]), Some(nvim), "Neovim on the path first");
        let only_vim = std::env::join_paths([a.path()]).unwrap();
        assert_eq!(pick_editor(&only_vim, std::slice::from_ref(&ours), &[]), Some(ours), "then our own Neovim");
        assert_eq!(pick_editor(&only_vim, &[], &[]), Some(vim), "then Vim");
        let fallback = program(a.path(), "fallback");
        let none = OsStr::new("");
        assert_eq!(pick_editor(none, &[], &[a.path().join("missing"), fallback.clone()]), Some(fallback));
        assert_eq!(pick_editor(none, &[], &[]), None);
        let relative = std::env::join_paths(["relative-dir"]).unwrap();
        assert_eq!(find_program("vim", &relative), None, "folders that aren't absolute are skipped");
    }

    #[cfg(windows)]
    #[test]
    fn windows_names_may_leave_out_exe_but_not_be_batch_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tool.cmd"), "").unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(find_program("tool", &path), None);
        let exe = program(dir.path(), "tool");
        assert_eq!(find_program("tool", &path), Some(exe.clone()));
        assert_eq!(find_program("tool.exe", &path), Some(exe));
    }

    #[test]
    fn msys_vim_is_told_the_terminal_type() {
        let git_vim = EditorProgram::new(PathBuf::from(r"C:\Program Files\Git\usr\bin\vim.exe"));
        let nvim = EditorProgram::new(PathBuf::from(r"C:\Program Files\Neovim\bin\nvim.exe"));
        assert!(nvim.env.is_empty());
        if cfg!(windows) {
            assert_eq!(git_vim.env, [("TERM".to_owned(), "xterm-256color".to_owned())]);
        } else {
            assert!(git_vim.env.is_empty(), "TERM is set for every terminal elsewhere");
        }
    }

    #[test]
    fn joining_round_trips() {
        for parts in [
            vec![r"C:\Program Files\Git\bin\bash.exe", "-i", "-l"],
            vec!["wsl.exe", "-d", "My Distro"],
            vec!["sh", "-c", r#"echo "it's""#, ""],
        ] {
            let line = join_command_line(&parts);
            let back = split_command_line(&line).unwrap();
            assert_eq!(std::iter::once(back.program).chain(back.args).collect::<Vec<_>>(), parts, "{line}");
        }
        assert_eq!(join_command_line(&["pwsh.exe", "-NoLogo"]), "pwsh.exe -NoLogo");
        assert_eq!(join_command_line(&[r"C:\Program Files\x.exe"]), r#""C:\Program Files\x.exe""#);
    }

    #[test]
    fn etc_shells() {
        let text = "# /etc/shells\n/bin/sh\n/bin/bash\n/usr/bin/bash\n/bin/rbash\n/usr/bin/tmux\n/usr/bin/zsh\n\
                    /usr/sbin/nologin\n/usr/bin/fish\n";
        let found = parse_etc_shells(text, |p| p != "/usr/bin/fish");
        let got: Vec<_> = found.iter().map(|o| (o.name.as_str(), o.command.as_str())).collect();
        assert_eq!(got, vec![("sh", "/bin/sh"), ("bash", "/bin/bash"), ("zsh", "/usr/bin/zsh")]);
    }

    #[test]
    fn wsl_list() {
        let utf16: Vec<u8> =
            "Ubuntu\r\ndocker-desktop\r\nDebian\r\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(parse_wsl_list(&utf16), vec!["Ubuntu", "Debian"]);
        assert_eq!(parse_wsl_list(b"Ubuntu-24.04\n"), vec!["Ubuntu-24.04"]);
        assert!(parse_wsl_list(b"").is_empty());
    }

    #[test]
    fn splitting() {
        let c = split_command_line(r#"pwsh -NoLogo -Command "echo hi there" ''"#).unwrap();
        assert_eq!(c.program, "pwsh");
        assert_eq!(c.args, vec!["-NoLogo", "-Command", "echo hi there", ""]);
        assert_eq!(split_command_line("   "), None);
        assert_eq!(resolve(Some("  "), Some("zsh -l")).program, "zsh");
    }
}
