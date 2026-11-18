//! The helper process that keeps one terminal running while TerminalRooster
//! is closed. It owns the PTY and the program in it, keeps a copy of the
//! screen up to date, and lets one client at a time show the terminal:
//! TerminalRooster, or a console window of its own.
//!
//! A client that attaches gets the whole screen, then the program's output
//! as it comes. When the program ends, the helper waits for a client to tell
//! (so what the program printed last isn't lost), then ends too.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as AEvent, EventListener, OnResize};
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite, Pty};
use alacritty_terminal::vte::ansi::{Processor, Rgb as ARgb};
use polling::{Event as PollEvent, Events, PollMode, Poller};

use super::proto::{self, ConnInfo, Exited, Hello, Refusal, Spec, Welcome};
use crate::session::render::Palette;
use crate::session::{GridSize, snapshot};

/// How long a helper whose program ended waits for a client to show it to.
const LINGER: Duration = Duration::from_secs(24 * 60 * 60);
/// How long a client has to say hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a new helper waits for ConPTY to clear the screen as it starts.
const STARTUP_CLEAR_WAIT: Duration = Duration::from_secs(2);

/// Runs a helper: reads its [`Spec`] from standard input, starts the program
/// and serves clients until the program has ended and a client was told, or
/// a client ended it. Returns the process exit code.
pub fn serve() -> i32 {
    let mut input = Vec::new();
    if let Err(e) = io::stdin().read_to_end(&mut input) {
        tracing::error!(error = %e, "could not read the terminal to run");
        return 2;
    }
    let spec: Spec = match serde_json::from_slice(&input) {
        Ok(spec) => spec,
        Err(e) => {
            tracing::error!(error = %e, "could not read the terminal to run");
            return 2;
        }
    };
    let conn_file = spec.conn_file.clone();
    let code = match run(spec) {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!(error = %e, "terminal helper failed");
            1
        }
    };
    proto::remove_conn_if_mine(&conn_file);
    code
}

/// What client threads ask of the thread that owns the PTY.
enum Command {
    Input(Vec<u8>),
    Resize(GridSize),
    End,
}

/// What goes to a client's writer thread.
enum Out {
    Frame(u8, Vec<u8>),
    /// Close the connection after the frames before this.
    Close,
}

/// A connected client.
struct Client {
    id: u64,
    tx: Sender<Out>,
    /// Stays connected, paused, while another client shows the terminal
    /// (TerminalRooster does; a console window closes instead).
    stay: bool,
    /// The size of its view, for when it shows the terminal (again).
    size: GridSize,
}

impl Client {
    fn send(&self, kind: u8, payload: Vec<u8>) {
        let _ = self.tx.send(Out::Frame(kind, payload));
    }

    fn close(&self) {
        let _ = self.tx.send(Out::Close);
    }
}

#[derive(Default)]
struct Clients {
    /// The client showing the terminal; its input goes to the program.
    active: Option<Client>,
    /// Paused clients, the most recent last. The last takes over when the
    /// active one leaves.
    waiting: Vec<Client>,
}

/// State the PTY thread, the client threads and the term's listener share.
struct Hub {
    /// Locked before `clients`.
    term: Mutex<Term<Listener>>,
    clients: Mutex<Clients>,
    shared: Arc<Shared>,
    commands: Sender<Command>,
    poller: Arc<Poller>,
    token: String,
    shell_pid: u32,
    /// Set (with `term` and `client` locked) once the program ended.
    exited: Mutex<Option<Option<i32>>>,
    /// Gets a message once a client has been sent that the program ended.
    told: Sender<()>,
    next_client: AtomicU64,
}

/// What the term's listener needs.
struct Shared {
    /// A client is attached; it answers the program's queries then.
    attached: AtomicBool,
    commands: Sender<Command>,
    poller: Arc<Poller>,
    title: Mutex<Option<String>>,
    size: Mutex<GridSize>,
}

impl Shared {
    fn reply(&self, bytes: Vec<u8>) {
        // An attached client's terminal answers for itself.
        if !self.attached.load(Ordering::Acquire) {
            let _ = self.commands.send(Command::Input(bytes));
            let _ = self.poller.notify();
        }
    }
}

#[derive(Clone)]
struct Listener(Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: AEvent) {
        let s = &self.0;
        match event {
            AEvent::Title(t) => *s.title.lock().unwrap() = Some(t),
            AEvent::ResetTitle => *s.title.lock().unwrap() = None,
            AEvent::PtyWrite(text) => s.reply(text.into_bytes()),
            AEvent::ColorRequest(index, fmt) => {
                let c = Palette::default().get(index);
                s.reply(fmt(ARgb { r: c.r, g: c.g, b: c.b }).into_bytes());
            }
            AEvent::TextAreaSizeRequest(fmt) => {
                let size = *s.size.lock().unwrap();
                s.reply(fmt(size.window_size()).into_bytes());
            }
            _ => {}
        }
    }
}

