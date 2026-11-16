//! Removing a git worktree from the Worktrees dialog. The sessions working
//! in it are closed, and git deletes the folder once the processes that ran
//! there have exited (Windows cannot delete a folder a process works in).

use std::path::PathBuf;
use std::time::Duration;

use iced::widget::{button, checkbox, column, row, space, text};
use iced::{Element, Task};
use termroost_core::TabId;
use termroost_core::git::{self, Head, Repo};

use crate::app::{App, Dialog, Message, RunningTerminal};
use crate::sidebar::{BOLD, branch_label, display_dir, muted_text};
use crate::worktree::WorktreeForm;

/// How long to wait for the stopped terminals' processes before git deletes
/// the folder anyway.
const EXIT_WAIT: Duration = Duration::from_secs(10);

/// State of the "Remove worktree?" dialog.
#[derive(Clone, Debug)]
pub struct RemoveForm {
    /// The Worktrees dialog to go back to.
    pub back: Box<WorktreeForm>,
    pub path: PathBuf,
    pub head: Option<Head>,
    /// The sessions working in it, which get closed.
    pub sessions: Vec<TabId>,
    /// Terminals of those sessions running a program.
    pub running: Vec<RunningTerminal>,
    /// Terminals of other sessions in the folder running a program: they
    /// stay, and can keep git from deleting the folder.
    pub blocking: Vec<RunningTerminal>,
    /// Uncommitted changes, counted in the background.
    pub changes: Option<Result<usize, String>>,
    /// Delete uncommitted changes too.
    pub discard: bool,
    pub removing: bool,
    /// Sessions an earlier attempt closed before git refused.
    pub closed: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum RemoveMsg {
    /// Ask before removing this worktree (from the Worktrees dialog).
    Ask(PathBuf),
    Changes(PathBuf, Result<usize, String>),
    Discard(bool),
    Confirm,
    Removed {
        path: PathBuf,
        result: Result<(), String>,
    },
}

impl RemoveForm {
    fn can_remove(&self) -> bool {
        !self.removing
            && match &self.changes {
                None => false,
                Some(Ok(n)) => *n == 0 || self.discard,
                // git checks again, and refuses without `discard`.
                Some(Err(_)) => true,
            }
    }
}

fn msg(m: RemoveMsg) -> Message {
    Message::RemoveWorktree(m)
}

impl App {
    pub(crate) fn update_remove(&mut self, m: RemoveMsg) -> Task<Message> {
        match m {
            RemoveMsg::Ask(path) => return self.ask_remove(path),
            RemoveMsg::Changes(path, changes) => {
                if let Some(f) = self.remove_form().filter(|f| f.path == path) {
                    f.changes = Some(changes);
                }
            }
            RemoveMsg::Discard(on) => {
                if let Some(f) = self.remove_form() {
                    f.discard = on;
                }
            }
            RemoveMsg::Confirm => return self.confirm_remove(),
            RemoveMsg::Removed { path, result } => {
                let Some(f) = self.remove_form().filter(|f| f.path == path) else { return Task::none() };
                f.removing = false;
                match result {
                    Ok(()) => {
                        tracing::info!(path = %path.display(), "worktree removed");
                        let closed = f.closed;
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        self.dialog = None;
                        self.notice = Some(match closed {
                            0 => format!("Removed the worktree {name}."),
                            1 => format!("Removed the worktree {name} and closed its session."),
                            n => format!("Removed the worktree {name} and closed its {n} sessions."),
                        });
                        return Task::batch([self.read_git(), self.focus_active()]);
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "removing a worktree failed");
                        f.error = Some(e);
                    }
                }
            }
        }
        Task::none()
    }

    pub(crate) fn remove_form(&mut self) -> Option<&mut RemoveForm> {
        match &mut self.dialog {
            Some(Dialog::RemoveWorktree(f)) => Some(f),
            _ => None,
        }
    }

