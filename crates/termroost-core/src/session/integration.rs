//! Shell integration: makes bash and PowerShell report how each command
//! ended (`OSC 133 ; D ; <status>`), so failed commands can be noticed.
//!
//! Nothing in the user's own startup files changes. TerminalRooster writes two
//! small scripts into its data directory and starts the shells so that they
//! run them after their usual startup:
//!
//! * **bash** gets `PROMPT_COMMAND` in its environment, which sources the
//!   script on the first prompt (so it works with `--login` too). If
//!   `~/.bashrc` replaces `PROMPT_COMMAND` instead of adding to it, this does
//!   nothing.
//! * **PowerShell** (`pwsh`, `powershell`) gets `-NoExit -Command . <script>`
//!   added to its command line, unless the command line already runs a
//!   command or a file.
//!
//! Other shells (cmd, zsh, fish, WSL) are started unchanged; any shell
//! integration that sends `OSC 133 ; D ; <status>` itself works as well.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use super::shell::ShellCommand;

/// Folder in the data directory that holds the scripts.
pub const DIR: &str = "shell-integration";
const BASH_FILE: &str = "terminalrooster.bash";
const PWSH_FILE: &str = "terminalrooster.ps1";
const BASH: &str = include_str!("integration/terminalrooster.bash");
const PWSH: &str = include_str!("integration/terminalrooster.ps1");

/// Writes (or refreshes) the scripts; returns their folder.
pub(crate) fn install(data_dir: &Path) -> io::Result<PathBuf> {
    let dir = data_dir.join(DIR);
    std::fs::create_dir_all(&dir)?;
    for (name, text) in [(BASH_FILE, BASH), (PWSH_FILE, PWSH)] {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
            std::fs::write(&path, text)?;
        }
    }
    Ok(dir)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Bash,
    PowerShell,
}

fn kind(program: &str) -> Option<Kind> {
    let name = program.trim_matches('"').rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "bash" => Some(Kind::Bash),
        "pwsh" | "powershell" => Some(Kind::PowerShell),
        _ => None,
    }
}

/// A path as bash sees it: forward slashes work for Git Bash and MSYS2 as well.
fn bash_path(path: &Path) -> String {
    let p = path.display().to_string();
    if cfg!(windows) { p.replace('\\', "/") } else { p }
}

/// Single-quotes a string for bash or PowerShell (same rules except the escape).
fn quote(s: &str, ps: bool) -> String {
    if ps { format!("'{}'", s.replace('\'', "''")) } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

/// Whether PowerShell's arguments leave room for ours: only switches, and
/// none that runs a command or a file (PowerShell accepts any prefix of a
/// parameter name, so `-c`, `-com` and `-Command` are all the same).
fn powershell_takes_command(args: &[String]) -> bool {
    args.iter().all(|a| {
        let Some(name) = a.strip_prefix('-').or_else(|| a.strip_prefix('/')) else { return false };
        let name = name.to_ascii_lowercase();
        !name.is_empty()
            && !["command", "file", "encodedcommand", "commandwithargs", "noninteractive"]
                .iter()
                .any(|p| p.starts_with(&name))
            && name != "ec"
            && name != "cwa"
    })
}

/// Sets up integration for `shell`, if it is a shell this knows. `dir` is
/// where [`install`] put the scripts.
pub(crate) fn apply(shell: &mut ShellCommand, env: &mut HashMap<String, String>, dir: &Path) {
    match kind(&shell.program) {
        Some(Kind::Bash) => {
            let script = quote(&bash_path(&dir.join(BASH_FILE)), false);
            let source = format!("[ -f {script} ] && . {script}");
            env.insert("PROMPT_COMMAND".into(), source.clone());
            // Tells the script which text to take out of PROMPT_COMMAND again.
            env.insert("TERMINALROOSTER_BASH_INTEGRATION".into(), source);
        }
        Some(Kind::PowerShell) if powershell_takes_command(&shell.args) => {
            if !shell.args.iter().any(|a| a.eq_ignore_ascii_case("-noexit")) {
                shell.args.push("-NoExit".into());
            }
            shell.args.push("-Command".into());
            shell.args.push(format!(". {}", quote(&dir.join(PWSH_FILE).display().to_string(), true)));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(line: &str) -> ShellCommand {
        super::super::shell::split_command_line(line).unwrap()
    }

    #[test]
    fn knows_bash_and_powershell() {
        assert_eq!(kind("/bin/bash"), Some(Kind::Bash));
        assert_eq!(kind(r"C:\Program Files\Git\bin\bash.exe"), Some(Kind::Bash));
        assert_eq!(kind("pwsh.exe"), Some(Kind::PowerShell));
        assert_eq!(kind(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"), Some(Kind::PowerShell));
        assert_eq!(kind("cmd.exe"), None);
        assert_eq!(kind("wsl.exe"), None);
        assert_eq!(kind("/usr/bin/zsh"), None);
    }

    #[test]
    fn powershell_gets_the_script_after_its_profile() {
        let dir = Path::new("/data/shell-integration");
        let mut env = HashMap::new();
        let mut shell = cmd("pwsh.exe -NoLogo");
        apply(&mut shell, &mut env, dir);
        assert_eq!(shell.args[..3], ["-NoLogo", "-NoExit", "-Command"]);
        assert!(shell.args[3].starts_with(". '") && shell.args[3].ends_with("terminalrooster.ps1'"));
        assert!(env.is_empty());

        for line in ["pwsh -c Get-Date", "pwsh -Command x", "pwsh -File a.ps1", "pwsh a.ps1", "pwsh -NonInteractive"] {
            let mut shell = cmd(line);
            let before = shell.clone();
            apply(&mut shell, &mut env, dir);
            assert_eq!(shell, before, "{line}");
        }
        let mut shell = cmd("powershell -NoExit");
        apply(&mut shell, &mut env, dir);
        assert_eq!(shell.args.iter().filter(|a| a.eq_ignore_ascii_case("-noexit")).count(), 1);
    }

    #[test]
    fn bash_sources_the_script_from_prompt_command() {
        let mut env = HashMap::new();
        let mut shell = cmd("bash --login -i");
        let before = shell.clone();
        apply(&mut shell, &mut env, Path::new("/data/it's here"));
        assert_eq!(shell, before);
        let pc = &env["PROMPT_COMMAND"];
        assert!(pc.contains(r"'/data/it'\''s here/terminalrooster.bash'"), "{pc}");
    }

    #[test]
    fn install_writes_and_refreshes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(BASH_FILE)).unwrap(), BASH);
        std::fs::write(dir.join(PWSH_FILE), "old").unwrap();
        install(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(PWSH_FILE)).unwrap(), PWSH);
    }
}
