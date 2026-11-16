//! Terminal sessions and the background autosave/agent-detection thread.

pub mod bundled_bash;
pub mod input;
pub mod render;
pub mod shell;
mod snapshot;
mod terminal;

pub(crate) use terminal::SpawnSpec;
pub use terminal::{GridSize, SelectionKind, TerminalHandle};

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::command::Event;
use crate::detect::{Detector, ObservedAgent};
use crate::model::{PaneId, now};
use crate::store::{StoreOp, StoreWriter};

pub(crate) type SessionMap = Arc<Mutex<HashMap<PaneId, TerminalHandle>>>;

/// State shared with the background thread.
#[derive(Clone)]
pub(crate) struct Background {
    pub sessions: SessionMap,
    pub detector: Arc<Mutex<Detector>>,
    /// Where the Codex hook drops its reports.
    pub data_dir: std::path::PathBuf,
    /// Hook reports written before this are stale.
    pub started: std::time::SystemTime,
    pub last_codex_report: Arc<Mutex<Option<std::time::SystemTime>>>,
    pub store: StoreWriter,
    pub events: async_channel::Sender<Event>,
    pub saved_lines: Arc<std::sync::atomic::AtomicUsize>,
}

impl Background {
    /// Saves the scrollback of every terminal that changed since its last save.
    pub fn save_dirty(&self, force: bool) {
        let handles: Vec<TerminalHandle> = self.sessions.lock().unwrap().values().cloned().collect();
        let max = self.saved_lines.load(Ordering::Relaxed);
        let mut ops = Vec::new();
        for h in handles {
            let dirty = h.0.shared.save_dirty.swap(false, Ordering::AcqRel);
            if !(dirty || force) {
                continue;
            }
            if let Some(data) = h.snapshot(max) {
                ops.push(StoreOp::Scrollback { pane: h.pane(), data, saved_at: now() });
            }
        }
        if !ops.is_empty() {
            tracing::debug!(terminals = ops.len(), "saving scrollback");
        }
        for op in ops {
            // One batch per pane: a pane closed meanwhile cannot fail the others.
            self.store.write(vec![op]);
        }
    }

    /// Looks for agents in every running terminal; returns what changed.
    pub fn detect_agents(&self) -> Vec<(PaneId, Option<ObservedAgent>)> {
        let reports = crate::codex_hook::drain(&self.data_dir, self.started);
        if let Some(last) = reports.last() {
            *self.last_codex_report.lock().unwrap() = Some(last.at);
            let _ = self.events.try_send(Event::CodexHookReported);
        }
        let mut shells = Vec::new();
        let mut submits = HashMap::new();
        for h in self.sessions.lock().unwrap().values() {
            if let Some(pid) = h.shell_pid() {
                shells.push((h.pane(), pid));
                submits.insert(h.pane(), h.submits());
            }
        }
        let changes = self.detector.lock().unwrap().scan_with(&shells, &submits, &reports);
        for (pane, agent) in &changes {
            match agent {
                Some(a) => {
                    tracing::info!(%pane, agent = a.kind.as_str(), session = ?a.session_id, pid = a.pid, "agent detected")
                }
                None => tracing::info!(%pane, "agent exited"),
            }
        }
        changes
    }

    pub fn spawn(self, stop: mpsc::Receiver<()>, interval: Duration) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("terminalrooster-autosave".into())
            .spawn(move || {
                // Runs until the core sends a stop signal (or is dropped).
                while let Err(mpsc::RecvTimeoutError::Timeout) = stop.recv_timeout(interval) {
                    self.save_dirty(false);
                    for (pane, agent) in self.detect_agents() {
                        let _ = self.events.try_send(Event::AgentObserved { pane, agent });
                    }
                }
            })
            .expect("spawn autosave thread")
    }
}
