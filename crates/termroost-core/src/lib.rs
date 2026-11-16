//! GUI-agnostic core of TerminalRooster.
//!
//! A front end talks to [`Core`] only:
//!
//! * [`Core::state`] — read model (projects, tabs, panes, tasks) to render lists.
//! * [`Core::dispatch`] — every change, as a [`Command`].
//! * [`Core::terminal`] — a [`TerminalHandle`] per visible pane, for drawing
//!   ([`TerminalHandle::with_content`]) and input.
//! * [`Core::events`] — asynchronous [`Event`]s; pass each one to
//!   [`Core::apply_event`] before handling it.
//!
//! All state is persisted to SQLite as it changes, so the app comes back as it
//! was after a crash or restart. No GUI, terminal-emulator or database types
//! appear in this API.

pub mod agent;
pub mod alert;
pub mod codex_hook;
pub mod command;
pub mod detect;
pub mod error;
pub mod layout;
pub mod model;
pub mod paths;
pub mod session;
pub mod store;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

pub use agent::{AgentKind, AgentSession};
pub use alert::{AgentEvent, Alert};
pub use codex_hook::CodexHookStatus;
pub use command::{Command, Event, Outcome};
pub use detect::{AgentHomes, ObservedAgent};
pub use error::{Error, Result};
pub use layout::{Axis, Layout};
pub use model::*;
pub use session::input::{Key, KeyInput, Modifiers, MouseButton, MouseEventKind, MouseInput, NamedKey};
pub use session::render::{CellStyle, CursorInfo, CursorShape, Palette, RenderCell, RenderContent, Rgb};
pub use session::{GridSize, SelectionKind, TerminalHandle};

use paths::InstanceLock;
use session::{Background, SpawnSpec};
use store::{Store, StoreOp};

/// Options for [`Core::open`].
#[derive(Clone, Debug)]
pub struct Config {
    /// Where the database lives; `None` uses [`paths::default_data_dir`].
    pub data_dir: Option<PathBuf>,
    pub palette: Palette,
    /// How often changed scrollback is saved and terminals are checked for agents.
    pub autosave_interval: Duration,
    /// Where Claude Code, Codex and Grok keep their session records.
    pub agent_homes: AgentHomes,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: None,
            palette: Palette::default(),
            autosave_interval: Duration::from_secs(5),
            agent_homes: AgentHomes::from_env(),
        }
    }
}

/// What [`Core::open`] carried over from TermRoost, the app's previous name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Renamed {
    /// TermRoost's data directory, whose contents were moved into the data directory.
    pub moved_from: Option<PathBuf>,
    /// TermRoost's Codex hook was rewritten, so Codex needs it trusted again.
    pub codex_hook_updated: bool,
}

pub struct Core {
    state: AppState,
    store: Store,
    bg: Background,
    events_rx: async_channel::Receiver<Event>,
    palette: session::render::SharedPalette,
    next_id: i64,
    /// Panes loaded from the database whose terminal has not been started yet.
    restored: HashSet<PaneId>,
    /// Temporary terminals outside any project (negative ids), never saved.
    scratch: HashMap<PaneId, TerminalHandle>,
    next_scratch: i64,
    autosave: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
    data_dir: PathBuf,
    homes: AgentHomes,
    renamed: Renamed,
    /// Where the shell integration scripts are, if they could be written.
    integration: Option<PathBuf>,
    _lock: InstanceLock,
}

