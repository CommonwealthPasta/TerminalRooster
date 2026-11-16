//! GUI-neutral keyboard/mouse/paste encoding into terminal byte sequences.

use alacritty_terminal::term::TermMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, logo: false };

    fn any(self) -> bool {
        self.shift || self.ctrl || self.alt
    }

    /// xterm modifier parameter (1 + shift + 2*alt + 4*ctrl).
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Enter,
    Tab,
    Backspace,
    Escape,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1..=F12
    F(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Named(NamedKey),
    /// The unmodified character of the key (e.g. "a" for Ctrl+A).
    Char(String),
}

/// A key press as reported by the front end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub key: Key,
    pub mods: Modifiers,
    /// Text the key produces with the current layout and modifiers, if any.
    pub text: Option<String>,
}

/// Encodes a key press for the terminal, or `None` if it produces nothing.
pub fn encode_key(input: &KeyInput, mode: TermMode) -> Option<Vec<u8>> {
    let mods = input.mods;
    let kitty = mode.contains(TermMode::DISAMBIGUATE_ESC_CODES);
    match &input.key {
        Key::Named(named) => encode_named(*named, mods, mode, kitty),
        Key::Char(c) => encode_char(c, input.text.as_deref(), mods, kitty),
    }
}

fn csi_u(code: u32, mods: Modifiers) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[{code};{}u", mods.param()).into_bytes()
    } else {
        format!("\x1b[{code}u").into_bytes()
    }
}

