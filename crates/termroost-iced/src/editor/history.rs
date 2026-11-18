//! Undo and redo for the file editor (iced's editor has neither).
//!
//! Each step is a copy of the whole text from before it, with the cursor.
//! Typing in a row makes one step; a pause, a different kind of change or
//! moving the cursor starts the next.

use std::time::{Duration, Instant};

use iced::widget::text_editor::{Content, Cursor, Edit, Position};

/// Typing that pauses this long starts a new step.
const PAUSE: Duration = Duration::from_secs(1);
/// Steps kept per file, and their total size, before the oldest are dropped.
const MAX_STEPS: usize = 200;
const MAX_BYTES: usize = 64 * 1024 * 1024;

/// The text at one point, to go back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The lines joined with `\n`.
    pub text: String,
    pub cursor: (usize, usize),
    /// The document version of this text (see `TextDoc::version`).
    pub version: u64,
}

impl Snapshot {
    pub fn of(content: &Content, version: u64) -> Snapshot {
        let text = content.lines().map(|l| l.text).collect::<Vec<_>>().join("\n");
        let at = content.cursor().position;
        Snapshot { text, cursor: (at.line, at.column), version }
    }

    /// Puts this text and cursor into `content`.
    pub fn restore(&self, content: &mut Content) {
        use iced::widget::text_editor::Action;
        content.perform(Action::SelectAll);
        if self.text.is_empty() {
            content.perform(Action::Edit(Edit::Delete));
        } else {
            content.perform(Action::Edit(Edit::Paste(std::sync::Arc::new(self.text.clone()))));
        }
        // On a character boundary of an existing line, or Backspace would panic.
        let line = self.cursor.0.min(content.line_count().saturating_sub(1));
        let text = content.line(line).map(|l| l.text.into_owned()).unwrap_or_default();
        let mut column = self.cursor.1.min(text.len());
        while !text.is_char_boundary(column) {
            column -= 1;
        }
        content.move_to(Cursor { position: Position { line, column }, selection: None });
    }
}

/// What an edit does, for grouping edits into steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Word,
    Space,
    Delete,
    /// Enter, paste, indent: a step of its own.
    Single,
}

