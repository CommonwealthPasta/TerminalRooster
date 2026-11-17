//! Scrollback persistence: grid + history -> ANSI bytes, and replay.

use std::fmt::Write as _;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};

const STYLE_FLAGS: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Color,
    bg: Color,
    flags: Flags,
}

impl Style {
    const DEFAULT: Style = Style {
        fg: Color::Named(NamedColor::Foreground),
        bg: Color::Named(NamedColor::Background),
        flags: Flags::empty(),
    };

    fn of(cell: &Cell) -> Style {
        Style { fg: cell.fg, bg: cell.bg, flags: cell.flags & STYLE_FLAGS }
    }

    fn write_sgr(&self, out: &mut String) {
        out.push_str("\x1b[0");
        let f = self.flags;
        for (flag, code) in [
            (Flags::BOLD, "1"),
            (Flags::DIM, "2"),
            (Flags::ITALIC, "3"),
            (Flags::INVERSE, "7"),
            (Flags::HIDDEN, "8"),
            (Flags::STRIKEOUT, "9"),
        ] {
            if f.contains(flag) {
                out.push(';');
                out.push_str(code);
            }
        }
        if f.intersects(Flags::ALL_UNDERLINES) {
            out.push_str(";4");
        }
        write_color(out, self.fg, true);
        write_color(out, self.bg, false);
        out.push('m');
    }
}

fn write_color(out: &mut String, color: Color, fg: bool) {
    match color {
        Color::Named(n) => {
            let i = n as usize;
            let code = match i {
                0..=7 => (if fg { 30 } else { 40 }) + i,
                8..=15 => (if fg { 90 } else { 100 }) + i - 8,
                _ => return, // default/dim/cursor colors: leave at default
            };
            let _ = write!(out, ";{code}");
        }
        Color::Indexed(i) => {
            let _ = write!(out, ";{};5;{i}", if fg { 38 } else { 48 });
        }
        Color::Spec(c) => {
            let _ = write!(out, ";{};2;{};{};{}", if fg { 38 } else { 48 }, c.r, c.g, c.b);
        }
    }
}

fn is_blank(cell: &Cell) -> bool {
    (cell.c == ' ' || cell.c == '\t')
        && cell.bg == Color::Named(NamedColor::Background)
        && !cell.flags.intersects(Flags::INVERSE | Flags::ALL_UNDERLINES)
}

