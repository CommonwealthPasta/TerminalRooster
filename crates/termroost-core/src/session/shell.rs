//! Shell selection and command-line splitting.

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
fn find_in_path(exe: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file())).unwrap_or(false)
}

/// Resolves the shell for a pane: project override, then global default, then platform.
pub fn resolve(project: Option<&str>, global: Option<&str>) -> ShellCommand {
    project.and_then(split_command_line).or_else(|| global.and_then(split_command_line)).unwrap_or_else(default_shell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting() {
        let c = split_command_line(r#"pwsh -NoLogo -Command "echo hi there" ''"#).unwrap();
        assert_eq!(c.program, "pwsh");
        assert_eq!(c.args, vec!["-NoLogo", "-Command", "echo hi there", ""]);
        assert_eq!(split_command_line("   "), None);
        assert_eq!(resolve(Some("  "), Some("zsh -l")).program, "zsh");
    }
}
