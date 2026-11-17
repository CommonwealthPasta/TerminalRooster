//! How commits and their changes look: a card with the message and who
//! made it, then each file in a frame, its hunks under it.

use iced::widget::{Column, Row, button, center, column, container, row, space, text};
use iced::{Alignment, Element, Length};
use termroost_core::git::{CommitDetails, FileDiff, FileStatus, Hunk, Signature};

use super::{COMMIT_SCROLL, CommitDoc, GitViewMsg};
use crate::app::{App, Message};
use crate::diff_lines::{self, DiffLines};
use crate::editor::DocId;
use crate::files::FilesMsg;
use crate::icons::{Icon, icon};
use crate::sidebar::muted_text;
use crate::theme::{self, MONO, SEMIBOLD, Tone};
use crate::ui::{self, icon_button};

/// Width of the labels before a commit's author, committer, id and parents.
const LABEL_WIDTH: f32 = 76.0;

impl App {
    /// A commit, in place of the terminals.
    pub(crate) fn view_commit<'a>(&'a self, doc: DocId, c: &'a CommitDoc) -> Element<'a, Message> {
        let d = match &c.details {
            None => return center(muted_text("Reading the commit…").size(theme::TEXT)).into(),
            Some(Err(e)) => return container(ui::note_text(Tone::Danger, e)).padding(12).into(),
            Some(Ok(d)) => d,
        };
        // Its branches and tags, as the log shows them.
        let log = self.git_log.as_ref().filter(|l| l.worktree == c.repo.worktree);
        let refs = log
            .and_then(|l| l.commits.as_ref()?.as_ref().ok()?.iter().find(|x| x.id == d.id))
            .map_or(&[][..], |x| &x.refs[..]);
        let mut page = column![commit_card(d, refs), summary(&d.files)].spacing(12);
        for file in &d.files {
            let open = (file.status != FileStatus::Deleted).then(|| {
                let path = c.repo.worktree.join(&file.path);
                icon_button(Icon::Open, Message::Files(FilesMsg::Open(path)), "Open the file as it is now")
            });
            let toggle = Message::GitView(GitViewMsg::ToggleFile(doc, file.path.clone()));
            let folded = c.folded(file);
            let mut block = column![file_header(file, folded, toggle, row(open))];
            if !folded {
                block = block.push(file_body(file, self.prefs.font_size));
            }
            page = page.push(container(block).width(Length::Fill).style(theme::diff_file));
        }
        ui::scroll(container(page).padding(iced::Padding { top: 4.0, right: 14.0, bottom: 12.0, left: 4.0 }))
            .id(COMMIT_SCROLL)
            .height(Length::Fill)
            .into()
    }
}

