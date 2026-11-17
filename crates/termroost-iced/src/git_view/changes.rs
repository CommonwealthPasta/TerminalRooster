//! What is not committed yet in a session's repository, shown in place of
//! its terminals: files with conflicts, changes not staged, new files, and
//! staged changes. Whole files, single hunks or all of a section are staged
//! and unstaged from here.
//!
//! While it is on screen, it is read again at every git poll, since agents
//! change files all the time. Each read is numbered: a read that started
//! before one already shown (a poll overtaken by a stage) is dropped.

use std::collections::HashSet;

use iced::Task;
use termroost_core::git::{FileDiff, FileStatus, Repo, WorkingChanges};

use super::{GitViewMsg, starts_folded};
use crate::app::{App, Message};
use crate::editor::{DocId, DocKind, OpenFile};

/// More untracked files than this start folded.
const FOLD_UNTRACKED: usize = 20;

/// A part of the uncommitted changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// Files with a merge conflict.
    Conflicts,
    /// Changes not staged yet.
    Unstaged,
    /// Files git doesn't track yet.
    Untracked,
    Staged,
}

impl Section {
    pub const ALL: [Section; 4] = [Section::Conflicts, Section::Unstaged, Section::Untracked, Section::Staged];

    pub fn title(self) -> &'static str {
        match self {
            Section::Conflicts => "Merge conflicts",
            Section::Unstaged => "Unstaged changes",
            Section::Untracked => "Untracked files",
            Section::Staged => "Staged changes",
        }
    }

    /// Its files of `changes`.
    pub fn files(self, changes: &WorkingChanges) -> Box<dyn Iterator<Item = &FileDiff> + '_> {
        let conflict = |f: &&FileDiff| f.status == FileStatus::Conflicted;
        match self {
            Section::Conflicts => Box::new(changes.unstaged.iter().filter(conflict)),
            Section::Unstaged => Box::new(changes.unstaged.iter().filter(move |f| !conflict(f))),
            Section::Untracked => Box::new(changes.untracked.iter()),
            Section::Staged => Box::new(changes.staged.iter()),
        }
    }
}

/// What a stage or unstage acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Every file of a section.
    All(Section),
    /// A file of a section, by path.
    File(Section, String),
    /// A hunk of a file of a section.
    Hunk(Section, String, usize),
}

impl Pick {
    fn section(&self) -> Section {
        match self {
            Pick::All(s) | Pick::File(s, _) | Pick::Hunk(s, _, _) => *s,
        }
    }
}

/// The uncommitted changes of a repository, in place of a session's
/// terminals. A session has one at most.
pub struct ChangesDoc {
    pub repo: Repo,
    /// `None` until first read.
    pub changes: Option<Result<WorkingChanges, String>>,
    /// Files the user folded or unfolded (big ones start folded).
    toggled: HashSet<(Section, String)>,
    /// Reads started so far, and the number of the one shown.
    reads: u64,
    shown_read: u64,
    /// A read is running: polls wait for it.
    reading: bool,
    /// A stage or unstage is running.
    pub busy: bool,
    /// Why the last stage or unstage failed, until the next one.
    pub error: Option<String>,
}

impl ChangesDoc {
    fn new(repo: Repo) -> ChangesDoc {
        ChangesDoc {
            repo,
            changes: None,
            toggled: HashSet::new(),
            reads: 0,
            shown_read: 0,
            reading: false,
            busy: false,
            error: None,
        }
    }

    pub fn folded(&self, section: Section, file: &FileDiff, files_in_section: usize) -> bool {
        let starts = starts_folded(file) || (section == Section::Untracked && files_in_section > FOLD_UNTRACKED);
        starts != self.toggled.contains(&(section, file.path.clone()))
    }

    /// Takes in a read numbered `n`, unless a later one is shown already.
    fn take(&mut self, n: u64, read: Result<WorkingChanges, String>) {
        if n == self.reads {
            self.reading = false;
        }
        if n < self.shown_read {
            return;
        }
        self.shown_read = n;
        if let Err(e) = &read {
            tracing::warn!(repo = %self.repo.worktree.display(), error = %e, "reading the changes failed");
        }
        if self.changes.as_ref() != Some(&read) {
            self.changes = Some(read);
        }
    }
}

impl std::fmt::Debug for ChangesDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the changes: logs must not show what is in people's files.
        f.debug_struct("ChangesDoc").field("repo", &self.repo.worktree).field("busy", &self.busy).finish()
    }
}

impl App {
    /// Whether the session on screen shows its uncommitted changes.
    pub(crate) fn changes_shown(&self) -> bool {
        matches!(self.shown_doc().map(|d| &d.kind), Some(DocKind::Changes(_)))
    }

    pub(super) fn changes_doc_mut(&mut self, id: DocId) -> Option<&mut ChangesDoc> {
        let doc = self.editors.values_mut().flat_map(|f| f.open.iter_mut()).find(|d| d.id == id)?;
        match &mut doc.kind {
            DocKind::Changes(c) => Some(c),
            _ => None,
        }
    }