fn run(spec: Spec) -> io::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let poller = Arc::new(Poller::new()?);
    let (commands, command_rx) = mpsc::channel();
    let shared = Arc::new(Shared {
        attached: AtomicBool::new(false),
        commands: commands.clone(),
        poller: poller.clone(),
        title: Mutex::new(None),
        size: Mutex::new(spec.size),
    });
    let config = term::Config { scrolling_history: spec.scrollback_lines, ..Default::default() };
    let mut term = Term::new(config, &spec.size, Listener(shared.clone()));
    if let Some(preload) = &spec.preload {
        snapshot::replay(&mut term, preload.as_bytes());
        // ConPTY clears the screen as it starts: the earlier output goes
        // into the history first (as `make_room_for_conpty` does).
        if cfg!(windows) {
            snapshot::replay(&mut term, b"\x1b[0m\x1b[2J\x1b[H");
        }
    }

    #[cfg(windows)]
    let program = crate::session::quote_program(&spec.program);
    #[cfg(not(windows))]
    let program = spec.program.clone();
    #[allow(clippy::needless_update)]
    let options = tty::Options {
        shell: Some(tty::Shell::new(program, spec.args.clone())),
        working_directory: Some(spec.cwd.clone()),
        drain_on_exit: true,
        env: spec.env.clone(),
        #[cfg(windows)]
        escape_args: true,
        ..Default::default()
    };
    let mut pty = tty::new(&options, spec.size.window_size(), spec.pane.0 as u64)?;
    let shell = ShellProcess::of(&pty);
    tracing::info!(pane = %spec.pane, pid = shell.pid, port, program = %spec.program, "terminal helper started");

    let (told, told_rx) = mpsc::channel();
    let hub = Arc::new(Hub {
        term: Mutex::new(term),
        clients: Mutex::new(Clients::default()),
        shared,
        commands,
        poller: poller.clone(),
        token: spec.token.clone(),
        shell_pid: shell.pid,
        exited: Mutex::new(None),
        told,
        next_client: AtomicU64::new(1),
    });
    let accepting = hub.clone();
    std::thread::Builder::new().name("host-accept".into()).spawn(move || accept(listener, &accepting))?;

    let info = ConnInfo { version: proto::VERSION, pid: std::process::id(), port, token: spec.token.clone() };
    let conn_file = spec.conn_file.clone();
    let mut ready = Some(move || {
        proto::write_conn(&conn_file, &info)?;
        // Whoever started the helper waits for this line before connecting.
        // On a line of its own even after another's unfinished one (a test
        // harness's `test name ... `).
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\nready\n").and_then(|()| stdout.flush());
        Ok(())
    });
    match pump(&mut pty, &hub, &command_rx, &poller, &mut ready)? {
        Ending::Ended => {
            tracing::info!(pane = %spec.pane, "terminal ended by its client");
            // No client may attach to an ending terminal.
            proto::remove_conn_if_mine(&spec.conn_file);
            drop(pty);
            shell.end_if_running();
        }
        Ending::Exited(code) => {
            tracing::info!(pane = %spec.pane, ?code, "program exited");
            drop(pty);
            let attached = finish(&hub, code);
            // Until a client was told, so what it printed last is seen.
            let wait = if attached { Duration::from_secs(5) } else { LINGER };
            if told_rx.recv_timeout(wait).is_err() {
                tracing::info!(pane = %spec.pane, "no client came to see the program's end");
            }
        }
    }
    Ok(())
}

/// Records the program's end and tells the clients, which are let go.
/// Returns whether there were any.
fn finish(hub: &Hub, code: Option<i32>) -> bool {
    let _term = hub.term.lock().unwrap();
    let mut clients = hub.clients.lock().unwrap();
    *hub.exited.lock().unwrap() = Some(code);
    hub.shared.attached.store(false, Ordering::Release);
    let all: Vec<Client> = clients.active.take().into_iter().chain(clients.waiting.drain(..)).collect();
    for c in &all {
        tell_exit(c, code);
    }
    !all.is_empty()
}

/// Sends the client that the program ended; its writer then tells `told`.
fn tell_exit(client: &Client, code: Option<i32>) {
    client.send(proto::EXITED, serde_json::to_vec(&Exited { code }).unwrap_or_default());
    client.close();
}

