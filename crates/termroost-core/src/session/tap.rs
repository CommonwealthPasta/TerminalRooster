//! A PTY wrapper that lets the core see the program's output on its way to
//! alacritty's parser, which ignores the sequences notifications rely on
//! (see [`super::osc`]).

use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite, Pty};
use polling::{Event as PollEvent, PollMode, Poller};

use super::osc::{OscScanner, Signal};
use super::terminal::Shared;

pub(crate) struct TapPty {
    inner: Pty,
    scanner: OscScanner,
    signals: Vec<Signal>,
    shared: Arc<Shared>,
    /// Declared after `inner`, so it drops once the PTY is closed.
    _end: EndShell,
}

impl TapPty {
    pub fn new(inner: Pty, shared: Arc<Shared>) -> Self {
        shared.ptys.fetch_add(1, Ordering::AcqRel);
        let end = EndShell {
            ptys: shared.ptys.clone(),
            #[cfg(windows)]
            shell: shell_process(&inner),
        };
        Self { inner, scanner: OscScanner::default(), signals: Vec::new(), shared, _end: end }
    }
}

/// How long a shell gets to exit once its console is closed, before it is
/// ended.
#[cfg(windows)]
const SHELL_EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Ends the shell once its PTY is closed, if closing did not. On Windows a
/// console closed right after it started does not end its shell, which then
/// runs on without a window. Counts the PTY in [`Shared::ptys`] until done.
struct EndShell {
    ptys: Arc<std::sync::atomic::AtomicUsize>,
    /// The shell's process id, and a handle to the process that keeps the
    /// id from going to another process meanwhile.
    #[cfg(windows)]
    shell: Option<(u32, std::os::windows::io::OwnedHandle)>,
}

impl Drop for EndShell {
    fn drop(&mut self) {
        let ptys = self.ptys.clone();
        #[cfg(windows)]
        if let Some((pid, process)) = self.shell.take() {
            let end = move || {
                if crate::detect::end_unless_exited(pid, &process, SHELL_EXIT_GRACE) {
                    tracing::info!(pid, "ended a shell that outlived its terminal");
                }
                ptys.fetch_sub(1, Ordering::AcqRel);
            };
            if let Err(e) = std::thread::Builder::new().name("end-shell".into()).spawn(end) {
                tracing::warn!(error = %e, "could not watch a closed terminal's shell");
                self.ptys.fetch_sub(1, Ordering::AcqRel);
            }
            return;
        }
        ptys.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The PTY's shell, if it started.
#[cfg(windows)]
fn shell_process(pty: &Pty) -> Option<(u32, std::os::windows::io::OwnedHandle)> {
    use std::os::windows::io::{BorrowedHandle, RawHandle};
    let watcher = pty.child_watcher();
    let pid = watcher.pid()?.get();
    // SAFETY: the handle is valid while the PTY is; the copy outlives it.
    let process = unsafe { BorrowedHandle::borrow_raw(watcher.raw_handle() as RawHandle) };
    Some((pid, process.try_clone_to_owned().ok()?))
}

impl Read for TapPty {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.reader().read(buf)?;
        if n > 0 {
            self.scanner.feed(&buf[..n], &mut self.signals);
            for signal in self.signals.drain(..) {
                self.shared.on_signal(signal);
            }
        }
        Ok(n)
    }
}

impl EventedReadWrite for TapPty {
    type Reader = TapPty;
    type Writer = <Pty as EventedReadWrite>::Writer;

    unsafe fn register(&mut self, poll: &Arc<Poller>, interest: PollEvent, mode: PollMode) -> io::Result<()> {
        // SAFETY: forwarded unchanged; the caller upholds the contract.
        unsafe { self.inner.register(poll, interest, mode) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, interest: PollEvent, mode: PollMode) -> io::Result<()> {
        self.inner.reregister(poll, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.inner.deregister(poll)
    }

    fn reader(&mut self) -> &mut TapPty {
        self
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.inner.writer()
    }
}

impl EventedPty for TapPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.inner.next_child_event()
    }
}

impl OnResize for TapPty {
    fn on_resize(&mut self, size: WindowSize) {
        self.inner.on_resize(size);
    }
}