    /// Swaps the Worktrees dialog for the question whether to remove `path`.
    fn ask_remove(&mut self, path: PathBuf) -> Task<Message> {
        let Some(Dialog::Worktree(back)) = self.dialog.take() else { return Task::none() };
        let s = self.core.state();
        let sessions: Vec<TabId> =
            s.tabs.iter().filter(|t| git::is_within(&s.tab_working_dir(t), &path)).map(|t| t.id).collect();
        let in_folder =
            |pane| s.pane(pane).and_then(|p| p.cwd.as_deref()).is_some_and(|cwd| git::is_within(cwd, &path));
        let (running, blocking) = self
            .running_terminals()
            .into_iter()
            .filter(|t| s.pane(t.pane).is_some_and(|p| sessions.contains(&p.tab)) || in_folder(t.pane))
            .partition(|t| s.pane(t.pane).is_some_and(|p| sessions.contains(&p.tab)));
        let head = back
            .branches
            .as_ref()
            .and_then(|b| b.as_ref().ok())
            .and_then(|b| b.worktrees.iter().find(|w| w.path == path))
            .and_then(|w| w.head.clone());
        self.dialog = Some(Dialog::RemoveWorktree(Box::new(RemoveForm {
            back,
            path: path.clone(),
            head,
            sessions,
            running,
            blocking,
            changes: None,
            discard: false,
            removing: false,
            closed: 0,
            error: None,
        })));
        let (tx, rx) = async_channel::bounded(1);
        let dir = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(Repo::changes(&dir));
        });
        Task::perform(async move { rx.recv().await.unwrap_or_else(|e| Err(e.to_string())) }, move |changes| {
            msg(RemoveMsg::Changes(path.clone(), changes))
        })
    }

    /// Back to the Worktrees dialog, unless its session was closed meanwhile.
    pub(crate) fn cancel_remove(&mut self) {
        if let Some(Dialog::RemoveWorktree(f)) = self.dialog.take()
            && self.core.state().tab(f.back.tab).is_some()
            && f.closed == 0
        {
            self.dialog = Some(Dialog::Worktree(f.back));
        }
    }

    /// Closes the worktree's sessions, then has git remove it on a thread
    /// once their processes are gone.
    fn confirm_remove(&mut self) -> Task<Message> {
        let Some(f) = self.remove_form().filter(|f| f.can_remove()) else { return Task::none() };
        let (path, force, repo) = (f.path.clone(), f.discard, f.back.repo.clone());
        let left = self.core.leave_dir(&path);
        for tab in &left.closed {
            self.grids.remove(tab);
            self.activity.remove(tab);
        }
        if let Some(f) = self.remove_form() {
            f.removing = true;
            f.error = None;
            f.closed += left.closed.len();
        }
        let (tx, rx) = async_channel::bounded(1);
        let target = path.clone();
        std::thread::spawn(move || {
            if !left.wait(EXIT_WAIT) {
                tracing::warn!(path = %target.display(), "processes still running in a worktree being removed");
            }
            let _ = tx.send_blocking(repo.remove_worktree(&target, force));
        });
        let removed = Task::perform(async move { rx.recv().await.unwrap_or_else(|e| Err(e.to_string())) }, move |r| {
            msg(RemoveMsg::Removed { path: path.clone(), result: r })
        });
        Task::batch([removed, self.after_select()])
    }

    pub(crate) fn view_remove<'a>(&'a self, f: &'a RemoveForm) -> Element<'a, Message> {
        let s = self.core.state();
        let project = s.tab(f.back.tab).map(|t| t.project);
        let mut col = column![text("Remove worktree?").size(18).font(BOLD)].spacing(12);

        let mut what = row![].spacing(8);
        if let Some(head) = &f.head {
            what = what.push(branch_label(head, 13.0, 40));
        }
        col = col.push(what.push(muted_text(display_dir(&f.path)).size(12)));
        col = col.push(
            text(match &f.head {
                Some(Head::Branch(b)) => {
                    format!("This deletes the folder and everything in it. The branch {b} is kept.")
                }
                _ => "This deletes the folder and everything in it.".to_owned(),
            })
            .size(13),
        );

        let names: Vec<String> = f
            .sessions
            .iter()
            .filter_map(|t| s.tab(*t))
            .map(|t| match s.project(t.project) {
                Some(p) if Some(t.project) != project => format!("{} ({})", t.title, p.name),
                _ => t.title.clone(),
            })
            .collect();
        let this_one = if f.sessions.contains(&f.back.tab) { ", this one included" } else { "" };
        col = col.push(
            text(match names.len() {
                0 => "No session works there.".to_owned(),
                1 => format!("It closes the session working there{this_one}: {}.", names[0]),
                n => format!("It closes the {n} sessions working there{this_one}: {}.", names.join(", ")),
            })
            .size(13),
        );
        if !f.running.is_empty() {
            let lines = f.running.iter().map(|t| muted_text(format!("{}: {}", t.place, t.programs)).size(12).into());
            col = col.push(column![text("These stop:").size(13), column(lines).spacing(2).padding([0, 12])].spacing(4));
        }
        for t in &f.blocking {
            col = col.push(
                text(format!(
                    "{} has a terminal in this folder running {}. Stop it first, or git may not be able to delete \
                     the folder.",
                    t.place, t.programs
                ))
                .size(12)
                .style(text::danger),
            );
        }

        let changes: Element<'_, Message> = match &f.changes {
            None => muted_text("Checking for uncommitted changes…").size(12).into(),
            Some(Ok(0)) => muted_text("No uncommitted changes.").size(12).into(),
            Some(Ok(n)) => column![
                text(format!(
                    "It has {n} uncommitted change{}: edited files, or new files git does not ignore.",
                    if *n == 1 { "" } else { "s" }
                ))
                .size(13)
                .style(text::danger),
                checkbox(f.discard)
                    .label("Delete them too")
                    .on_toggle(|on| msg(RemoveMsg::Discard(on)))
                    .size(14)
                    .text_size(13),
            ]
            .spacing(6)
            .into(),
            Some(Err(e)) => text(format!("git could not check for changes: {e}")).size(12).style(text::danger).into(),
        };
        col = col.push(changes);
        if let Some(e) = &f.error {
            let closed = if f.closed > 0 { " Its sessions are closed already." } else { "" };
            col = col.push(text(format!("git did not remove it: {e}{closed}")).size(13).style(text::danger));
        }

        let back = button(text("Back")).padding([6, 14]).style(button::secondary);
        col.push(
            row![
                space::horizontal(),
                if f.removing { back } else { back.on_press(Message::DialogCancel) },
                button(text(if f.removing { "Removing…" } else { "Remove worktree" }))
                    .padding([6, 14])
                    .style(button::danger)
                    .on_press_maybe(f.can_remove().then(|| msg(RemoveMsg::Confirm))),
            ]
            .spacing(8),
        )
        .into()
    }
}