/// Makes `client` the one showing the terminal: sends it the whole screen,
/// at the size of its view. The program's output follows.
fn show(hub: &Hub, term: &mut Term<Listener>, clients: &mut Clients, client: Client) {
    *hub.shared.size.lock().unwrap() = client.size;
    resize_term(term, client.size);
    // The PTY follows (it repaints then).
    let _ = hub.commands.send(Command::Resize(client.size));
    let _ = hub.poller.notify();
    let welcome =
        Welcome { version: proto::VERSION, shell_pid: hub.shell_pid, title: hub.shared.title.lock().unwrap().clone() };
    client.send(proto::WELCOME, serde_json::to_vec(&welcome).unwrap_or_default());
    client.send(proto::OUTPUT, snapshot::serialize_state(term));
    tracing::info!(client = client.id, "client shows the terminal");
    clients.active = Some(client);
    hub.shared.attached.store(true, Ordering::Release);
}

/// The client that showed the terminal makes way for another: it waits, or
/// is let go.
fn step_aside(clients: &mut Clients, client: Client) {
    if client.stay {
        client.send(proto::PAUSED, Vec::new());
        clients.waiting.push(client);
    } else {
        client.send(proto::KICKED, Vec::new());
        client.close();
    }
}

enum Ending {
    /// A client asked to end the program.
    Ended,
    /// The program exited by itself, with this code.
    Exited(Option<i32>),
}

/// Moves bytes between the PTY and the screen copy (and the attached client)
/// until the program ends. Calls `ready` once clients may attach: on
/// Windows after ConPTY's clear of the screen as it starts, so a client
/// never sees it (see `Pin`).
fn pump(
    pty: &mut Pty,
    hub: &Hub,
    commands: &Receiver<Command>,
    poller: &Arc<Poller>,
    ready: &mut Option<impl FnOnce() -> io::Result<()>>,
) -> io::Result<Ending> {
    let mut parser: Processor = Processor::new();
    let mut interest = PollEvent::readable(0);
    // SAFETY: the PTY is deregistered before it is dropped.
    unsafe { pty.register(poller, interest, PollMode::Level)? };
    let mut events = Events::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut pending: VecDeque<u8> = VecDeque::new();
    // Without a clear by then, it isn't coming.
    let ready_by = Instant::now() + if cfg!(windows) { STARTUP_CLEAR_WAIT } else { Duration::ZERO };
    let mut cleared = false;
    let ending = loop {
        if ready.is_some() && (cleared || Instant::now() >= ready_by) {
            ready.take().expect("checked")()?;
        }
        // A synchronized update (DEC mode 2026) shows at the latest by then.
        let deadline = parser.sync_timeout().sync_timeout();
        let wake = if ready.is_some() { Some(deadline.map_or(ready_by, |d| d.min(ready_by))) } else { deadline };
        events.clear();
        if let Err(e) = poller.wait(&mut events, wake.map(|d| d.saturating_duration_since(Instant::now()))) {
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break Err(e);
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            parser.stop_sync(&mut *hub.term.lock().unwrap());
        }
        let mut end = false;
        while let Ok(cmd) = commands.try_recv() {
            match cmd {
                Command::Input(bytes) => pending.extend(bytes),
                Command::Resize(size) => resize(pty, hub, size),
                Command::End => end = true,
            }
        }
        if end {
            break Ok(Ending::Ended);
        }
        match read_output(pty, hub, &mut parser, &mut buf) {
            Ok(clear) => cleared |= clear,
            Err(e) => break Err(e),
        }
        if let Err(e) = write_input(pty, &mut pending) {
            break Err(e);
        }
        if let Some(ChildEvent::Exited(status)) = pty.next_child_event() {
            // What it printed last.
            let _ = read_output(pty, hub, &mut parser, &mut buf);
            break Ok(Ending::Exited(status.and_then(|s| s.code())));
        }
        let writable = !pending.is_empty();
        if writable != interest.writable {
            interest.writable = writable;
            if let Err(e) = pty.reregister(poller, interest, PollMode::Level) {
                break Err(e);
            }
        }
    };
    let _ = pty.deregister(poller);
    ending
}

