//! The strip of open files above the terminals, and the file shown.

use iced::widget::{button, center, column, container, row, space, text, text_editor, tooltip};
use iced::{Alignment, Element, Length};

use super::text::char_column;
use super::{Conflict, DocId, DocKind, EditorMsg, FILE_EDITOR, OpenFile, TextDoc, highlight, keys};
use crate::app::{App, Message};
use crate::icons::Icon;
use crate::sidebar::{display_dir, muted_text};
use crate::terminal_view::TerminalView;
use crate::theme::{self, MONO, SEMIBOLD, Tone};
use crate::ui::{self, icon_button, tip};

impl App {
    /// "Terminals", then each file the visible session has open. Nothing
    /// while it has none open.
    pub(crate) fn view_file_strip(&self) -> Element<'_, Message> {
        let Some(files) = self.session_files().filter(|f| !f.open.is_empty()) else {
            return space::vertical().height(0).into();
        };
        let tab = |label: String, selected: bool, msg: Message| {
            let font = if selected { SEMIBOLD } else { theme::UI };
            button(text(label).size(12).font(font).wrapping(text::Wrapping::None))
                .padding([3, 10])
                .style(theme::segment(selected))
                .on_press(msg)
        };
        let mut strip = row![tab("Terminals".into(), files.shown.is_none(), Message::Editor(EditorMsg::Show(None)))]
            .spacing(4)
            .align_y(Alignment::Center);
        for doc in &files.open {
            let mut name =
                doc.path.file_name().map_or_else(|| doc.path.display().to_string(), |n| n.to_string_lossy().into());
            if doc.modified() {
                name.push_str(" ●");
            }
            let item = row![
                tab(name, files.shown == Some(doc.id), Message::Editor(EditorMsg::Show(Some(doc.id)))),
                icon_button(Icon::Close, Message::Editor(EditorMsg::Close(doc.id)), "Close"),
            ]
            .align_y(Alignment::Center);
            strip = strip.push(tip(item, display_dir(&doc.path), tooltip::Position::Bottom));
        }
        container(column![container(strip.wrap()).padding([5, 8]), ui::divider()]).into()
    }

    /// A file shown in place of the terminals.
    pub(crate) fn view_doc<'a>(&'a self, doc: &'a OpenFile) -> Element<'a, Message> {
        match &doc.kind {
            DocKind::Loading | DocKind::Restored { .. } => center(muted_text("Reading…")).into(),
            // Neovim or Vim: a terminal like the session's, with the keyboard
            // unless a dialog is open.
            DocKind::Terminal { handle, .. } => container(TerminalView::new(
                handle.clone(),
                self.keymap.clone(),
                self.keyboard_focus.clone(),
                self.dialog.is_none(),
                self.prefs.font_size,
                Message::Noop,
                Message::Noop,
            ))
            .padding(1)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::terminal_frame)
            .into(),
            DocKind::Text(t) => {
                let id = doc.id;
                let keymap = self.keymap.clone();
                let mut editor = text_editor(&t.content)
                    .id(FILE_EDITOR)
                    .font(MONO)
                    .size(self.prefs.font_size)
                    .padding(8)
                    .wrapping(text::Wrapping::WordOrGlyph)
                    .height(Length::Fill)
                    .key_binding(move |press| keys::binding(&keymap, id, press))
                    .style(theme::file_editor);
                // Keys go to a dialog, not to the editor behind it (which keeps
                // the keyboard for when the dialog closes).
                if self.dialog.is_none() {
                    editor = editor.on_action(move |a| Message::Editor(EditorMsg::Action(id, a)));
                }
                let dark = self.theme().extended_palette().is_dark;
                let editor = editor.highlight_with::<highlight::DocHighlighter>(
                    highlight::Settings::new(id, &doc.path, dark),
                    highlight::format,
                );
                // The banner goes below the text: above it, appearing moves the
                // editor, which then loses the keyboard (seen with iced 0.14;
                // cause not found). The text doesn't jump this way either.
                column![editor, conflict_banner(id, t.conflict), self.status_line(doc, t)].into()
            }
        }
    }
}

/// Under the text when the file went its own way on disk: what happened,
/// and what to do about it.
fn conflict_banner<'a>(id: DocId, conflict: Option<Conflict>) -> Element<'a, Message> {
    let small =
        |label: &'a str, msg: EditorMsg| ui::small(label, theme::button_secondary).on_press(Message::Editor(msg));
    let (message, buttons, tone) = match conflict {
        None => return space::vertical().height(0).into(),
        Some(Conflict::Changed(_)) => (
            "This file changed on disk, and you have unsaved changes here.",
            row![small("Reload", EditorMsg::Reload(id)), small("Keep mine", EditorMsg::KeepMine(id))],
            Tone::Accent,
        ),
        Some(Conflict::Deleted) => (
            "This file was deleted from disk.",
            row![small("Save it again", EditorMsg::SaveAnyway(id)), small("Close", EditorMsg::Close(id))],
            Tone::Danger,
        ),
    };
    let line = row![text(message).size(13).width(Length::Fill), buttons.spacing(6)].align_y(Alignment::Center);
    container(ui::note(tone, line)).padding([6, 0]).into()
}

impl App {
    /// Under the editor: the file, where the cursor is, and how the file is stored.
    fn status_line(&self, doc: &OpenFile, t: &TextDoc) -> Element<'_, Message> {
        let at = t.content.cursor().position;
        let column = t.content.line(at.line).map_or(0, |l| char_column(&l.text, at.column));
        let mut info = vec![format!("Ln {}, Col {}", at.line + 1, column + 1), t.format.eol.label().to_owned()];
        info.push(if t.format.bom { "UTF-8 with BOM" } else { "UTF-8" }.to_owned());
        info.push(t.indent.label());
        if t.saving {
            info.push("Saving…".to_owned());
        }
        row![
            container(muted_text(self.doc_label(&doc.path)).wrapping(text::Wrapping::None))
                .width(Length::Fill)
                .clip(true),
            muted_text(info.join("  ·  ")).wrapping(text::Wrapping::None),
        ]
        .spacing(12)
        .padding([3, 8])
        .into()
    }
}
