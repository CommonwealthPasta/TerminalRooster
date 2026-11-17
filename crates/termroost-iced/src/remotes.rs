//! The Remotes dialog, opened from the Git tab: the remotes of the session's
//! repository, each with its URL to change, and a form to add one. Which
//! remote the branch uses is picked in the Git tab itself.

use std::path::PathBuf;

use iced::widget::{column, container, operation, row, text};
use iced::{Alignment, Element, Length, Task};
use termroost_core::git::{Remote, Repo};

use crate::app::{App, DIALOG_FIRST_INPUT, Dialog, Message};
use crate::dialogs::DialogParts;
use crate::sidebar::muted_text;
use crate::theme::{self, SEMIBOLD, Tone};
use crate::ui::{self, field};

/// The field a remote's new URL is typed in.
const URL_INPUT: &str = "remote-url-input";

/// What git is asked to do to the remotes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Add,
    SetUrl,
}

/// State of the Remotes dialog.
#[derive(Clone, Debug)]
pub struct RemotesForm {
    pub repo: Repo,
    /// Read when the dialog opens, and again after each change.
    pub remotes: Option<Result<Vec<Remote>, String>>,
    /// The remote whose URL is being changed, and the URL typed so far.
    pub editing: Option<(String, String)>,
    /// Name and URL of a remote to add.
    pub name: String,
    pub url: String,
    /// What git is doing now.
    pub busy: Option<Change>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum RemotesMsg {
    /// Open the dialog for the Git tab's repository.
    Open,
    Loaded(PathBuf, Result<Vec<Remote>, String>),
    /// Start changing this remote's URL.
    Edit(String),
    EditUrl(String),
    SaveUrl,
    CancelEdit,
    Name(String),
    Url(String),
    Add,
    /// git made the change and the remotes were read again, or what went wrong.
    Changed {
        worktree: PathBuf,
        change: Change,
        result: Result<Vec<Remote>, String>,
    },
}

impl RemotesForm {
    fn can_add(&self) -> bool {
        self.busy.is_none() && !self.name.trim().is_empty() && !self.url.trim().is_empty()
    }
}

fn msg(m: RemotesMsg) -> Message {
    Message::Remotes(m)
}

/// A heading over a part of the dialog.
fn subheading(label: &str) -> text::Text<'_> {
    text(label).size(13).font(SEMIBOLD)
}

/// One remote: its name and URLs, or the field its URL is changed in.
fn remote_item<'a>(f: &'a RemotesForm, r: &'a Remote, in_use: bool) -> Element<'a, Message> {
    let idle = f.busy.is_none();
    let mut name = row![text(&r.name).size(13).font(SEMIBOLD)].spacing(6).align_y(Alignment::Center);
    if in_use {
        name = name.push(ui::badge("in use", Tone::Accent));
    }
    let body: Element<'_, Message> = match f.editing.as_ref().filter(|(n, _)| *n == r.name) {
        Some((_, url)) => {
            let can_save = idle && !url.trim().is_empty();
            let save = if f.busy == Some(Change::SetUrl) { "Saving…" } else { "Save" };
            let edit = row![
                ui::input("https://github.com/owner/repo.git", url)
                    .id(URL_INPUT)
                    .on_input(|v| msg(RemotesMsg::EditUrl(v)))
                    .on_submit(msg(RemotesMsg::SaveUrl)),
                ui::small("Cancel", theme::button_secondary).on_press_maybe(idle.then(|| msg(RemotesMsg::CancelEdit))),
                ui::small(save, theme::button_primary).on_press_maybe(can_save.then(|| msg(RemotesMsg::SaveUrl))),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            column![name, edit].spacing(6).into()
        }
        None => {
            // URLs have no spaces to break at.
            let url = |s: String| muted_text(s).size(12).wrapping(text::Wrapping::WordOrGlyph);
            let mut info = column![name].spacing(2).width(Length::Fill);
            info = info.push(if r.url.is_empty() { muted_text("No URL").size(12) } else { url(r.url.clone()) });
            if let Some(push) = &r.push_url {
                info = info.push(url(format!("Pushes to {push}")));
            }
            let change = ui::small("Change URL", theme::button_secondary)
                .on_press_maybe(idle.then(|| msg(RemotesMsg::Edit(r.name.clone()))));
            row![info, change].spacing(8).align_y(Alignment::Center).into()
        }
    };
    container(body).padding([8, 12]).width(Length::Fill).into()
}