/// The message, then who made the commit and when, its id and parents.
fn commit_card<'a>(d: &'a CommitDetails, refs: &'a [String]) -> Element<'a, Message> {
    let (subject, body) = match d.message.split_once('\n') {
        Some((subject, body)) => (subject, body.trim()),
        None => (d.message.as_str(), ""),
    };
    let mut card = column![text(subject).size(theme::TEXT_TITLE).font(SEMIBOLD)].spacing(10);
    if !body.is_empty() {
        card = card.push(text(body).size(theme::TEXT));
    }
    if !refs.is_empty() {
        let badges = refs.iter().map(|r| ui::badge(r.as_str(), Tone::Neutral));
        card = card.push(row(badges).spacing(4).wrap().vertical_spacing(4));
    }
    let now = chrono::Utc::now().timestamp();
    let mut facts = column![fact("Author", signature(&d.author, now))].spacing(4);
    let other = (&d.committer.name, &d.committer.email) != (&d.author.name, &d.author.email);
    if other || d.committer.time.abs_diff(d.author.time) > 60 {
        facts = facts.push(fact("Committed", signature(&d.committer, now)));
    }
    let id = row![
        text(&d.id).font(MONO).size(theme::TEXT_SM),
        icon_button(Icon::Copy, Message::CopyText(d.id.clone()), "Copy the commit's id"),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    facts = facts.push(fact("Commit", id.into()));
    if !d.parents.is_empty() {
        let label = if d.parents.len() > 1 { "Parents" } else { "Parent" };
        let links = d.parents.iter().map(|p| {
            let short = p.get(..7).unwrap_or(p);
            button(text(short).font(MONO).size(theme::TEXT_SM))
                .padding([1, 4])
                .style(theme::button_link(Tone::Neutral))
                .on_press(Message::GitView(GitViewMsg::OpenCommit(p.clone())))
                .into()
        });
        facts = facts.push(fact(label, row(links).spacing(4).into()));
    }
    container(card.push(facts)).padding([12, 14]).width(Length::Fill).style(theme::card).into()
}

/// One line of a commit's facts: a label, then what it says.
fn fact<'a>(label: &'a str, value: Element<'a, Message>) -> Element<'a, Message> {
    row![text(label).size(theme::TEXT_SM).style(ui::muted).width(LABEL_WIDTH), value].align_y(Alignment::Center).into()
}

/// "Ann <ann@example.com> · 2026-11-17 14:03 (3h ago)".
fn signature<'a>(s: &Signature, now: i64) -> Element<'a, Message> {
    let when = chrono::DateTime::from_timestamp(s.time, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default();
    row![
        text(format!("{} <{}>", s.name, s.email)).size(theme::TEXT_SM),
        muted_text(format!("{when} ({})", crate::git_panel::ago(s.time, now))).size(theme::TEXT_SM),
    ]
    .spacing(8)
    .wrap()
    .into()
}

/// "3 files changed  +120 −33".
fn summary<'a>(files: &[FileDiff]) -> Element<'a, Message> {
    let (added, removed) = files.iter().map(FileDiff::counts).fold((0, 0), |(a, r), (fa, fr)| (a + fa, r + fr));
    let n = files.len();
    let files = if n == 1 { "1 file changed".to_owned() } else { format!("{n} files changed") };
    container(counts_row(row![text(files).size(theme::TEXT_SM).font(SEMIBOLD)], added, removed)).padding([0, 4]).into()
}

/// `content`, then the lines added and removed in green and red (unless
/// no line changed, as in a binary file).
fn counts_row<'a>(content: Row<'a, Message>, added: usize, removed: usize) -> Row<'a, Message> {
    let content = content.spacing(8).align_y(Alignment::Center);
    if added == 0 && removed == 0 {
        return content;
    }
    content
        .push(text(format!("+{added}")).size(theme::TEXT_SM).font(MONO).style(theme::diff_count(true)))
        .push(text(format!("−{removed}")).size(theme::TEXT_SM).font(MONO).style(theme::diff_count(false)))
}

/// The bar over a file: pressed, it folds or unfolds the file. Its status,
/// path (both, for a rename) and counts, then `actions`.
pub(super) fn file_header<'a>(
    file: &'a FileDiff,
    folded: bool,
    toggle: Message,
    actions: Row<'a, Message>,
) -> Element<'a, Message> {
    let chevron = icon(if folded { Icon::ChevronRight } else { Icon::ChevronDown }, 13.0, theme::muted);
    let mut name = row![chevron, status_badge(file.status)].spacing(8).align_y(Alignment::Center);
    if let Some(old) = &file.old_path {
        name = name.push(text(old).size(theme::TEXT).style(ui::muted).wrapping(text::Wrapping::WordOrGlyph));
        name = name.push(text("→").size(theme::TEXT).style(ui::muted));
    }
    name = name.push(text(&file.path).size(theme::TEXT).font(SEMIBOLD).wrapping(text::Wrapping::WordOrGlyph));
    let (added, removed) = file.counts();
    let name = counts_row(name, added, removed);
    let toggle = button(name).padding(0).style(theme::button_bare).on_press(toggle).width(Length::Fill);
    container(row![toggle, actions.spacing(2).align_y(Alignment::Center)].spacing(8).align_y(Alignment::Center))
        .padding([6, 10])
        .width(Length::Fill)
        .style(theme::diff_file_header)
        .into()
}

