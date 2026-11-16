//! Application state, messages and update logic.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

use iced::widget::{column, container, operation, pane_grid, row, stack};
use iced::{Element, Length, Subscription, Task, keyboard, window};
use serde::{Deserialize, Serialize};
use termroost_core::session::bundled_bash;
use termroost_core::session::shell::{self, ShellOption};
use termroost_core::{AgentKind, Axis, Command, Core, Event, Outcome, PaneId, ProjectId, TabId, TaskId};

use crate::keymap::{self, Direction, Shortcut};
use crate::layout_conv;
use crate::terminal_view;
use crate::theme;
use crate::themes::{self, ColorField, Hex, ThemeDef, ThemeStore};

const PREFS_KEY: &str = "iced.prefs";
pub const RENAME_INPUT: &str = "rename-input";
pub const TASK_INPUT: &str = "task-input";
pub const TASK_EDIT_INPUT: &str = "task-edit-input";
pub const DIALOG_FIRST_INPUT: &str = "dialog-first-input";

/// Front-end preferences, stored in the core as an opaque JSON value.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub font_size: f32,
    pub tasks_open: bool,
    pub show_done: bool,
    pub tasks_this_tab_only: bool,
    pub window: Option<WindowGeom>,
    /// Extra arguments last used per agent kind.
    pub agent_args: HashMap<String, String>,
    /// Switch between `light_theme` and `dark_theme` with the system setting.
    pub follow_system: bool,
    /// Theme used when not following the system.
    pub theme: String,
    /// Theme used in light mode when following the system (the last light theme picked).
    pub light_theme: String,
    /// Theme used in dark mode when following the system (the last dark theme picked).
    pub dark_theme: String,
    /// Terminal colors come from this theme; `None` matches the interface theme.
    pub terminal_theme: Option<String>,
    /// The user was already asked about the Codex hook at startup.
    pub codex_hook_asked: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            font_size: theme::DEFAULT_FONT_SIZE,
            tasks_open: true,
            show_done: true,
            tasks_this_tab_only: false,
            window: None,
            agent_args: HashMap::new(),
            follow_system: true,
            theme: themes::DARK.into(),
            light_theme: themes::LIGHT.into(),
            dark_theme: themes::DARK.into(),
            terminal_theme: None,
            codex_hook_asked: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct WindowGeom {
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

impl Prefs {
    pub fn load(core: &Core) -> Prefs {
        core.state().gui_value(PREFS_KEY).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameTarget {
    Project(ProjectId),
    Tab(TabId),
}

#[derive(Clone, Debug)]
pub struct Rename {
    pub target: RenameTarget,
    pub value: String,
}

#[derive(Clone, Debug)]
pub enum Dialog {
    Project {
        editing: Option<ProjectId>,
        name: String,
        dir: String,
        shell: String,
        error: Option<String>,
    },
    ConfirmDeleteProject(ProjectId),
    ConfirmDeleteTab(TabId),
    Pane {
        pane: PaneId,
        resume: String,
        args: String,
    },
    /// Offer to add the Codex hook (shown once at startup).
    CodexHook,
    /// A small Codex window for trusting the hook (`/hooks`).
    CodexTrust(termroost_core::TerminalHandle),
}

/// State of the settings page while it is open.
#[derive(Clone, Debug, Default)]
pub struct SettingsState {
    /// Custom theme being edited.
    pub editing: Option<String>,
    /// Raw text of color fields (kept while not yet a valid color).
    pub inputs: HashMap<ColorField, String>,
    pub name_input: String,
    pub name_error: Option<String>,
    pub shell_input: String,
}

/// Choice in a task's "linked tab" dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabChoice {
    pub id: Option<TabId>,
    pub label: String,
}

impl std::fmt::Display for TabChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// Installing TermRoost's own bash (Settings → Terminal, Windows only).
#[derive(Clone, Debug, Default)]
pub enum BashInstall {
    #[default]
    Idle,
    Running(bundled_bash::Progress),
    Failed(String),
}

/// News from the thread installing the bash.
#[derive(Clone, Debug)]
pub enum BashInstallEvent {
    Progress(bundled_bash::Progress),
    Done(Result<PathBuf, String>),
}

#[derive(Clone, Debug)]
pub enum Message {
    Core(Event),
    Tick,
    WindowMoved(iced::Point),
    WindowResized(iced::Size),
    CloseRequested,
    Key(keyboard::Event),
    /// Escape, even when a text input captured it (closes dialogs and editors).
    Escape,
    /// Tab/Shift+Tab, even when captured (moves between dialog fields).
    TabKey {
        shift: bool,
    },
    /// Enter, even when captured (confirms dialogs without text fields).
    Enter,
    Shortcut(Shortcut),

    SelectProject(ProjectId),
    SelectTab(TabId),
    NewTab,

    NewProject,
    EditProject(ProjectId),

    OpenSettings,
    CloseSettings,
    AddCodexHook,
    RemoveCodexHook,
    /// Open the Codex window to trust the hook.
    TrustCodexHook,
    Noop,
    SystemTheme(iced::theme::Mode),
    FollowSystem(bool),
    /// Use a theme for the interface.
    UseTheme(String),
    /// Take terminal colors from a theme (`None`: match the interface).
    TerminalTheme(Option<String>),
    EditTheme(String),
    DuplicateTheme(String),
    DeleteTheme(String),
    ThemeNameInput(String),
    ThemeColor(ColorField, String),
    FontSize(f32),
    ShellInput(String),
    ShellCommit,

    PaneSettings(PaneId),
    AskDeleteProject(ProjectId),
    AskDeleteTab(TabId),
    DialogName(String),
    DialogDir(String),
    DialogShell(String),
    DialogResume(String),
    DialogArgs(String),
    BrowseDir,
    DirPicked(Option<PathBuf>),
    DialogSubmit,
    DialogCancel,
    DialogStartAgent(AgentKind),
    DialogClearAgent,

    StartRename(RenameTarget),
    RenameInput(String),
    RenameCommit,
    RenameCancel,

    FocusPane(PaneId),
    PaneClicked(pane_grid::Pane),
    PaneDragged(pane_grid::DragEvent),
    PaneResized(pane_grid::ResizeEvent),
    Split(PaneId, Axis),
    ClosePane(PaneId),
    RestartPane(PaneId),
    StartAgent(PaneId, AgentKind),

    ToggleTasks,
    NewTaskInput(String),
    AddTask,
    TaskDone(TaskId, bool),
    TaskTab(TaskId, TabChoice),
    DeleteTask(TaskId),
    EditTask(TaskId),
    TaskEditInput(String),
    TaskEditCommit,
    TaskEditCancel,
    ShowDone(bool),
    ThisTabOnly(bool),

    /// Shells found on this machine (detected in the background).
    ShellsDetected(Vec<ShellOption>),
    /// A shell picked from the list in settings becomes the default.
    PickDefaultShell(ShellOption),
    InstallBash,
    BashInstall(BashInstallEvent),

    DismissError,
}

pub struct App {
    pub(crate) core: Core,
    pub(crate) grids: HashMap<TabId, pane_grid::State<PaneId>>,
    pub(crate) prefs: Prefs,
    prefs_dirty: bool,
    layout_dirty: HashSet<TabId>,
    pub(crate) rename: Option<Rename>,
    pub(crate) dialog: Option<Dialog>,
    pub(crate) new_task: String,
    pub(crate) task_edit: Option<(TaskId, String)>,
    /// Tabs with output the user has not looked at yet.
    pub(crate) activity: HashSet<TabId>,
    pub(crate) error: Option<String>,
    pub(crate) themes: ThemeStore,
    themes_dirty: bool,
    pub(crate) settings: Option<SettingsState>,
    /// Shells installed on this machine, for the shell pickers.
    pub(crate) shells: Vec<ShellOption>,
    pub(crate) bash_install: BashInstall,
    pub(crate) system_mode: iced::theme::Mode,
    ui_theme: iced::Theme,
    /// Cached Codex integration state (reads files, so refreshed on demand).
    pub(crate) codex_hook: termroost_core::CodexHookStatus,
    closing: bool,
}

/// Hashable wrapper so the event receiver can back a subscription.
struct Events(async_channel::Receiver<Event>);

impl Hash for Events {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "termroost-core-events".hash(state);
    }
}

impl App {
    pub fn new(core: Core) -> (App, Task<Message>) {
        let prefs = Prefs::load(&core);
        let themes = ThemeStore::load(&core);
        let mut app = App {
            core,
            grids: HashMap::new(),
            prefs,
            prefs_dirty: false,
            layout_dirty: HashSet::new(),
            rename: None,
            dialog: None,
            new_task: String::new(),
            task_edit: None,
            activity: HashSet::new(),
            error: None,
            themes,
            themes_dirty: false,
            settings: None,
            shells: Vec::new(),
            bash_install: BashInstall::Idle,
            system_mode: iced::theme::Mode::None,
            ui_theme: ThemeDef::dark().iced_theme(),
            codex_hook: termroost_core::CodexHookStatus::default(),
            closing: false,
        };
        app.apply_theme();
        app.ensure_active_grid();
        app.codex_hook = app.core.codex_hook_status();
        if app.codex_hook.codex_found && app.codex_hook.installed_in.is_none() && !app.prefs.codex_hook_asked {
            app.dialog = Some(Dialog::CodexHook);
        }
        let task = Task::batch([app.focus_active(), iced::system::theme().map(Message::SystemTheme), detect_shells()]);
        (app, task)
    }

    pub fn theme(&self) -> iced::Theme {
        self.ui_theme.clone()
    }

    pub(crate) fn system_is_dark(&self) -> bool {
        self.system_mode != iced::theme::Mode::Light
    }

    /// The interface theme currently applied.
    pub(crate) fn active_theme(&self) -> ThemeDef {
        let name = if !self.prefs.follow_system {
            &self.prefs.theme
        } else if self.system_is_dark() {
            &self.prefs.dark_theme
        } else {
            &self.prefs.light_theme
        };
        self.themes.find(name).cloned().unwrap_or_else(|| {
            if self.prefs.follow_system && !self.system_is_dark() { ThemeDef::light() } else { ThemeDef::dark() }
        })
    }

    /// The theme the terminal colors come from.
    pub(crate) fn terminal_theme(&self) -> ThemeDef {
        self.prefs
            .terminal_theme
            .as_deref()
            .and_then(|n| self.themes.find(n))
            .cloned()
            .unwrap_or_else(|| self.active_theme())
    }

    /// Applies the active theme to the interface and to every terminal.
    fn apply_theme(&mut self) {
        let def = self.active_theme();
        let term = self.terminal_theme();
        tracing::debug!(theme = %def.name, terminal = %term.name, "applying theme");
        self.ui_theme = def.iced_theme();
        self.core.set_palette(term.palette());
    }

    pub fn title(&self) -> String {
        let s = self.core.state();
        match (s.selected_project.and_then(|p| s.project(p)), s.active_tab()) {
            (Some(p), Some(t)) => format!("{} · {} — TermRoost", t.title, p.name),
            (Some(p), None) => format!("{} — TermRoost", p.name),
            _ => "TermRoost".to_owned(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run_with(Events(self.core.events()), |e| e.0.clone()).map(Message::Core),
            keyboard::listen().map(Message::Key),
            iced::system::theme_changes().map(Message::SystemTheme),
            iced::event::listen_with(|event, _status, _id| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::Escape),
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Tab),
                    modifiers,
                    ..
                }) if !modifiers.control() && !modifiers.alt() => Some(Message::TabKey { shift: modifiers.shift() }),
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Enter),
                    ..
                }) => Some(Message::Enter),
                iced::Event::Window(window::Event::Moved(p)) => Some(Message::WindowMoved(p)),
                iced::Event::Window(window::Event::Resized(s)) => Some(Message::WindowResized(s)),
                iced::Event::Window(window::Event::CloseRequested) => Some(Message::CloseRequested),
                _ => None,
            }),
        ];
        if !self.layout_dirty.is_empty() || self.prefs_dirty || self.themes_dirty {
            subs.push(iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick));
        }
        Subscription::batch(subs)
    }

    fn dispatch(&mut self, cmd: Command) -> Option<Outcome> {
        match self.core.dispatch(cmd) {
            Ok(o) => Some(o),
            Err(e) => {
                tracing::warn!(error = %e, "command failed");
                self.error = Some(e.to_string());
                None
            }
        }
    }

    pub(crate) fn active_tab_id(&self) -> Option<TabId> {
        self.core.state().active_tab().map(|t| t.id)
    }

    fn ensure_active_grid(&mut self) {
        if let Some(tab) = self.core.state().active_tab() {
            self.grids.entry(tab.id).or_insert_with(|| layout_conv::grid_state(&tab.layout));
        }
    }

    fn rebuild_grid(&mut self, tab: TabId) {
        if let Some(t) = self.core.state().tab(tab) {
            self.grids.insert(tab, layout_conv::grid_state(&t.layout));
        }
    }

    /// Moves keyboard focus to the focused terminal of the visible tab.
    fn focus_active(&self) -> Task<Message> {
        if self.dialog.is_some() || self.rename.is_some() || self.task_edit.is_some() {
            return Task::none();
        }
        match self.core.state().active_tab().and_then(|t| t.focused_pane) {
            Some(p) => operation::focus(terminal_view::widget_id(p)),
            None => Task::none(),
        }
    }

    fn mark_prefs(&mut self) {
        self.prefs_dirty = true;
    }

    fn flush_pending(&mut self) {
        for tab in std::mem::take(&mut self.layout_dirty) {
            if let Some(layout) = self.grids.get(&tab).and_then(layout_conv::layout_of) {
                self.dispatch(Command::SetLayout { tab, layout });
            }
        }
        if self.prefs_dirty {
            self.prefs_dirty = false;
            let value = serde_json::to_string(&self.prefs).expect("prefs serialize");
            self.dispatch(Command::SetGuiValue { key: PREFS_KEY.into(), value });
        }
        if self.themes_dirty {
            self.themes_dirty = false;
            let value = self.themes.to_json();
            self.dispatch(Command::SetGuiValue { key: themes::THEMES_KEY.into(), value });
        }
    }

    fn after_select(&mut self) -> Task<Message> {
        if let Some(tab) = self.active_tab_id() {
            self.activity.remove(&tab);
        }
        self.ensure_active_grid();
        self.focus_active()
    }

    fn grid_pane(&self, pane: pane_grid::Pane) -> Option<(TabId, PaneId)> {
        let tab = self.active_tab_id()?;
        let id = *self.grids.get(&tab)?.get(pane)?;
        Some((tab, id))
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        if self.closing {
            return Task::none();
        }
        // Key events are never logged: they can carry what the user types.
        if !matches!(message, Message::Core(_) | Message::Tick | Message::Key(_)) {
            tracing::trace!(?message, "update");
        }
        match message {
            Message::Core(ev) => {
                self.core.apply_event(&ev);
                match ev {
                    Event::TerminalDirty(pane) | Event::Bell(pane) => {
                        let tab = self.core.state().pane(pane).map(|p| p.tab);
                        if let Some(tab) = tab.filter(|t| Some(*t) != self.active_tab_id()) {
                            self.activity.insert(tab);
                        }
                    }
                    Event::ClipboardStore(text) => return iced::clipboard::write(text),
                    _ => {}
                }
            }
            Message::Tick => self.flush_pending(),
            Message::WindowMoved(p) => {
                let w = self.prefs.window.get_or_insert_with(Default::default);
                w.x = Some(p.x);
                w.y = Some(p.y);
                self.mark_prefs();
            }
            Message::WindowResized(s) => {
                let w = self.prefs.window.get_or_insert_with(Default::default);
                w.width = s.width;
                w.height = s.height;
                self.mark_prefs();
            }
            Message::CloseRequested => {
                self.flush_pending();
                self.core.shutdown();
                self.closing = true;
                return iced::exit();
            }
            Message::Escape => {
                if matches!(self.dialog, Some(Dialog::CodexTrust(_))) {
                    return Task::none();
                }
                if self.dialog.is_some() {
                    return self.update(Message::DialogCancel);
                }
                if self.settings.is_some() {
                    return self.update(Message::CloseSettings);
                }
                if self.rename.is_some() {
                    return self.update(Message::RenameCancel);
                }
                if self.task_edit.is_some() {
                    return self.update(Message::TaskEditCancel);
                }
            }
            Message::Enter => {
                // Dialogs with text fields submit through the fields themselves.
                if matches!(self.dialog, Some(Dialog::ConfirmDeleteProject(_) | Dialog::ConfirmDeleteTab(_))) {
                    return self.submit_dialog();
                }
            }
            Message::TabKey { shift } => {
                if self.dialog.is_some() && !matches!(self.dialog, Some(Dialog::CodexTrust(_))) {
                    return if shift { operation::focus_previous() } else { operation::focus_next() };
                }
            }
            Message::Key(keyboard::Event::KeyPressed { key, physical_key, modifiers, .. }) => {
                if let Some(sc) = keymap::shortcut(&key, physical_key, modifiers) {
                    return self.update(Message::Shortcut(sc));
                }
            }
            Message::Key(_) => {}
            Message::Shortcut(sc) => return self.shortcut(sc),

            Message::SelectProject(p) => {
                self.rename = None;
                self.dispatch(Command::SelectProject(p));
                return self.after_select();
            }
            Message::SelectTab(t) => {
                if self.rename.as_ref().is_some_and(|r| r.target != RenameTarget::Tab(t)) {
                    self.rename = None;
                }
                self.dispatch(Command::SelectTab(t));
                return self.after_select();
            }
            Message::NewTab => {
                if let Some(project) = self.core.state().selected_project {
                    self.dispatch(Command::CreateTab { project, title: None });
                    return self.after_select();
                }
            }

            Message::NewProject => {
                self.dialog = Some(Dialog::Project {
                    editing: None,
                    name: String::new(),
                    dir: String::new(),
                    shell: String::new(),
                    error: None,
                });
                return operation::focus(DIALOG_FIRST_INPUT);
            }
            Message::EditProject(id) => {
                if let Some(p) = self.core.state().project(id) {
                    self.dialog = Some(Dialog::Project {
                        editing: Some(id),
                        name: p.name.clone(),
                        dir: p.working_dir.display().to_string(),
                        shell: p.shell.clone().unwrap_or_default(),
                        error: None,
                    });
                    return operation::focus(DIALOG_FIRST_INPUT);
                }
            }
            Message::OpenSettings => {
                self.rename = None;
                self.task_edit = None;
                let shell_input = self.core.state().settings.default_shell.clone().unwrap_or_default();
                self.settings = Some(SettingsState { shell_input, ..SettingsState::default() });
                self.codex_hook = self.core.codex_hook_status();
                // Pick up shells installed since the last look.
                return detect_shells();
            }
            Message::AddCodexHook => {
                self.prefs.codex_hook_asked = true;
                self.mark_prefs();
                let added = self.core.install_codex_hook();
                self.codex_hook = self.core.codex_hook_status();
                match added {
                    Ok(_) => return self.update(Message::TrustCodexHook),
                    Err(e) => self.error = Some(format!("Could not add the Codex hook: {e}")),
                }
            }
            Message::TrustCodexHook => {
                let s = self.core.state();
                let cwd = s
                    .selected_project
                    .and_then(|p| s.project(p))
                    .map(|p| p.working_dir.clone())
                    .filter(|d| d.is_dir())
                    .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
                    .unwrap_or_default();
                let handle = self.core.open_scratch_terminal(&cwd, Some("codex"));
                let focus = operation::focus(terminal_view::widget_id(handle.pane()));
                self.dialog = Some(Dialog::CodexTrust(handle));
                return focus;
            }
            Message::ShellsDetected(list) => self.shells = list,
            Message::PickDefaultShell(option) => {
                if let Some(st) = &mut self.settings {
                    st.shell_input = option.command.clone();
                }
                self.dispatch(Command::SetDefaultShell(Some(option.command)));
            }
            Message::InstallBash => {
                if matches!(self.bash_install, BashInstall::Running(_)) {
                    return Task::none();
                }
                let total = bundled_bash::download_size();
                self.bash_install = BashInstall::Running(bundled_bash::Progress::Downloading { done: 0, total });
                let (tx, rx) = async_channel::unbounded();
                std::thread::spawn(move || {
                    let report = tx.clone();
                    let result = bundled_bash::install(move |p| {
                        let _ = report.send_blocking(BashInstallEvent::Progress(p));
                    });
                    let _ = tx.send_blocking(BashInstallEvent::Done(result.map_err(|e| e.to_string())));
                });
                return Task::run(rx, Message::BashInstall);
            }
            Message::BashInstall(BashInstallEvent::Progress(p)) => {
                if matches!(self.bash_install, BashInstall::Running(_)) {
                    self.bash_install = BashInstall::Running(p);
                }
            }
            Message::BashInstall(BashInstallEvent::Done(Ok(exe))) => {
                self.bash_install = BashInstall::Idle;
                // The button says "Install and use": new terminals start bash.
                let command = bundled_bash::shell_option(&exe).command;
                if let Some(st) = &mut self.settings {
                    st.shell_input = command.clone();
                }
                self.dispatch(Command::SetDefaultShell(Some(command)));
                return detect_shells();
            }
            Message::BashInstall(BashInstallEvent::Done(Err(e))) => {
                tracing::warn!(error = %e, "installing TermRoost bash failed");
                self.bash_install = BashInstall::Failed(e);
            }
            Message::Noop => {}
            Message::RemoveCodexHook => {
                if let Err(e) = self.core.remove_codex_hook() {
                    self.error = Some(format!("Could not remove the Codex hook: {e}"));
                }
                self.codex_hook = self.core.codex_hook_status();
            }
            Message::CloseSettings => {
                if self.settings.is_some() {
                    self.commit_shell();
                    self.settings = None;
                }
                return self.focus_active();
            }
            Message::SystemTheme(mode) => {
                self.system_mode = mode;
                self.apply_theme();
            }
            Message::FollowSystem(on) => {
                self.prefs.follow_system = on;
                if !on {
                    // Keep showing what was visible.
                    self.prefs.theme = self.active_theme().name;
                }
                self.mark_prefs();
                self.apply_theme();
            }
            Message::UseTheme(name) => {
                if let Some(t) = self.themes.find(&name) {
                    let dark = t.is_dark();
                    if dark {
                        self.prefs.dark_theme = name.clone();
                    } else {
                        self.prefs.light_theme = name.clone();
                    }
                    // Picking a theme of the other tone than the system shows it
                    // now, instead of silently waiting for the system to switch.
                    if self.prefs.follow_system && dark != self.system_is_dark() {
                        self.prefs.follow_system = false;
                    }
                    self.prefs.theme = name;
                    self.mark_prefs();
                    self.apply_theme();
                }
            }
            Message::TerminalTheme(name) => {
                self.prefs.terminal_theme = name;
                self.mark_prefs();
                self.apply_theme();
            }
            Message::EditTheme(name) => {
                if let Some(st) = &mut self.settings {
                    st.inputs.clear();
                    st.name_error = None;
                    st.name_input = name.clone();
                    st.editing = Some(name);
                }
            }
            Message::DuplicateTheme(name) => {
                if let Some(mut copy) = self.themes.find(&name).cloned() {
                    copy.name = self.themes.unique_name(&format!("{name} copy"));
                    let new_name = copy.name.clone();
                    self.themes.custom.push(copy);
                    self.themes_dirty = true;
                    return self.update(Message::EditTheme(new_name));
                }
            }
            Message::DeleteTheme(name) => {
                if !self.themes.is_builtin(&name) {
                    self.themes.custom.retain(|t| t.name != name);
                    self.themes_dirty = true;
                    let p = &mut self.prefs;
                    for (slot, fallback) in [
                        (&mut p.theme, themes::DARK),
                        (&mut p.light_theme, themes::LIGHT),
                        (&mut p.dark_theme, themes::DARK),
                    ] {
                        if *slot == name {
                            *slot = fallback.into();
                        }
                    }
                    if p.terminal_theme.as_deref() == Some(name.as_str()) {
                        p.terminal_theme = None;
                    }
                    self.mark_prefs();
                    if let Some(st) = &mut self.settings
                        && st.editing.as_deref() == Some(name.as_str())
                    {
                        st.editing = None;
                    }
                    self.apply_theme();
                }
            }
            Message::ThemeNameInput(value) => self.rename_theme(value),
            Message::ThemeColor(field, value) => {
                let Some(st) = &mut self.settings else { return Task::none() };
                st.inputs.insert(field, value.clone());
                if let (Some(hex), Some(name)) = (Hex::parse(&value), st.editing.clone())
                    && let Some(t) = self.themes.custom_mut(&name)
                {
                    t.set(field, hex);
                    self.themes_dirty = true;
                    if self.active_theme().name == name || self.terminal_theme().name == name {
                        self.apply_theme();
                    }
                }
            }
            Message::FontSize(size) => {
                self.prefs.font_size = size.round().clamp(8.0, 32.0);
                self.mark_prefs();
            }
            Message::ShellInput(v) => {
                if let Some(st) = &mut self.settings {
                    st.shell_input = v;
                }
            }
            Message::ShellCommit => self.commit_shell(),
            Message::PaneSettings(pane) => {
                if let Some(p) = self.core.state().pane(pane) {
                    let args = p.agent.as_ref().map(|a| a.args.clone()).unwrap_or_default();
                    self.dialog =
                        Some(Dialog::Pane { pane, resume: p.resume_command.clone().unwrap_or_default(), args });
                    return operation::focus(DIALOG_FIRST_INPUT);
                }
            }
            Message::AskDeleteProject(p) => self.dialog = Some(Dialog::ConfirmDeleteProject(p)),
            Message::AskDeleteTab(t) => self.dialog = Some(Dialog::ConfirmDeleteTab(t)),
            Message::DialogName(v) => {
                if let Some(Dialog::Project { name, .. }) = &mut self.dialog {
                    *name = v;
                }
            }
            Message::DialogDir(v) => {
                if let Some(Dialog::Project { dir, .. }) = &mut self.dialog {
                    *dir = v;
                }
            }
            Message::DialogShell(v) => {
                if let Some(Dialog::Project { shell, .. }) = &mut self.dialog {
                    *shell = v;
                }
            }
            Message::DialogResume(v) => {
                if let Some(Dialog::Pane { resume, .. }) = &mut self.dialog {
                    *resume = v;
                }
            }
            Message::DialogArgs(v) => {
                if let Some(Dialog::Pane { args, .. }) = &mut self.dialog {
                    *args = v;
                }
            }
            Message::BrowseDir => {
                let start = match &self.dialog {
                    Some(Dialog::Project { dir, .. }) if !dir.is_empty() => Some(PathBuf::from(dir)),
                    _ => directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()),
                };
                return Task::perform(
                    async move {
                        let mut dlg = rfd::AsyncFileDialog::new().set_title("Project working directory");
                        if let Some(start) = start {
                            dlg = dlg.set_directory(start);
                        }
                        dlg.pick_folder().await.map(|h| h.path().to_path_buf())
                    },
                    Message::DirPicked,
                );
            }
            Message::DirPicked(path) => {
                if let (Some(path), Some(Dialog::Project { dir, name, .. })) = (path, &mut self.dialog) {
                    *dir = path.display().to_string();
                    fill_name_from_dir(name, dir);
                }
            }
            Message::DialogSubmit => return self.submit_dialog(),
            Message::DialogCancel => {
                match &self.dialog {
                    Some(Dialog::CodexHook) => {
                        // "Not now": don't ask again; Settings → Integrations offers it.
                        self.prefs.codex_hook_asked = true;
                        self.mark_prefs();
                    }
                    Some(Dialog::CodexTrust(h)) => {
                        let pane = h.pane();
                        self.core.close_scratch_terminal(pane);
                        self.codex_hook = self.core.codex_hook_status();
                    }
                    _ => {}
                }
                self.dialog = None;
                return self.focus_active();
            }
            Message::DialogStartAgent(kind) => {
                if let Some(Dialog::Pane { pane, resume, args }) = self.dialog.take() {
                    self.dispatch(Command::SetResumeCommand { pane, command: Some(resume) });
                    self.prefs.agent_args.insert(kind.as_str().to_owned(), args.trim().to_owned());
                    self.mark_prefs();
                    self.dispatch(Command::StartAgent { pane, kind, args });
                    return self.focus_active();
                }
            }
            Message::DialogClearAgent => {
                if let Some(Dialog::Pane { pane, .. }) = &self.dialog {
                    let pane = *pane;
                    self.dispatch(Command::ClearAgent(pane));
                }
            }

            Message::StartRename(target) => {
                let s = self.core.state();
                let value = match target {
                    RenameTarget::Project(p) => s.project(p).map(|p| p.name.clone()),
                    RenameTarget::Tab(t) => s.tab(t).map(|t| t.title.clone()),
                };
                if let Some(value) = value {
                    self.rename = Some(Rename { target, value });
                    return Task::batch([operation::focus(RENAME_INPUT), operation::select_all(RENAME_INPUT)]);
                }
            }
            Message::RenameInput(v) => {
                if let Some(r) = &mut self.rename {
                    r.value = v;
                }
            }
            Message::RenameCommit => {
                if let Some(r) = self.rename.take() {
                    let cmd = match r.target {
                        RenameTarget::Project(project) => Command::RenameProject { project, name: r.value },
                        RenameTarget::Tab(tab) => Command::RenameTab { tab, title: r.value },
                    };
                    self.dispatch(cmd);
                }
                return self.focus_active();
            }
            Message::RenameCancel => {
                self.rename = None;
                return self.focus_active();
            }

            Message::FocusPane(pane) => {
                self.dispatch(Command::FocusPane(pane));
                return self.focus_active();
            }
            Message::PaneClicked(gp) => {
                if let Some((_, pane)) = self.grid_pane(gp) {
                    return self.update(Message::FocusPane(pane));
                }
            }
            Message::PaneDragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                if let Some(tab) = self.active_tab_id()
                    && let Some(grid) = self.grids.get_mut(&tab)
                {
                    grid.drop(pane, target);
                    if let Some(layout) = layout_conv::layout_of(grid) {
                        self.dispatch(Command::SetLayout { tab, layout });
                    }
                }
            }
            Message::PaneDragged(_) => {}
            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                if let Some(tab) = self.active_tab_id()
                    && let Some(grid) = self.grids.get_mut(&tab)
                {
                    grid.resize(split, ratio);
                    self.layout_dirty.insert(tab);
                }
            }
            Message::Split(pane, axis) => {
                self.flush_pending();
                if let Some(Outcome::Pane(_)) = self.dispatch(Command::SplitPane { pane, axis })
                    && let Some(tab) = self.core.state().pane(pane).map(|p| p.tab)
                {
                    self.rebuild_grid(tab);
                }
                return self.focus_active();
            }
            Message::ClosePane(pane) => {
                self.flush_pending();
                let tab = self.core.state().pane(pane).map(|p| p.tab);
                if self.dispatch(Command::ClosePane(pane)).is_some()
                    && let Some(tab) = tab
                {
                    self.rebuild_grid(tab);
                }
                return self.focus_active();
            }
            Message::RestartPane(pane) => {
                self.dispatch(Command::RestartPane(pane));
            }
            Message::StartAgent(pane, kind) => {
                let args = self.prefs.agent_args.get(kind.as_str()).cloned().unwrap_or_default();
                self.dispatch(Command::StartAgent { pane, kind, args });
                self.dispatch(Command::FocusPane(pane));
                return self.focus_active();
            }

            Message::ToggleTasks => {
                self.prefs.tasks_open = !self.prefs.tasks_open;
                self.mark_prefs();
                if self.prefs.tasks_open {
                    return operation::focus(TASK_INPUT);
                }
                return self.focus_active();
            }
            Message::NewTaskInput(v) => self.new_task = v,
            Message::AddTask => {
                let title = std::mem::take(&mut self.new_task);
                if let Some(project) = self.core.state().selected_project
                    && !title.trim().is_empty()
                {
                    let tab = if self.prefs.tasks_this_tab_only { self.active_tab_id() } else { None };
                    self.dispatch(Command::CreateTask { project, title, tab });
                }
            }
            Message::TaskDone(task, done) => {
                self.dispatch(Command::SetTaskDone { task, done });
            }
            Message::TaskTab(task, choice) => {
                self.dispatch(Command::AssignTask { task, tab: choice.id });
            }
            Message::DeleteTask(task) => {
                if self.task_edit.as_ref().is_some_and(|(t, _)| *t == task) {
                    self.task_edit = None;
                }
                self.dispatch(Command::DeleteTask(task));
            }
            Message::EditTask(task) => {
                if let Some(t) = self.core.state().task(task) {
                    self.task_edit = Some((task, t.title.clone()));
                    return operation::focus(TASK_EDIT_INPUT);
                }
            }
            Message::TaskEditInput(v) => {
                if let Some((_, value)) = &mut self.task_edit {
                    *value = v;
                }
            }
            Message::TaskEditCommit => {
                if let Some((task, title)) = self.task_edit.take() {
                    let notes = self.core.state().task(task).map(|t| t.notes.clone()).unwrap_or_default();
                    self.dispatch(Command::EditTask { task, title, notes });
                }
            }
            Message::TaskEditCancel => self.task_edit = None,
            Message::ShowDone(v) => {
                self.prefs.show_done = v;
                self.mark_prefs();
            }
            Message::ThisTabOnly(v) => {
                self.prefs.tasks_this_tab_only = v;
                self.mark_prefs();
            }
            Message::DismissError => self.error = None,
        }
        Task::none()
    }

    fn submit_dialog(&mut self) -> Task<Message> {
        let Some(dialog) = self.dialog.take() else { return Task::none() };
        match dialog {
            Dialog::Project { editing, name, dir, shell, .. } => {
                let path = expand_home(dir.trim());
                let problem = if name.trim().is_empty() {
                    Some("Give the project a name.")
                } else if dir.trim().is_empty() {
                    Some("Choose a working directory.")
                } else if !path.is_dir() {
                    Some("That directory does not exist.")
                } else {
                    None
                };
                if let Some(problem) = problem {
                    self.dialog = Some(Dialog::Project { editing, name, dir, shell, error: Some(problem.into()) });
                    return Task::none();
                }
                let shell = Some(shell).filter(|s| !s.trim().is_empty());
                match editing {
                    None => {
                        self.dispatch(Command::CreateProject { name, working_dir: path, shell });
                        return self.after_select();
                    }
                    Some(project) => {
                        self.dispatch(Command::RenameProject { project, name });
                        self.dispatch(Command::SetProjectDir { project, working_dir: path });
                        self.dispatch(Command::SetProjectShell { project, shell });
                    }
                }
            }
            Dialog::ConfirmDeleteProject(project) => {
                let tabs: Vec<TabId> = self.core.state().tabs_of(project).map(|t| t.id).collect();
                self.dispatch(Command::DeleteProject(project));
                for t in tabs {
                    self.grids.remove(&t);
                    self.activity.remove(&t);
                }
                return self.after_select();
            }
            Dialog::ConfirmDeleteTab(tab) => {
                self.dispatch(Command::DeleteTab(tab));
                self.grids.remove(&tab);
                self.activity.remove(&tab);
                return self.after_select();
            }
            Dialog::Pane { pane, resume, .. } => {
                self.dispatch(Command::SetResumeCommand { pane, command: Some(resume) });
            }
            Dialog::CodexHook => return self.update(Message::AddCodexHook),
            Dialog::CodexTrust(h) => {
                self.dialog = Some(Dialog::CodexTrust(h));
                return self.update(Message::DialogCancel);
            }
        }
        self.focus_active()
    }

    fn commit_shell(&mut self) {
        let Some(st) = &self.settings else { return };
        let wanted = Some(st.shell_input.trim().to_owned()).filter(|s| !s.is_empty());
        if wanted != self.core.state().settings.default_shell {
            self.dispatch(Command::SetDefaultShell(wanted));
        }
    }

    /// Renames the edited custom theme when the new name is valid and free.
    fn rename_theme(&mut self, value: String) {
        let Some(st) = &mut self.settings else { return };
        st.name_input = value.clone();
        let Some(old) = st.editing.clone() else { return };
        let new = value.trim().to_owned();
        st.name_error = if new.is_empty() {
            Some("A theme needs a name.".into())
        } else if new != old && self.themes.find(&new).is_some() {
            Some("Another theme already has this name.".into())
        } else {
            None
        };
        if st.name_error.is_some() || new == old {
            return;
        }
        st.editing = Some(new.clone());
        if let Some(t) = self.themes.custom_mut(&old) {
            t.name = new.clone();
        }
        let p = &mut self.prefs;
        for slot in [&mut p.theme, &mut p.light_theme, &mut p.dark_theme] {
            if *slot == old {
                *slot = new.clone();
            }
        }
        if p.terminal_theme.as_deref() == Some(old.as_str()) {
            p.terminal_theme = Some(new);
        }
        self.themes_dirty = true;
        self.mark_prefs();
        self.apply_theme();
    }

    fn shortcut(&mut self, sc: Shortcut) -> Task<Message> {
        if self.dialog.is_some() || self.settings.is_some() {
            return Task::none();
        }
        let s = self.core.state();
        let focused = s.active_tab().and_then(|t| t.focused_pane);
        match sc {
            Shortcut::NewTab => return self.update(Message::NewTab),
            Shortcut::SplitRight => {
                if let Some(p) = focused {
                    return self.update(Message::Split(p, Axis::Vertical));
                }
            }
            Shortcut::SplitDown => {
                if let Some(p) = focused {
                    return self.update(Message::Split(p, Axis::Horizontal));
                }
            }
            Shortcut::ClosePane => {
                if let Some(p) = focused {
                    return self.update(Message::ClosePane(p));
                }
            }
            Shortcut::NextTab | Shortcut::PrevTab => {
                if let Some(project) = s.selected_project {
                    let tabs: Vec<TabId> = s.tabs_of(project).map(|t| t.id).collect();
                    if let Some(next) = cycle(&tabs, self.active_tab_id(), sc == Shortcut::NextTab) {
                        return self.update(Message::SelectTab(next));
                    }
                }
            }
            Shortcut::NextProject | Shortcut::PrevProject => {
                let projects: Vec<ProjectId> = s.projects.iter().map(|p| p.id).collect();
                if let Some(next) = cycle(&projects, s.selected_project, sc == Shortcut::NextProject) {
                    return self.update(Message::SelectProject(next));
                }
            }
            Shortcut::Focus(dir) => {
                let (Some(tab), Some(pane)) = (self.active_tab_id(), focused) else { return Task::none() };
                let Some(grid) = self.grids.get(&tab) else { return Task::none() };
                let Some(gp) = layout_conv::find(grid, pane) else { return Task::none() };
                let d = match dir {
                    Direction::Left => pane_grid::Direction::Left,
                    Direction::Right => pane_grid::Direction::Right,
                    Direction::Up => pane_grid::Direction::Up,
                    Direction::Down => pane_grid::Direction::Down,
                };
                if let Some(target) = grid.adjacent(gp, d).and_then(|g| grid.get(g)).copied() {
                    return self.update(Message::FocusPane(target));
                }
            }
            Shortcut::ToggleTasks => return self.update(Message::ToggleTasks),
            Shortcut::FontBigger | Shortcut::FontSmaller | Shortcut::FontReset => {
                self.prefs.font_size = match sc {
                    Shortcut::FontBigger => (self.prefs.font_size + 1.0).min(32.0),
                    Shortcut::FontSmaller => (self.prefs.font_size - 1.0).max(8.0),
                    _ => theme::DEFAULT_FONT_SIZE,
                };
                self.mark_prefs();
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let base: Element<'_, Message> = if self.settings.is_some() {
            self.view_settings()
        } else {
            let mut main = row![self.view_projects(), self.view_tabs(), self.view_main()].height(Length::Fill);
            if self.prefs.tasks_open && self.core.state().selected_project.is_some() {
                main = main.push(self.view_tasks());
            }
            container(main).width(Length::Fill).height(Length::Fill).into()
        };
        match &self.dialog {
            Some(d) => stack![base, self.view_dialog(d)].into(),
            None => column![base].into(),
        }
    }
}

