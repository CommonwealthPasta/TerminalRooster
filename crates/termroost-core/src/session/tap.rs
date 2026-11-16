//! A PTY wrapper that lets the core see the program's output on its way to
//! alacritty's parser, which ignores the sequences notifications rely on
//! (see [`super::osc`]).

use std::io::{self, Read};
use std::sync::Arc;

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
}

impl TapPty {
    pub fn new(inner: Pty, shared: Arc<Shared>) -> Self {
        Self { inner, scanner: OscScanner::default(), signals: Vec::new(), shared }
    }
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