impl Core {
    /// Opens the database (creating it if needed), restores the previous state
    /// and starts the terminals of the visible tab. With the default data
    /// directory, TermRoost's data is moved over first.
    pub fn open(config: Config) -> Result<Core> {
        let mut renamed = Renamed::default();
        let data_dir = match config.data_dir {
            Some(d) => d,
            None => {
                let dir = paths::default_data_dir()?;
                if std::env::var_os(paths::DATA_DIR_ENV).is_none()
                    && let Some(legacy) = paths::legacy_data_dir()
                    && paths::migrate_legacy(&legacy, &dir)?
                {
                    renamed.moved_from = Some(legacy);
                }
                dir
            }
        };
        std::fs::create_dir_all(&data_dir)?;
        let lock = InstanceLock::acquire(&data_dir)?;
        // Checked on every start, so a failed rewrite is retried next time.
        match codex_hook::migrate(&config.agent_homes.codex, &data_dir) {
            Ok(updated) => renamed.codex_hook_updated = updated,
            Err(e) => tracing::warn!(error = %e, "could not update TermRoost's Codex hook"),
        }
        let (store, mut state) = Store::open(&data_dir.join(paths::DB_FILE))?;
        let integration = session::integration::install(&data_dir)
            .map_err(|e| tracing::warn!(error = %e, "could not write the shell integration scripts"))
            .ok();

        let mut next_id = max_id(&state) + 1;
        let repairs = repair(&mut state, &mut next_id);
        if !repairs.is_empty() {
            tracing::warn!("repaired {} inconsistencies in saved state", repairs.len());
            store.write(repairs);
        }

        let (events_tx, events_rx) = async_channel::unbounded();
        let bg = Background {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            detector: Arc::new(Mutex::new(detect::Detector::new(config.agent_homes.clone()))),
            data_dir: data_dir.clone(),
            started: SystemTime::now(),
            last_codex_report: Arc::new(Mutex::new(None)),
            store: store.writer(),
            events: events_tx,
            saved_lines: Arc::new(AtomicUsize::new(state.settings.saved_scrollback_lines)),
        };
        codex_hook::set_running(&data_dir, true);
        let (stop_tx, stop_rx) = mpsc::channel();
        let autosave = bg.clone().spawn(stop_rx, config.autosave_interval);

        let restored = state.panes.keys().copied().collect();
        let mut core = Core {
            state,
            store,
            bg,
            events_rx,
            palette: Arc::new(std::sync::RwLock::new(config.palette)),
            next_id,
            restored,
            scratch: HashMap::new(),
            next_scratch: 1,
            autosave: Some((stop_tx, autosave)),
            data_dir,
            homes: config.agent_homes,
            renamed,
            integration,
            _lock: lock,
        };
        tracing::info!(
            data_dir = %core.data_dir.display(),
            projects = core.state.projects.len(),
            tabs = core.state.tabs.len(),
            terminals = core.state.panes.len(),
            tasks = core.state.tasks.len(),
            "core opened"
        );
        if let Some(tab) = core.state.active_tab().map(|t| t.id) {
            core.activate_tab(tab);
        }
        Ok(core)
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// What this start carried over from TermRoost (usually nothing).
    pub fn renamed(&self) -> &Renamed {
        &self.renamed
    }

    /// Receiver of asynchronous events (clone it freely).
    pub fn events(&self) -> async_channel::Receiver<Event> {
        self.events_rx.clone()
    }

    /// Replaces the terminal color palette (e.g. on a theme change). All
    /// terminals pick it up on their next redraw, and color queries from
    /// programs (OSC 10/11) answer with the new colors.
    pub fn set_palette(&self, palette: Palette) {
        tracing::debug!("terminal palette replaced");
        *self.palette.write().unwrap() = palette;
        for h in self.bg.sessions.lock().unwrap().values() {
            h.invalidate();
        }
    }

    /// Whether Codex is installed and TerminalRooster's Codex hook is set up.
    pub fn codex_hook_status(&self) -> CodexHookStatus {
        codex_hook::status(&self.homes.codex)
    }

    /// When the Codex hook last reported (since TerminalRooster started).
    pub fn codex_hook_last_report(&self) -> Option<SystemTime> {
        *self.bg.last_codex_report.lock().unwrap()
    }

    /// Adds TerminalRooster's hook to Codex (`~/.codex/hooks.json`) so Codex
    /// sessions are tracked exactly. Codex asks the user to trust it once
    /// (`/hooks`). Returns the file that was changed.
    pub fn install_codex_hook(&self) -> Result<PathBuf> {
        Ok(codex_hook::install(&self.homes.codex, &self.data_dir)?)
    }

    /// Removes TerminalRooster's Codex hook, leaving any other hooks in place.
    pub fn remove_codex_hook(&self) -> Result<()> {
        Ok(codex_hook::remove(&self.homes.codex)?)
    }

    /// The terminal of a pane, if it has been started in this run.
    pub fn terminal(&self, pane: PaneId) -> Option<TerminalHandle> {
        self.bg.sessions.lock().unwrap().get(&pane).cloned()
    }

    /// Records the effects of an event on the state. Call for every event.
    pub fn apply_event(&mut self, event: &Event) {
        if let Event::AgentObserved { pane, agent } = event {
            self.record_agent(*pane, agent.as_ref());
        }
    }

    /// Updates a pane's agent from what the detector saw running in it.
    fn record_agent(&mut self, pane: PaneId, observed: Option<&ObservedAgent>) {
        let Some(p) = self.state.panes.get_mut(&pane) else { return };
        let new = observed.map(|obs| {
            let same_kind = p.agent.as_ref().filter(|a| a.kind == obs.kind);
            AgentSession {
                kind: obs.kind,
                // A Codex without a thread yet keeps the session it was resumed into.
                session_id: obs.session_id.clone().or_else(|| same_kind.and_then(|a| a.session_id.clone())),
                args: same_kind.map(|a| a.args.clone()).unwrap_or_default(),
            }
        });
        if p.agent != new {
            p.agent = new;
            self.store.write(vec![StoreOp::UpsertPane(p.clone())]);
        }
        if let Some(h) = self.terminal(pane) {
            h.set_agent(observed.map(|o| o.kind));
        }
    }

    /// The programs running in each terminal (below its shell), by name, for
    /// asking before quitting. Terminals with nothing running are left out.
    /// Inspects every process, so it can take a moment.
    pub fn running_programs(&self) -> Vec<(PaneId, Vec<String>)> {
        let shells: Vec<(PaneId, u32)> =
            self.bg.sessions.lock().unwrap().values().filter_map(|h| Some((h.pane(), h.shell_pid()?))).collect();
        let mut found: Vec<(PaneId, Vec<String>)> =
            self.bg.detector.lock().unwrap().programs(&shells).into_iter().filter(|(_, n)| !n.is_empty()).collect();
        found.sort_by_key(|(p, _)| *p);
        found
    }

    /// Blocks until all queued database writes are committed.
    pub fn flush(&self) {
        self.store.flush();
    }

    /// Saves the scrollback of every running terminal now and waits for it.
    pub fn save_scrollback_now(&self) {
        self.bg.save_dirty(true);
        self.store.flush();
    }

    /// Applies a command. Changes are persisted before the next command runs.
    pub fn dispatch(&mut self, cmd: Command) -> Result<Outcome> {
        tracing::debug!(?cmd, "dispatch");
        let mut ops = Vec::new();
        let result = self.apply(cmd, &mut ops);
        if let Err(e) = &result {
            tracing::debug!(error = %e, "command rejected");
        }
        self.store.write(ops);
        result
    }

    fn alloc(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn apply(&mut self, cmd: Command, ops: &mut Vec<StoreOp>) -> Result<Outcome> {
        use Command as C;
        match cmd {
            C::CreateProject { name, working_dir, shell } => {
                let name = non_empty(&name, "project name")?;
                let id = ProjectId(self.alloc());
                let sort_order = self.state.projects.iter().map(|p| p.sort_order + 1).max().unwrap_or(0);
                let project = Project {
                    id,
                    name,
                    working_dir,
                    shell: shell.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
                    sort_order,
                    active_tab: None,
                    created_at: model::now(),
                };
                ops.push(StoreOp::UpsertProject(project.clone()));
                self.state.projects.push(project);
                self.state.sort();
                self.create_tab(id, None, ops)?;
                Ok(Outcome::Project(id))
            }
            C::RenameProject { project, name } => {
                let name = non_empty(&name, "project name")?;
                let p = self.state.project_mut(project).ok_or(Error::NotFound("project"))?;
                p.name = name;
                ops.push(StoreOp::UpsertProject(p.clone()));
                Ok(Outcome::None)
            }
            C::SetProjectDir { project, working_dir } => {
                let p = self.state.project_mut(project).ok_or(Error::NotFound("project"))?;
                p.working_dir = working_dir;
                ops.push(StoreOp::UpsertProject(p.clone()));
                Ok(Outcome::None)
            }
            C::SetProjectShell { project, shell } => {
                let p = self.state.project_mut(project).ok_or(Error::NotFound("project"))?;
                p.shell = shell.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
                ops.push(StoreOp::UpsertProject(p.clone()));
                Ok(Outcome::None)
            }
            C::DeleteProject(project) => {
                let index =
                    self.state.projects.iter().position(|p| p.id == project).ok_or(Error::NotFound("project"))?;
                let tabs: Vec<TabId> = self.state.tabs_of(project).map(|t| t.id).collect();
                for tab in &tabs {
                    self.stop_tab_sessions(*tab);
                }
                self.state.panes.retain(|_, p| !tabs.contains(&p.tab));
                self.state.tabs.retain(|t| t.project != project);
                self.state.tasks.retain(|t| t.project != project);
                self.state.projects.remove(index);
                ops.push(StoreOp::DeleteProject(project));
                if self.state.selected_project == Some(project) {
                    let next = self
                        .state
                        .projects
                        .get(index)
                        .or_else(|| index.checked_sub(1).and_then(|i| self.state.projects.get(i)))
                        .map(|p| p.id);
                    self.select_project(next, ops);
                }
                Ok(Outcome::None)
            }
            C::MoveProject { project, to_index } => {
                let mut ids: Vec<ProjectId> = self.state.projects.iter().map(|p| p.id).collect();
                reorder(&mut ids, project, to_index).ok_or(Error::NotFound("project"))?;
                for (i, id) in ids.iter().enumerate() {
                    let p = self.state.project_mut(*id).expect("exists");
                    p.sort_order = i as i64;
                    ops.push(StoreOp::UpsertProject(p.clone()));
                }
                self.state.sort();
                Ok(Outcome::None)
            }
            C::SelectProject(project) => {
                self.state.project(project).ok_or(Error::NotFound("project"))?;
                self.select_project(Some(project), ops);
                Ok(Outcome::None)
            }

            C::CreateTab { project, title } => {
                self.state.project(project).ok_or(Error::NotFound("project"))?;
                let tab = self.create_tab(project, title, ops)?;
                Ok(Outcome::Tab(tab))
            }
            C::RenameTab { tab, title } => {
                let title = non_empty(&title, "session name")?;
                let t = self.state.tab_mut(tab).ok_or(Error::NotFound("session"))?;
                t.title = title;
                ops.push(StoreOp::UpsertTab(t.clone()));
                Ok(Outcome::None)
            }
            C::SetTabDir { tab, working_dir } => {
                let t = self.state.tab_mut(tab).ok_or(Error::NotFound("session"))?;
                t.working_dir = working_dir.filter(|d| !d.as_os_str().is_empty());
                ops.push(StoreOp::UpsertTab(t.clone()));
                Ok(Outcome::None)
            }
            C::DeleteTab(tab) => {
                let project = self.state.tab(tab).ok_or(Error::NotFound("session"))?.project;
                let siblings: Vec<TabId> = self.state.tabs_of(project).map(|t| t.id).collect();
                let index = siblings.iter().position(|t| *t == tab).expect("tab in project");
                self.stop_tab_sessions(tab);
                self.state.panes.retain(|_, p| p.tab != tab);
                for task in self.state.tasks.iter_mut().filter(|t| t.tab == Some(tab)) {
                    task.tab = None; // the database does this via ON DELETE SET NULL
                }
                self.state.tabs.retain(|t| t.id != tab);
                ops.push(StoreOp::DeleteTab(tab));

                let p = self.state.project_mut(project).expect("project exists");
                if p.active_tab == Some(tab) {
                    let next =
                        siblings.get(index + 1).or_else(|| index.checked_sub(1).and_then(|i| siblings.get(i))).copied();
                    p.active_tab = next;
                    ops.push(StoreOp::UpsertProject(p.clone()));
                    if self.state.selected_project == Some(project)
                        && let Some(next) = next
                    {
                        self.activate_tab(next);
                    }
                }
                Ok(Outcome::None)
            }
            C::MoveTab { tab, to_index } => {
                let project = self.state.tab(tab).ok_or(Error::NotFound("session"))?.project;
                let mut ids: Vec<TabId> = self.state.tabs_of(project).map(|t| t.id).collect();
                reorder(&mut ids, tab, to_index).ok_or(Error::NotFound("session"))?;
                for (i, id) in ids.iter().enumerate() {
                    let t = self.state.tab_mut(*id).expect("exists");
                    t.sort_order = i as i64;
                    ops.push(StoreOp::UpsertTab(t.clone()));
                }
                self.state.sort();
                Ok(Outcome::None)
            }
            C::SelectTab(tab) => {
                let project = self.state.tab(tab).ok_or(Error::NotFound("session"))?.project;
                let p = self.state.project_mut(project).expect("project exists");
                if p.active_tab != Some(tab) {
                    p.active_tab = Some(tab);
                    ops.push(StoreOp::UpsertProject(p.clone()));
                }
                if self.state.selected_project != Some(project) {
                    self.state.selected_project = Some(project);
                    ops.push(StoreOp::SelectedProject(Some(project)));
                }
                self.activate_tab(tab);
                Ok(Outcome::None)
            }

            C::SplitPane { pane, axis } => {
                let tab_id = self.state.pane(pane).ok_or(Error::NotFound("pane"))?.tab;
                let new = PaneId(self.alloc());
                let tab = self.state.tab_mut(tab_id).ok_or(Error::NotFound("session"))?;
                if !tab.layout.split(pane, axis, new) {
                    return Err(Error::Invalid("pane is not in its tab layout".into()));
                }
                tab.focused_pane = Some(new);
                ops.push(StoreOp::UpsertTab(tab.clone()));
                let record = Pane { id: new, tab: tab_id, resume_command: None, agent: None, created_at: model::now() };
                ops.push(StoreOp::UpsertPane(record.clone()));
                self.state.panes.insert(new, record);

                let size = self.terminal(pane).map(|h| h.size()).unwrap_or_default();
                let size = match axis {
                    Axis::Vertical => GridSize { cols: (size.cols / 2).max(2), ..size },
                    Axis::Horizontal => GridSize { rows: (size.rows / 2).max(1), ..size },
                };
                self.ensure_session(new, size);
                Ok(Outcome::Pane(new))
            }
            C::ClosePane(pane) => {
                let tab_id = self.state.pane(pane).ok_or(Error::NotFound("pane"))?.tab;
                let tab = self.state.tab_mut(tab_id).ok_or(Error::NotFound("session"))?;
                if !tab.layout.remove(pane) {
                    return Err(Error::Invalid(
                        "the last terminal of a session cannot be closed; delete the session".into(),
                    ));
                }
                if tab.focused_pane == Some(pane) || !tab.focused_pane.is_some_and(|f| tab.layout.contains(f)) {
                    tab.focused_pane = Some(tab.layout.first_pane());
                }
                ops.push(StoreOp::UpsertTab(tab.clone()));
                ops.push(StoreOp::DeletePane(pane));
                self.state.panes.remove(&pane);
                self.stop_session(pane);
                Ok(Outcome::None)
            }
            C::FocusPane(pane) => {
                let tab_id = self.state.pane(pane).ok_or(Error::NotFound("pane"))?.tab;
                let tab = self.state.tab_mut(tab_id).ok_or(Error::NotFound("session"))?;
                if tab.focused_pane != Some(pane) {
                    tab.focused_pane = Some(pane);
                    ops.push(StoreOp::UpsertTab(tab.clone()));
                }
                Ok(Outcome::None)
            }
            C::SetLayout { tab, mut layout } => {
                let t = self.state.tab_mut(tab).ok_or(Error::NotFound("session"))?;
                let mut a = t.layout.panes();
                let mut b = layout.panes();
                a.sort();
                b.sort();
                if a != b {
                    return Err(Error::Invalid("layout must contain exactly the tab's panes".into()));
                }
                layout.sanitize();
                if t.layout != layout {
                    t.layout = layout;
                    ops.push(StoreOp::UpsertTab(t.clone()));
                }
                Ok(Outcome::None)
            }
            C::SetResumeCommand { pane, command } => {
                let p = self.state.panes.get_mut(&pane).ok_or(Error::NotFound("pane"))?;
                p.resume_command = command.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty());
                ops.push(StoreOp::UpsertPane(p.clone()));
                Ok(Outcome::None)
            }
            C::StartAgent { pane, kind, args } => {
                self.state.pane(pane).ok_or(Error::NotFound("pane"))?;
                let handle = self.ensure_session(pane, GridSize::default());
                let fresh = !handle.is_running();
                if fresh {
                    self.start_process(pane, &handle, false);
                }
                let session = AgentSession::new(kind, args);
                tracing::info!(%pane, agent = kind.as_str(), session = ?session.session_id, "starting agent");
                let line = session.launch_command();
                if fresh {
                    type_later(&handle, line)
                } else {
                    handle.write(format!("{line}\r"))
                }
                handle.set_agent(Some(kind));
                let p = self.state.panes.get_mut(&pane).expect("checked");
                p.agent = Some(session);
                ops.push(StoreOp::UpsertPane(p.clone()));
                Ok(Outcome::None)
            }
            C::ClearAgent(pane) => {
                let p = self.state.panes.get_mut(&pane).ok_or(Error::NotFound("pane"))?;
                p.agent = None;
                ops.push(StoreOp::UpsertPane(p.clone()));
                if let Some(h) = self.terminal(pane) {
                    h.set_agent(None);
                }
                Ok(Outcome::None)
            }
            C::RestartPane(pane) => {
                self.state.pane(pane).ok_or(Error::NotFound("pane"))?;
                match self.terminal(pane) {
                    Some(handle) => {
                        handle.print_notice("─── restarted ───");
                        self.start_process(pane, &handle, true);
                    }
                    None => {
                        self.ensure_session(pane, GridSize::default());
                    }
                }
                Ok(Outcome::None)
            }

            C::CreateTask { project, title, tab } => {
                let title = non_empty(&title, "task title")?;
                self.state.project(project).ok_or(Error::NotFound("project"))?;
                self.check_tab_in_project(tab, project)?;
                let id = TaskId(self.alloc());
                let sort_order = self.state.tasks_of(project).map(|t| t.sort_order + 1).max().unwrap_or(0);
                let task = Task {
                    id,
                    project,
                    tab,
                    title,
                    notes: String::new(),
                    done: false,
                    sort_order,
                    created_at: model::now(),
                    completed_at: None,
                };
                ops.push(StoreOp::UpsertTask(task.clone()));
                self.state.tasks.push(task);
                self.state.sort();
                Ok(Outcome::Task(id))
            }
            C::EditTask { task, title, notes } => {
                let title = non_empty(&title, "task title")?;
                let t = self.state.task_mut(task).ok_or(Error::NotFound("task"))?;
                t.title = title;
                t.notes = notes;
                ops.push(StoreOp::UpsertTask(t.clone()));
                Ok(Outcome::None)
            }
            C::SetTaskDone { task, done } => {
                let t = self.state.task_mut(task).ok_or(Error::NotFound("task"))?;
                t.done = done;
                t.completed_at = done.then(model::now);
                ops.push(StoreOp::UpsertTask(t.clone()));
                Ok(Outcome::None)
            }
            C::AssignTask { task, tab } => {
                let project = self.state.task(task).ok_or(Error::NotFound("task"))?.project;
                self.check_tab_in_project(tab, project)?;
                let t = self.state.task_mut(task).expect("checked");
                t.tab = tab;
                ops.push(StoreOp::UpsertTask(t.clone()));
                Ok(Outcome::None)
            }
            C::MoveTask { task, to_index } => {
                let project = self.state.task(task).ok_or(Error::NotFound("task"))?.project;
                let mut ids: Vec<TaskId> = self.state.tasks_of(project).map(|t| t.id).collect();
                reorder(&mut ids, task, to_index).ok_or(Error::NotFound("task"))?;
                for (i, id) in ids.iter().enumerate() {
                    let t = self.state.task_mut(*id).expect("exists");
                    t.sort_order = i as i64;
                    ops.push(StoreOp::UpsertTask(t.clone()));
                }
                self.state.sort();
                Ok(Outcome::None)
            }
            C::DeleteTask(task) => {
                let before = self.state.tasks.len();
                self.state.tasks.retain(|t| t.id != task);
                if self.state.tasks.len() == before {
                    return Err(Error::NotFound("task"));
                }
                ops.push(StoreOp::DeleteTask(task));
                Ok(Outcome::None)
            }

            C::SetDefaultShell(shell) => {
                self.state.settings.default_shell = shell.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
                ops.push(StoreOp::Settings(self.state.settings.clone()));
                Ok(Outcome::None)
            }
            C::SetShellIntegration(on) => {
                if self.state.settings.shell_integration != on {
                    self.state.settings.shell_integration = on;
                    ops.push(StoreOp::Settings(self.state.settings.clone()));
                }
                Ok(Outcome::None)
            }
            C::SetGuiValue { key, value } => {
                if self.state.gui.get(&key) != Some(&value) {
                    ops.push(StoreOp::Gui(key.clone(), value.clone()));
                    self.state.gui.insert(key, value);
                }
                Ok(Outcome::None)
            }
        }
    }

    fn check_tab_in_project(&self, tab: Option<TabId>, project: ProjectId) -> Result<()> {
        match tab {
            Some(t) if self.state.tab(t).map(|t| t.project) != Some(project) => {
                Err(Error::Invalid("tab belongs to another project".into()))
            }
            _ => Ok(()),
        }
    }

    fn create_tab(&mut self, project: ProjectId, title: Option<String>, ops: &mut Vec<StoreOp>) -> Result<TabId> {
        let tab_id = TabId(self.alloc());
        let pane_id = PaneId(self.alloc());
        let count = self.state.tabs_of(project).count();
        let sort_order = self.state.tabs_of(project).map(|t| t.sort_order + 1).max().unwrap_or(0);
        let title = title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("Session {}", count + 1));
        let tab = Tab {
            id: tab_id,
            project,
            title,
            working_dir: None,
            layout: Layout::Pane(pane_id),
            focused_pane: Some(pane_id),
            sort_order,
            created_at: model::now(),
        };
        let pane = Pane { id: pane_id, tab: tab_id, resume_command: None, agent: None, created_at: model::now() };
        ops.push(StoreOp::UpsertTab(tab.clone()));
        ops.push(StoreOp::UpsertPane(pane.clone()));
        self.state.tabs.push(tab);
        self.state.panes.insert(pane_id, pane);
        self.state.sort();

        let p = self.state.project_mut(project).ok_or(Error::NotFound("project"))?;
        p.active_tab = Some(tab_id);
        ops.push(StoreOp::UpsertProject(p.clone()));
        if self.state.selected_project != Some(project) {
            self.state.selected_project = Some(project);
            ops.push(StoreOp::SelectedProject(Some(project)));
        }
        self.activate_tab(tab_id);
        Ok(tab_id)
    }

