//! The Git tab of the side panel: the log of the visible session's
//! repository, next to the Tasks tab. It follows new commits: every git poll
//! compares the repository's refs, and reads the log again when they moved.

use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Alignment, Element, Length, Task};
use serde::{Deserialize, Serialize};
use termroost_core::git::{Commit, Repo};

use crate::app::{App, Message};
use crate::sidebar::{BOLD, branch_label, muted_text};
use crate::theme;

/// How many commits the log shows.
const LOG_LIMIT: usize = 200;

/// The tabs of the side panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SideTab {
    #[default]
    Tasks,
    Git,
}

/// The log on screen, and the working tree it comes from.
#[derive(Clone, Debug)]
pub struct GitLog {
    pub worktree: PathBuf,
    /// `None` while it is read.
    pub commits: Option<Result<Vec<Commit>, String>>,
    /// [`Repo::ref_stamp`] when `commits` were read.
    pub stamp: Option<u64>,
}

/// A log read in the background.
#[derive(Clone, Debug)]
pub struct LogRead {
    pub worktree: PathBuf,
    pub stamp: u64,
    /// `None` when the refs have not moved since the log on screen was read.
    pub commits: Option<Result<Vec<Commit>, String>>,
}

/// "5m ago", or the date for anything older than a week.
fn ago(time: i64, now: i64) -> String {
    let secs = (now - time).max(0);
    match secs {
        0..60 => "just now".to_owned(),
        60..3_600 => format!("{}m ago", secs / 60),
        3_600..86_400 => format!("{}h ago", secs / 3_600),
        86_400..604_800 => format!("{}d ago", secs / 86_400),
        _ => chrono::DateTime::from_timestamp(time, 0)
            .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
    }
}

impl App {
    /// The repository of the session on screen, when it is in one.
    fn active_repo(&self) -> Option<&Repo> {
        let tab = self.core.state().active_tab()?;
        Some(&self.git_of(tab)?.repo)
    }

    /// Whether the side panel shows the Git tab now. It needs a session in a
    /// repository; otherwise the panel shows the tasks.
    pub(crate) fn git_tab_shown(&self) -> bool {
        self.prefs.tasks_open && self.prefs.side_tab == SideTab::Git && self.active_repo().is_some()
    }

