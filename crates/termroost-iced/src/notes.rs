//! The right panel's Notes tab: text kept for a while, like a scratch file.
//! A note stays, across restarts, until it is closed. Notes belong to no
//! project, so every project shows the same ones.

use std::collections::HashMap;

use iced::keyboard::Key;
use iced::keyboard::key::Named;
use iced::widget::text_editor::{Action, Binding, KeyPress, Status};
use iced::widget::{column, container, mouse_area, operation, row, space, text, text_editor, tooltip};
use iced::{Alignment, Element, Length, Task, mouse};
use termroost_core::{Command, Note, NoteId, Outcome};

use crate::app::{App, Dialog, Message, RENAME_INPUT, RenameTarget};
use crate::editor::history::{self, History, Snapshot};
use crate::icons::Icon;
use crate::keymap::{self, Keymap};
use crate::sidebar::muted_text;
use crate::theme::{self, MONO, SEMIBOLD};
use crate::ui::{self, icon_button, tip};

/// Widget id of the note editor, for focusing it.
pub const NOTE_EDITOR: &str = "note-editor";

/// A note's text as it is being edited.
#[derive(Default)]
pub struct NoteDoc {
    pub content: text_editor::Content,
    history: History,
    /// Changed since the core last got its text.
    unsaved: bool,
}

impl NoteDoc {
    /// With the cursor at the end, so what is typed or pasted goes after the text.
    fn new(text: &str) -> NoteDoc {
        let mut content = text_editor::Content::with_text(text);
        content.perform(Action::Move(text_editor::Motion::DocumentEnd));
        NoteDoc { content, ..NoteDoc::default() }
    }
}

impl std::fmt::Debug for NoteDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the text: logs must not show what people paste.
        f.debug_struct("NoteDoc").field("lines", &self.content.line_count()).field("unsaved", &self.unsaved).finish()
    }
}

/// The text of the notes, and an empty page for when there are none.
#[derive(Debug, Default)]
pub struct Notes {
    docs: HashMap<NoteId, NoteDoc>,
    /// Shown while there is no note: typing in it makes one.
    draft: NoteDoc,
}

impl Notes {
    pub fn of(notes: &[Note]) -> Notes {
        Notes { docs: notes.iter().map(|n| (n.id, NoteDoc::new(&n.text))).collect(), draft: NoteDoc::default() }
    }

    /// Some note has changes the core doesn't have yet.
    pub fn unsaved(&self) -> bool {
        self.docs.values().any(|d| d.unsaved)
    }

    /// The text of each note with changes the core doesn't have yet.
    pub fn take_unsaved(&mut self) -> Vec<(NoteId, String)> {
        let changed = self.docs.iter_mut().filter(|(_, d)| d.unsaved);
        changed
            .map(|(id, d)| {
                d.unsaved = false;
                (*id, d.content.text())
            })
            .collect()
    }

    /// The text being edited for `note` (`None`: the empty page).
    fn doc_mut(&mut self, note: Option<NoteId>) -> Option<&mut NoteDoc> {
        match note {
            Some(id) => self.docs.get_mut(&id),
            None => Some(&mut self.draft),
        }
    }
}

#[derive(Clone, Debug)]
pub enum NotesMsg {
    Show(NoteId),
    New,
    /// Close a note; its text is gone.
    Close(NoteId),
    /// Close every note, after asking.
    AskCloseAll,
    CloseAll,
    /// Something done in the editor of a note (`None`: the empty page):
    /// typing, moving, selecting, scrolling.
    Action(Option<NoteId>, Action),
    Undo(Option<NoteId>),
    Redo(Option<NoteId>),
}

impl NotesMsg {
    /// Whether the message may be logged: actions carry what the user types or pastes.
    pub fn loggable(&self) -> bool {
        !matches!(self, NotesMsg::Action(..))
    }
}

