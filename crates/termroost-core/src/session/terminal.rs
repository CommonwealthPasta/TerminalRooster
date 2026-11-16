//! One terminal: an alacritty `Term`, its PTY and the PTY reader thread.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event as AEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term, TermMode, viewport_to_point};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::Rgb as ARgb;

use crate::agent::AgentKind;
use crate::alert::{Alert, Tracker};
use crate::command::Event;
use crate::detect::Program;
use crate::model::PaneId;
use crate::session::input::{self, KeyInput, MouseInput};
use crate::session::osc::Signal;
use crate::session::render::{RenderContent, SharedPalette};
use crate::session::shell::ShellCommand;
use crate::session::snapshot;
use crate::session::tap::TapPty;

/// Size of the terminal grid and of one cell in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Default for GridSize {
    fn default() -> Self {
        Self { cols: 100, rows: 30, cell_width: 8, cell_height: 16 }
    }
}

impl GridSize {
    fn window_size(self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

/// How a mouse selection grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    /// Character-wise (single click and drag).
    Simple,
    /// Word-wise (double click).
    Word,
    /// Line-wise (triple click).
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcState {
    NotStarted,
    Running(u64),
    Exited(Option<i32>),
}

/// What to run in the PTY.
#[derive(Clone, Debug)]
pub(crate) struct SpawnSpec {
    pub shell: ShellCommand,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
}

/// State shared between the handle, the term's event proxy and the PTY thread.
pub(crate) struct Shared {
    pane: PaneId,
    events: async_channel::Sender<Event>,
    sender: Mutex<Option<EventLoopSender>>,
    /// A `TerminalDirty` event is outstanding.
    render_pending: AtomicBool,
    /// Content changed since the last scrollback save.
    pub(crate) save_dirty: AtomicBool,
    generation: AtomicU64,
    proc: Mutex<ProcState>,
    run_counter: AtomicU64,
    /// When the user last pressed Enter (newest last), to match Codex hook
    /// reports to the terminal that submitted the prompt.
    submits: Mutex<std::collections::VecDeque<std::time::SystemTime>>,
    /// Process id of the shell, 0 when none is running.
    shell_pid: std::sync::atomic::AtomicU32,
    title: Mutex<Option<String>>,
    size: Mutex<GridSize>,
    palette: SharedPalette,
    /// Turns what the program sends into alerts (failed commands, finished agents).
    tracker: Mutex<Tracker>,
    /// What runs below the shell, as the background scan last saw it.
    programs: Mutex<Vec<Program>>,
    /// The directory the shell last reported (shells report it at every prompt).
    cwd: Mutex<Option<PathBuf>>,
    /// Locked after the term, never before.
    pin: Mutex<Pin>,
    /// PTYs of the core's terminals that are open, or closed with their shell
    /// not seen gone yet (see `tap::EndShell`).
    pub(crate) ptys: Arc<std::sync::atomic::AtomicUsize>,
}

impl Shared {
    fn send(&self, ev: Event) {
        let _ = self.events.try_send(ev);
    }

    fn alert(&self, alert: Option<Alert>) {
        if let Some(alert) = alert {
            tracing::debug!(pane = %self.pane, ?alert, "alert");
            self.send(Event::Alert { pane: self.pane, alert });
        }
    }

    /// A sequence found in the output by the PTY tap. Runs while the PTY
    /// thread may hold the term lock, so it must not take it.
    pub(crate) fn on_signal(&self, signal: Signal) {
        match signal {
            Signal::Cwd(dir) => {
                let mut cwd = self.cwd.lock().unwrap();
                if cwd.as_ref() != Some(&dir) {
                    *cwd = Some(dir.clone());
                    drop(cwd);
                    self.send(Event::CwdChanged { pane: self.pane, dir });
                }
            }
            Signal::Cleared => self.pin.lock().unwrap().screen_cleared(),
            signal => {
                let alert = self.track(|t| t.signal(signal));
                self.alert(alert);
            }
        }
    }

    /// Runs `f` on the tracker, and tells the front end when that started or
    /// ended an agent's turn or a command typed at the prompt.
    fn track<R>(&self, f: impl FnOnce(&mut Tracker) -> R) -> R {
        let mut tracker = self.tracker.lock().unwrap();
        let was = (tracker.busy(), tracker.running().map(str::to_owned));
        let out = f(&mut tracker);
        let changed = (tracker.busy(), tracker.running()) != (was.0, was.1.as_deref());
        drop(tracker);
        if changed {
            self.send(Event::ActivityChanged(self.pane));
        }
        out
    }

    /// Records what runs below the shell; tells the front end if that changed.
    fn set_programs(&self, programs: Vec<Program>) {
        let mut current = self.programs.lock().unwrap();
        if *current != programs {
            *current = programs;
            drop(current);
            self.send(Event::ActivityChanged(self.pane));
        }
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    fn wake(&self) {
        self.bump();
        self.save_dirty.store(true, Ordering::Relaxed);
        if !self.render_pending.swap(true, Ordering::AcqRel) {
            self.send(Event::TerminalDirty(self.pane));
        }
    }

    fn write(&self, bytes: Cow<'static, [u8]>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(s) = self.sender.lock().unwrap().as_ref() {
            let _ = s.send(Msg::Input(bytes));
        }
    }

    fn mark_exited(&self, run: Option<u64>, code: Option<i32>) {
        {
            let mut proc = self.proc.lock().unwrap();
            match (*proc, run) {
                (ProcState::Running(cur), Some(r)) if cur == r => {}
                (ProcState::Running(_), None) => {}
                _ => return,
            }
            *proc = ProcState::Exited(code);
        }
        *self.sender.lock().unwrap() = None;
        self.shell_pid.store(0, Ordering::Release);
        tracing::info!(pane = %self.pane, ?code, "process exited");
        self.send(Event::ProcessExited { pane: self.pane, code });
        // Nothing runs in it any more (see `TerminalHandle::activity`).
        self.programs.lock().unwrap().clear();
        self.send(Event::ActivityChanged(self.pane));
        self.wake();
    }
}

/// Event listener for the term (`run: None`) and for each PTY event loop.
#[derive(Clone)]
pub(crate) struct Proxy {
    shared: Arc<Shared>,
    run: Option<u64>,
}

impl EventListener for Proxy {
    fn send_event(&self, event: AEvent) {
        let s = &self.shared;
        match event {
            AEvent::Wakeup => s.wake(),
            AEvent::Title(t) => {
                *s.title.lock().unwrap() = Some(t.clone());
                let alert = s.track(|tracker| tracker.title(&t));
                s.send(Event::TitleChanged { pane: s.pane, title: t });
                s.alert(alert);
            }
            AEvent::ResetTitle => {
                *s.title.lock().unwrap() = None;
                s.track(|tracker| tracker.title(""));
                s.send(Event::TitleChanged { pane: s.pane, title: String::new() });
            }
            AEvent::Bell => {
                s.send(Event::Bell(s.pane));
                let alert = s.tracker.lock().unwrap().bell();
                s.alert(alert);
            }
            AEvent::PtyWrite(text) => s.write(Cow::Owned(text.into_bytes())),
            AEvent::ClipboardStore(_, text) => s.send(Event::ClipboardStore(text)),
            AEvent::ColorRequest(index, fmt) => {
                let c = s.palette.read().unwrap().get(index);
                s.write(Cow::Owned(fmt(ARgb { r: c.r, g: c.g, b: c.b }).into_bytes()));
            }
            AEvent::TextAreaSizeRequest(fmt) => {
                let size = *s.size.lock().unwrap();
                s.write(Cow::Owned(fmt(size.window_size()).into_bytes()));
            }
            AEvent::ChildExit(status) => s.mark_exited(self.run, status.code()),
            AEvent::Exit => s.mark_exited(None, None),
            // Reading the clipboard from programs is not allowed.
            AEvent::ClipboardLoad(..) | AEvent::MouseCursorDirty | AEvent::CursorBlinkingChange => {}
        }
    }
}

pub(crate) struct Session {
    pub(crate) shared: Arc<Shared>,
    term: Arc<FairMutex<Term<Proxy>>>,
    render: Mutex<(u64, RenderContent)>,
}

/// What runs in a terminal now, besides its shell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// The coding agent in it is in the middle of a turn.
    pub agent_working: bool,
    /// The programs below the shell, coding agents included, as the last
    /// scan saw them (every few seconds). When it saw none, the command typed
    /// at the prompt if one is running (bash reports it).
    pub programs: Vec<Program>,
}

impl Activity {
    /// The programs that keep the terminal busy: all but the coding agents,
    /// which count only while working.
    pub fn busy_programs(&self) -> impl Iterator<Item = &Program> {
        self.programs.iter().filter(|p| p.agent.is_none())
    }

