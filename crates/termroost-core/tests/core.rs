//! End-to-end tests of the core with real PTYs and a real database.

use std::path::Path;
use std::time::{Duration, Instant};

use termroost_core::*;

/// Agent state directories inside the test's temp dir, never the user's real ones.
fn test_homes(dir: &Path) -> AgentHomes {
    let agents = dir.join("agent-homes");
    AgentHomes { claude: agents.join("claude"), codex: agents.join("codex"), grok: agents.join("grok") }
}

fn open(dir: &Path) -> Core {
    Core::open(Config {
        data_dir: Some(dir.to_path_buf()),
        autosave_interval: Duration::from_millis(200),
        agent_homes: test_homes(dir),
        ..Default::default()
    })
    .expect("open core")
}

/// A shell command line that prints `text`, then stays alive (or exits with `code`).
fn printer(text: &str, exit_code: Option<i32>) -> String {
    if cfg!(windows) {
        match exit_code {
            Some(c) => format!("cmd /c \"echo {text} & exit {c}\""),
            None => format!("cmd /c \"echo {text} & ping -n 60 127.0.0.1 >nul\""),
        }
    } else {
        match exit_code {
            Some(c) => format!("sh -c \"printf '{text}\\n'; exit {c}\""),
            None => format!("sh -c \"printf '{text}\\n'; exec cat\""),
        }
    }
}

fn wait_for(core: &mut Core, timeout: Duration, mut done: impl FnMut(&mut Core, Option<&Event>) -> bool) {
    let events = core.events();
    let deadline = Instant::now() + timeout;
    loop {
        let ev = events.try_recv().ok();
        if let Some(e) = &ev {
            core.apply_event(e);
        }
        if done(core, ev.as_ref()) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting");
        if ev.is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn screen_text(core: &Core, pane: PaneId) -> String {
    core.terminal(pane).map(|h| h.with_content(|c| c.text())).unwrap_or_default()
}

fn wait_for_text(core: &mut Core, pane: PaneId, needle: &str) {
    wait_for(core, Duration::from_secs(15), |c, _| screen_text(c, pane).contains(needle));
}

fn only_pane(core: &Core) -> PaneId {
    core.state().active_tab().unwrap().layout.first_pane()
}

#[test]
fn terminal_runs_and_reports_exit() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = open(dir.path());
    core.dispatch(Command::SetDefaultShell(Some(printer("hello terminalrooster", Some(3))))).unwrap();
    core.dispatch(Command::CreateProject { name: "p".into(), working_dir: dir.path().into(), shell: None }).unwrap();
    let pane = only_pane(&core);

    let mut code = None;
    wait_for(&mut core, Duration::from_secs(15), |_, ev| {
        if let Some(Event::ProcessExited { pane: p, code: c }) = ev
            && *p == pane
        {
            code = Some(*c);
            return true;
        }
        false
    });
    assert_eq!(code, Some(Some(3)));
    let handle = core.terminal(pane).unwrap();
    assert!(!handle.is_running());
    assert_eq!(handle.exit_status(), Some(Some(3)));
    assert!(screen_text(&core, pane).contains("hello terminalrooster"));

    // Resizing updates the grid.
    handle.resize(GridSize { cols: 40, rows: 10, cell_width: 8, cell_height: 16 });
    handle.with_content(|c| assert_eq!((c.cols, c.rows), (40, 10)));
}

#[test]
fn state_and_scrollback_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (project, tab, panes, task) = {
        let mut core = open(dir.path());
        core.dispatch(Command::SetDefaultShell(Some(printer("marker-123", None)))).unwrap();
        let Outcome::Project(project) = core
            .dispatch(Command::CreateProject { name: "api".into(), working_dir: dir.path().into(), shell: None })
            .unwrap()
        else {
            panic!()
        };
        let tab = core.state().active_tab().unwrap().id;
        core.dispatch(Command::RenameTab { tab, title: "agents".into() }).unwrap();
        let first = only_pane(&core);
        let Outcome::Pane(second) = core.dispatch(Command::SplitPane { pane: first, axis: Axis::Vertical }).unwrap()
        else {
            panic!()
        };
        core.dispatch(Command::SetResumeCommand { pane: first, command: Some("echo resumed-ok".into()) }).unwrap();
        let Outcome::Task(task) =
            core.dispatch(Command::CreateTask { project, title: "fix auth".into(), tab: Some(tab) }).unwrap()
        else {
            panic!()
        };
        core.dispatch(Command::CreateTask { project, title: "write docs".into(), tab: None }).unwrap();
        core.dispatch(Command::SetTaskDone { task, done: true }).unwrap();
        wait_for_text(&mut core, first, "marker-123");
        wait_for_text(&mut core, second, "marker-123");
        // Clean shutdown saves scrollback.
        (project, tab, vec![first, second], task)
    };

    let mut core = open(dir.path());
    let s = core.state();
    assert_eq!(s.selected_project, Some(project));
    assert_eq!(s.project(project).unwrap().name, "api");
    let t = s.tab(tab).unwrap();
    assert_eq!(t.title, "agents");
    assert_eq!(t.layout.panes(), panes);
    assert_eq!(s.tasks_of(project).count(), 2);
    assert!(s.task(task).unwrap().done);
    assert_eq!(s.open_task_count(tab), 0);

    // The visible tab is started again with its old scrollback and resume command.
    wait_for_text(&mut core, panes[0], "restored session");
    let text = screen_text(&core, panes[0]);
    assert!(text.contains("marker-123"), "{text}");
    wait_for_text(&mut core, panes[0], "resumed-ok");
}

