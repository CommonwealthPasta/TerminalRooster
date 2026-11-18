//! crossterm keys and mouse buttons in the core's terms.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton as CMouseButton};
use termroost_core::{Key, KeyInput, Modifiers, MouseButton, NamedKey};

pub fn modifiers(m: KeyModifiers) -> Modifiers {
    Modifiers {
        shift: m.contains(KeyModifiers::SHIFT),
        ctrl: m.contains(KeyModifiers::CONTROL),
        alt: m.contains(KeyModifiers::ALT),
        logo: m.intersects(KeyModifiers::SUPER | KeyModifiers::META),
    }
}

/// The key as the core encodes it for a terminal, or `None` for keys a
/// terminal has no use for (media keys, lone modifiers...).
pub fn key(k: &KeyEvent) -> Option<KeyInput> {
    let mut mods = modifiers(k.modifiers);
    let named = |n| Some(KeyInput { key: Key::Named(n), mods, text: None });
    match k.code {
        KeyCode::Char(c) => {
            // AltGr arrives as Ctrl+Alt with the character it makes ('@', '€').
            if mods.ctrl && mods.alt && !c.is_ascii_alphanumeric() {
                mods.ctrl = false;
                mods.alt = false;
            }
            let key = if mods.ctrl { c.to_ascii_lowercase() } else { c };
            Some(KeyInput { key: Key::Char(key.to_string()), mods, text: (!mods.ctrl).then(|| c.to_string()) })
        }
        KeyCode::Enter => named(NamedKey::Enter),
        KeyCode::Tab => named(NamedKey::Tab),
        KeyCode::BackTab => {
            Some(KeyInput { key: Key::Named(NamedKey::Tab), mods: Modifiers { shift: true, ..mods }, text: None })
        }
        KeyCode::Backspace => named(NamedKey::Backspace),
        KeyCode::Esc => named(NamedKey::Escape),
        KeyCode::Up => named(NamedKey::Up),
        KeyCode::Down => named(NamedKey::Down),
        KeyCode::Left => named(NamedKey::Left),
        KeyCode::Right => named(NamedKey::Right),
        KeyCode::Home => named(NamedKey::Home),
        KeyCode::End => named(NamedKey::End),
        KeyCode::PageUp => named(NamedKey::PageUp),
        KeyCode::PageDown => named(NamedKey::PageDown),
        KeyCode::Insert => named(NamedKey::Insert),
        KeyCode::Delete => named(NamedKey::Delete),
        KeyCode::F(n) => named(NamedKey::F(n)),
        _ => None,
    }
}

pub fn button(b: CMouseButton) -> MouseButton {
    match b {
        CMouseButton::Left => MouseButton::Left,
        CMouseButton::Right => MouseButton::Right,
        CMouseButton::Middle => MouseButton::Middle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyInput {
        key(&KeyEvent::new(code, mods)).unwrap()
    }

    #[test]
    fn characters() {
        let a = ev(KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!((a.key, a.text.as_deref()), (Key::Char("A".into()), Some("A")));
        let c = ev(KeyCode::Char('C'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!((c.key, c.text), (Key::Char("c".into()), None));
        assert!(c.mods.ctrl);
    }

    #[test]
    fn altgr_types_its_character() {
        let at = ev(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert_eq!(at.text.as_deref(), Some("@"));
        assert!(!at.mods.ctrl && !at.mods.alt);
        // Real Ctrl+Alt+letter stays a chord.
        let x = ev(KeyCode::Char('x'), KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert!(x.mods.ctrl && x.mods.alt);
    }

    #[test]
    fn shift_tab() {
        let t = ev(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(t.key, Key::Named(NamedKey::Tab));
        assert!(t.mods.shift);
    }
}