    /// Reads the log when the Git tab shows a repository it has not read yet
    /// (such as after switching sessions), or when its refs moved since.
    pub(crate) fn refresh_log(&mut self) -> Task<Message> {
        if !self.git_tab_shown() || self.log_reading {
            return Task::none();
        }
        let Some(repo) = self.active_repo().cloned() else { return Task::none() };
        let known = match &self.git_log {
            Some(log) if log.worktree == repo.worktree => log.stamp,
            _ => {
                self.git_log = Some(GitLog { worktree: repo.worktree.clone(), commits: None, stamp: None });
                None
            }
        };
        self.log_reading = true;
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            // The stamp first: a commit landing in between is caught next time.
            let stamp = repo.ref_stamp();
            let commits = (known != Some(stamp)).then(|| repo.log(false, LOG_LIMIT));
            let _ = tx.send_blocking(LogRead { worktree: repo.worktree, stamp, commits });
        });
        Task::perform(async move { rx.recv().await.ok() }, |read| match read {
            Some(read) => Message::LogRead(read),
            None => Message::Noop,
        })
    }

    /// Takes in a log read in the background, unless the panel moved on to
    /// another repository meanwhile.
    pub(crate) fn log_read(&mut self, read: LogRead) -> Task<Message> {
        self.log_reading = false;
        match &mut self.git_log {
            Some(log) if log.worktree == read.worktree => {
                match read.commits {
                    None => {}
                    Some(Ok(commits)) => {
                        log.commits = Some(Ok(commits));
                        log.stamp = Some(read.stamp);
                    }
                    // Without a stamp, the next poll tries again.
                    Some(Err(e)) => {
                        tracing::warn!(repo = %read.worktree.display(), error = %e, "reading the git log failed");
                        log.commits = Some(Err(e));
                        log.stamp = None;
                    }
                }
                Task::none()
            }
            _ => self.refresh_log(),
        }
    }

    /// The Tasks and Git tabs; Git only for a session in a repository.
    pub(crate) fn side_tabs(&self) -> Element<'_, Message> {
        let tab = |label: &'static str, which: SideTab| {
            let selected = self.prefs.side_tab == which || (which == SideTab::Tasks && self.active_repo().is_none());
            button(text(label).size(11).font(BOLD))
                .padding([3, 10])
                .style(theme::side_tab(selected))
                .on_press(Message::SideTab(which))
        };
        let mut tabs = row![tab("TASKS", SideTab::Tasks)].spacing(4);
        if self.active_repo().is_some() {
            tabs = tabs.push(tab("GIT", SideTab::Git));
        }
        tabs.into()
    }

    /// The Git tab: the branch on screen and its latest commits.
    pub(crate) fn view_git_log(&self) -> Element<'_, Message> {
        let s = self.core.state();
        let head = s.active_tab().and_then(|t| self.git_of(t)).and_then(|g| g.head.as_ref());
        let folder = self.active_repo().and_then(|r| r.worktree.file_name()).map(|n| n.to_string_lossy().into_owned());
        let mut title = row![].spacing(8).align_y(Alignment::Center);
        if let Some(head) = head {
            title = title.push(branch_label(head, 13.0, 28));
        }
        if let Some(folder) = folder {
            title = title.push(muted_text(format!("in {folder}")).wrapping(text::Wrapping::None));
        }

        let body: Element<'_, Message> = match self.git_log.as_ref().and_then(|l| l.commits.as_ref()) {
            None => muted_text("Reading the log…").size(12).into(),
            Some(Err(e)) => text(e).size(12).style(text::danger).into(),
            Some(Ok(commits)) if commits.is_empty() => muted_text("No commits yet.").size(12).into(),
            Some(Ok(commits)) => {
                let now = chrono::Utc::now().timestamp();
                scrollable(column(commits.iter().map(|c| commit_item(c, now))).spacing(12).padding([0, 10]))
                    .height(Length::Fill)
                    .into()
            }
        };
        column![title, body].spacing(10).into()
    }
}

/// One commit: subject, then id, author and age, then branches and tags.
fn commit_item(c: &Commit, now: i64) -> Element<'_, Message> {
    let meta = row![
        text(&c.short)
            .font(theme::MONO)
            .size(11)
            .style(|t: &iced::Theme| text::Style { color: Some(theme::accent(t)) }),
        muted_text(&c.author).wrapping(text::Wrapping::None),
        muted_text(ago(c.time, now)).wrapping(text::Wrapping::None),
    ]
    .spacing(6);
    let mut item = column![text(&c.subject).size(13), meta].spacing(2);
    if !c.refs.is_empty() {
        let head_branch = c.head.then(|| c.refs.first()).flatten();
        let badges = c.refs.iter().map(|r| {
            let style = if Some(r) == head_branch { theme::accent_badge } else { theme::badge };
            container(text(r).size(10).wrapping(text::Wrapping::None)).padding([1, 6]).style(style).into()
        });
        item = item.push(row(badges).spacing(4).wrap());
    }
    item.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        let now = 1_700_000_000;
        assert_eq!(ago(now - 5, now), "just now");
        assert_eq!(ago(now + 30, now), "just now");
        assert_eq!(ago(now - 125, now), "2m ago");
        assert_eq!(ago(now - 3 * 3_600, now), "3h ago");
        assert_eq!(ago(now - 2 * 86_400, now), "2d ago");
        assert_eq!(ago(now - 30 * 86_400, now).len(), "2023-10-15".len());
    }
}