    fn select_project(&mut self, project: Option<ProjectId>, ops: &mut Vec<StoreOp>) {
        if self.state.selected_project != project {
            self.state.selected_project = project;
            ops.push(StoreOp::SelectedProject(project));
        }
        if let Some(tab) = self.state.active_tab().map(|t| t.id) {
            self.activate_tab(tab);
        }
    }

    /// Starts the terminals of a tab that are not running yet.
    fn activate_tab(&mut self, tab: TabId) {
        let Some(panes) = self.state.tab(tab).map(|t| t.layout.panes()) else { return };
        for pane in panes {
            self.ensure_session(pane, GridSize::default());
        }
    }

    /// Returns the pane's terminal, creating and starting it if needed.
    fn ensure_session(&mut self, pane: PaneId, size: GridSize) -> TerminalHandle {
        if let Some(h) = self.terminal(pane) {
            return h;
        }
        let handle = TerminalHandle::new(
            pane,
            self.bg.events.clone(),
            self.palette.clone(),
            self.state.settings.scrollback_lines,
            size,
        );
        let restored = self.restored.remove(&pane);
        if restored
            && let Some((data, saved_at)) = self.store.load_scrollback(pane)
            && !data.is_empty()
        {
            let when = chrono::DateTime::from_timestamp(saved_at, 0)
                .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            handle.replay(&data, Some(&format!("restored session from {when}")));
        }
        handle.set_agent(self.state.pane(pane).and_then(|p| p.agent.as_ref()).map(|a| a.kind));
        self.bg.sessions.lock().unwrap().insert(pane, handle.clone());
        self.start_process(pane, &handle, restored);
        handle
    }

