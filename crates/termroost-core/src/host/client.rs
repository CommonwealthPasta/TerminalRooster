//! TerminalRooster's side of a helper: starting one, connecting to it, and
//! sending it the user's input.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use super::proto::{self, ConnInfo, FrameReader, Hello, Refusal, Spec, Welcome};
use crate::session::GridSize;

/// How long a new helper gets to start its program and listen.
const START_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a helper gets to answer a client's hello.
const WELCOME_TIMEOUT: Duration = Duration::from_secs(5);

/// How to start a helper process: TerminalRooster's own executable with a
/// `host` argument, say. The helper reads its [`Spec`] from standard input.
#[derive(Clone, Debug)]
pub struct HostCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Starts a helper that runs `spec` and returns how to reach it, once it
/// listens. The helper is not a child that ends with this process: it runs
/// on when TerminalRooster quits. Blocks until it is ready.
pub(crate) fn start(cmd: &HostCommand, spec: &Spec, cwd: &Path) -> io::Result<ConnInfo> {
    let mut command = Command::new(&cmd.program);
    command
        .args(&cmd.args)
        .envs(cmd.env.iter().map(|(k, v)| (k, v)))
        // Not the terminal's folder: the helper would keep it from being
        // deleted after the terminal moved on.
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = spawn_detached(&mut command)?;
    let input = serde_json::to_vec(spec)?;
    let mut stdin = child.stdin.take().ok_or_else(|| io::Error::other("no stdin"))?;
    stdin.write_all(&input)?;
    drop(stdin);

    // Waits for its "ready" line on another thread, so a stuck helper
    // can't hold this one forever.
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other("no stdout"))?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Lines before it are someone else's (a test harness's, say).
        let ready = BufReader::new(stdout).lines().any(|line| line.is_ok_and(|l| l.trim() == "ready"));
        let _ = tx.send(ready);
    });
    let ready = rx.recv_timeout(START_TIMEOUT);
    #[cfg(unix)]
    // Reaps it when it ends, so it doesn't linger as a zombie meanwhile.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    match ready {
        Ok(true) => {}
        Ok(false) => return Err(io::Error::other("the helper ended as it started (see its log)")),
        Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "the helper didn't start")),
    }
    proto::read_conn(&spec.conn_file).ok_or_else(|| io::Error::other("the helper didn't say where it listens"))
}

#[cfg(windows)]
fn spawn_detached(command: &mut Command) -> io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    // No console of its own, out of TerminalRooster's Ctrl+C group, and out
    // of any job that ends its processes with it (when allowed to leave).
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
    match command.spawn() {
        Err(e) if e.raw_os_error() == Some(5) => {
            tracing::warn!(
                "may not leave the job TerminalRooster runs in; the terminal ends if that job ends its processes"
            );
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            command.spawn()
        }
        r => r,
    }
}

#[cfg(unix)]
fn spawn_detached(command: &mut Command) -> io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    // Out of TerminalRooster's process group, so signals to it pass by.
    command.process_group(0);
    command.spawn()
}

/// A connection to a helper, attached.
pub(crate) struct Connection {
    /// `None` while another client shows the terminal: this one waits
    /// (a `WELCOME` comes when its turn does).
    pub welcome: Option<Welcome>,
    pub reader: FrameReader<TcpStream>,
    pub writer: TcpStream,
}

pub(crate) enum Connected {
    Attached(Connection),
    Refused(Refusal),
}

/// Connects to a helper and attaches, at `size`. `steal` and `stay`: see
/// [`Hello`].
pub(crate) fn connect(info: &ConnInfo, steal: bool, stay: bool, size: GridSize) -> io::Result<Connected> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let stream = TcpStream::connect_timeout(&addr, WELCOME_TIMEOUT)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(WELCOME_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let hello = Hello { token: info.token.clone(), version: proto::VERSION, steal, stay, size };
    proto::write_json(&mut writer, proto::HELLO, &hello)?;
    let mut reader = FrameReader::new(stream);
    let welcome = match reader.next()? {
        Some(f) if f.kind == proto::WELCOME => Some(proto::parse(&f.payload)?),
        Some(f) if f.kind == proto::PAUSED => None,
        Some(f) if f.kind == proto::REFUSED => return Ok(Connected::Refused(proto::parse(&f.payload)?)),
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "not a terminal helper")),
    };
    reader.get_ref().set_read_timeout(None)?;
    Ok(Connected::Attached(Connection { welcome, reader, writer }))
}

/// Asks the helper of `info` to end its program, without attaching.
pub(crate) fn end(info: &ConnInfo) -> io::Result<()> {
    let size = GridSize::default();
    match connect(info, true, false, size)? {
        Connected::Attached(mut c) => {
            proto::write_frame(&mut c.writer, proto::END, &[])?;
            let _ = c.writer.shutdown(Shutdown::Write);
            Ok(())
        }
        Connected::Refused(r) => Err(io::Error::other(format!("refused: {r:?}"))),
    }
}

enum LinkMsg {
    Frame(u8, Vec<u8>),
    /// Disconnect (the helper runs on).
    Detach,
}

/// Sends to a helper. What is sent before the connection is made waits for
/// it; what is sent after it broke is dropped.
#[derive(Clone)]
pub(crate) struct HostLink {
    tx: Sender<LinkMsg>,
    /// Set once the connection is left on purpose: its end is no news.
    detached: Arc<AtomicBool>,
}

impl HostLink {
    /// A link, and what its writer takes the messages from once connected.
    pub fn new() -> (HostLink, LinkQueue) {
        let (tx, rx) = mpsc::channel();
        (HostLink { tx, detached: Arc::new(AtomicBool::new(false)) }, LinkQueue(rx))
    }

    pub fn input(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(LinkMsg::Frame(proto::INPUT, bytes));
    }

    pub fn resize(&self, size: GridSize) {
        if let Ok(json) = serde_json::to_vec(&size) {
            let _ = self.tx.send(LinkMsg::Frame(proto::RESIZE, json));
        }
    }

    /// Shows the terminal here again, in place of the client showing it now.
    pub fn take(&self) {
        let _ = self.tx.send(LinkMsg::Frame(proto::TAKE, Vec::new()));
    }

    /// Ends the program, and with it the helper.
    pub fn end(&self) {
        self.detached.store(true, Ordering::Release);
        let _ = self.tx.send(LinkMsg::Frame(proto::END, Vec::new()));
        let _ = self.tx.send(LinkMsg::Detach);
    }

    /// Leaves the helper running without this client.
    pub fn detach(&self) {
        self.detached.store(true, Ordering::Release);
        let _ = self.tx.send(LinkMsg::Detach);
    }

    pub fn is_detached(&self) -> bool {
        self.detached.load(Ordering::Acquire)
    }
}

/// The messages of a [`HostLink`], for its writer thread.
pub(crate) struct LinkQueue(Receiver<LinkMsg>);

impl LinkQueue {
    /// Sends the link's messages over `stream` until it detaches, the link
    /// is dropped or the connection breaks. Run it on a thread of its own.
    pub fn run(self, mut stream: TcpStream) {
        for msg in self.0 {
            match msg {
                LinkMsg::Frame(kind, payload) => {
                    if let Err(e) = proto::write_frame(&mut stream, kind, &payload) {
                        tracing::debug!(error = %e, "helper connection broke");
                        break;
                    }
                }
                LinkMsg::Detach => break,
            }
        }
        let _ = stream.shutdown(Shutdown::Both);
    }
}