impl App {
    /// The note in the editor: the one picked last, else the first.
    pub(crate) fn shown_note(&self) -> Option<NoteId> {
        let notes = &self.core.state().notes;
        self.prefs.note.filter(|id| notes.iter().any(|n| n.id == *id)).or_else(|| notes.first().map(|n| n.id))
    }

    fn show_note(&mut self, note: Option<NoteId>) {
        if self.prefs.note != note {
            self.prefs.note = note;
            self.mark_prefs();
        }
    }

    pub(crate) fn update_notes(&mut self, msg: NotesMsg) -> Task<Message> {
        match msg {
            NotesMsg::Show(id) => {
                if self.rename.as_ref().is_some_and(|r| r.target != RenameTarget::Note(id)) {
                    self.rename = None;
                }
                // The second press of a double click starts renaming it,
                // which needs the keyboard.
                if self.shown_note() == Some(id) {
                    return Task::none();
                }
                self.show_note(Some(id));
            }
            NotesMsg::New => {
                if let Some(Outcome::Note(id)) = self.dispatch(Command::CreateNote { title: None }) {
                    self.notes.docs.insert(id, NoteDoc::default());
                    self.show_note(Some(id));
                }
            }
            NotesMsg::Close(id) => {
                let notes = &self.core.state().notes;
                let next = match notes.iter().position(|n| n.id == id) {
                    // The note after it, else the one before.
                    Some(i) if self.shown_note() == Some(id) => {
                        notes.get(i + 1).or_else(|| i.checked_sub(1).and_then(|i| notes.get(i))).map(|n| n.id)
                    }
                    _ => self.shown_note(),
                };
                self.dispatch(Command::CloseNote(id));
                self.notes.docs.remove(&id);
                if self.rename.as_ref().is_some_and(|r| r.target == RenameTarget::Note(id)) {
                    self.rename = None;
                }
                self.show_note(next);
            }
            NotesMsg::AskCloseAll => {
                self.rename = None;
                self.dialog = Some(Dialog::CloseAllNotes);
                return Task::none();
            }
            NotesMsg::CloseAll => {
                self.dialog = None;
                self.dispatch(Command::CloseAllNotes);
                self.notes.docs.clear();
                self.show_note(None);
            }
            NotesMsg::Action(note, action) => return self.note_action(note, action),
            NotesMsg::Undo(note) | NotesMsg::Redo(note) => {
                let undo = matches!(msg, NotesMsg::Undo(_));
                if let Some(doc) = self.notes.doc_mut(note) {
                    let current = Snapshot::of(&doc.content, 0);
                    let target = if undo { doc.history.undo(current) } else { doc.history.redo(current) };
                    if let Some(target) = target {
                        target.restore(&mut doc.content);
                        doc.unsaved = note.is_some();
                    }
                }
                return Task::none();
            }
        }
        operation::focus(NOTE_EDITOR)
    }

    /// Typing on the empty page makes it a note.
    fn note_action(&mut self, note: Option<NoteId>, action: Action) -> Task<Message> {
        let mut task = Task::none();
        let note = match (note, &action) {
            (None, Action::Edit(_)) => {
                let Some(Outcome::Note(id)) = self.dispatch(Command::CreateNote { title: None }) else { return task };
                let draft = std::mem::take(&mut self.notes.draft);
                self.notes.docs.insert(id, draft);
                self.show_note(Some(id));
                // Its tab appears above the editor, which then loses the
                // keyboard (iced 0.14; see `editor::view`).
                task = operation::focus(NOTE_EDITOR);
                Some(id)
            }
            _ => note,
        };
        let Some(doc) = self.notes.doc_mut(note) else { return task };
        match &action {
            Action::Edit(edit) => {
                doc.history.before_edit(history::Kind::of(edit), &doc.content, 0);
                doc.content.perform(action);
                doc.unsaved = true;
            }
            // Scrolling doesn't end what is being typed; moving the cursor does.
            Action::Scroll { .. } => doc.content.perform(action),
            _ => {
                doc.history.close_step();
                doc.content.perform(action);
            }
        }
        task
    }