/// Reads what the program wrote, into the screen copy and to the client.
/// Returns whether that cleared the screen.
fn read_output(pty: &mut Pty, hub: &Hub, parser: &mut Processor, buf: &mut [u8]) -> io::Result<bool> {
    let mut cleared = false;
    loop {
        let n = match pty.reader().read(buf) {
            // Nothing more for now (Windows), or the end (the exit follows).
            Ok(0) => return Ok(cleared),
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => return Ok(cleared),
            // Linux reports a hung-up PTY so; the exit follows.
            #[cfg(target_os = "linux")]
            Err(e) if e.raw_os_error() == Some(5) => return Ok(cleared),
            Err(e) => return Err(e),
        };
        cleared |= buf[..n].windows(4).any(|w| w == b"\x1b[2J");
        // Both locked: a client attaching gets either the screen before
        // these bytes and then the bytes, or the screen after them.
        let mut term = hub.term.lock().unwrap();
        parser.advance(&mut *term, &buf[..n]);
        if let Some(c) = &hub.clients.lock().unwrap().active {
            c.send(proto::OUTPUT, buf[..n].to_vec());
        }
    }
}

fn write_input(pty: &mut Pty, pending: &mut VecDeque<u8>) -> io::Result<()> {
    while !pending.is_empty() {
        let (front, _) = pending.as_slices();
        match pty.writer().write(front) {
            Ok(0) => break,
            Ok(n) => {
                pending.drain(..n);
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn resize(pty: &mut Pty, hub: &Hub, size: GridSize) {
    let size = GridSize { cols: size.cols.max(2), rows: size.rows.max(1), ..size };
    *hub.shared.size.lock().unwrap() = size;
    resize_term(&mut hub.term.lock().unwrap(), size);
    pty.on_resize(size.window_size());
}

fn resize_term(term: &mut Term<Listener>, size: GridSize) {
    use alacritty_terminal::grid::Dimensions;
    if term.columns() == size.cols as usize && term.screen_lines() == size.rows as usize {
        return;
    }
    if cfg!(windows) {
        crate::session::resize_like_conpty(term, size);
    } else {
        term.resize(size);
    }
}

/// Takes clients as they connect, each on a thread of its own.
fn accept(listener: TcpListener, hub: &Arc<Hub>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let hub = hub.clone();
        let spawned = std::thread::Builder::new().name("host-client".into()).spawn(move || {
            if let Err(e) = serve_client(stream, &hub) {
                tracing::debug!(error = %e, "client connection ended");
            }
        });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not take a client");
        }
    }
}

fn serve_client(stream: TcpStream, hub: &Hub) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let mut writer = stream.try_clone()?;
    // Set and cleared on the copy that is read: on Windows, clearing it on
    // another copy left it on this one, which then timed out when idle.
    stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
    let mut reader = proto::FrameReader::new(stream);
    let hello = match reader.next()? {
        Some(f) if f.kind == proto::HELLO => proto::parse::<Hello>(&f.payload)?,
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "no hello")),
    };
    let refusal = if !proto::token_matches(&hello.token, &hub.token) {
        Some(Refusal::Token)
    } else if hello.version != proto::VERSION {
        Some(Refusal::Version)
    } else {
        None
    };
    if let Some(refusal) = refusal {
        tracing::warn!(?refusal, "refused a client");
        proto::write_json(&mut writer, proto::REFUSED, &refusal)?;
        return Ok(());
    }
    reader.get_ref().set_read_timeout(None)?;

    let id = hub.next_client.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    let told = hub.told.clone();
    std::thread::Builder::new().name("host-writer".into()).spawn(move || write_out(writer, &rx, &told))?;
    let client = Client { id, tx, stay: hello.stay, size: hello.size };
    if !attach(hub, client, hello.steal) {
        return Ok(());
    }
    loop {
        let frame = match reader.next() {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(error = %e, "client connection lost");
                break;
            }
        };
        let cmd = match frame.kind {
            proto::INPUT if is_active(hub, id) => Command::Input(frame.payload),
            proto::RESIZE => {
                let size = proto::parse(&frame.payload)?;
                if !resized(hub, id, size) {
                    continue;
                }
                Command::Resize(size)
            }
            proto::TAKE => {
                take(hub, id);
                continue;
            }
            proto::END => Command::End,
            _ => continue,
        };
        let _ = hub.commands.send(cmd);
        let _ = hub.poller.notify();
    }
    leave(hub, id);
    Ok(())
}

fn is_active(hub: &Hub, id: u64) -> bool {
    hub.clients.lock().unwrap().active.as_ref().is_some_and(|c| c.id == id)
}

/// Records a client's new size. Returns whether it shows the terminal (whose
/// size then changes too).
fn resized(hub: &Hub, id: u64, size: GridSize) -> bool {
    let mut clients = hub.clients.lock().unwrap();
    let Clients { active, waiting } = &mut *clients;
    match active.iter_mut().chain(waiting.iter_mut()).find(|c| c.id == id) {
        Some(c) => {
            c.size = size;
            active.as_ref().is_some_and(|c| c.id == id)
        }
        None => false,
    }
}