impl App {
    pub(crate) fn update_remotes(&mut self, m: RemotesMsg) -> Task<Message> {
        match m {
            RemotesMsg::Open => return self.open_remotes(),
            RemotesMsg::Loaded(worktree, result) => {
                if let Some(f) = self.remotes_form().filter(|f| f.repo.worktree == worktree) {
                    f.remotes = Some(result);
                }
            }
            RemotesMsg::Edit(name) => {
                let Some(f) = self.remotes_form().filter(|f| f.busy.is_none()) else { return Task::none() };
                let remotes = f.remotes.as_ref().and_then(|r| r.as_ref().ok());
                let url = remotes.and_then(|rs| rs.iter().find(|r| r.name == name)).map(|r| r.url.clone());
                f.editing = Some((name, url.unwrap_or_default()));
                f.error = None;
                return operation::focus(URL_INPUT);
            }
            RemotesMsg::EditUrl(v) => {
                if let Some((_, url)) = self.remotes_form().and_then(|f| f.editing.as_mut()) {
                    *url = v;
                }
            }
            RemotesMsg::SaveUrl => return self.change_remotes(Change::SetUrl),
            RemotesMsg::CancelEdit => {
                if let Some(f) = self.remotes_form().filter(|f| f.busy.is_none()) {
                    f.editing = None;
                    f.error = None;
                }
            }
            RemotesMsg::Name(v) => {
                if let Some(f) = self.remotes_form() {
                    f.name = v;
                    f.error = None;
                }
            }
            RemotesMsg::Url(v) => {
                if let Some(f) = self.remotes_form() {
                    f.url = v;
                    f.error = None;
                }
            }
            RemotesMsg::Add => return self.change_remotes(Change::Add),
            RemotesMsg::Changed { worktree, change, result } => {
                match &result {
                    Ok(_) => tracing::info!(repo = %worktree.display(), ?change, "remotes changed"),
                    Err(e) => {
                        tracing::warn!(repo = %worktree.display(), ?change, error = %e, "changing remotes failed")
                    }
                }
                if let Some(f) = self.remotes_form().filter(|f| f.repo.worktree == worktree) {
                    f.busy = None;
                    match result {
                        Ok(remotes) => {
                            f.remotes = Some(Ok(remotes));
                            match change {
                                Change::Add => {
                                    f.name.clear();
                                    f.url.clear();
                                }
                                Change::SetUrl => f.editing = None,
                            }
                        }
                        Err(e) => f.error = Some(e),
                    }
                }
                // The Git tab shows the new remote or URL right away.
                if let Some(log) = self.git_log.as_mut().filter(|l| l.worktree == worktree) {
                    log.stamp = None;
                }
                return self.refresh_log();
            }
        }
        Task::none()
    }

    fn remotes_form(&mut self) -> Option<&mut RemotesForm> {
        match &mut self.dialog {
            Some(Dialog::Remotes(f)) => Some(f),
            _ => None,
        }
    }