fn encode_named(key: NamedKey, mods: Modifiers, mode: TermMode, kitty: bool) -> Option<Vec<u8>> {
    let esc_prefix = |bytes: &[u8]| -> Vec<u8> {
        let mut v = Vec::with_capacity(bytes.len() + 1);
        if mods.alt {
            v.push(0x1b);
        }
        v.extend_from_slice(bytes);
        v
    };
    // CSI sequences with an xterm modifier parameter.
    let cursor = |final_byte: char| -> Vec<u8> {
        if mods.any() {
            format!("\x1b[1;{}{final_byte}", mods.param()).into_bytes()
        } else if mode.contains(TermMode::APP_CURSOR) {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if mods.any() { format!("\x1b[{n};{}~", mods.param()).into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    Some(match key {
        NamedKey::Enter => {
            if kitty && mods.any() {
                csi_u(13, mods)
            } else if mods.shift || mods.alt {
                // ESC CR: inserts a newline in Claude Code, Codex and readline.
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        NamedKey::Tab => {
            if kitty && (mods.ctrl || mods.alt) {
                csi_u(9, mods)
            } else if mods.shift {
                b"\x1b[Z".to_vec()
            } else {
                esc_prefix(b"\t")
            }
        }
        NamedKey::Backspace => {
            if kitty && (mods.ctrl || mods.shift) {
                csi_u(127, mods)
            } else if mods.ctrl {
                esc_prefix(b"\x08")
            } else {
                esc_prefix(b"\x7f")
            }
        }
        NamedKey::Escape => {
            if kitty {
                csi_u(27, mods)
            } else {
                esc_prefix(b"\x1b")
            }
        }
        NamedKey::Space => {
            if kitty && (mods.ctrl || mods.shift && mods.alt) {
                csi_u(32, mods)
            } else if mods.ctrl {
                esc_prefix(b"\x00")
            } else {
                esc_prefix(b" ")
            }
        }
        NamedKey::Up => cursor('A'),
        NamedKey::Down => cursor('B'),
        NamedKey::Right => cursor('C'),
        NamedKey::Left => cursor('D'),
        NamedKey::Home => cursor('H'),
        NamedKey::End => cursor('F'),
        NamedKey::Insert => tilde(2),
        NamedKey::Delete => tilde(3),
        NamedKey::PageUp => tilde(5),
        NamedKey::PageDown => tilde(6),
        NamedKey::F(n @ 1..=4) => {
            let c = (b'P' + (n - 1)) as char;
            if mods.any() {
                format!("\x1b[1;{}{c}", mods.param()).into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        NamedKey::F(n @ 5..=12) => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde(CODES[(n - 5) as usize])
        }
        NamedKey::F(_) => return None,
    })
}

fn encode_char(key: &str, text: Option<&str>, mods: Modifiers, kitty: bool) -> Option<Vec<u8>> {
    let mut chars = key.chars();
    let single = match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    };

    if mods.ctrl {
        let c = single?.to_ascii_lowercase();
        if kitty {
            return Some(csi_u(c as u32, mods));
        }
        let ctrl = match c {
            'a'..='z' => Some(c as u8 - b'a' + 1),
            '@' | '2' | ' ' => Some(0),
            '[' | '3' => Some(0x1b),
            '\\' | '4' => Some(0x1c),
            ']' | '5' => Some(0x1d),
            '^' | '6' => Some(0x1e),
            '_' | '-' | '/' | '7' => Some(0x1f),
            '8' | '?' => Some(0x7f),
            _ => None,
        }?;
        let mut v = Vec::with_capacity(2);
        if mods.alt {
            v.push(0x1b);
        }
        v.push(ctrl);
        return Some(v);
    }

    let produced = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control)).unwrap_or(key);
    if produced.is_empty() {
        return None;
    }
    if mods.alt {
        if kitty && let Some(c) = single {
            return Some(csi_u(c.to_ascii_lowercase() as u32, mods));
        }
        let mut v = vec![0x1b];
        v.extend_from_slice(produced.as_bytes());
        return Some(v);
    }
    Some(produced.as_bytes().to_vec())
}

/// Encodes pasted text, using bracketed paste when the program asked for it.
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        // Strip ESC so pasted text cannot end the bracket early.
        let body: String = text.chars().filter(|&c| c != '\x1b').collect();
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(body.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(MouseButton),
    Release(MouseButton),
    /// Pointer moved; `held` is the button being dragged, if any.
    Move {
        held: Option<MouseButton>,
    },
    WheelUp,
    WheelDown,
}

/// A mouse event in viewport cell coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseInput {
    pub kind: MouseEventKind,
    pub col: usize,
    pub row: usize,
    pub mods: Modifiers,
}

/// Encodes a mouse report if the program enabled mouse reporting for it.
pub fn encode_mouse(input: &MouseInput, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let button_code = |b: MouseButton| match b {
        MouseButton::Left => 0u8,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut code, pressed) = match input.kind {
        MouseEventKind::Press(b) => (button_code(b), true),
        MouseEventKind::Release(b) => (if mode.contains(TermMode::SGR_MOUSE) { button_code(b) } else { 3 }, false),
        MouseEventKind::Move { held } => {
            match held {
                Some(_) if !mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => {
                    return None;
                }
                None if !mode.contains(TermMode::MOUSE_MOTION) => return None,
                _ => {}
            }
            (32 + held.map(button_code).unwrap_or(3), true)
        }
        MouseEventKind::WheelUp => (64, true),
        MouseEventKind::WheelDown => (65, true),
    };
    if input.mods.shift {
        code += 4;
    }
    if input.mods.alt {
        code += 8;
    }
    if input.mods.ctrl {
        code += 16;
    }

    let (col, row) = (input.col + 1, input.row + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let c = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{code};{col};{row}{c}").into_bytes());
    }
    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let max = if utf8 { 2015 } else { 223 };
    if col > max || row > max {
        return None;
    }
    let mut v = vec![0x1b, b'[', b'M', 32 + code];
    for pos in [col, row] {
        let value = 32 + pos as u32;
        if utf8 {
            let mut buf = [0; 4];
            v.extend_from_slice(char::from_u32(value)?.encode_utf8(&mut buf).as_bytes());
        } else {
            v.push(value as u8);
        }
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(k: NamedKey, mods: Modifiers) -> KeyInput {
        KeyInput { key: Key::Named(k), mods, text: None }
    }

    fn ch(c: &str, text: Option<&str>, mods: Modifiers) -> KeyInput {
        KeyInput { key: Key::Char(c.into()), mods, text: text.map(Into::into) }
    }

    const CTRL: Modifiers = Modifiers { ctrl: true, ..Modifiers::NONE };
    const ALT: Modifiers = Modifiers { alt: true, ..Modifiers::NONE };
    const SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::NONE };

    fn enc(i: KeyInput, mode: TermMode) -> Vec<u8> {
        encode_key(&i, mode).unwrap_or_default()
    }

    #[test]
    fn plain_and_control() {
        let m = TermMode::default();
        assert_eq!(enc(ch("a", Some("a"), Modifiers::NONE), m), b"a");
        assert_eq!(enc(ch("a", Some("A"), SHIFT), m), b"A");
        assert_eq!(enc(ch("c", None, CTRL), m), [0x03]);
        assert_eq!(enc(ch("[", None, CTRL), m), [0x1b]);
        assert_eq!(enc(ch("x", Some("x"), ALT), m), b"\x1bx");
        assert_eq!(enc(ch("é", Some("é"), Modifiers::NONE), m), "é".as_bytes());
        assert_eq!(enc(named(NamedKey::Enter, Modifiers::NONE), m), b"\r");
        assert_eq!(enc(named(NamedKey::Enter, SHIFT), m), b"\x1b\r");
        assert_eq!(enc(named(NamedKey::Backspace, Modifiers::NONE), m), [0x7f]);
        assert_eq!(enc(named(NamedKey::Tab, SHIFT), m), b"\x1b[Z");
        assert_eq!(enc(named(NamedKey::Escape, Modifiers::NONE), m), [0x1b]);
    }

    #[test]
    fn cursor_keys_follow_mode() {
        let normal = TermMode::default();
        let app = TermMode::default() | TermMode::APP_CURSOR;
        assert_eq!(enc(named(NamedKey::Up, Modifiers::NONE), normal), b"\x1b[A");
        assert_eq!(enc(named(NamedKey::Up, Modifiers::NONE), app), b"\x1bOA");
        assert_eq!(enc(named(NamedKey::Left, CTRL), app), b"\x1b[1;5D");
        assert_eq!(enc(named(NamedKey::Delete, Modifiers::NONE), normal), b"\x1b[3~");
        assert_eq!(enc(named(NamedKey::PageUp, SHIFT), normal), b"\x1b[5;2~");
        assert_eq!(enc(named(NamedKey::F(1), Modifiers::NONE), normal), b"\x1bOP");
        assert_eq!(enc(named(NamedKey::F(5), Modifiers::NONE), normal), b"\x1b[15~");
        assert_eq!(enc(named(NamedKey::F(12), CTRL), normal), b"\x1b[24;5~");
    }

    #[test]
    fn kitty_disambiguation() {
        let k = TermMode::default() | TermMode::DISAMBIGUATE_ESC_CODES;
        assert_eq!(enc(named(NamedKey::Enter, SHIFT), k), b"\x1b[13;2u");
        assert_eq!(enc(named(NamedKey::Enter, Modifiers::NONE), k), b"\r");
        assert_eq!(enc(named(NamedKey::Escape, Modifiers::NONE), k), b"\x1b[27u");
        assert_eq!(enc(ch("c", None, CTRL), k), b"\x1b[99;5u");
    }

    #[test]
    fn paste() {
        let m = TermMode::default();
        assert_eq!(encode_paste("a\nb\r\nc", m), b"a\rb\rc");
        let b = m | TermMode::BRACKETED_PASTE;
        assert_eq!(encode_paste("x\x1by\n", b), b"\x1b[200~xy\n\x1b[201~");
    }

    #[test]
    fn mouse() {
        let off = TermMode::default();
        let ev = MouseInput { kind: MouseEventKind::Press(MouseButton::Left), col: 4, row: 2, mods: Modifiers::NONE };
        assert_eq!(encode_mouse(&ev, off), None);
        let sgr = off | TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert_eq!(encode_mouse(&ev, sgr).unwrap(), b"\x1b[<0;5;3M");
        let rel = MouseInput { kind: MouseEventKind::Release(MouseButton::Left), ..ev };
        assert_eq!(encode_mouse(&rel, sgr).unwrap(), b"\x1b[<0;5;3m");
        let wheel = MouseInput { kind: MouseEventKind::WheelDown, ..ev };
        assert_eq!(encode_mouse(&wheel, sgr).unwrap(), b"\x1b[<65;5;3M");
        let mv = MouseInput { kind: MouseEventKind::Move { held: None }, ..ev };
        assert_eq!(encode_mouse(&mv, sgr), None, "plain motion needs MOUSE_MOTION");
        let x10 = off | TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(encode_mouse(&ev, x10).unwrap(), vec![0x1b, b'[', b'M', 32, 37, 35]);
    }
}
