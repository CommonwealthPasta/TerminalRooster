//! iced keyboard events -> core key input, plus the app's shortcut table.
//!
//! App shortcuts only use Ctrl+Shift, Ctrl+Tab/PageUp/PageDown and Alt+Arrows,
//! so plain Ctrl+letter combinations keep reaching the programs in terminals.

use iced::keyboard::key::{Named, Physical};
use iced::keyboard::{Key, Modifiers};
use termroost_core as core;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    NewTab,
    SplitRight,
    SplitDown,
    ClosePane,
    NextTab,
    PrevTab,
    NextProject,
    PrevProject,
    Focus(Direction),
    ToggleTasks,
    FontBigger,
    FontSmaller,
    FontReset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// The base latin character of a key (layout independent for letters).
fn latin(key: &Key, physical: Physical) -> Option<char> {
    key.to_latin(physical).map(|c| c.to_ascii_lowercase())
}

/// Matches an app shortcut.
pub fn shortcut(key: &Key, physical: Physical, mods: Modifiers) -> Option<Shortcut> {
    let ctrl_shift = mods.control() && mods.shift() && !mods.alt();
    let ctrl_only = mods.control() && !mods.shift() && !mods.alt();
    match key {
        Key::Named(Named::Tab) if mods.control() && !mods.alt() => {
            return Some(if mods.shift() { Shortcut::PrevTab } else { Shortcut::NextTab });
        }
        Key::Named(Named::PageDown) if ctrl_only => return Some(Shortcut::NextTab),
        Key::Named(Named::PageUp) if ctrl_only => return Some(Shortcut::PrevTab),
        Key::Named(Named::PageDown) if ctrl_shift => return Some(Shortcut::NextProject),
        Key::Named(Named::PageUp) if ctrl_shift => return Some(Shortcut::PrevProject),
        Key::Named(arrow) if mods.alt() && !mods.control() && !mods.shift() => {
            let dir = match arrow {
                Named::ArrowLeft => Direction::Left,
                Named::ArrowRight => Direction::Right,
                Named::ArrowUp => Direction::Up,
                Named::ArrowDown => Direction::Down,
                _ => return None,
            };
            return Some(Shortcut::Focus(dir));
        }
        _ => {}
    }
    if !ctrl_shift {
        return None;
    }
    Some(match latin(key, physical)? {
        't' => Shortcut::NewTab,
        'e' => Shortcut::SplitRight,
        'o' => Shortcut::SplitDown,
        'w' => Shortcut::ClosePane,
        'k' => Shortcut::ToggleTasks,
        '=' | '+' => Shortcut::FontBigger,
        '-' | '_' => Shortcut::FontSmaller,
        '0' | ')' => Shortcut::FontReset,
        _ => return None,
    })
}

/// Terminal-local actions handled by the terminal widget itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermAction {
    Copy,
    Paste,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
}

pub fn term_action(key: &Key, physical: Physical, mods: Modifiers) -> Option<TermAction> {
    let ctrl_shift = mods.control() && mods.shift() && !mods.alt();
    match key {
        Key::Named(Named::Insert) if mods.shift() && !mods.control() => return Some(TermAction::Paste),
        Key::Named(Named::Insert) if mods.control() && !mods.shift() => return Some(TermAction::Copy),
        Key::Named(Named::PageUp) if mods.shift() && !mods.control() => return Some(TermAction::ScrollPageUp),
        Key::Named(Named::PageDown) if mods.shift() && !mods.control() => {
            return Some(TermAction::ScrollPageDown);
        }
        Key::Named(Named::Home) if mods.shift() && !mods.control() => return Some(TermAction::ScrollTop),
        Key::Named(Named::End) if mods.shift() && !mods.control() => return Some(TermAction::ScrollBottom),
        _ => {}
    }
    if !ctrl_shift {
        return None;
    }
    match latin(key, physical)? {
        'c' => Some(TermAction::Copy),
        'v' => Some(TermAction::Paste),
        _ => None,
    }
}

pub fn modifiers(m: Modifiers) -> core::Modifiers {
    core::Modifiers { shift: m.shift(), ctrl: m.control(), alt: m.alt(), logo: m.logo() }
}

/// Converts a key press into the core's GUI-neutral key input.
pub fn key_input(key: &Key, physical: Physical, mods: Modifiers, text: Option<&str>) -> Option<core::KeyInput> {
    let named = |k| Some(core::Key::Named(k));
    let k = match key {
        Key::Named(n) => match n {
            Named::Enter => named(core::NamedKey::Enter),
            Named::Tab => named(core::NamedKey::Tab),
            Named::Backspace => named(core::NamedKey::Backspace),
            Named::Escape => named(core::NamedKey::Escape),
            Named::Space => named(core::NamedKey::Space),
            Named::ArrowUp => named(core::NamedKey::Up),
            Named::ArrowDown => named(core::NamedKey::Down),
            Named::ArrowLeft => named(core::NamedKey::Left),
            Named::ArrowRight => named(core::NamedKey::Right),
            Named::Home => named(core::NamedKey::Home),
            Named::End => named(core::NamedKey::End),
            Named::PageUp => named(core::NamedKey::PageUp),
            Named::PageDown => named(core::NamedKey::PageDown),
            Named::Insert => named(core::NamedKey::Insert),
            Named::Delete => named(core::NamedKey::Delete),
            Named::F1 => named(core::NamedKey::F(1)),
            Named::F2 => named(core::NamedKey::F(2)),
            Named::F3 => named(core::NamedKey::F(3)),
            Named::F4 => named(core::NamedKey::F(4)),
            Named::F5 => named(core::NamedKey::F(5)),
            Named::F6 => named(core::NamedKey::F(6)),
            Named::F7 => named(core::NamedKey::F(7)),
            Named::F8 => named(core::NamedKey::F(8)),
            Named::F9 => named(core::NamedKey::F(9)),
            Named::F10 => named(core::NamedKey::F(10)),
            Named::F11 => named(core::NamedKey::F(11)),
            Named::F12 => named(core::NamedKey::F(12)),
            _ => None,
        },
        Key::Character(c) => {
            // For Ctrl/Alt combos use the latin base key so shortcuts work on any layout.
            let base = if mods.control() || mods.alt() {
                latin(key, physical).map(String::from).unwrap_or_else(|| c.to_string())
            } else {
                c.to_string()
            };
            Some(core::Key::Char(base))
        }
        Key::Unidentified => None,
    }?;
    Some(core::KeyInput { key: k, mods: modifiers(mods), text: text.map(str::to_owned) })
}
