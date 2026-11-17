//! The Git tab of the side panel: the log of the visible session's
//! repository, between the Tasks and Files tabs. It follows new commits:
//! every git poll compares the repository's refs, and reads the log again
//! when they moved. Commits that are on no remote are marked, and the
//! branch's distance to its upstream is shown, with Pull (rebasing by
//! default) and Push.

use std::path::PathBuf;

use iced::widget::{button, checkbox, column, container, row, scrollable, space, text};
use iced::{Alignment, Element, Length, Task};
use termroost_core::git::{Commit, Head, Repo, Tracking};

use crate::app::{App, Message};
use crate::icons::Icon;
use crate::side_panel::SideTab;
use crate::sidebar::{branch_label, icon_button, muted_text};
use crate::theme;

/// How many commits the log shows.
const LOG_LIMIT: usize = 200;

/// What the Git tab's buttons do with the remote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncOp {
    Pull,
    Push,
}

/// A pull or push: running while `result` is `None`.
#[derive(Clone, Debug)]
pub struct GitSync {
    pub worktree: PathBuf,
    pub op: SyncOp,
    /// What happened, in a sentence, or what git said went wrong.
    pub result: Option<Result<String, String>>,
}

/// The log on screen, and the working tree it comes from.
#[derive(Clone, Debug)]
pub struct GitLog {
    pub worktree: PathBuf,
    /// Every branch, not just the one checked out.
    pub all: bool,
    /// `None` while it is read.
    pub commits: Option<Result<Vec<Commit>, String>>,
    /// [`Repo::ref_stamp`] when `commits` were read.
    pub stamp: Option<u64>,
    /// How the commits stand against the remotes; `None` until read, or
    /// when git could not tell.
    pub tracking: Option<Tracking>,
}

/// A log read in the background.
#[derive(Clone, Debug)]
pub struct LogRead {
    pub worktree: PathBuf,
    pub all: bool,
    pub stamp: u64,
    /// `None` when the refs have not moved since the log on screen was read.
    pub commits: Option<Result<Vec<Commit>, String>>,
    pub tracking: Option<Tracking>,
}

