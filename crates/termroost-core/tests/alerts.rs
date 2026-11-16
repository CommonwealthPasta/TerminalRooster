//! Alerts from real shells: the shell integration reports failed commands,
//! and the sequences agents send get through the PTY (ConPTY on Windows).

use std::path::Path;
use std::time::{Duration, Instant};

use termroost_core::*;

fn open(dir: &Path) -> Core {
    let agents = dir.join("agent-homes");
    Core::open(Config {
        data_dir: Some(dir.to_path_buf()),
        autosave_interval: Duration::from_millis(500),
        agent_homes: AgentHomes {
            claude: agents.join("claude"),
            codex: agents.join("codex"),
            grok: agents.join("grok"),
        },
        ..Default::default()
    })
    .expect("open core")
}

/// The shell under test, and how to make it print raw bytes given as `\e`-style text.
struct Shell {
    command: String,
    prompt: &'static str,
    fail: &'static str,
    print: fn(&str) -> String,
}

#[cfg_attr(windows, allow(dead_code))]
fn bash(command: String) -> Shell {
    Shell {
        command,
        // Lines come back without trailing spaces.
        prompt: "$",
        fail: "(exit 3)",
        print: |s| format!("printf '{}'", s.replace(r"\e", r"\033").replace(r"\a", r"\007")),
    }
}

#[cfg(windows)]
#[test]
fn powershell() {
    check(Shell {
        command: "powershell.exe -NoLogo -NoProfile".into(),
        prompt: "PS ",
        fail: "cmd /c exit 3",
        print: |s| {
            let s = s.replace('\'', "''").replace(r"\e", "' + [char]27 + '").replace(r"\a", "' + [char]7 + '");
            // UTF-8, as agents write it (Windows PowerShell defaults to the OEM code page).
            format!("[Console]::OutputEncoding = [Text.Encoding]::UTF8; [Console]::Write('{s}')")
        },
    });
}

/// Git Bash as TerminalRooster starts it: an interactive login shell, which
/// runs Git's own startup files first.
#[cfg(windows)]
#[test]
fn git_bash() {
    let exe = r"C:\Program Files\Git\bin\bash.exe";
    if !Path::new(exe).is_file() {
        eprintln!("Git Bash is not installed; skipped");
        return;
    }
    check(bash(format!("\"{exe}\" -i -l")));
}

#[cfg(not(windows))]
#[test]
fn bash_without_startup_files() {
    check(bash("bash --norc --noprofile".into()));
}

fn next_alert(core: &mut Core, pane: PaneId, timeout: Duration) -> Alert {
    let events = core.events();
    let deadline = Instant::now() + timeout;
    loop {
        match events.try_recv() {
            Ok(e) => {
                core.apply_event(&e);
                if let Event::Alert { pane: p, alert } = e
                    && p == pane
                {
                    return alert;
                }
            }
            Err(_) => {
                assert!(Instant::now() < deadline, "timed out waiting for an alert");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn wait_for_text(core: &mut Core, pane: PaneId, needle: &str) {
    let events = core.events();
    let deadline = Instant::now() + Duration::from_secs(20);
    let text = |core: &Core| core.terminal(pane).map(|h| h.with_content(|c| c.text())).unwrap_or_default();
    while !text(core).contains(needle) {
        while let Ok(e) = events.try_recv() {
            core.apply_event(&e);
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needle:?}; the screen shows:
{}",
            text(core)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn check(shell: Shell) {
    let dir = tempfile::tempdir().unwrap();
    let mut core = open(dir.path());
    core.dispatch(Command::SetDefaultShell(Some(shell.command.clone()))).unwrap();
    core.dispatch(Command::CreateProject { name: "p".into(), working_dir: dir.path().into(), shell: None }).unwrap();
    let pane = core.state().active_tab().unwrap().layout.first_pane();
    wait_for_text(&mut core, pane, shell.prompt);
    let h = core.terminal(pane).unwrap();
    let run = |line: &str| h.write(format!("{line}\r"));
    let long = Duration::from_secs(20);

    // A failing command, reported by the shell integration with its command line.
    run(shell.fail);
    assert_eq!(next_alert(&mut core, pane, long), Alert::CommandFailed { code: 3, command: Some(shell.fail.into()) });
    // An empty line repeats no status.
    run("");

    // Codex's lifecycle state (OSC 7501).
    run(&(shell.print)(r"\e]7501;state=working:app=codex\e\"));
    run(&(shell.print)(r"\e]7501;state=done:app=codex\e\"));
    assert_eq!(
        next_alert(&mut core, pane, long),
        Alert::Agent { agent: Some(AgentKind::Codex), event: AgentEvent::Done, message: None }
    );

    // Claude Code's title.
    run(&(shell.print)("\\e]0;\u{25d0} Fixing\\a"));
    run(&(shell.print)("\\e]0;\u{2733} Fixing\\a"));
    assert_eq!(
        next_alert(&mut core, pane, long),
        Alert::Agent { agent: Some(AgentKind::Claude), event: AgentEvent::Done, message: None }
    );

    // A program's own notification (OSC 9).
    run(&(shell.print)(r"\e]9;Build finished\a"));
    assert_eq!(next_alert(&mut core, pane, long), Alert::Notification { title: None, body: "Build finished".into() });

    // Another failure after all that: the integration still runs.
    run(shell.fail);
    assert_eq!(next_alert(&mut core, pane, long), Alert::CommandFailed { code: 3, command: Some(shell.fail.into()) });
}

/// What the quit confirmation lists: programs started from the shell, not the shell.
#[test]
fn running_programs_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = open(dir.path());
    let (shell, prompt, sleep, name) = if cfg!(windows) {
        ("powershell.exe -NoLogo -NoProfile", "PS ", "ping -n 30 127.0.0.1", "ping")
    } else {
        ("bash --norc --noprofile", "$", "sleep 30", "sleep")
    };
    core.dispatch(Command::SetDefaultShell(Some(shell.into()))).unwrap();
    core.dispatch(Command::CreateProject { name: "p".into(), working_dir: dir.path().into(), shell: None }).unwrap();
    let pane = core.state().active_tab().unwrap().layout.first_pane();
    wait_for_text(&mut core, pane, prompt);
    assert_eq!(core.running_programs(), vec![], "an idle shell runs nothing");

    core.terminal(pane).unwrap().write(format!("{sleep}\r"));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let running = core.running_programs();
        if let [(p, names)] = running.as_slice()
            && *p == pane
            && names.iter().any(|n| n.eq_ignore_ascii_case(name))
        {
            break;
        }
        assert!(Instant::now() < deadline, "{name} not listed: {running:?}");
        std::thread::sleep(Duration::from_millis(200));
    }
}
