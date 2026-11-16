//! Keys in the file editor, on top of iced's own: saving, indenting, and
//! letting the app's shortcuts through.

use iced::keyboard::Key;
use iced::keyboard::key::Named;
use iced::widget::text_editor::{Binding, KeyPress, Status};

use super::{DocId, EditorMsg};
use crate::app::Message;
use crate::keymap::{Action, Keymap};

/// What a key press does in the editor of file `doc`. Ctrl+S and the others
/// are handled here and not as app shortcuts, so terminals keep those keys.
pub fn binding(keymap: &Keymap, doc: DocId, press: KeyPress) -> Option<Binding<Message>> {
    // iced's own bindings check this; a replacement has to as well, or an
    // editor without the keyboard would take keys.
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    let editor = |msg| Some(Binding::Custom(Message::Editor(msg)));
    match keymap.action(&press.key, press.physical_key, press.modifiers) {
        // The terminal copy/paste shortcuts work here too (Ctrl+Insert, Shift+Insert).
        Some(Action::Copy) => return Some(Binding::Copy),
        Some(Action::Paste) => return Some(Binding::Paste),
        // App shortcuts reach the app (iced's bindings take Ctrl+Shift+C/X/V/A).
        Some(action) if !action.in_terminal() => return None,
        _ => {}
    }
    let mods = press.modifiers;
    // Not with Alt: AltGr arrives as Ctrl+Alt and types characters.
    if mods.command() && !mods.alt() {
        match press.key.to_latin(press.physical_key) {
            Some('s') => return editor(EditorMsg::Save(doc)),
            Some('z') if mods.shift() => return editor(EditorMsg::Redo(doc)),
            Some('z') => return editor(EditorMsg::Undo(doc)),
            Some('y') => return editor(EditorMsg::Redo(doc)),
            _ => {}
        }
    }
    match press.key.as_ref() {
        // iced lets go of the keyboard on Escape, then typing goes nowhere.
        Key::Named(Named::Escape) => None,
        Key::Named(Named::Tab) if !mods.control() && !mods.alt() && !mods.logo() => {
            editor(EditorMsg::Indent(doc, mods.shift()))
        }
        _ => Binding::from_key_press(press),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use iced::keyboard::Modifiers;
    use iced::keyboard::key::{Code, Physical};

    use super::*;

    fn press(key: Key, code: Code, mods: Modifiers, text: Option<&str>, focused: bool) -> KeyPress {
        KeyPress {
            key: key.clone(),
            modified_key: key,
            physical_key: Physical::Code(code),
            modifiers: mods,
            text: text.map(Into::into),
            status: if focused { Status::Focused { is_hovered: false } } else { Status::Active },
        }
    }

    fn letter(c: &str, code: Code, mods: Modifiers) -> KeyPress {
        // With Ctrl held the text is a control character, as on Windows.
        let text = if mods.control() { None } else { Some(c) };
        press(Key::Character(c.into()), code, mods, text, true)
    }

    fn describe(b: Option<Binding<Message>>) -> String {
        match b {
            None => "none".into(),
            Some(Binding::Custom(Message::Editor(EditorMsg::Save(_)))) => "save".into(),
            Some(Binding::Custom(Message::Editor(EditorMsg::Undo(_)))) => "undo".into(),
            Some(Binding::Custom(Message::Editor(EditorMsg::Redo(_)))) => "redo".into(),
            Some(Binding::Custom(Message::Editor(EditorMsg::Indent(_, back)))) => format!("indent back={back}"),
            Some(Binding::Insert(c)) => format!("insert {c}"),
            Some(Binding::Copy) => "copy".into(),
            Some(Binding::Paste) => "paste".into(),
            Some(other) => format!("{other:?}"),
        }
    }

    #[test]
    fn editor_keys() {
        let keymap = Keymap::new(&BTreeMap::new());
        let doc = DocId(1);
        let b = |p| describe(binding(&keymap, doc, p));
        let ctrl = Modifiers::CTRL;
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        assert_eq!(b(letter("s", Code::KeyS, ctrl)), "save");
        assert_eq!(b(letter("z", Code::KeyZ, ctrl)), "undo");
        assert_eq!(b(letter("y", Code::KeyY, ctrl)), "redo");
        assert_eq!(b(letter("z", Code::KeyZ, ctrl_shift)), "redo");
        assert_eq!(b(letter("a", Code::KeyA, Modifiers::empty())), "insert a");
        assert_eq!(b(letter("f", Code::KeyF, ctrl_shift)), "none", "Show / hide files reaches the app");
        assert_eq!(b(letter("c", Code::KeyC, ctrl_shift)), "copy");
        assert_eq!(b(letter("v", Code::KeyV, ctrl_shift)), "paste");
        let insert = |mods| press(Key::Named(Named::Insert), Code::Insert, mods, None, true);
        assert_eq!(b(insert(ctrl)), "copy", "Ctrl+Insert");
        assert_eq!(b(insert(Modifiers::SHIFT)), "paste", "Shift+Insert");
        let named = |key, code, mods| press(Key::Named(key), code, mods, None, true);
        assert_eq!(b(named(Named::Escape, Code::Escape, Modifiers::empty())), "none", "keeps the keyboard");
        assert_eq!(b(named(Named::Tab, Code::Tab, Modifiers::empty())), "indent back=false");
        assert_eq!(b(named(Named::Tab, Code::Tab, Modifiers::SHIFT)), "indent back=true");
        assert_eq!(b(named(Named::Tab, Code::Tab, ctrl)), "none", "Ctrl+Tab is the next session");
        let alt_gr = press(Key::Character("@".into()), Code::KeyQ, ctrl | Modifiers::ALT, Some("@"), true);
        assert_eq!(b(alt_gr), "insert @", "AltGr types");
    }

    #[test]
    fn an_editor_without_the_keyboard_takes_no_keys() {
        let keymap = Keymap::new(&BTreeMap::new());
        let p = press(Key::Character("s".into()), Code::KeyS, Modifiers::CTRL, None, false);
        assert!(binding(&keymap, DocId(1), p).is_none());
    }
}