    /// Nothing goes on: the shell, or an agent, waits for the user.
    pub fn is_idle(&self) -> bool {
        !self.agent_working && self.busy_programs().next().is_none()
    }
}

/// Cheap, cloneable, thread-safe handle to one terminal.
#[derive(Clone)]
pub struct TerminalHandle(pub(crate) Arc<Session>);

impl std::fmt::Debug for TerminalHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TerminalHandle").field(&self.pane()).finish()
    }
}

impl TerminalHandle {
    pub(crate) fn new(
        pane: PaneId,
        events: async_channel::Sender<Event>,
        palette: SharedPalette,
        scrollback_lines: usize,
        size: GridSize,
        ptys: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Self {
        let shared = Arc::new(Shared {
            pane,
            events,
            sender: Mutex::new(None),
            render_pending: AtomicBool::new(false),
            save_dirty: AtomicBool::new(false),
            generation: AtomicU64::new(1),
            proc: Mutex::new(ProcState::NotStarted),
            run_counter: AtomicU64::new(0),
            shell_pid: std::sync::atomic::AtomicU32::new(0),
            submits: Mutex::new(std::collections::VecDeque::new()),
            title: Mutex::new(None),
            size: Mutex::new(size),
            palette,
            tracker: Mutex::new(Tracker::default()),
            programs: Mutex::new(Vec::new()),
            cwd: Mutex::new(None),
            pin: Mutex::new(Pin::Off),
            ptys,
        });
        let config = term::Config { scrolling_history: scrollback_lines, ..Default::default() };
        let term = Term::new(config, &size, Proxy { shared: shared.clone(), run: None });
        TerminalHandle(Arc::new(Session {
            shared,
            term: Arc::new(FairMutex::new(term)),
            render: Mutex::new((0, RenderContent::default())),
        }))
    }

    pub fn pane(&self) -> PaneId {
        self.0.shared.pane
    }

    /// Replays saved scrollback before the shell starts.
    pub(crate) fn replay(&self, data: &[u8], banner: Option<&str>) {
        snapshot::replay(&mut *self.0.term.lock(), data, banner);
        self.0.shared.bump();
    }

    /// Prints a message into the terminal (e.g. a spawn error).
    pub(crate) fn print_notice(&self, text: &str) {
        let msg = format!("\r\n\x1b[0;33m{text}\x1b[0m\r\n");
        self.replay(msg.as_bytes(), None);
        self.0.shared.wake();
    }

    /// Starts (or restarts) the shell process.
    pub(crate) fn spawn(&self, spec: &SpawnSpec) -> std::io::Result<()> {
        let shared = &self.0.shared;
        if let Some(old) = shared.sender.lock().unwrap().take() {
            let _ = old.send(Msg::Shutdown);
        }
        // On Windows the command line is built by joining the program and the
        // arguments: quote a program path with spaces (`C:\Program Files\…`),
        // and arguments too (their quotes were removed when splitting).
        #[cfg(windows)]
        let program = quote_program(&spec.shell.program);
        #[cfg(not(windows))]
        let program = spec.shell.program.clone();
        #[allow(clippy::needless_update)]
        let options = tty::Options {
            shell: Some(tty::Shell::new(program, spec.shell.args.clone())),
            working_directory: Some(spec.cwd.clone()),
            drain_on_exit: true,
            env: spec.env.clone(),
            #[cfg(windows)]
            escape_args: true,
            ..Default::default()
        };
        let size = *shared.size.lock().unwrap();
        // The new shell reports its directory afresh.
        *shared.cwd.lock().unwrap() = None;
        if cfg!(windows) {
            self.make_room_for_conpty();
        }
        let pty = tty::new(&options, size.window_size(), shared.pane.0 as u64)?;
        #[cfg(not(windows))]
        let shell_pid = pty.child().id();
        #[cfg(windows)]
        let shell_pid = pty.child_watcher().pid().map_or(0, |p| p.get());
        shared.shell_pid.store(shell_pid, Ordering::Release);
        shared.track(Tracker::reset);
        shared.set_programs(Vec::new());
        let pty = TapPty::new(pty, shared.clone());
        let run = shared.run_counter.fetch_add(1, Ordering::AcqRel) + 1;
        let proxy = Proxy { shared: shared.clone(), run: Some(run) };
        let event_loop = EventLoop::new(self.0.term.clone(), proxy, pty, true, false)?;
        *shared.sender.lock().unwrap() = Some(event_loop.channel());
        *shared.proc.lock().unwrap() = ProcState::Running(run);
        let _ = event_loop.spawn();
        shared.wake();
        Ok(())
    }

    /// ConPTY clears the screen when it starts and from then on draws the
    /// whole screen itself, from the top. So what is on the screen now (a
    /// restored session, or the last shell's output) moves into the history,
    /// and the view is pinned to keep it in sight above the new shell's output.
    fn make_room_for_conpty(&self) {
        let mut term = self.0.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        // Scrolls everything up to the last non-blank line into the history.
        snapshot::replay(&mut term, b"\x1b[0m\x1b[2J\x1b[H", None);
        let pin = if term.grid().history_size() > 0 { Pin::starting() } else { Pin::Off };
        *self.0.shared.pin.lock().unwrap() = pin;
        drop(term);
        self.0.shared.bump();
    }

    /// Stops the shell process (the content is kept).
    pub(crate) fn shutdown(&self) {
        self.0.shared.shell_pid.store(0, Ordering::Release);
        if let Some(s) = self.0.shared.sender.lock().unwrap().take() {
            let _ = s.send(Msg::Shutdown);
        }
    }

    /// Times Enter was pressed recently, oldest first.
    pub(crate) fn submits(&self) -> Vec<std::time::SystemTime> {
        self.0.shared.submits.lock().unwrap().iter().copied().collect()
    }

    /// Process id of the running shell.
    pub fn shell_pid(&self) -> Option<u32> {
        Some(self.0.shared.shell_pid.load(Ordering::Acquire)).filter(|p| *p != 0)
    }

    pub fn is_running(&self) -> bool {
        matches!(*self.0.shared.proc.lock().unwrap(), ProcState::Running(_))
    }

    /// `Some(code)` once the process has exited.
    pub fn exit_status(&self) -> Option<Option<i32>> {
        match *self.0.shared.proc.lock().unwrap() {
            ProcState::Exited(code) => Some(code),
            _ => None,
        }
    }

    /// Tells the terminal which coding agent runs in it, so its signals (the
    /// bell, progress, notifications) count as the agent finishing a turn.
    pub(crate) fn set_agent(&self, agent: Option<AgentKind>) {
        self.0.shared.tracker.lock().unwrap().agent = agent;
    }

    /// The agent in this terminal is working on a turn, as far as its own
    /// signals tell (Claude Code's title, a progress indicator).
    pub fn agent_working(&self) -> bool {
        self.0.shared.tracker.lock().unwrap().busy()
    }

    /// What runs in the terminal now. [`Event::ActivityChanged`] tells when
    /// this changes.
    pub fn activity(&self) -> Activity {
        if !self.is_running() {
            return Activity::default();
        }
        let tracker = self.0.shared.tracker.lock().unwrap();
        let mut programs = self.0.shared.programs.lock().unwrap().clone();
        // What the scan can't see: Git Bash's own tools (vim, less, ssh) leave
        // the Windows process tree, but the shell integration names them.
        if programs.is_empty()
            && let Some(name) = tracker.running()
        {
            programs.push(Program { name: name.to_owned(), agent: None, started_by: None });
        }
        Activity { agent_working: tracker.busy(), programs }
    }

    /// Records what the background scan found below the shell.
    pub(crate) fn set_programs(&self, programs: Vec<Program>) {
        self.0.shared.set_programs(programs);
    }

    /// Title set by the running program (OSC 0/2), if any.
    pub fn title(&self) -> Option<String> {
        self.0.shared.title.lock().unwrap().clone().filter(|t| !t.is_empty())
    }

    /// Forces the next [`Self::with_content`] to rebuild (e.g. after a palette change).
    pub(crate) fn invalidate(&self) {
        self.0.shared.bump();
    }

    /// Increases whenever the visible content may have changed.
    pub fn generation(&self) -> u64 {
        self.0.shared.generation.load(Ordering::Acquire)
    }

    fn mode(&self) -> TermMode {
        *self.0.term.lock().mode()
    }

    /// Writes raw bytes to the program.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        self.0.shared.write(Cow::Owned(bytes.into()));
    }