/// Looks for installed shells on a thread (it may wait on `wsl.exe`).
fn detect_shells() -> Task<Message> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(shell::detect_shells());
    });
    Task::perform(async move { rx.recv().await.unwrap_or_default() }, Message::ShellsDetected)
}

/// The entry of `shells` whose command line is `command`, for showing the
/// current choice in a shell picker.
pub(crate) fn shell_matching<'a>(shells: &'a [ShellOption], command: &str) -> Option<&'a ShellOption> {
    let wanted = shell::split_command_line(command)?;
    shells.iter().find(|o| shell::split_command_line(&o.command).as_ref() == Some(&wanted))
}

fn cycle<T: Copy + PartialEq>(items: &[T], current: Option<T>, forward: bool) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let i = current.and_then(|c| items.iter().position(|x| *x == c)).unwrap_or(0);
    let n = items.len();
    Some(items[if forward { (i + 1) % n } else { (i + n - 1) % n }])
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~")
        && (rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\'))
        && let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
    {
        return home.join(rest.trim_start_matches(['/', '\\']));
    }
    PathBuf::from(p)
}

/// Suggests the folder name as project name while the name is still empty.
fn fill_name_from_dir(name: &mut String, dir: &str) {
    if name.trim().is_empty()
        && let Some(base) = PathBuf::from(dir.trim_end_matches(['/', '\\'])).file_name()
    {
        *name = base.to_string_lossy().into_owned();
    }
}