#[test]
fn agents_resume_their_exact_session() {
    let dir = tempfile::tempdir().unwrap();
    let pane;
    let session_id;
    {
        let mut core = open(dir.path());
        // `cat` echoes what is typed, so the launch line shows without running an agent.
        core.dispatch(Command::SetDefaultShell(Some(printer("ready", None)))).unwrap();
        core.dispatch(Command::CreateProject { name: "a".into(), working_dir: dir.path().into(), shell: None })
            .unwrap();
        pane = only_pane(&core);
        wait_for_text(&mut core, pane, "ready");
        core.dispatch(Command::StartAgent { pane, kind: AgentKind::Claude, args: "--model opus".into() }).unwrap();
        let agent = core.state().pane(pane).unwrap().agent.clone().unwrap();
        session_id = agent.session_id.clone().unwrap();
        wait_for_text(&mut core, pane, &format!("claude --session-id {session_id}"));
    }
    // No conversation was saved for that session yet, so Claude starts fresh.
    let projects = test_homes(dir.path()).claude.join("projects/-tmp-a");
    std::fs::create_dir_all(&projects).unwrap();
    {
        let mut core = open(dir.path());
        wait_for_text(&mut core, pane, "\nclaude --model opus");
    }
    // Once the conversation exists on disk, the exact session is resumed.
    std::fs::write(projects.join(format!("{session_id}.jsonl")), "{}").unwrap();
    let mut core = open(dir.path());
    wait_for_text(&mut core, pane, &format!("claude --resume {session_id} --model opus"));
    core.dispatch(Command::ClearAgent(pane)).unwrap();
    assert!(core.state().pane(pane).unwrap().agent.is_none());
}

#[test]
fn second_instance_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let _core = open(dir.path());
    let config =
        Config { data_dir: Some(dir.path().into()), agent_homes: test_homes(dir.path()), ..Default::default() };
    let err = Core::open(config).err().unwrap();
    assert!(matches!(err, Error::AlreadyRunning(_)), "{err}");
}

#[test]
fn delete_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = open(dir.path());
    core.dispatch(Command::SetDefaultShell(Some(printer("x", None)))).unwrap();
    let Outcome::Project(p) = core
        .dispatch(Command::CreateProject { name: "p".into(), working_dir: dir.path().into(), shell: None })
        .unwrap()
    else {
        panic!()
    };
    let tab1 = core.state().active_tab().unwrap().id;
    let Outcome::Tab(tab2) = core.dispatch(Command::CreateTab { project: p, title: None }).unwrap() else { panic!() };
    assert_eq!(core.state().tab(tab2).unwrap().title, "Session 2");
    let Outcome::Task(task) =
        core.dispatch(Command::CreateTask { project: p, title: "t".into(), tab: Some(tab2) }).unwrap()
    else {
        panic!()
    };
    let pane2 = core.state().tab(tab2).unwrap().layout.first_pane();
    assert!(core.terminal(pane2).is_some());

    // Last pane of a tab cannot be closed.
    assert!(core.dispatch(Command::ClosePane(pane2)).is_err());

    core.dispatch(Command::DeleteTab(tab2)).unwrap();
    assert!(core.terminal(pane2).is_none());
    assert_eq!(core.state().task(task).unwrap().tab, None, "task unlinked, not deleted");
    assert_eq!(core.state().active_tab().unwrap().id, tab1);

    core.dispatch(Command::DeleteTask(task)).unwrap();
    assert!(core.state().task(task).is_none());

    core.dispatch(Command::DeleteProject(p)).unwrap();
    assert!(core.state().projects.is_empty());
    assert!(core.state().tabs.is_empty());
    assert_eq!(core.state().selected_project, None);
    drop(core);
    let core = open(dir.path());
    assert!(core.state().projects.is_empty());
}