    /// User input: jump back to the live output and drop the selection.
    fn on_user_input(&self) {
        let mut term = self.0.term.lock();
        let mut changed = term.selection.take().is_some();
        let offset = term.grid().display_offset();
        if offset != 0 {
            term.scroll_display(Scroll::Bottom);
            // A pinned view stops at the newest output instead.
            self.user_scrolled(&mut term);
            changed |= term.grid().display_offset() != offset;
        }
        drop(term);
        if changed {
            self.0.shared.bump();
        }
    }

    /// After the user moved the view: keeps it within the pin.
    fn user_scrolled(&self, term: &mut Term<Proxy>) {
        apply_pin(term, &mut self.0.shared.pin.lock().unwrap(), true);
    }

    /// Sends a key press. Returns `false` if the key produces no input.
    pub fn key(&self, input: &KeyInput) -> bool {
        match input::encode_key(input, self.mode()) {
            Some(bytes) => {
                if input.key == input::Key::Named(input::NamedKey::Enter) {
                    let mut submits = self.0.shared.submits.lock().unwrap();
                    if submits.len() == 16 {
                        submits.pop_front();
                    }
                    submits.push_back(std::time::SystemTime::now());
                }
                self.on_user_input();
                self.write(bytes);
                true
            }
            None => false,
        }
    }

