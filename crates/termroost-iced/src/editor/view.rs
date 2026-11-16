//! The strip of open files above the terminals, and the file shown.

use iced::widget::{button, center, column, container, row, space, text, text_editor, tooltip};
use iced::{Alignment, Element, Length};

use super::text::char_column;
use super::{DocKind, EditorMsg, FILE_EDITOR, OpenFile, TextDoc, keys};
use crate::app::{App, Message};
use crate::sidebar::{display_dir, icon_button, muted_text};
use crate::theme::{self, MONO};

impl App {
    /// "Terminals", then each file the visible session has open. Nothing
    /// while it has none open.
    pub(crate) fn view_file_strip(&self) -> Element<'_, Message> {
        let Some(files) = self.session_files().filter(|f| !f.open.is_empty()) else {
            return space::vertical().height(0).into();
        };
        let tab = |label: String, selected: bool, msg: Message| {
            button(text(label).size(12).wrapping(text::Wrapping::None))
                .padding([3, 10])
                .style(theme::panel_tab(selected))
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
                icon_button("✕", Message::Editor(EditorMsg::Close(doc.id)), "Close"),
            ]
            .align_y(Alignment::Center);
            strip = strip.push(tooltip(
                item,
                container(text(display_dir(&doc.path)).size(12)).padding([4, 8]).style(container::rounded_box),
                tooltip::Position::Bottom,
            ));
        }
        container(strip.wrap()).padding([2, 8]).into()
    }

    /// A file shown in place of the terminals.
    pub(crate) fn view_doc<'a>(&'a self, doc: &'a OpenFile) -> Element<'a, Message> {
        match &doc.kind {
            DocKind::Loading => center(muted_text("Reading…")).into(),
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
                column![editor, self.status_line(doc, t)].into()
            }
        }
    }

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