#[test]
fn reordering_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = open(dir.path());
    core.dispatch(Command::SetDefaultShell(Some(printer("x", None)))).unwrap();
    let mut project = |name: &str| match core
        .dispatch(Command::CreateProject { name: name.into(), working_dir: dir.path().into(), shell: None })
        .unwrap()
    {
        Outcome::Project(p) => p,
        _ => panic!(),
    };
    let (a, b, c) = (project("a"), project("b"), project("c"));
    let tab1 = core.state().active_tab().unwrap().id;
    let Outcome::Tab(tab2) = core.dispatch(Command::CreateTab { project: c, title: None }).unwrap() else { panic!() };
    let Outcome::Tab(tab3) = core.dispatch(Command::CreateTab { project: c, title: None }).unwrap() else { panic!() };
    let projects = |core: &Core| core.state().projects.iter().map(|p| p.id).collect::<Vec<_>>();
    let tabs = |core: &Core| core.state().tabs_of(c).map(|t| t.id).collect::<Vec<_>>();

    // `to_index` is the place in the list without the moved item.
    core.dispatch(Command::MoveProject { project: c, to_index: 0 }).unwrap();
    assert_eq!(projects(&core), [c, a, b]);
    core.dispatch(Command::MoveProject { project: a, to_index: 2 }).unwrap();
    assert_eq!(projects(&core), [c, b, a]);
    core.dispatch(Command::MoveProject { project: b, to_index: 99 }).unwrap();
    assert_eq!(projects(&core), [c, a, b], "past the end means last");

    core.dispatch(Command::MoveTab { tab: tab3, to_index: 0 }).unwrap();
    core.dispatch(Command::MoveTab { tab: tab1, to_index: 2 }).unwrap();
    assert_eq!(tabs(&core), [tab3, tab2, tab1]);
    assert_eq!(core.state().active_tab().unwrap().id, tab3, "moving doesn't change what is shown");

    drop(core);
    let core = open(dir.path());
    assert_eq!(projects(&core), [c, a, b]);
    assert_eq!(tabs(&core), [tab3, tab2, tab1]);
}

const CRASH_CHILD_ENV: &str = "TERMINALROOSTER_TEST_CRASH_CHILD";

/// Runs in a child process: builds some state, then aborts without cleanup.
fn crash_child(dir: &Path) -> ! {
    let mut core = open(dir);
    core.dispatch(Command::SetDefaultShell(Some(printer("crash-marker", None)))).unwrap();
    let Outcome::Project(p) = core
        .dispatch(Command::CreateProject { name: "survivor".into(), working_dir: dir.into(), shell: None })
        .unwrap()
    else {
        panic!()
    };
    let tab = core.state().active_tab().unwrap().id;
    core.dispatch(Command::CreateTask { project: p, title: "persist me".into(), tab: Some(tab) }).unwrap();
    let pane = only_pane(&core);
    core.dispatch(Command::SplitPane { pane, axis: Axis::Horizontal }).unwrap();
    wait_for_text(&mut core, pane, "crash-marker");
    // Wait for the periodic autosave rather than forcing a save.
    std::thread::sleep(Duration::from_millis(800));
    core.dispatch(Command::RenameTab { tab, title: "after-save".into() }).unwrap();
    core.flush();
    std::process::abort();
}

#[test]
fn state_survives_a_crash() {
    if let Some(dir) = std::env::var_os(CRASH_CHILD_ENV) {
        crash_child(Path::new(&dir));
    }
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "state_survives_a_crash", "--nocapture", "--test-threads=1"])
        .env(CRASH_CHILD_ENV, dir.path())
        .status()
        .unwrap();
    assert!(!status.success(), "child should have aborted");

    // The crashed process's lock is gone, and everything it committed is back.
    let mut core = open(dir.path());
    let s = core.state();
    assert_eq!(s.projects.len(), 1);
    assert_eq!(s.projects[0].name, "survivor");
    let tab = s.active_tab().unwrap();
    assert_eq!(tab.title, "after-save");
    assert_eq!(tab.layout.panes().len(), 2);
    assert!(matches!(tab.layout, Layout::Split { axis: Axis::Horizontal, .. }));
    assert_eq!(s.tasks[0].title, "persist me");
    assert_eq!(s.open_task_count(tab.id), 1);
    let pane = tab.layout.first_pane();
    wait_for_text(&mut core, pane, "restored session");
    assert!(screen_text(&core, pane).contains("crash-marker"));
}