    /// The Notes tab of the right panel.
    pub(crate) fn view_notes(&self) -> Element<'_, Message> {
        let notes = &self.core.state().notes;
        let shown = self.shown_note();
        let count = match notes.len() {
            0 => "No notes".to_owned(),
            1 => "1 note".to_owned(),
            n => format!("{n} notes"),
        };
        let close_all: Element<'_, Message> = if notes.is_empty() {
            space::horizontal().width(0).into()
        } else {
            ui::small("Close all", theme::button_secondary).on_press(Message::Notes(NotesMsg::AskCloseAll)).into()
        };
        let header = row![
            muted_text(count),
            space::horizontal(),
            icon_button(Icon::Plus, Message::Notes(NotesMsg::New), "New note"),
            close_all,
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        let tabs = row(notes.iter().map(|n| self.note_tab(n, shown == Some(n.id)))).spacing(4).wrap();

        let doc = shown.and_then(|id| self.notes.docs.get(&id)).unwrap_or(&self.notes.draft);
        let keymap = self.keymap.clone();
        let mut editor = text_editor(&doc.content)
            .id(NOTE_EDITOR)
            .placeholder("Type or paste…")
            .font(MONO)
            .size(self.prefs.font_size)
            .padding(8)
            .wrapping(text::Wrapping::WordOrGlyph)
            .height(Length::Fill)
            .key_binding(move |press| binding(&keymap, shown, press))
            .style(theme::note_editor);
        // Keys go to a dialog, not to the editor behind it.
        if self.dialog.is_none() {
            editor = editor.on_action(move |a| Message::Notes(NotesMsg::Action(shown, a)));
        }
        column![header, tabs, editor].spacing(8).into()
    }

    /// A note's tab: its name (a double click renames it) and ✕.
    fn note_tab<'a>(&'a self, note: &'a Note, selected: bool) -> Element<'a, Message> {
        let id = note.id;
        let close = icon_button(Icon::Close, Message::Notes(NotesMsg::Close(id)), "Close");
        if let Some(r) = self.rename.as_ref().filter(|r| r.target == RenameTarget::Note(id)) {
            let input = ui::input("Name", &r.value)
                .id(RENAME_INPUT)
                .on_input(Message::RenameInput)
                .on_submit(Message::RenameCommit)
                .size(theme::TEXT_SM)
                .padding([2, 8])
                .width(150);
            return row![input, close].align_y(Alignment::Center).into();
        }
        let font = if selected { SEMIBOLD } else { theme::UI };
        let name = container(text(&note.title).size(theme::TEXT_SM).font(font).wrapping(text::Wrapping::None))
            .padding([3, 10])
            .style(theme::note_tab(selected));
        let name = mouse_area(name)
            .on_press(Message::Notes(NotesMsg::Show(id)))
            .on_double_click(Message::StartRename(RenameTarget::Note(id)))
            .interaction(mouse::Interaction::Pointer);
        row![tip(name, "Double-click to rename", tooltip::Position::Bottom), close].align_y(Alignment::Center).into()
    }
}