    fn open_remotes(&mut self) -> Task<Message> {
        let Some(repo) = self.active_repo().cloned() else { return Task::none() };
        self.dialog = Some(Dialog::Remotes(Box::new(RemotesForm {
            repo: repo.clone(),
            remotes: None,
            editing: None,
            name: String::new(),
            url: String::new(),
            busy: None,
            error: None,
        })));
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking((repo.worktree.clone(), repo.remotes()));
        });
        let loaded = Task::perform(async move { rx.recv().await.ok() }, |read| match read {
            Some((worktree, result)) => msg(RemotesMsg::Loaded(worktree, result)),
            None => Message::Noop,
        });
        Task::batch([loaded, operation::focus(DIALOG_FIRST_INPUT)])
    }

    /// Adds the remote typed in, or saves the URL being changed, on a thread.
    fn change_remotes(&mut self, change: Change) -> Task<Message> {
        let Some(f) = self.remotes_form().filter(|f| f.busy.is_none()) else { return Task::none() };
        let (name, url) = match change {
            Change::Add => (f.name.trim().to_owned(), f.url.trim().to_owned()),
            Change::SetUrl => match &f.editing {
                Some((name, url)) => (name.clone(), url.trim().to_owned()),
                None => return Task::none(),
            },
        };
        if name.is_empty() || url.is_empty() {
            return Task::none();
        }
        f.busy = Some(change);
        f.error = None;
        let repo = f.repo.clone();
        let worktree = repo.worktree.clone();
        // The URL is not logged: it can hold a password or token.
        tracing::info!(repo = %worktree.display(), ?change, remote = %name, "changing remotes");
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let done = match change {
                Change::Add => repo.add_remote(&name, &url),
                Change::SetUrl => repo.set_remote_url(&name, &url),
            };
            let _ = tx.send_blocking(done.and_then(|()| repo.remotes()));
        });
        Task::perform(async move { rx.recv().await.unwrap_or_else(|e| Err(e.to_string())) }, move |result| {
            msg(RemotesMsg::Changed { worktree: worktree.clone(), change, result })
        })
    }

    pub(crate) fn view_remotes<'a>(&'a self, f: &'a RemotesForm) -> DialogParts<'a> {
        // The remote the Git tab shows the branch using.
        let in_use = self
            .git_log
            .as_ref()
            .filter(|l| l.worktree == f.repo.worktree)
            .and_then(|l| l.tracking.as_ref())
            .and_then(|t| t.remote.as_deref());
        let list: Element<'_, Message> = match &f.remotes {
            None => muted_text("Looking up remotes…").size(12).into(),
            Some(Err(e)) => ui::note_text(Tone::Danger, e),
            Some(Ok(remotes)) if remotes.is_empty() => {
                muted_text("This repository has no remotes yet.").size(12).into()
            }
            Some(Ok(remotes)) => {
                // A hairline between each remote and the next.
                let mut list = column![];
                for (i, r) in remotes.iter().enumerate() {
                    if i > 0 {
                        list = list.push(ui::divider());
                    }
                    list = list.push(remote_item(f, r, in_use == Some(r.name.as_str())));
                }
                container(ui::scroll(list).height(Length::Shrink)).max_height(280).style(theme::list_box).into()
            }
        };

        let add = column![
            subheading("Add a remote"),
            row![
                container(field(
                    "Name",
                    ui::input("upstream", &f.name)
                        .id(DIALOG_FIRST_INPUT)
                        .on_input(|v| msg(RemotesMsg::Name(v)))
                        .on_submit(msg(RemotesMsg::Add)),
                ))
                .width(150),
                container(field(
                    "URL",
                    ui::input("https://github.com/owner/repo.git", &f.url)
                        .on_input(|v| msg(RemotesMsg::Url(v)))
                        .on_submit(msg(RemotesMsg::Add)),
                ))
                .width(Length::Fill),
            ]
            .spacing(10),
            muted_text("To pull from and push to it, pick it beside Remote in the Git tab.").size(12),
        ]
        .spacing(8);

        let mut col = column![list, add].spacing(18);
        if let Some(e) = &f.error {
            col = col.push(ui::note_text(Tone::Danger, e));
        }
        let idle = f.busy.is_none();
        let confirm = if f.busy == Some(Change::Add) { "Adding…" } else { "Add remote" };
        let mut parts = DialogParts::new("Remotes", col)
            .button(ui::secondary("Close").on_press_maybe(idle.then_some(Message::DialogCancel)))
            .button(ui::primary(confirm).on_press_maybe(f.can_add().then(|| msg(RemotesMsg::Add))));
        parts.closable = idle;
        parts
    }
}