impl Kind {
    pub fn of(edit: &Edit) -> Kind {
        match edit {
            Edit::Insert(c) if c.is_whitespace() => Kind::Space,
            Edit::Insert(_) => Kind::Word,
            Edit::Backspace | Edit::Delete => Kind::Delete,
            Edit::Paste(_) | Edit::Enter | Edit::Indent | Edit::Unindent => Kind::Single,
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The kind of the step being typed, and when its last edit was.
    open: Option<(Kind, Instant)>,
}

impl History {
    /// Before an edit of `kind`: whether it starts a new step, so the text
    /// before it has to be kept.
    pub fn starts_step(&self, kind: Kind, now: Instant) -> bool {
        match self.open {
            Some((open, last)) => kind == Kind::Single || kind != open || now.duration_since(last) > PAUSE,
            None => true,
        }
    }

    /// Keeps the text before a new step. Whatever was undone can't be redone after it.
    pub fn push(&mut self, before: Snapshot) {
        self.undo.push(before);
        self.redo.clear();
        let mut bytes: usize = self.undo.iter().map(|s| s.text.len()).sum();
        while self.undo.len() > MAX_STEPS || (bytes > MAX_BYTES && self.undo.len() > 1) {
            bytes -= self.undo.remove(0).text.len();
        }
    }

    /// Before an edit of `kind` to `content` (at `version`): keeps the text
    /// for undo if the edit starts a new step, and notes the edit.
    pub fn before_edit(&mut self, kind: Kind, content: &Content, version: u64) {
        let now = Instant::now();
        if self.starts_step(kind, now) {
            self.push(Snapshot::of(content, version));
        }
        self.edited(kind, now);
    }

    /// An edit of `kind` was made.
    pub fn edited(&mut self, kind: Kind, now: Instant) {
        self.open = (kind != Kind::Single).then_some((kind, now));
    }

    /// The next edit starts a new step (the cursor moved, the file was saved).
    pub fn close_step(&mut self) {
        self.open = None;
    }

    /// The text to go back to from `current`, if any.
    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let back = self.undo.pop()?;
        self.redo.push(current);
        self.open = None;
        Some(back)
    }

    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let forward = self.redo.pop()?;
        self.undo.push(current);
        self.open = None;
        Some(forward)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(text: &str, version: u64) -> Snapshot {
        Snapshot { text: text.into(), cursor: (0, 0), version }
    }

    /// Types `edits` into a history, each `gap` after the last, keeping a
    /// snapshot (named by the edit's index) before each new step.
    fn steps(edits: &[(Kind, u64)]) -> Vec<String> {
        let mut h = History::default();
        let start = Instant::now();
        let mut kept = Vec::new();
        for (i, (kind, ms)) in edits.iter().enumerate() {
            let now = start + Duration::from_millis(*ms);
            if h.starts_step(*kind, now) {
                h.push(snap(&i.to_string(), i as u64));
                kept.push(i.to_string());
            }
            h.edited(*kind, now);
        }
        kept
    }

    #[test]
    fn typing_in_a_row_is_one_step() {
        let w = Kind::Word;
        assert_eq!(steps(&[(w, 0), (w, 100), (w, 200)]), ["0"]);
        assert_eq!(steps(&[(w, 0), (w, 100), (w, 1500)]), ["0", "2"], "a pause starts a step");
        assert_eq!(steps(&[(w, 0), (Kind::Space, 100), (w, 200)]), ["0", "1", "2"], "words and spaces");
        assert_eq!(steps(&[(w, 0), (Kind::Delete, 100), (Kind::Delete, 200)]), ["0", "1"]);
        assert_eq!(steps(&[(Kind::Single, 0), (Kind::Single, 50), (w, 100)]), ["0", "1", "2"], "paste, Enter");
    }

    #[test]
    fn moving_the_cursor_ends_a_step() {
        let mut h = History::default();
        let now = Instant::now();
        h.edited(Kind::Word, now);
        assert!(!h.starts_step(Kind::Word, now));
        h.close_step();
        assert!(h.starts_step(Kind::Word, now));
    }

    #[test]
    fn undo_and_redo_go_back_and_forth() {
        let mut h = History::default();
        assert!(h.undo(snap("now", 3)).is_none());
        h.push(snap("a", 1));
        h.push(snap("ab", 2));
        assert_eq!(h.undo(snap("abc", 3)), Some(snap("ab", 2)));
        assert_eq!(h.undo(snap("ab", 2)), Some(snap("a", 1)));
        assert!(h.undo.is_empty());
        assert_eq!(h.redo(snap("a", 1)), Some(snap("ab", 2)));
        assert_eq!(h.redo(snap("ab", 2)), Some(snap("abc", 3)));
        assert_eq!(h.redo(snap("abc", 3)), None);
        // A new change after undoing drops what could be redone.
        h.undo(snap("abc", 3));
        h.push(snap("ab", 2));
        assert_eq!(h.redo(snap("abX", 4)), None);
    }

    #[test]
    fn old_steps_are_dropped_past_the_limits() {
        let mut h = History::default();
        for i in 0..MAX_STEPS + 5 {
            h.push(snap(&i.to_string(), i as u64));
        }
        assert_eq!(h.undo.len(), MAX_STEPS);
        assert_eq!(h.undo[0].text, "5", "the oldest go first");
        let big = "x".repeat(MAX_BYTES / 2 + 1);
        h.push(snap(&big, 1));
        h.push(snap(&big, 2));
        assert_eq!(h.undo.len(), 1, "two halves don't fit");
        h.push(snap(&"y".repeat(MAX_BYTES + 1), 3));
        assert_eq!(h.undo.len(), 1, "a single big step is still kept");
    }

    #[test]
    fn restoring_puts_back_the_text_and_a_valid_cursor() {
        let mut content = Content::with_text("one\ntwo\nthree");
        Snapshot { text: "é\nb".into(), cursor: (0, 1), version: 0 }.restore(&mut content);
        let lines: Vec<String> = content.lines().map(|l| l.text.into_owned()).collect();
        assert_eq!(lines, ["é", "b"]);
        assert_eq!(content.cursor().position, Position { line: 0, column: 0 }, "inside é: before it");
        Snapshot { text: String::new(), cursor: (5, 9), version: 0 }.restore(&mut content);
        assert_eq!(content.lines().map(|l| l.text.into_owned()).collect::<Vec<_>>(), [""]);
        assert_eq!(content.cursor().position, Position { line: 0, column: 0 });
    }
}