/// What a key press does in a note's editor (`note`, `None` for the empty
/// page), on top of iced's own keys: undo, redo, Tab, and the app's shortcuts.
fn binding(keymap: &Keymap, note: Option<NoteId>, press: KeyPress) -> Option<Binding<Message>> {
    // An editor without the keyboard takes no keys.
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    match keymap.action(&press.key, press.physical_key, press.modifiers) {
        // The terminal copy/paste shortcuts work here too (Ctrl+Insert, Shift+Insert).
        Some(keymap::Action::Copy) => return Some(Binding::Copy),
        Some(keymap::Action::Paste) => return Some(Binding::Paste),
        // App shortcuts reach the app (iced's bindings take Ctrl+Shift+C/X/V/A).
        Some(action) if !action.in_terminal() => return None,
        _ => {}
    }
    let mods = press.modifiers;
    // Not with Alt: AltGr arrives as Ctrl+Alt and types characters.
    if mods.command() && !mods.alt() {
        let msg = match press.key.to_latin(press.physical_key) {
            Some('z') if mods.shift() => Some(NotesMsg::Redo(note)),
            Some('z') => Some(NotesMsg::Undo(note)),
            Some('y') => Some(NotesMsg::Redo(note)),
            _ => None,
        };
        if let Some(msg) = msg {
            return Some(Binding::Custom(Message::Notes(msg)));
        }
    }
    match press.key.as_ref() {
        // iced lets go of the keyboard on Escape, then typing goes nowhere.
        Key::Named(Named::Escape) => None,
        Key::Named(Named::Tab) if mods.is_empty() => Some(Binding::Insert('\t')),
        _ => Binding::from_key_press(press),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use iced::keyboard::Modifiers;
    use iced::keyboard::key::{Code, Physical};

    use super::*;

    fn press(key: Key, code: Code, mods: Modifiers, text: Option<&str>) -> KeyPress {
        KeyPress {
            key: key.clone(),
            modified_key: key,
            physical_key: Physical::Code(code),
            modifiers: mods,
            text: text.map(Into::into),
            status: Status::Focused { is_hovered: false },
        }
    }

    fn describe(b: Option<Binding<Message>>) -> String {
        match b {
            None => "none".into(),
            Some(Binding::Custom(Message::Notes(NotesMsg::Undo(_)))) => "undo".into(),
            Some(Binding::Custom(Message::Notes(NotesMsg::Redo(_)))) => "redo".into(),
            Some(Binding::Insert(c)) => format!("insert {c:?}"),
            Some(Binding::Copy) => "copy".into(),
            Some(Binding::Paste) => "paste".into(),
            Some(other) => format!("{other:?}"),
        }
    }

    #[test]
    fn note_keys() {
        let keymap = Keymap::new(&BTreeMap::new());
        let b = |p| describe(binding(&keymap, Some(NoteId(1)), p));
        let ctrl = Modifiers::CTRL;
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let letter = |c: &str, code, mods: Modifiers| {
            // With Ctrl held the text is a control character, as on Windows.
            press(Key::Character(c.into()), code, mods, (!mods.control()).then_some(c))
        };
        assert_eq!(b(letter("z", Code::KeyZ, ctrl)), "undo");
        assert_eq!(b(letter("y", Code::KeyY, ctrl)), "redo");
        assert_eq!(b(letter("z", Code::KeyZ, ctrl_shift)), "redo");
        assert_eq!(b(letter("v", Code::KeyV, ctrl)), "paste");
        assert_eq!(b(letter("v", Code::KeyV, ctrl_shift)), "paste");
        assert_eq!(b(letter("a", Code::KeyA, Modifiers::empty())), "insert 'a'");
        assert_eq!(b(letter("s", Code::KeyS, ctrl)), "none", "nothing to save: notes keep themselves");
        assert_eq!(b(letter("k", Code::KeyK, ctrl_shift)), "none", "Show / hide tasks reaches the app");
        let named = |key, code, mods| press(Key::Named(key), code, mods, None);
        assert_eq!(b(named(Named::Tab, Code::Tab, Modifiers::empty())), "insert '\\t'");
        assert_eq!(b(named(Named::Tab, Code::Tab, ctrl)), "none", "Ctrl+Tab is the next session");
        assert_eq!(b(named(Named::Escape, Code::Escape, Modifiers::empty())), "none", "keeps the keyboard");
    }

    #[test]
    fn a_note_without_the_keyboard_takes_no_keys() {
        let keymap = Keymap::new(&BTreeMap::new());
        let mut p = press(Key::Character("z".into()), Code::KeyZ, Modifiers::CTRL, None);
        p.status = Status::Active;
        assert!(binding(&keymap, None, p).is_none());
    }
}
