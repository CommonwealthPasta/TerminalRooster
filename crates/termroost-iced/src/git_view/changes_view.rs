//! How the uncommitted changes look: a section for each kind of change,
//! with a button to stage (or unstage) all of it, then its files, each with
//! a button for the whole file and one for each hunk. Lines chosen in a
//! hunk are marked, and its button then stages just those.

use iced::widget::{Row, button, center, column, container, row, space, text, tooltip};
use iced::{Alignment, Element, Length, Padding};
use termroost_core::git::{FileDiff, FileStatus};

use super::GitViewMsg;
use super::changes::{ChangesDoc, Pick, Section};
use super::view::{file_body_with, file_digits, file_header};
use crate::app::{App, Message};
use crate::diff_lines::DiffLines;
use crate::editor::DocId;
use crate::files::FilesMsg;
use crate::icons::Icon;
use crate::sidebar::muted_text;
use crate::theme::{self, SEMIBOLD, Tone};
use crate::ui::{self, icon_button, tip};

/// Files shown per section; the rest are counted.
const MAX_FILES: usize = 200;

/// What the buttons of a section do to its files.
fn verb(section: Section) -> &'static str {
    match section {
        Section::Conflicts => "Mark resolved",
        Section::Staged => "Unstage",
        Section::Unstaged | Section::Untracked => "Stage",
    }
}

impl App {
    /// The uncommitted changes, in place of the terminals.
    pub(crate) fn view_changes<'a>(&'a self, doc: DocId, c: &'a ChangesDoc) -> Element<'a, Message> {
        let changes = match &c.changes {
            None => return center(muted_text("Reading the changes…").size(theme::TEXT)).into(),
            Some(Err(e)) => return container(ui::note_text(Tone::Danger, e)).padding(12).into(),
            Some(Ok(changes)) => changes,
        };
        let mut page = column![].spacing(12);
        if let Some(e) = &c.error {
            let dismiss = Message::GitView(GitViewMsg::DismissChangesError(doc));
            let line =
                row![text(e).size(theme::TEXT).width(Length::Fill), icon_button(Icon::Close, dismiss, "Dismiss")]
                    .spacing(8)
                    .align_y(Alignment::Center);
            page = page.push(ui::note(Tone::Danger, line));
        }
        let mut empty = true;
        for section in Section::ALL {
            let files: Vec<&FileDiff> = section.files(changes).collect();
            if files.is_empty() {
                continue;
            }
            empty = false;
            page = page.push(section_header(doc, section, files.len(), c.busy));
            for file in files.iter().take(MAX_FILES) {
                page = page.push(self.changed_file(doc, c, section, file, files.len()));
            }
            if files.len() > MAX_FILES {
                let more = format!("{} more files are not shown.", files.len() - MAX_FILES);
                page = page.push(container(muted_text(more).size(theme::TEXT_SM)).padding([0, 4]));
            }
        }
        if empty {
            let what = if changes.unborn {
                "Nothing to commit yet: add some files to the folder."
            } else {
                "Nothing to commit: the files are as the last commit left them."
            };
            page = page.push(container(text(what).size(theme::TEXT).style(ui::muted)).padding([8, 4]));
        }
        // In a container, so its scroll position is not the commit view's.
        container(
            ui::scroll(container(page).padding(Padding { top: 4.0, right: 14.0, bottom: 12.0, left: 4.0 }))
                .height(Length::Fill),
        )
        .into()
    }