/// Serializes the primary screen and its history (at most `max_lines` lines).
///
/// Returns `None` while a full-screen program uses the alternate screen, since
/// the primary screen is not accessible then; the previous snapshot is kept.
pub(crate) fn serialize<T>(term: &Term<T>, max_lines: usize) -> Option<Vec<u8>> {
    if term.mode().contains(TermMode::ALT_SCREEN) {
        return None;
    }
    let grid = term.grid();
    let cols = grid.columns();
    let top = grid.topmost_line().0;
    let mut bottom = grid.bottommost_line().0;
    let row_is_empty = |l: i32| (0..cols).all(|c| is_blank(&grid[Line(l)][Column(c)]));
    while bottom >= top && row_is_empty(bottom) {
        bottom -= 1;
    }
    if bottom < top {
        return Some(Vec::new());
    }
    let start = top.max(bottom - max_lines as i32 + 1);

    let mut out = String::new();
    let mut style = Style::DEFAULT;
    for l in start..=bottom {
        let row = &grid[Line(l)];
        let wrapped = l != bottom && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        // Wrapped rows are emitted in full so the text reflows correctly.
        let end =
            if wrapped { cols } else { (0..cols).rev().find(|&c| !is_blank(&row[Column(c)])).map_or(0, |c| c + 1) };
        for c in 0..end {
            let cell = &row[Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let s = Style::of(cell);
            if s != style {
                s.write_sgr(&mut out);
                style = s;
            }
            out.push(if cell.c == '\t' { ' ' } else { cell.c });
            if let Some(z) = cell.zerowidth() {
                out.extend(z.iter());
            }
        }
        if !wrapped {
            if style != Style::DEFAULT {
                out.push_str("\x1b[0m");
                style = Style::DEFAULT;
            }
            if l != bottom {
                out.push_str("\r\n");
            }
        }
    }
    out.push_str("\x1b[0m");
    Some(out.into_bytes())
}

/// Feeds saved bytes into a terminal.
pub(crate) fn replay<T: alacritty_terminal::event::EventListener>(term: &mut Term<T>, data: &[u8]) {
    let mut parser: Processor = Processor::new();
    parser.advance(term, data);
}

const BANNER_EDGE: &str = "───";

/// Saved scrollback without the `─── restored session from … ───` lines
/// older versions printed under each restored terminal's output, which were
/// saved along with it.
pub(crate) fn without_restore_banners(data: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(data);
    let kept: Vec<&str> = text
        .split("\r\n")
        .filter(|l| {
            let l = strip_sgr(l);
            let l = l.trim();
            !(l.starts_with(&format!("{BANNER_EDGE} restored session from ")) && l.ends_with(BANNER_EDGE))
        })
        .collect();
    kept.join("\r\n").into_bytes()
}

/// Whether saved scrollback holds nothing but a shell prompt, so there's
/// nothing worth bringing back: every non-blank line other than a restore
/// banner reads the same. Older versions saved idle terminals too, as the
/// same prompt again under each restore banner.
pub(crate) fn just_a_prompt(data: &[u8]) -> bool {
    let text = String::from_utf8_lossy(data);
    let mut lines = text.split("\r\n").map(strip_sgr).filter(|l| {
        let l = l.trim();
        !l.is_empty() && !(l.starts_with(BANNER_EDGE) && l.ends_with(BANNER_EDGE))
    });
    let Some(first) = lines.next() else { return true };
    lines.all(|l| l.trim_end() == first.trim_end())
}

/// The text of a serialized line, without its color codes (`ESC [ … m`, the
/// only sequences `serialize` writes).
fn strip_sgr(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                chars.by_ref().find(|c| ('\x40'..='\x7e').contains(c));
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::term::test::TermSize;

    fn term(cols: usize, rows: usize) -> Term<VoidListener> {
        Term::new(Config::default(), &TermSize::new(cols, rows), VoidListener)
    }

    fn feed(t: &mut Term<VoidListener>, s: &str) {
        let mut p: Processor = Processor::new();
        p.advance(t, s.as_bytes());
    }

    fn visible_text(t: &Term<VoidListener>) -> Vec<String> {
        let g = t.grid();
        (g.topmost_line().0..=g.bottommost_line().0)
            .map(|l| {
                let row = &g[Line(l)];
                (0..g.columns())
                    .map(|c| &row[Column(c)])
                    .filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER))
                    .map(|c| c.c)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn round_trip_text_and_attributes() {
        let mut a = term(20, 5);
        feed(&mut a, "plain line\r\n\x1b[1;31mbold red\x1b[0m and \x1b[38;2;1;2;3mrgb\x1b[0m\r\n");
        feed(&mut a, "wide: 漢字 ok\r\n");
        for i in 0..10 {
            feed(&mut a, &format!("history {i}\r\n"));
        }
        feed(&mut a, "a very long line that wraps around the edge");

        let data = serialize(&a, 1000).unwrap();
        let mut b = term(20, 5);
        replay(&mut b, &data);
        assert_eq!(visible_text(&a), visible_text(&b));

        // Attributes survive.
        let find = |t: &Term<VoidListener>, needle: char| -> Cell {
            let g = t.grid();
            for l in g.topmost_line().0..=g.bottommost_line().0 {
                for c in 0..g.columns() {
                    let cell = &g[Line(l)][Column(c)];
                    if cell.c == needle {
                        return cell.clone();
                    }
                }
            }
            panic!("{needle} not found");
        };
        let bold = find(&b, 'b');
        assert!(bold.flags.contains(Flags::BOLD));
        assert_eq!(bold.fg, Color::Named(NamedColor::Red));
        let rgb = find(&b, 'g');
        assert!(matches!(rgb.fg, Color::Spec(c) if (c.r, c.g, c.b) == (1, 2, 3)));
    }

    #[test]
    fn wrapped_lines_reflow_on_wider_terminal() {
        let mut a = term(10, 3);
        feed(&mut a, "0123456789abcdefghij\r\nnext");
        let data = serialize(&a, 1000).unwrap();
        let mut b = term(40, 3);
        replay(&mut b, &data);
        assert_eq!(visible_text(&b), ["0123456789abcdefghij", "next"]);
    }

    /// What older versions printed under a restored terminal's output.
    fn old_banner(t: &mut Term<VoidListener>) {
        feed(t, "\r\n\x1b[0;2m─── restored session from 2026-11-16 19:27 ───\x1b[0m\r\n");
    }

    #[test]
    fn old_restore_banners_are_dropped() {
        let mut a = term(40, 10);
        feed(&mut a, "before\r\n─── restarted ───");
        old_banner(&mut a);
        feed(&mut a, "after");
        let data = without_restore_banners(&serialize(&a, 100).unwrap());
        let mut b = term(40, 10);
        replay(&mut b, &data);
        assert_eq!(visible_text(&b), ["before", "─── restarted ───", "after"]);
    }

    #[test]
    fn limits_lines_and_skips_alt_screen() {
        let mut a = term(10, 3);
        for i in 0..50 {
            feed(&mut a, &format!("{i}\r\n"));
        }
        let data = serialize(&a, 5).unwrap();
        let s = String::from_utf8(data).unwrap();
        assert!(s.contains("49") && !s.contains("40\r\n"));

        feed(&mut a, "\x1b[?1049h");
        assert!(serialize(&a, 5).is_none());
    }

    #[test]
    fn a_lone_prompt_is_not_worth_restoring() {
        let prompt = "\x1b[1;32mme@host\x1b[0m:\x1b[1;34m~/my proj\x1b[0m$ ";
        assert!(just_a_prompt(b""));
        let mut a = term(40, 10);
        feed(&mut a, prompt);
        let idle = serialize(&a, 100).unwrap();
        assert!(just_a_prompt(&idle));

        // What older versions saved after restoring that: the prompt again
        // under a banner, any number of times.
        let mut b = term(40, 10);
        replay(&mut b, &idle);
        old_banner(&mut b);
        feed(&mut b, prompt);
        let again = serialize(&b, 100).unwrap();
        assert!(String::from_utf8_lossy(&again).contains("restored session"));
        assert!(just_a_prompt(&again));

        // A command that ran leaves lines of its own.
        feed(&mut b, "ls\r\nCargo.toml\r\n");
        feed(&mut b, prompt);
        assert!(!just_a_prompt(&serialize(&b, 100).unwrap()));
        let mut c = term(40, 10);
        feed(&mut c, "first line of output\r\n");
        feed(&mut c, prompt);
        assert!(!just_a_prompt(&serialize(&c, 100).unwrap()));
    }
}