#[test]
fn palette_can_change_at_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = open(dir.path());
    core.dispatch(Command::SetDefaultShell(Some(printer("colors", None)))).unwrap();
    core.dispatch(Command::CreateProject { name: "p".into(), working_dir: dir.path().into(), shell: None }).unwrap();
    let pane = only_pane(&core);
    wait_for_text(&mut core, pane, "colors");
    let handle = core.terminal(pane).unwrap();
    let dark_bg = Palette::default().get(257);
    handle.with_content(|c| assert_eq!(c.default_bg, dark_bg));

    let white = Rgb::new(255, 255, 255);
    let black = Rgb::new(0, 0, 0);
    core.set_palette(Palette::new([black; 16], black, white, black, Rgb::new(200, 220, 255)));
    handle.with_content(|c| {
        assert_eq!(c.default_bg, white);
        assert_eq!(c.default_fg, black);
        assert!(c.cells.iter().all(|cell| cell.bg == white || cell.selected));
    });
}

/// A stand-in for Claude Code: registers `sessions/<pid>.json` the way the
/// real CLI does, then runs until interrupted.
#[cfg(unix)]
fn fake_claude(dir: &Path, homes: &AgentHomes) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let sessions = homes.claude.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join("claude");
    let script = format!(
        "#!/bin/sh\nprintf '{{\"pid\": %s, \"sessionId\": \"%s\", \"kind\": \"interactive\"}}' $$ \"$1\" > \"{}/$$.json\"\n\
         echo \"FAKE-CLAUDE running $1\"\nwhile :; do sleep 1; done\n",
        sessions.display()
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn agents_started_by_hand_are_detected_and_resumed_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let homes = AgentHomes {
        claude: dir.path().join("claude-home"),
        codex: dir.path().join("codex-home"),
        grok: dir.path().join("grok-home"),
    };
    let fake = fake_claude(dir.path(), &homes);
    let open_with_homes = || {
        Core::open(Config {
            data_dir: Some(dir.path().join("data")),
            autosave_interval: Duration::from_millis(200),
            agent_homes: homes.clone(),
            ..Default::default()
        })
        .unwrap()
    };
    let agent_of = |c: &Core, pane: PaneId| c.state().pane(pane).and_then(|p| p.agent.clone());
    let wait_agent = |c: &mut Core, pane: PaneId, want: Option<&str>| {
        wait_for(c, Duration::from_secs(20), |c, _| {
            agent_of(c, pane).and_then(|a| a.session_id).as_deref() == want
                && (want.is_none() || agent_of(c, pane).is_some_and(|a| a.kind == AgentKind::Claude))
        });
    };
    const A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
    const C: &str = "cccccccc-0000-4000-8000-000000000003";

    let pane;
    {
        let mut core = open_with_homes();
        core.dispatch(Command::SetDefaultShell(Some("sh".into()))).unwrap();
        core.dispatch(Command::CreateProject { name: "p".into(), working_dir: dir.path().into(), shell: None })
            .unwrap();
        pane = only_pane(&core);
        let term = core.terminal(pane).unwrap();
        std::thread::sleep(Duration::from_millis(500));

        // Typed by hand, not through "Start agent".
        term.text(&format!("{} {A}\r", fake.display()));
        wait_for_text(&mut core, pane, &format!("FAKE-CLAUDE running {A}"));
        wait_agent(&mut core, pane, Some(A));

        // `/clear` rewrites the registry with a new session id.
        let reg = std::fs::read_dir(homes.claude.join("sessions")).unwrap().next().unwrap().unwrap().path();
        let text = std::fs::read_to_string(&reg).unwrap().replace(A, B);
        std::fs::write(&reg, text).unwrap();
        wait_agent(&mut core, pane, Some(B));

        // Quitting the agent clears it.
        term.write(vec![3u8]);
        wait_for(&mut core, Duration::from_secs(20), |c, _| agent_of(c, pane).is_none());

        term.text(&format!("{} {C}\r", fake.display()));
        wait_agent(&mut core, pane, Some(C));
        // C has a saved conversation, so it can be resumed.
        std::fs::create_dir_all(homes.claude.join("projects/p")).unwrap();
        std::fs::write(homes.claude.join(format!("projects/p/{C}.jsonl")), "{}").unwrap();
    }

    let mut core = open_with_homes();
    assert_eq!(agent_of(&core, pane).and_then(|a| a.session_id).as_deref(), Some(C));
    wait_for_text(&mut core, pane, &format!("claude --resume {C}"));
}