    fn spawn_spec(&self, pane: PaneId) -> Option<SpawnSpec> {
        let tab = self.state.tab(self.state.pane(pane)?.tab)?;
        let project = self.state.project(tab.project)?;
        let mut shell = session::shell::resolve(project.shell.as_deref(), self.state.settings.default_shell.as_deref());
        let cwd = self.state.tab_working_dir(tab);
        let mut env = base_env();
        env.insert("TERMINALROOSTER_PANE_ID".to_owned(), pane.to_string());
        env.insert("TERMINALROOSTER_PROJECT".to_owned(), project.name.clone());
        if self.state.settings.shell_integration
            && let Some(dir) = &self.integration
        {
            session::integration::apply(&mut shell, &mut env, dir);
        }
        Some(SpawnSpec { shell, cwd, env })
    }

    /// Opens a terminal that belongs to no project: it is not saved, not
    /// restored and not checked for agents. `command` is typed into its shell
    /// once it has started (e.g. `codex`). Close it with [`Core::close_scratch_terminal`].
    pub fn open_scratch_terminal(&mut self, cwd: &std::path::Path, command: Option<&str>) -> TerminalHandle {
        let pane = PaneId(-self.next_scratch);
        self.next_scratch += 1;
        let handle = TerminalHandle::new(
            pane,
            self.bg.events.clone(),
            self.palette.clone(),
            self.state.settings.scrollback_lines,
            GridSize::default(),
        );
        let spec = SpawnSpec {
            shell: session::shell::resolve(None, self.state.settings.default_shell.as_deref()),
            cwd: cwd.to_path_buf(),
            env: base_env(),
        };
        match handle.spawn(&spec) {
            Ok(()) => {
                if let Some(command) = command {
                    type_later(&handle, command.to_owned());
                }
            }
            Err(e) => handle.print_notice(&format!("failed to start {}: {e}", spec.shell.program)),
        }
        self.scratch.insert(pane, handle.clone());
        handle
    }