/// A new client: it shows the terminal, unless another does and it may not
/// take over; it then waits (or is refused, if it doesn't stay). Returns
/// false if the connection is closing: refused, or the program already ended
/// (it is told so).
fn attach(hub: &Hub, client: Client, steal: bool) -> bool {
    let mut term = hub.term.lock().unwrap();
    let mut clients = hub.clients.lock().unwrap();
    if let Some(code) = *hub.exited.lock().unwrap() {
        // What the program left on the screen, and that it ended.
        let welcome = Welcome { version: proto::VERSION, shell_pid: hub.shell_pid, title: None };
        client.send(proto::WELCOME, serde_json::to_vec(&welcome).unwrap_or_default());
        client.send(proto::OUTPUT, snapshot::serialize_state(&term));
        tell_exit(&client, code);
        return false;
    }
    match clients.active.take() {
        Some(active) if !steal => {
            clients.active = Some(active);
            if client.stay {
                tracing::info!(client = client.id, "client waits for its turn");
                client.send(proto::PAUSED, Vec::new());
                clients.waiting.push(client);
                return true;
            }
            client.send(proto::REFUSED, serde_json::to_vec(&Refusal::Busy).unwrap_or_default());
            client.close();
            false
        }
        Some(active) => {
            tracing::info!(old = active.id, new = client.id, "another client took over");
            step_aside(&mut clients, active);
            show(hub, &mut term, &mut clients, client);
            true
        }
        None => {
            show(hub, &mut term, &mut clients, client);
            true
        }
    }
}

/// A waiting client takes over showing the terminal.
fn take(hub: &Hub, id: u64) {
    let mut term = hub.term.lock().unwrap();
    let mut clients = hub.clients.lock().unwrap();
    let Some(i) = clients.waiting.iter().position(|c| c.id == id) else { return };
    let client = clients.waiting.remove(i);
    if let Some(active) = clients.active.take() {
        tracing::info!(old = active.id, new = id, "a waiting client took over");
        step_aside(&mut clients, active);
    }
    show(hub, &mut term, &mut clients, client);
}

/// A client disconnected. If it showed the terminal, the most recent waiting
/// client shows it now.
fn leave(hub: &Hub, id: u64) {
    let mut term = hub.term.lock().unwrap();
    let mut clients = hub.clients.lock().unwrap();
    clients.waiting.retain(|c| c.id != id);
    if clients.active.as_ref().is_some_and(|c| c.id == id) {
        clients.active = None;
        hub.shared.attached.store(false, Ordering::Release);
        tracing::info!(client = id, "client detached");
        if let Some(next) = clients.waiting.pop() {
            show(hub, &mut term, &mut clients, next);
        }
    }
}

/// Sends a client its frames until told to close or the connection breaks.
/// Tells `told` if one of them said that the program ended.
fn write_out(mut stream: TcpStream, rx: &Receiver<Out>, told: &Sender<()>) {
    let mut exit_sent = false;
    for out in rx {
        match out {
            Out::Frame(kind, payload) => {
                exit_sent |= kind == proto::EXITED;
                if proto::write_frame(&mut stream, kind, &payload).is_err() {
                    break;
                }
            }
            Out::Close => break,
        }
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
    if exit_sent {
        let _ = told.send(());
    }
}

/// The program's process, to end it should closing its PTY not.
struct ShellProcess {
    pid: u32,
    #[cfg(windows)]
    handle: Option<std::os::windows::io::OwnedHandle>,
}

impl ShellProcess {
    #[cfg(windows)]
    fn of(pty: &Pty) -> Self {
        use std::os::windows::io::{BorrowedHandle, RawHandle};
        let watcher = pty.child_watcher();
        let pid = watcher.pid().map_or(0, |p| p.get());
        // SAFETY: the handle is valid while the PTY is; the copy outlives it.
        let handle = unsafe { BorrowedHandle::borrow_raw(watcher.raw_handle() as RawHandle) };
        Self { pid, handle: handle.try_clone_to_owned().ok() }
    }

    #[cfg(not(windows))]
    fn of(pty: &Pty) -> Self {
        Self { pid: pty.child().id() }
    }

    /// On Windows a console closed early leaves its program running (see
    /// `tap::EndShell`). Elsewhere closing the PTY hangs the program up.
    fn end_if_running(&self) {
        #[cfg(windows)]
        if let Some(handle) = &self.handle
            && crate::detect::end_unless_exited(self.pid, handle, Duration::from_secs(5))
        {
            tracing::info!(pid = self.pid, "ended a program that outlived its terminal");
        }
    }
}