    /// Sends typed text (e.g. an IME commit) as-is.
    pub fn text(&self, text: &str) {
        if !text.is_empty() {
            self.on_user_input();
            self.write(text.as_bytes().to_vec());
        }
    }

    /// Pastes text, bracketed if the program enabled bracketed paste.
    pub fn paste(&self, text: &str) {
        let bytes = input::encode_paste(text, self.mode());
        self.on_user_input();
        self.write(bytes);
    }

    /// Reports focus changes to programs that asked for them.
    pub fn focus_changed(&self, focused: bool) {
        if self.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.write(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
        }
    }

    pub fn size(&self) -> GridSize {
        *self.0.shared.size.lock().unwrap()
    }

    /// Resizes the grid and the PTY. No-op if the size is unchanged.
    pub fn resize(&self, size: GridSize) {
        let size = GridSize { cols: size.cols.max(2), rows: size.rows.max(1), ..size };
        {
            let mut cur = self.0.shared.size.lock().unwrap();
            if *cur == size {
                return;
            }
            *cur = size;
        }
        {
            let mut term = self.0.term.lock();
            if cfg!(windows) {
                resize_like_conpty(&mut term, size);
            } else {
                term.resize(size);
            }
            apply_pin(&mut term, &mut self.0.shared.pin.lock().unwrap(), false);
        }
        if let Some(s) = self.0.shared.sender.lock().unwrap().as_ref() {
            let _ = s.send(Msg::Resize(size.window_size()));
        }
        self.0.shared.bump();
    }