    /// Stops a terminal opened with [`Core::open_scratch_terminal`].
    pub fn close_scratch_terminal(&mut self, pane: PaneId) {
        if let Some(h) = self.scratch.remove(&pane) {
            h.shutdown();
        }
    }

    fn start_process(&mut self, pane: PaneId, handle: &TerminalHandle, resume: bool) {
        let Some(mut spec) = self.spawn_spec(pane) else { return };
        if !spec.cwd.is_dir() {
            let fallback = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()).unwrap_or_default();
            handle.print_notice(&format!(
                "working directory {} does not exist; starting in {}",
                spec.cwd.display(),
                fallback.display()
            ));
            spec.cwd = fallback;
        }
        tracing::info!(%pane, shell = %spec.shell.program, cwd = %spec.cwd.display(), resume, "starting shell");
        if let Err(e) = handle.spawn(&spec) {
            tracing::error!(%pane, shell = %spec.shell.program, error = %e, "failed to start shell");
            handle.print_notice(&format!("failed to start {}: {e}", spec.shell.program));
            return;
        }
        if resume && let Some(line) = self.resume_line(pane) {
            tracing::debug!(%pane, command = %line, "typing resume command");
            type_later(handle, line);
        }
    }

    /// What to type into a restored terminal: the agent's exact session when it
    /// is saved on disk, a fresh start of the agent when it is not (the session
    /// never got a message), else the pane's own resume command.
    fn resume_line(&self, pane: PaneId) -> Option<String> {
        let p = self.state.pane(pane)?;
        match &p.agent {
            Some(a) => Some(match &a.session_id {
                Some(id) if self.homes.session_exists(a.kind, id) => a.resume_command()?,
                _ => AgentSession { session_id: None, ..a.clone() }.launch_command(),
            }),
            None => p.resume_command.clone(),
        }
    }

    fn stop_session(&mut self, pane: PaneId) {
        if let Some(h) = self.bg.sessions.lock().unwrap().remove(&pane) {
            h.shutdown();
        }
        self.restored.remove(&pane);
    }

    fn stop_tab_sessions(&mut self, tab: TabId) {
        let panes: Vec<PaneId> = self.state.panes.values().filter(|p| p.tab == tab).map(|p| p.id).collect();
        for pane in panes {
            self.stop_session(pane);
        }
    }
}