    /// A file of a section: its header with a button for the whole file,
    /// and unless folded its hunks, each with a button of its own (when
    /// hunks can be staged one by one).
    fn changed_file<'a>(
        &'a self,
        doc: DocId,
        c: &'a ChangesDoc,
        section: Section,
        file: &'a FileDiff,
        in_section: usize,
    ) -> Element<'a, Message> {
        let busy = c.busy;
        let pick = |p: Pick| (!busy).then_some(Message::GitView(GitViewMsg::Move(doc, p)));
        let mut actions = Row::new();
        if file.status != FileStatus::Deleted {
            let path = c.repo.worktree.join(&file.path);
            actions = actions.push(icon_button(Icon::Open, Message::Files(FilesMsg::Open(path)), "Open the file"));
        }
        let whole = ui::small(verb(section), theme::button_secondary)
            .padding([2, 10])
            .on_press_maybe(pick(Pick::File(section, file.path.clone())));
        actions = actions.push(whole);
        let toggle = Message::GitView(GitViewMsg::ToggleChange(doc, section, file.path.clone()));
        let folded = c.folded(section, file, in_section);
        let mut block = column![file_header(file, folded, toggle, actions)];
        if !folded {
            let digits = file_digits(file);
            let font_size = self.prefs.font_size;
            let splittable = file.splittable();
            let chosen = |i: usize| c.chosen.as_ref().filter(|ch| ch.is(section, &file.path, i)).map(|ch| &ch.lines);
            let lines = |i: usize, hunk: &'a termroost_core::git::Hunk| {
                let lines = DiffLines::new(&hunk.lines, digits, font_size);
                if !splittable {
                    return lines;
                }
                let path = file.path.clone();
                let press = move |line, mods| {
                    Message::GitView(GitViewMsg::ChooseLine(doc, section, path.clone(), i, line, mods))
                };
                let drag = move |line| Message::GitView(GitViewMsg::DragLine(doc, line));
                lines.chosen(chosen(i)).on_choose(press, drag)
            };
            let actions = |i: usize| {
                if !splittable {
                    return Row::new();
                }
                let Some(lines) = chosen(i) else {
                    let press = pick(Pick::Hunk(section, file.path.clone(), i));
                    let hint = format!(
                        "Or press lines to {} only those: drag over them, or hold Shift for a range and Ctrl for \
                         one more",
                        verb(section).to_lowercase()
                    );
                    return row![tip(
                        hunk_button(format!("{} hunk", verb(section)), press),
                        hint,
                        tooltip::Position::Top
                    )];
                };
                let n = lines.len();
                let label = format!("{} {n} line{}", verb(section), if n == 1 { "" } else { "s" });
                let press = pick(Pick::Lines(section, file.path.clone(), i, lines.clone()));
                let clear = Message::GitView(GitViewMsg::ClearChosen(doc));
                row![hunk_button(label, press), icon_button(Icon::Close, clear, "Choose no lines")]
            };
            block = block.push(file_body_with(file, lines, actions));
        }
        container(block).width(Length::Fill).style(theme::diff_file).into()
    }
}

/// A small button on a hunk's header line.
fn hunk_button<'a>(label: String, press: Option<Message>) -> Element<'a, Message> {
    button(text(label).size(11).wrapping(text::Wrapping::None))
        .padding([1, 8])
        .style(theme::button_secondary)
        .on_press_maybe(press)
        .into()
}

/// A section's title and how many files it has, with a button for all of them.
fn section_header<'a>(doc: DocId, section: Section, files: usize, busy: bool) -> Element<'a, Message> {
    let all = match section {
        Section::Staged => "Unstage all",
        Section::Conflicts => "Mark all resolved",
        Section::Unstaged | Section::Untracked => "Stage all",
    };
    let press = (!busy).then_some(Message::GitView(GitViewMsg::Move(doc, Pick::All(section))));
    let tone = if section == Section::Conflicts { Tone::Warning } else { Tone::Neutral };
    row![
        text(section.title()).size(theme::TEXT).font(SEMIBOLD),
        ui::badge(files.to_string(), tone),
        space::horizontal(),
        ui::small(all, theme::button_secondary).on_press_maybe(press),
    ]
    .spacing(8)
    .padding(Padding { top: 6.0, right: 0.0, bottom: 0.0, left: 4.0 })
    .align_y(Alignment::Center)
    .into()
}