    /// Whether mouse events should go to the program instead of selecting text.
    pub fn wants_mouse(&self) -> bool {
        self.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// Forwards a mouse event to the program. Returns `false` if it was not reported.
    pub fn mouse(&self, ev: &MouseInput) -> bool {
        let (mode, offset) = {
            let term = self.0.term.lock();
            (*term.mode(), term.grid().display_offset())
        };
        // A view scrolled back (see `Pin`) shows history above the program's screen.
        let Some(row) = ev.row.checked_sub(offset) else { return false };
        match input::encode_mouse(&MouseInput { row, ..*ev }, mode) {
            Some(bytes) => {
                self.write(bytes);
                true
            }
            None => false,
        }
    }

    /// Mouse wheel: `delta > 0` scrolls up. Goes to the program in mouse mode,
    /// becomes arrow keys on the alternate screen, else scrolls the history.
    pub fn scroll_lines(&self, delta: i32, col: usize, row: usize) {
        if delta == 0 {
            return;
        }
        let mode = self.mode();
        if mode.intersects(TermMode::MOUSE_MODE) {
            let kind = if delta > 0 { input::MouseEventKind::WheelUp } else { input::MouseEventKind::WheelDown };
            let ev = MouseInput { kind, col, row, mods: input::Modifiers::NONE };
            for _ in 0..delta.unsigned_abs().min(10) {
                self.mouse(&ev);
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let app = mode.contains(TermMode::APP_CURSOR);
            let seq: &[u8] = match (delta > 0, app) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.write(seq.repeat(delta.unsigned_abs().min(20) as usize));
        } else {
            self.scroll(Scroll::Delta(delta));
        }
    }

    pub fn scroll_page(&self, up: bool) {
        self.scroll(if up { Scroll::PageUp } else { Scroll::PageDown });
    }

    pub fn scroll_to_bottom(&self) {
        self.scroll(Scroll::Bottom);
    }

    fn scroll(&self, scroll: Scroll) {
        let mut term = self.0.term.lock();
        term.scroll_display(scroll);
        self.user_scrolled(&mut term);
        drop(term);
        self.0.shared.bump();
    }

    fn point(term: &Term<Proxy>, col: usize, row: usize) -> Point {
        let col = col.min(term.columns().saturating_sub(1));
        let row = row.min(term.screen_lines().saturating_sub(1));
        viewport_to_point(term.grid().display_offset(), Point::new(row, Column(col)))
    }

    /// Starts a selection at a viewport cell; `right_half` is the side of the cell clicked.
    pub fn select_start(&self, col: usize, row: usize, right_half: bool, kind: SelectionKind) {
        let mut term = self.0.term.lock();
        let point = Self::point(&term, col, row);
        let ty = match kind {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Word => SelectionType::Semantic,
            SelectionKind::Line => SelectionType::Lines,
        };
        let side = if right_half { Side::Right } else { Side::Left };
        term.selection = Some(Selection::new(ty, point, side));
        drop(term);
        self.0.shared.bump();
    }

    pub fn select_update(&self, col: usize, row: usize, right_half: bool) {
        let mut term = self.0.term.lock();
        let point = Self::point(&term, col, row);
        let side = if right_half { Side::Right } else { Side::Left };
        if let Some(sel) = term.selection.as_mut() {
            sel.update(point, side);
        }
        drop(term);
        self.0.shared.bump();
    }

    pub fn clear_selection(&self) {
        if self.0.term.lock().selection.take().is_some() {
            self.0.shared.bump();
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        self.0.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    /// Runs `f` on the current visible content, rebuilding it if needed.
    pub fn with_content<R>(&self, f: impl FnOnce(&RenderContent) -> R) -> R {
        // Clear first so output arriving during the rebuild triggers a new event.
        self.0.shared.render_pending.store(false, Ordering::Release);
        let generation = self.generation();
        let mut cache = self.0.render.lock().unwrap();
        if cache.0 != generation {
            let mut term = self.0.term.lock();
            // The output moved on: the pinned view follows it.
            apply_pin(&mut term, &mut self.0.shared.pin.lock().unwrap(), false);
            cache.1.rebuild(&term, &self.0.shared.palette.read().unwrap());
            cache.0 = generation;
        }
        f(&cache.1)
    }

    /// Serialized scrollback, or `None` while the alternate screen is active.
    pub(crate) fn snapshot(&self, max_lines: usize) -> Option<Vec<u8>> {
        snapshot::serialize(&*self.0.term.lock(), max_lines)
    }
}

/// Keeps output from before a shell started in sight on Windows.
///
/// ConPTY clears the screen when a shell starts and from then on draws the
/// whole screen itself, from the top, so the earlier output (a restored
/// session, a previous shell's) can only be kept in the history (see
/// [`TerminalHandle::make_room_for_conpty`]). The new shell's first prompt
/// would then sit at the top of an empty screen. Instead, while the new
/// output has not reached the bottom row, the view stays scrolled back just
/// far enough that it ends at the cursor's row, with the earlier output right
/// above, as if the screen had never been cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pin {
    Off,
    /// `follow`: the view stays with the newest output (the user has not
    /// scrolled further back). `cleared`: ConPTY's clear of its first frame
    /// has come by; the next clear is a program's.
    On {
        follow: bool,
        cleared: bool,
    },
    /// A program cleared the screen: the view goes back to the live screen.
    Ending {
        follow: bool,
    },
}

impl Pin {
    fn starting() -> Pin {
        Pin::On { follow: true, cleared: false }
    }

    /// The output cleared the screen or reset the terminal.
    fn screen_cleared(&mut self) {
        *self = match *self {
            Pin::On { follow, cleared: false } => Pin::On { follow, cleared: true },
            Pin::On { follow, cleared: true } => Pin::Ending { follow },
            other => other,
        };
    }
}

/// Moves the view to where the pin keeps it. `user`: the user just moved the
/// view, which decides whether it follows the output from now on.
fn apply_pin<T: alacritty_terminal::event::EventListener>(term: &mut Term<T>, pin: &mut Pin, user: bool) {
    let follow = match *pin {
        Pin::Off => return,
        Pin::Ending { follow } => {
            if follow {
                term.scroll_display(Scroll::Bottom);
            }
            *pin = Pin::Off;
            return;
        }
        // Full-screen programs draw on a screen of their own.
        Pin::On { .. } if term.mode().contains(TermMode::ALT_SCREEN) => return,
        Pin::On { follow, .. } => follow,
    };
    let grid = term.grid();
    // How far back the view must be for its last row to be the cursor's.
    let below = term.screen_lines().saturating_sub(1).saturating_sub(grid.cursor.point.line.0.max(0) as usize);
    let floor = below.min(grid.history_size());
    let offset = grid.display_offset();
    let follow = if user { offset <= floor } else { follow };
    if floor == 0 {
        // The output reached the bottom row: the plain view shows it all.
        if follow {
            term.scroll_display(Scroll::Bottom);
        }
        *pin = Pin::Off;
        return;
    }
    if follow || offset < floor {
        term.scroll_display(Scroll::Delta(floor as i32 - offset as i32));
    }
    if let Pin::On { follow: f, .. } = pin {
        *f = follow;
    }
}

/// Resizes the way ConPTY resizes its screen, which it then redraws from the
/// top: when the screen gets taller, the content stays where it is and the
/// new rows are added below. (alacritty instead brings lines down from the
/// history to fill them, which ConPTY then draws over, so they were lost.)
fn resize_like_conpty<T: alacritty_terminal::event::EventListener>(term: &mut Term<T>, size: GridSize) {
    let rows = size.rows as usize;
    if rows > term.screen_lines() && !term.mode().contains(TermMode::ALT_SCREEN) {
        // The rows first, at the current width, to see how many lines came down.
        let line = term.grid().cursor.point.line;
        term.resize(GridSize { cols: term.columns() as u16, ..size });
        let pulled = (term.grid().cursor.point.line.0 - line.0).max(0) as usize;
        if pulled > 0 {
            let grid = term.grid_mut();
            grid.scroll_up(&(Line(0)..Line(rows as i32)), pulled);
            grid.cursor.point.line -= pulled;
            grid.saved_cursor.point.line = Line((grid.saved_cursor.point.line.0 - pulled as i32).max(0));
            term.selection = None;
        }
    }
    term.resize(size);
}

/// The program as the start of a Windows command line. Unquoted, a path like
/// `C:\Program Files\Git\bin\bash.exe` is split at the space and Windows
/// would first look for `C:\Program.exe`.
#[cfg_attr(not(windows), allow(dead_code))]
fn quote_program(program: &str) -> String {
    if program.contains(char::is_whitespace) && !program.starts_with('"') {
        format!("\"{program}\"")
    } else {
        program.to_owned()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quote_program() {
        use super::quote_program;
        assert_eq!(quote_program(r"C:\Program Files\Git\bin\bash.exe"), r#""C:\Program Files\Git\bin\bash.exe""#);
        assert_eq!(quote_program("pwsh.exe"), "pwsh.exe");
        assert_eq!(quote_program(r#""C:\a b\c.exe""#), r#""C:\a b\c.exe""#);
    }

    mod conpty {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::grid::{Dimensions, Scroll};
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::{self, Term};
        use alacritty_terminal::vte::ansi::Processor;

        use super::super::{GridSize, Pin, apply_pin, resize_like_conpty};

        fn term(cols: u16, rows: u16) -> Term<VoidListener> {
            let size = GridSize { cols, rows, cell_width: 1, cell_height: 1 };
            Term::new(term::Config::default(), &size, VoidListener)
        }

        fn feed(t: &mut Term<VoidListener>, s: &str) {
            let mut p: Processor = Processor::new();
            p.advance(t, s.as_bytes());
        }

        fn row(t: &Term<VoidListener>, line: i32) -> String {
            let g = t.grid();
            (0..g.columns()).map(|c| g[Line(line)][Column(c)].c).collect::<String>().trim_end().to_owned()
        }

        /// The rows on screen, scrolled back as the view is.
        fn view(t: &Term<VoidListener>) -> Vec<String> {
            let offset = t.grid().display_offset() as i32;
            (0..t.screen_lines() as i32).map(|r| row(t, r - offset)).collect()
        }

        /// A restored session as a shell on Windows starts: the old output
        /// is moved into the history, then ConPTY clears and draws its prompt.
        fn restored() -> (Term<VoidListener>, Pin) {
            let mut t = term(20, 6);
            for i in 1..=8 {
                feed(&mut t, &format!("old-{i}\r\n"));
            }
            feed(&mut t, "-- restored --\r\n");
            feed(&mut t, "\x1b[0m\x1b[2J\x1b[H");
            let mut pin = Pin::starting();
            pin.screen_cleared();
            feed(&mut t, "\x1b[2J\x1b[H\r\n$ ");
            apply_pin(&mut t, &mut pin, false);
            (t, pin)
        }

        #[test]
        fn earlier_output_stays_above_the_new_prompt() {
            let (mut t, mut pin) = restored();
            assert_eq!(view(&t), ["old-6", "old-7", "old-8", "-- restored --", "", "$"]);

            // Typing jumps to the newest output, which is the pinned view.
            t.scroll_display(Scroll::Bottom);
            apply_pin(&mut t, &mut pin, true);
            assert_eq!(view(&t)[5], "$");

            // Output follows on, pulling the earlier lines up.
            feed(&mut t, "ls\r\na b\r\n$ ");
            apply_pin(&mut t, &mut pin, false);
            assert_eq!(view(&t), ["old-8", "-- restored --", "", "$ ls", "a b", "$"]);
            assert!(matches!(pin, Pin::On { .. }));

            // Once it reaches the bottom row, the view is a plain one again.
            feed(&mut t, "x\r\ny\r\nz\r\n$ ");
            apply_pin(&mut t, &mut pin, false);
            assert_eq!(pin, Pin::Off);
            assert_eq!(t.grid().display_offset(), 0);
            assert_eq!(view(&t)[5], "$");
            assert_eq!(view(&t)[0], "$ ls");
        }

        #[test]
        fn a_program_clearing_the_screen_ends_the_pin() {
            let (mut t, mut pin) = restored();
            pin.screen_cleared();
            feed(&mut t, "\x1b[H\x1b[2J$ ");
            apply_pin(&mut t, &mut pin, false);
            assert_eq!(pin, Pin::Off);
            assert_eq!(t.grid().display_offset(), 0);
            assert_eq!(view(&t)[0], "$");
        }

        #[test]
        fn scrolling_back_further_stops_following() {
            let (mut t, mut pin) = restored();
            t.scroll_display(Scroll::Delta(2));
            apply_pin(&mut t, &mut pin, true);
            assert_eq!(view(&t)[0], "old-4");
            feed(&mut t, "echo\r\n$ ");
            apply_pin(&mut t, &mut pin, false);
            assert_eq!(view(&t)[0], "old-4", "the view stays where the user put it");

            // Scrolling down can't go past the pinned view.
            t.scroll_display(Scroll::Bottom);
            apply_pin(&mut t, &mut pin, true);
            assert_eq!(view(&t)[5], "$");
            assert_eq!(view(&t)[0], "old-7");
        }

        #[test]
        fn growing_keeps_the_history() {
            let mut t = term(10, 5);
            feed(&mut t, &(1..=12).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\r\n"));
            assert_eq!(t.grid().history_size(), 7);

            resize_like_conpty(&mut t, GridSize { cols: 12, rows: 8, cell_width: 1, cell_height: 1 });
            assert_eq!(t.grid().history_size(), 7);
            assert_eq!(t.grid().cursor.point.line, Line(4));
            assert_eq!(view(&t), ["l8", "l9", "l10", "l11", "l12", "", "", ""]);
            assert_eq!(row(&t, -1), "l7");

            // Shrinking works as before: the top lines go into the history.
            resize_like_conpty(&mut t, GridSize { cols: 12, rows: 3, cell_width: 1, cell_height: 1 });
            assert_eq!(view(&t), ["l10", "l11", "l12"]);
            assert_eq!(t.grid().history_size(), 9);
        }
    }
}