/// A letter for how a file changed, in a badge.
fn status_badge<'a>(status: FileStatus) -> Element<'a, Message> {
    let tone = match status {
        FileStatus::Added | FileStatus::Untracked => Tone::Success,
        FileStatus::Deleted => Tone::Danger,
        FileStatus::Conflicted => Tone::Warning,
        FileStatus::Modified | FileStatus::Renamed | FileStatus::Copied => Tone::Accent,
    };
    let label = match status {
        FileStatus::Untracked => "New".to_owned(),
        FileStatus::Conflicted => "Conflict".to_owned(),
        other => other.letter().to_string(),
    };
    ui::badge(label, tone)
}

/// What a file's frame says instead of (or besides) lines, if anything.
fn file_note(file: &FileDiff) -> Option<String> {
    let mode = file.mode.as_ref().map(|(old, new)| match (old.as_str(), new.as_str()) {
        ("100644", "100755") => "Made executable.".to_owned(),
        ("100755", "100644") => "No longer executable.".to_owned(),
        (old, new) => format!("Mode {old} → {new}."),
    });
    let what = if file.binary {
        Some("A binary file: no lines to show.".to_owned())
    } else if file.large {
        Some("Too big to show.".to_owned())
    } else if file.status == FileStatus::Conflicted {
        Some("Both sides changed it. Resolve the conflict in the file, then stage it.".to_owned())
    } else if !file.hunks.is_empty() {
        None
    } else if matches!(file.status, FileStatus::Renamed | FileStatus::Copied) {
        Some("Its text is the same.".to_owned())
    } else if file.mode.is_none() {
        Some("An empty file.".to_owned())
    } else {
        None
    };
    let lossy = file.lossy.then(|| "Not UTF-8 text: characters that don't read show as �.".to_owned());
    let notes: Vec<String> = [mode, what, lossy].into_iter().flatten().collect();
    (!notes.is_empty()).then(|| notes.join(" "))
}

/// A file's hunks, with notes about it first. Hunks show their lines as
/// `lines` makes them, under a header line with `actions(i)`.
pub(super) fn file_body_with<'a>(
    file: &'a FileDiff,
    mut lines: impl FnMut(usize, &'a Hunk) -> DiffLines<'a, Message>,
    mut actions: impl FnMut(usize) -> Row<'a, Message>,
) -> Element<'a, Message> {
    let mut body = Column::new();
    if let Some(note) = file_note(file) {
        body = body.push(container(text(note).size(theme::TEXT_SM).style(ui::muted)).padding([6, 12]));
    }
    for (i, hunk) in file.hunks.iter().enumerate() {
        let where_ = text(format!("{} {}", hunk.header(), hunk.context))
            .font(MONO)
            .size(theme::TEXT_SM)
            .wrapping(text::Wrapping::None);
        let header = row![container(where_).width(Length::Fill).clip(true), actions(i).spacing(6)]
            .spacing(8)
            .align_y(Alignment::Center);
        body = body.push(container(header).padding([3, 10]).width(Length::Fill).style(theme::hunk_header));
        body = body.push(lines(i, hunk));
    }
    container(body.push(space::vertical().height(4))).width(Length::Fill).into()
}

/// A file's hunks, to look at.
fn file_body<'a>(file: &'a FileDiff, font_size: f32) -> Element<'a, Message> {
    let digits = file_digits(file);
    file_body_with(file, |_, hunk| DiffLines::new(&hunk.lines, digits, font_size), |_| Row::new())
}

/// How many digits the line numbers of a file take.
pub(super) fn file_digits(file: &FileDiff) -> usize {
    let last = file.hunks.iter().map(|h| (h.old_start + h.old_lines).max(h.new_start + h.new_lines)).max();
    diff_lines::digits(last.unwrap_or(1))
}