    /// Shows the uncommitted changes of the session's repository, in its
    /// changes view (made if it has none), and reads them again.
    pub(super) fn open_changes(&mut self) -> Task<Message> {
        let Some(tab) = self.active_tab_id() else { return Task::none() };
        let Some(repo) = self.active_repo().cloned() else { return Task::none() };
        self.open_files_dirty = true;
        let existing = self.session_files().and_then(|f| {
            f.open.iter().find_map(|d| match &d.kind {
                DocKind::Changes(c) => Some((d.id, c.repo == repo)),
                _ => None,
            })
        });
        let fresh = existing.is_none().then(|| self.new_doc_id());
        let files = self.editors.entry(tab).or_default();
        let doc = match (existing, fresh) {
            (Some((doc, same)), _) => {
                // The session moved to another worktree since: start over there.
                if !same && let Some(d) = files.open.iter_mut().find(|d| d.id == doc) {
                    d.path = repo.worktree.clone();
                    d.kind = DocKind::Changes(Box::new(ChangesDoc::new(repo)));
                }
                doc
            }
            (None, Some(doc)) => {
                let kind = DocKind::Changes(Box::new(ChangesDoc::new(repo.clone())));
                files.open.push(OpenFile { id: doc, path: repo.worktree, kind });
                doc
            }
            (None, None) => return Task::none(),
        };
        files.shown = Some(doc);
        self.read_changes(doc)
    }

    /// Reads the changes of a changes view again, on a thread.
    fn read_changes(&mut self, doc: DocId) -> Task<Message> {
        let Some(c) = self.changes_doc_mut(doc) else { return Task::none() };
        c.reads += 1;
        c.reading = true;
        let n = c.reads;
        let repo = c.repo.clone();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(repo.working_changes());
        });
        Task::perform(
            async move { rx.recv().await.unwrap_or_else(|_| Err("reading the changes stopped".into())) },
            move |r| Message::GitView(GitViewMsg::ChangesRead(doc, n, Box::new(r))),
        )
    }

    /// At a git poll: reads the changes on screen again, unless they are
    /// being read or changed already.
    pub(crate) fn poll_changes(&mut self) -> Task<Message> {
        let Some(doc) = self.shown_doc() else { return Task::none() };
        match &doc.kind {
            DocKind::Changes(c) if !c.reading && !c.busy => {
                let id = doc.id;
                self.read_changes(id)
            }
            _ => Task::none(),
        }
    }

    /// Stages what is picked (or unstages it, from the staged changes) on a
    /// thread, then reads the changes again there.
    fn move_pick(&mut self, doc: DocId, pick: Pick) -> Task<Message> {
        let Some(c) = self.changes_doc_mut(doc) else { return Task::none() };
        let Some(Ok(changes)) = &c.changes else { return Task::none() };
        if c.busy {
            return Task::none();
        }
        let section = pick.section();
        let unstage = section == Section::Staged;
        // What the user saw: the hunk is applied as it was shown.
        let files: Vec<FileDiff> = match &pick {
            Pick::All(_) => section.files(changes).cloned().collect(),
            Pick::File(_, path) | Pick::Hunk(_, path, _) => {
                section.files(changes).filter(|f| &f.path == path).cloned().collect()
            }
        };
        if files.is_empty() {
            return Task::none();
        }
        let hunk = match pick {
            Pick::Hunk(_, _, i) => Some(i),
            _ => None,
        };
        c.busy = true;
        c.error = None;
        c.reads += 1;
        let n = c.reads;
        let repo = c.repo.clone();
        tracing::info!(repo = %repo.worktree.display(), ?section, files = files.len(), ?hunk, "staging changes");
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let paths = || files.iter().flat_map(FileDiff::paths);
            let done = match (hunk, unstage) {
                (Some(i), false) => repo.stage_hunk(&files[0], i, None),
                (Some(i), true) => repo.unstage_hunk(&files[0], i, None),
                (None, false) => repo.stage_paths(paths()),
                (None, true) => repo.unstage_paths(paths()),
            };
            let _ = tx.send_blocking((done, repo.working_changes()));
        });
        Task::perform(
            async move {
                let stopped = || Err("staging stopped".to_owned());
                rx.recv().await.unwrap_or_else(|_| (stopped(), Err("reading the changes stopped".into())))
            },
            move |(done, read)| Message::GitView(GitViewMsg::Moved(doc, n, done, Box::new(read))),
        )
    }

    pub(super) fn update_changes(&mut self, msg: GitViewMsg) -> Task<Message> {
        match msg {
            GitViewMsg::OpenChanges => return self.open_changes(),
            GitViewMsg::ChangesRead(doc, n, read) => {
                if let Some(c) = self.changes_doc_mut(doc) {
                    c.take(n, *read);
                }
            }
            GitViewMsg::ToggleChange(doc, section, path) => {
                if let Some(c) = self.changes_doc_mut(doc) {
                    let key = (section, path);
                    if !c.toggled.remove(&key) {
                        c.toggled.insert(key);
                    }
                }
            }
            GitViewMsg::Move(doc, pick) => return self.move_pick(doc, pick),
            GitViewMsg::Moved(doc, n, done, read) => {
                if let Some(c) = self.changes_doc_mut(doc) {
                    c.busy = false;
                    if let Err(e) = done {
                        tracing::warn!(repo = %c.repo.worktree.display(), error = %e, "staging failed");
                        c.error = Some(e);
                    }
                    c.take(n, *read);
                }
            }
            GitViewMsg::DismissChangesError(doc) => {
                if let Some(c) = self.changes_doc_mut(doc) {
                    c.error = None;
                }
            }
            _ => {}
        }
        Task::none()
    }
}