/// `s` cut to `max` characters, ending in "…" when cut.
fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
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

    /// Whether the session on screen is in a repository, which the Git tab
    /// needs.
    pub(crate) fn in_repo(&self) -> bool {
        self.active_repo().is_some()
    }

    /// Whether the side panel shows the Git tab now. It needs a session in a
    /// repository; otherwise the panel shows the tasks.
    pub(crate) fn git_tab_shown(&self) -> bool {
        self.prefs.panel_open && self.shown_side_tab() == SideTab::Git
    }

    /// Reads the log when the Git tab shows a repository it has not read yet
    /// (such as after switching sessions), or when its refs moved since.
    pub(crate) fn refresh_log(&mut self) -> Task<Message> {
        if !self.git_tab_shown() {
            return Task::none();
        }
        let Some(repo) = self.active_repo().cloned() else { return Task::none() };
        let all = self.prefs.log_all_branches;
        let known = match &self.git_log {
            Some(log) if log.worktree == repo.worktree && log.all == all => log.stamp,
            _ => {
                // A read still running is for the old log: dropped when it
                // arrives, which starts this one.
                self.git_log =
                    Some(GitLog { worktree: repo.worktree.clone(), all, commits: None, stamp: None, tracking: None });
                None
            }
        };
        if self.log_reading {
            return Task::none();
        }
        self.log_reading = true;
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            // The stamp first: a commit landing in between is caught next time.
            let stamp = repo.ref_stamp();
            let changed = known != Some(stamp);
            let commits = changed.then(|| repo.log(all, LOG_LIMIT));
            let tracking = changed
                .then(|| repo.tracking(all, LOG_LIMIT))
                .and_then(|t| t.map_err(|e| tracing::warn!(error = %e, "reading the git remotes failed")).ok());
            let _ = tx.send_blocking(LogRead { worktree: repo.worktree, all, stamp, commits, tracking });
        });
        Task::perform(async move { rx.recv().await.ok() }, |read| match read {
            Some(read) => Message::LogRead(read),
            None => Message::Noop,
        })
    }

    /// Takes in a log read in the background, unless the panel moved on to
    /// another repository (or the other branch setting) meanwhile.
    pub(crate) fn log_read(&mut self, read: LogRead) -> Task<Message> {
        self.log_reading = false;
        match &mut self.git_log {
            Some(log) if log.worktree == read.worktree && log.all == read.all => {
                match read.commits {
                    None => {}
                    Some(Ok(commits)) => {
                        log.commits = Some(Ok(commits));
                        log.stamp = Some(read.stamp);
                        log.tracking = read.tracking;
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

    /// The Git tab: the branch on screen and its latest commits.
    pub(crate) fn view_git_log(&self) -> Element<'_, Message> {
        let s = self.core.state();
        let head = s.active_tab().and_then(|t| self.git_of(t)).and_then(|g| g.head.as_ref());
        let folder = self.active_repo().and_then(|r| r.worktree.file_name()).map(|n| n.to_string_lossy().into_owned());
        let mut title = column![].spacing(4);
        if let Some(head) = head {
            title = title.push(branch_label(head, 13.0, 36));
        }
        let mut options = row![].spacing(8).align_y(Alignment::Center);
        if let Some(folder) = folder {
            options = options.push(muted_text(format!("in {}", shorten(&folder, 24))).wrapping(text::Wrapping::None));
        }
        let title = title.push(
            options.push(space::horizontal()).push(
                checkbox(self.prefs.log_all_branches)
                    .label("All branches")
                    .on_toggle(Message::LogAllBranches)
                    .size(14)
                    .text_size(12),
            ),
        );

        let tracking = self.git_log.as_ref().and_then(|l| l.tracking.as_ref());
        let mut title = title;
        if let Some(status) = tracking.and_then(|t| remote_status(t, head)) {
            title = title.push(muted_text(status).size(12));
        }
        if let Some(t) = tracking.filter(|t| t.remote.is_some()) {
            title = title.push(self.sync_buttons(t, head));
        }
        if let Some(note) = self.sync_note() {
            title = title.push(note);
        }

        let body: Element<'_, Message> = match self.git_log.as_ref().and_then(|l| l.commits.as_ref()) {
            None => muted_text("Reading the log…").size(12).into(),
            Some(Err(e)) => text(e).size(12).style(text::danger).into(),
            Some(Ok(commits)) if commits.is_empty() => muted_text("No commits yet.").size(12).into(),
            Some(Ok(commits)) => {
                let now = chrono::Utc::now().timestamp();
                let unpushed = |c: &Commit| tracking.is_some_and(|t| t.unpushed.contains(&c.id));
                scrollable(
                    column(commits.iter().map(|c| commit_item(c, unpushed(c), now))).spacing(12).padding([0, 10]),
                )
                .height(Length::Fill)
                .into()
            }
        };
        column![title, body].spacing(10).into()
    }

    /// Pull (rebasing or not) and Push, for a repository with a remote.
    fn sync_buttons<'a>(&'a self, t: &Tracking, head: Option<&Head>) -> Element<'a, Message> {
        let running = self.git_sync.as_ref().filter(|s| s.result.is_none()).map(|s| s.op);
        let on_branch = matches!(head, Some(Head::Branch(_)));
        let idle = running.is_none() && on_branch;
        let action = |label: &'a str, busy: &'a str, op: SyncOp, enabled: bool| {
            button(text(if running == Some(op) { busy } else { label }).size(12))
                .padding([3, 10])
                .style(button::secondary)
                .on_press_maybe((idle && enabled).then_some(Message::GitSync(op)))
        };
        row![
            // Pulling needs a remote branch to pull from.
            action("↓ Pull", "Pulling…", SyncOp::Pull, t.upstream.is_some()),
            checkbox(self.prefs.pull_rebase).label("Rebase").on_toggle(Message::PullRebase).size(14).text_size(12),
            space::horizontal(),
            action("↑ Push", "Pushing…", SyncOp::Push, true),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    }

    /// What the last pull or push of this repository did, until dismissed.
    fn sync_note(&self) -> Option<Element<'_, Message>> {
        let sync = self.git_sync.as_ref()?;
        let shown = self.git_log.as_ref().is_some_and(|l| l.worktree == sync.worktree);
        let result = sync.result.as_ref().filter(|_| shown)?;
        let message: Element<'_, Message> = match result {
            Ok(done) => muted_text(done).size(12).into(),
            Err(e) => text(e).size(12).style(text::danger).into(),
        };
        Some(
            row![container(message).width(Length::Fill), icon_button(Icon::Close, Message::DismissSync, "Dismiss")]
                .spacing(4)
                .align_y(Alignment::Start)
                .into(),
        )
    }

    /// Pulls or pushes the Git tab's repository on a thread.
    pub(crate) fn start_sync(&mut self, op: SyncOp) -> Task<Message> {
        if self.git_sync.as_ref().is_some_and(|s| s.result.is_none()) {
            return Task::none();
        }
        let Some(repo) = self.active_repo().cloned() else { return Task::none() };
        let rebase = self.prefs.pull_rebase;
        self.git_sync = Some(GitSync { worktree: repo.worktree.clone(), op, result: None });
        tracing::info!(repo = %repo.worktree.display(), ?op, rebase, "git sync started");
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let result = match op {
                SyncOp::Pull => repo.pull(rebase),
                SyncOp::Push => repo.push(),
            };
            let _ = tx.send_blocking((repo.worktree, result));
        });
        Task::perform(async move { rx.recv().await.ok() }, |done| match done {
            Some((worktree, result)) => Message::GitSynced(worktree, result),
            None => Message::Noop,
        })
    }

    /// Shows how a pull or push went, and reads the log again right away.
    pub(crate) fn sync_done(&mut self, worktree: PathBuf, result: Result<String, String>) -> Task<Message> {
        match &result {
            Ok(done) => tracing::info!(repo = %worktree.display(), "{done}"),
            Err(e) => tracing::warn!(repo = %worktree.display(), error = %e, "git sync failed"),
        }
        if let Some(sync) = self.git_sync.as_mut().filter(|s| s.worktree == worktree) {
            sync.result = Some(result);
        }
        // A pull that brought nothing leaves the refs as they were; read anyway.
        if let Some(log) = self.git_log.as_mut().filter(|l| l.worktree == worktree) {
            log.stamp = None;
        }
        Task::batch([self.refresh_log(), self.read_git()])
    }
}