impl Core {
    /// Saves all scrollback, commits pending writes and stops every terminal.
    /// Idempotent; also run on drop. The core should not be used afterwards.
    pub fn shutdown(&mut self) {
        let Some((stop, thread)) = self.autosave.take() else { return };
        tracing::info!(terminals = self.bg.sessions.lock().unwrap().len(), "shutting down; saving scrollback");
        let _ = stop.send(());
        let _ = thread.join();
        // Last look at what runs where, while the agents are still alive.
        for (pane, agent) in self.bg.detect_agents() {
            self.record_agent(pane, agent.as_ref());
        }
        codex_hook::set_running(&self.data_dir, false);
        self.bg.save_dirty(true);
        self.store.flush();
        for h in self.bg.sessions.lock().unwrap().drain().map(|(_, h)| h) {
            h.shutdown();
        }
        for (_, h) in self.scratch.drain() {
            h.shutdown();
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Environment every terminal gets.
fn base_env() -> HashMap<String, String> {
    let mut env = HashMap::new();
    #[cfg(not(windows))]
    env.insert("TERM".to_owned(), "xterm-256color".to_owned());
    env.insert("COLORTERM".to_owned(), "truecolor".to_owned());
    env.insert("TERM_PROGRAM".to_owned(), "terminalrooster".to_owned());
    env
}

/// Types a line into a freshly started shell once it had a moment to start.
fn type_later(handle: &TerminalHandle, line: String) {
    let handle = handle.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        handle.write(format!("{line}\r"));
    });
}

fn non_empty(s: &str, what: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() { Err(Error::Invalid(format!("{what} cannot be empty"))) } else { Ok(s.to_owned()) }
}

fn reorder<T: PartialEq + Copy>(ids: &mut Vec<T>, id: T, to: usize) -> Option<()> {
    let from = ids.iter().position(|x| *x == id)?;
    let item = ids.remove(from);
    ids.insert(to.min(ids.len()), item);
    Some(())
}

fn max_id(s: &AppState) -> i64 {
    let p = s.projects.iter().map(|x| x.id.0);
    let t = s.tabs.iter().map(|x| x.id.0);
    let pa = s.panes.keys().map(|x| x.0);
    let ta = s.tasks.iter().map(|x| x.id.0);
    p.chain(t).chain(pa).chain(ta).max().unwrap_or(0)
}

/// Fixes saved state that does not hang together (e.g. after a crash between
/// writes or a manual database edit). Returns the writes that persist the fixes.
fn repair(state: &mut AppState, next_id: &mut i64) -> Vec<StoreOp> {
    let mut ops = Vec::new();
    let project_ids: HashSet<ProjectId> = state.projects.iter().map(|p| p.id).collect();
    state.tabs.retain(|t| project_ids.contains(&t.project));
    let tab_ids: HashSet<TabId> = state.tabs.iter().map(|t| t.id).collect();
    state.panes.retain(|_, p| tab_ids.contains(&p.tab));

    for tab in &mut state.tabs {
        let mut owned: Vec<PaneId> = state.panes.values().filter(|p| p.tab == tab.id).map(|p| p.id).collect();
        owned.sort();
        let before = tab.clone();
        let leaves = tab.layout.panes();
        let unique: HashSet<PaneId> = leaves.iter().copied().collect();
        let valid = unique.len() == leaves.len() && leaves.iter().all(|l| owned.contains(l));
        if !valid {
            if let Some(l) = Layout::chain(&owned) {
                tab.layout = l;
            }
        } else {
            for p in owned.iter().filter(|p| !unique.contains(p)) {
                let last = *tab.layout.panes().last().expect("non-empty");
                tab.layout.split(last, Axis::Vertical, *p);
            }
        }
        if owned.is_empty() {
            let id = PaneId(*next_id);
            *next_id += 1;
            let pane = Pane { id, tab: tab.id, resume_command: None, agent: None, created_at: model::now() };
            state.panes.insert(id, pane.clone());
            tab.layout = Layout::Pane(id);
            ops.push(StoreOp::UpsertTab(tab.clone()));
            ops.push(StoreOp::UpsertPane(pane));
        }
        tab.layout.sanitize();
        if !tab.focused_pane.is_some_and(|f| tab.layout.contains(f)) {
            tab.focused_pane = Some(tab.layout.first_pane());
        }
        if *tab != before {
            ops.push(StoreOp::UpsertTab(tab.clone()));
        }
    }

    for project in &mut state.projects {
        let valid = project.active_tab.is_some_and(|t| state.tabs.iter().any(|x| x.id == t && x.project == project.id));
        if !valid {
            let first = state.tabs.iter().find(|t| t.project == project.id).map(|t| t.id);
            if first != project.active_tab {
                project.active_tab = first;
                ops.push(StoreOp::UpsertProject(project.clone()));
            }
        }
    }

    for task in &mut state.tasks {
        if let Some(tab) = task.tab
            && !state.tabs.iter().any(|t| t.id == tab && t.project == task.project)
        {
            task.tab = None;
            ops.push(StoreOp::UpsertTask(task.clone()));
        }
    }

    let selected_ok = state.selected_project.is_some_and(|p| project_ids.contains(&p));
    if !selected_ok {
        let first = state.projects.first().map(|p| p.id);
        if first != state.selected_project {
            state.selected_project = first;
            ops.push(StoreOp::SelectedProject(first));
        }
    }
    ops
}
