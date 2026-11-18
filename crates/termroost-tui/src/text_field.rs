//! A one-line text input, edited in place in the list.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextField {
    chars: Vec<char>,
    /// Insertion point, in chars.
    at: usize,
}

impl TextField {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        Self { at: chars.len(), chars }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn set(&mut self, text: &str) {
        *self = Self::new(text);
    }

    /// Inserts text at the insertion point; line breaks become spaces.
    pub fn insert(&mut self, text: &str) {
        let text = text.trim_end_matches(['\r', '\n']);
        for c in text.chars().map(|c| if c.is_control() { ' ' } else { c }) {
            self.chars.insert(self.at, c);
            self.at += 1;
        }
    }

    /// Applies an editing key. Returns `false` for keys it doesn't use.
    pub fn key(&mut self, k: &KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let word = ctrl || k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('a') if ctrl => self.at = 0,
            KeyCode::Char('e') if ctrl => self.at = self.chars.len(),
            KeyCode::Char('u') if ctrl => {
                self.chars.drain(..self.at);
                self.at = 0;
            }
            KeyCode::Char('w') | KeyCode::Char('h') if ctrl => self.delete_word(),
            KeyCode::Char(c) if !ctrl => self.insert(&c.to_string()),
            KeyCode::Backspace if word => self.delete_word(),
            KeyCode::Backspace if self.at > 0 => {
                self.at -= 1;
                self.chars.remove(self.at);
            }
            KeyCode::Delete if self.at < self.chars.len() => {
                self.chars.remove(self.at);
            }
            KeyCode::Left => self.at = self.at.saturating_sub(1),
            KeyCode::Right => self.at = (self.at + 1).min(self.chars.len()),
            KeyCode::Home => self.at = 0,
            KeyCode::End => self.at = self.chars.len(),
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }

    /// Deletes back to the start of the word before the insertion point.
    fn delete_word(&mut self) {
        let stop = |c: &char| c.is_whitespace() || matches!(c, '/' | '\\');
        let mut from = self.at;
        while from > 0 && stop(&self.chars[from - 1]) {
            from -= 1;
        }
        while from > 0 && !stop(&self.chars[from - 1]) {
            from -= 1;
        }
        self.chars.drain(from..self.at);
        self.at = from;
    }

    /// What fits in `width` columns, scrolled so the insertion point shows
    /// (keeping the end in view when it can), and the insertion point's column.
    pub fn view(&self, width: usize) -> (String, usize) {
        let w = |c: &char| c.width().unwrap_or(0);
        let width = width.max(1);
        // Start as late as still shows the insertion point and the end.
        let mut start = self.chars.len();
        let mut used = 1; // the caret after the last char
        while start > 0 && used + w(&self.chars[start - 1]) <= width {
            start -= 1;
            used += w(&self.chars[start]);
        }
        let start = start.min(self.at);
        let mut out = String::new();
        let mut col = 0;
        let mut caret = 0;
        for (i, c) in self.chars.iter().enumerate().skip(start) {
            if i == self.at {
                caret = col;
            }
            if col + w(c) > width {
                break;
            }
            out.push(*c);
            col += w(c);
        }
        if self.at >= self.chars.len() {
            caret = col;
        }
        // Text scrolled out on the left shows as an ellipsis.
        if start > 0
            && let Some(first) = out.chars().next()
        {
            let pad = " ".repeat(w(&first).saturating_sub(1));
            out = format!("…{pad}{}", &out[first.len_utf8()..]);
        }
        (out, caret.min(width - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: &mut TextField, code: KeyCode, mods: KeyModifiers) {
        f.key(&KeyEvent::new(code, mods));
    }

    #[test]
    fn editing() {
        let mut f = TextField::new("api");
        press(&mut f, KeyCode::Char('2'), KeyModifiers::NONE);
        assert_eq!(f.text(), "api2");
        press(&mut f, KeyCode::Home, KeyModifiers::NONE);
        press(&mut f, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(f.text(), "pi2");
        press(&mut f, KeyCode::End, KeyModifiers::NONE);
        press(&mut f, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(f.text(), "pi");
        f.insert("a\r\n");
        assert_eq!(f.text(), "pia");
    }

    #[test]
    fn word_deletion_stops_at_path_separators() {
        let mut f = TextField::new("~/projects/api");
        press(&mut f, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(f.text(), "~/projects/");
        press(&mut f, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(f.text(), "~/");
    }

    #[test]
    fn long_text_scrolls_to_the_caret() {
        let f = TextField::new("abcdefghij");
        assert_eq!(f.view(5), ("…hij".into(), 4));
        let mut f = f;
        press(&mut f, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(f.view(5), ("abcde".into(), 0));
        assert_eq!(TextField::new("ab").view(5), ("ab".into(), 2));
    }
}