/// Where the branch stands against its remote: "2 commits to push · 1 to
/// pull (origin/main)". `None` without a remote.
fn remote_status(t: &Tracking, head: Option<&Head>) -> Option<String> {
    let remote = t.remote.as_ref()?;
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    Some(match (&t.upstream, head) {
        (Some(upstream), _) if t.ahead == 0 && t.behind == 0 => format!("Up to date with {upstream}"),
        (Some(upstream), _) => {
            let mut parts = Vec::new();
            if t.ahead > 0 {
                parts.push(format!("{} commit{} to push", t.ahead, plural(t.ahead)));
            }
            if t.behind > 0 {
                parts.push(format!("{} to pull", t.behind));
            }
            format!("{} ({upstream})", parts.join(" · "))
        }
        (None, Some(Head::Branch(branch))) => format!("{branch} is not on {remote} yet"),
        (None, _) => format!("Not on a branch; {remote} is the remote"),
    })
}

/// One commit: subject, then id, author and age, then branches and tags,
/// and whether it is on no remote yet.
fn commit_item(c: &Commit, unpushed: bool, now: i64) -> Element<'_, Message> {
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
    if !c.refs.is_empty() || unpushed {
        let badge = |label: &str, style: fn(&iced::Theme) -> container::Style| -> Element<'_, Message> {
            container(text(label.to_owned()).size(10).wrapping(text::Wrapping::None))
                .padding([1, 6])
                .style(style)
                .into()
        };
        let head_branch = c.head.then(|| c.refs.first()).flatten();
        let mut badges: Vec<Element<'_, Message>> = Vec::new();
        if unpushed {
            badges.push(badge("not pushed", theme::warning_badge));
        }
        badges.extend(
            c.refs.iter().map(|r| badge(r, if Some(r) == head_branch { theme::accent_badge } else { theme::badge })),
        );
        item = item.push(row(badges).spacing(4).wrap());
    }
    item.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_statuses() {
        let main = Head::Branch("main".into());
        let t = |upstream: Option<&str>, ahead, behind| Tracking {
            remote: Some("origin".into()),
            upstream: upstream.map(Into::into),
            ahead,
            behind,
            unpushed: Default::default(),
        };
        let status = |t: Tracking| remote_status(&t, Some(&main));
        assert_eq!(status(t(Some("origin/main"), 0, 0)).unwrap(), "Up to date with origin/main");
        assert_eq!(status(t(Some("origin/main"), 1, 0)).unwrap(), "1 commit to push (origin/main)");
        assert_eq!(status(t(Some("origin/main"), 2, 3)).unwrap(), "2 commits to push · 3 to pull (origin/main)");
        assert_eq!(status(t(None, 0, 0)).unwrap(), "main is not on origin yet");
        assert_eq!(status(Tracking::default()), None, "no remote");
    }

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