#[cfg(unix)]
#[test]
fn codex_hook_reports_pin_threads_to_the_right_terminal() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let homes = test_homes(dir.path());
    std::fs::create_dir_all(&homes.codex).unwrap();
    std::fs::write(homes.codex.join("config.toml"), "").unwrap();
    // A stand-in `codex` that just keeps running.
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("codex");
    std::fs::write(&fake, "#!/bin/sh\necho FAKE-CODEX\nwhile :; do sleep 1; done\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut core = open(dir.path());
    assert!(core.codex_hook_status().codex_found);
    assert_eq!(core.codex_hook_status().installed_in, None);
    let hooks_file = core.install_codex_hook().unwrap();
    let hooks: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&hooks_file).unwrap()).unwrap();
    let command = hooks["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"].as_str().unwrap().to_owned();

    core.dispatch(Command::SetDefaultShell(Some("sh".into()))).unwrap();
    core.dispatch(Command::CreateProject { name: "p".into(), working_dir: dir.path().into(), shell: None }).unwrap();
    let one = only_pane(&core);
    let Outcome::Pane(two) = core.dispatch(Command::SplitPane { pane: one, axis: Axis::Vertical }).unwrap() else {
        panic!()
    };
    std::thread::sleep(Duration::from_millis(500));
    for pane in [one, two] {
        core.terminal(pane).unwrap().text(&format!("{}\r", fake.display()));
        wait_for_text(&mut core, pane, "FAKE-CODEX");
    }
    let codex_session = |c: &Core, pane: PaneId| {
        c.state().pane(pane).and_then(|p| p.agent.clone()).filter(|a| a.kind == AgentKind::Codex).map(|a| a.session_id)
    };
    wait_for(&mut core, Duration::from_secs(20), |c, _| {
        codex_session(c, one) == Some(None) && codex_session(c, two) == Some(None)
    });

    // What Codex does on a prompt: run the hook with the event on stdin.
    let enter = KeyInput { key: Key::Named(NamedKey::Enter), mods: Modifiers::NONE, text: None };
    let report = |thread: &str| {
        let event = serde_json::json!({
            "session_id": thread, "cwd": dir.path(), "hook_event_name": "UserPromptSubmit", "prompt": "hi"
        });
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(event.to_string().as_bytes()).unwrap();
        assert!(child.wait().unwrap().success());
    };
    const X: &str = "01a1e20f-eb7a-7971-a81c-602f1b0f4201";
    const Y: &str = "01a1e20f-eb7a-7971-a81c-602f1b0f4202";
    core.terminal(two).unwrap().key(&enter);
    report(X);
    wait_for(&mut core, Duration::from_secs(20), |c, _| codex_session(c, two) == Some(Some(X.into())));
    std::thread::sleep(Duration::from_millis(600));
    core.terminal(one).unwrap().key(&enter);
    report(Y);
    wait_for(&mut core, Duration::from_secs(20), |c, _| codex_session(c, one) == Some(Some(Y.into())));
    assert_eq!(codex_session(&core, two), Some(Some(X.into())), "pinned threads stay put");
    assert!(core.codex_hook_last_report().is_some());

    core.remove_codex_hook().unwrap();
    assert!(!hooks_file.exists());
}

/// Windows builds the command line by joining the program and its arguments:
/// a program under `C:\Program Files` and an argument with spaces must both
/// arrive intact. Uses Git Bash, so it is skipped where that is not installed.
#[cfg(windows)]
#[test]
fn windows_shell_paths_and_arguments_keep_their_spaces() {
    use termroost_core::session::shell;
    let Some(git_bash) = shell::detect_shells().into_iter().find(|o| o.name == "Git Bash") else {
        eprintln!("Git Bash is not installed; skipping");
        return;
    };
    let bash = shell::split_command_line(&git_bash.command).unwrap().program;
    // Before arguments were quoted, bash got `-c echo` and printed an empty line.
    let command = shell::join_command_line(&[bash.as_str(), "-c", "echo spaced  args-ok; exec cat"]);
    let dir = tempfile::tempdir().unwrap();
    let mut core = open(dir.path());
    core.dispatch(Command::SetDefaultShell(Some(command))).unwrap();
    core.dispatch(Command::CreateProject { name: "p".into(), working_dir: dir.path().into(), shell: None }).unwrap();
    let pane = only_pane(&core);
    wait_for_text(&mut core, pane, "spaced args-ok");
}
