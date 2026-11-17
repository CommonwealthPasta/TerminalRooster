//! The lines of a hunk, drawn as a diff tool draws them: the old and the new
//! line number, a `+` or `-`, and the text, on green for added lines and on
//! red for removed ones. Long lines wrap.
//!
//! A widget of its own rather than a column of text widgets: a commit can
//! change thousands of lines. Its height comes from counting characters (the
//! font is monospaced), so no text is laid out, and only the lines in view
//! are drawn.
//!
//! Lines can be pressed, and dragged over, to choose some of them (for
//! staging a few lines of a hunk); the app keeps which are chosen.

use std::collections::BTreeSet;

use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse;
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::text::{self, Paragraph as _, Renderer as _};
use iced::advanced::widget::{self, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::alignment::Vertical;
use iced::{Element, Event, Length, Pixels, Point, Rectangle, Size, Theme, keyboard};
use termroost_core::git::{DiffLine, LineKind};

use crate::theme::{self, MONO};

/// Space before the line numbers, and after the text.
const PADDING: f32 = 8.0;
/// Width of the bar that marks chosen lines.
const BAR: f32 = 3.0;
/// Columns a tab advances to the next multiple of.
const TAB: usize = 4;
/// Row height over the font size.
const LINE_FACTOR: f32 = 1.4;

pub struct DiffLines<'a, M> {
    lines: &'a [DiffLine],
    /// Digits the line numbers get (the same for all hunks of a file, so
    /// they line up).
    digits: usize,
    font_size: f32,
    chosen: Option<&'a BTreeSet<usize>>,
    on_press: Option<Box<dyn Fn(usize, keyboard::Modifiers) -> M + 'a>>,
    on_drag: Option<Box<dyn Fn(usize) -> M + 'a>>,
}

impl<'a, M> DiffLines<'a, M> {
    pub fn new(lines: &'a [DiffLine], digits: usize, font_size: f32) -> Self {
        DiffLines { lines, digits, font_size, chosen: None, on_press: None, on_drag: None }
    }

    /// Lines drawn as chosen (indexes into the lines).
    pub fn chosen(mut self, chosen: Option<&'a BTreeSet<usize>>) -> Self {
        self.chosen = chosen;
        self
    }

    /// Lines can be chosen: `press` gets the line pressed and the modifier
    /// keys held, `drag` each line the pointer is then dragged onto.
    pub fn on_choose(
        mut self,
        press: impl Fn(usize, keyboard::Modifiers) -> M + 'a,
        drag: impl Fn(usize) -> M + 'a,
    ) -> Self {
        self.on_press = Some(Box::new(press));
        self.on_drag = Some(Box::new(drag));
        self
    }
}

/// How many digits line numbers up to `max` take.
pub fn digits(max: u32) -> usize {
    max.max(1).ilog10() as usize + 1
}

/// The size of the monospaced font's characters.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Metrics {
    font_size: f32,
    cell_w: f32,
    row_h: f32,
}

impl Metrics {
    fn measure(font_size: f32) -> Metrics {
        const SAMPLE: &str = "MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM";
        let paragraph = <iced::Renderer as text::Renderer>::Paragraph::with_text(text::Text {
            content: SAMPLE,
            bounds: Size::INFINITE,
            size: Pixels(font_size),
            line_height: text::LineHeight::Relative(1.0),
            font: MONO,
            align_x: text::Alignment::Left,
            align_y: Vertical::Top,
            shaping: text::Shaping::Basic,
            wrapping: text::Wrapping::None,
        });
        let w = paragraph.min_bounds().width / SAMPLE.len() as f32;
        let cell_w = if w.is_finite() && w > 1.0 { w } else { font_size * 0.6 };
        Metrics { font_size, cell_w, row_h: (font_size * LINE_FACTOR).ceil() }
    }
}

/// Where each line's rows start, for the lines and width it was counted for.
#[derive(Debug, Default, PartialEq)]
struct Rows {
    /// The lines (by address and number), the columns of text a row holds,
    /// and the font size.
    key: (usize, usize, usize, u32),
    /// The first row of each line, then the number of rows.
    starts: Vec<usize>,
}

#[derive(Default)]
struct State {
    metrics: Option<Metrics>,
    rows: Rows,
    /// The line last pressed or dragged onto, while the button is held.
    drag: Option<usize>,
    modifiers: keyboard::Modifiers,
}

impl<M> DiffLines<'_, M> {
    fn metrics(&self, state: &State) -> Metrics {
        state.metrics.filter(|m| m.font_size == self.font_size).unwrap_or_else(|| Metrics::measure(self.font_size))
    }

    /// Where the text starts, past the line numbers and the sign.
    fn text_x(&self, m: &Metrics) -> f32 {
        PADDING + (2 * self.digits + 4) as f32 * m.cell_w
    }

    /// The line at `y` (from the widget's top), if any.
    fn line_at(state: &State, m: &Metrics, y: f32) -> Option<usize> {
        let row = (y / m.row_h).floor().max(0.0) as usize;
        let starts = &state.rows.starts;
        let line = starts.partition_point(|s| *s <= row).checked_sub(1)?;
        (line + 1 < starts.len()).then_some(line)
    }
}

/// How a line shows: tabs as spaces, without the `\r` of a CRLF ending.
fn display(text: &str) -> String {
    let text = text.strip_suffix('\r').unwrap_or(text);
    if !text.contains('\t') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut col = 0;
    for c in text.chars() {
        if c == '\t' {
            let n = TAB - col % TAB;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

/// The columns a line takes, as [`display`] shows it.
fn width(text: &str) -> usize {
    let text = text.strip_suffix('\r').unwrap_or(text);
    text.chars().fold(0, |col, c| if c == '\t' { col + TAB - col % TAB } else { col + 1 })
}

/// The rows each line takes with `cols` columns to a row: where each line
/// starts, then the total.
fn count_rows(lines: &[DiffLine], cols: usize) -> Vec<usize> {
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut row = 0;
    for line in lines {
        starts.push(row);
        row += width(&line.text).div_ceil(cols).max(1);
    }
    starts.push(row);
    starts
}

impl<M: Clone> Widget<M, Theme, iced::Renderer> for DiffLines<'_, M> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn layout(&mut self, tree: &mut widget::Tree, _renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        let m = self.metrics(state);
        state.metrics = Some(m);
        let width = limits.max().width;
        let room = width - self.text_x(&m) - PADDING;
        let cols = if room.is_finite() { (room / m.cell_w).floor().max(8.0) as usize } else { usize::MAX };
        let key = (self.lines.as_ptr() as usize, self.lines.len(), cols, m.font_size.to_bits());
        if state.rows.key != key || state.rows.starts.is_empty() {
            state.rows = Rows { key, starts: count_rows(self.lines, cols) };
        }
        let rows = state.rows.starts.last().copied().unwrap_or(0);
        layout::Node::new(limits.resolve(Length::Fill, Length::Shrink, Size::new(width, rows as f32 * m.row_h)))
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let m = self.metrics(state);
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(mods)) => state.modifiers = *mods,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let (Some(press), Some(at)) = (&self.on_press, cursor.position_in(bounds)) else { return };
                if let Some(line) = Self::line_at(state, &m, at.y) {
                    shell.publish(press(line, state.modifiers));
                    state.drag = Some(line);
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let (Some(last), Some(drag)) = (state.drag, &self.on_drag) else { return };
                // Above or below the hunk: its first or last line.
                let y = (position.y - bounds.y).clamp(0.0, (bounds.height - 1.0).max(0.0));
                if let Some(line) = Self::line_at(state, &m, y).filter(|l| *l != last) {
                    state.drag = Some(line);
                    shell.publish(drag(line));
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => state.drag = None,
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        let m = self.metrics(state);
        // A hand over the lines that can be chosen: the changed ones.
        let changed = cursor
            .position_in(layout.bounds())
            .and_then(|p| Self::line_at(state, &m, p.y))
            .and_then(|i| self.lines.get(i))
            .is_some_and(|l| l.kind != LineKind::Context);
        if self.on_press.is_some() && (changed || state.drag.is_some()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let Some(visible) = bounds.intersection(viewport) else { return };
        let state = tree.state.downcast_ref::<State>();
        let m = self.metrics(state);
        let starts = &state.rows.starts;
        if starts.len() != self.lines.len() + 1 {
            return;
        }
        let colors = theme::diff_colors(theme);
        let text_x = bounds.x + self.text_x(&m);
        let cols = ((bounds.x + bounds.width - PADDING - text_x) / m.cell_w).floor().max(8.0) as usize;
        let first_row = ((visible.y - bounds.y) / m.row_h).floor().max(0.0) as usize;
        let last_row = ((visible.y + visible.height - bounds.y) / m.row_h).ceil() as usize;
        let first = starts.partition_point(|s| *s <= first_row).saturating_sub(1);
        let quad = |r: &mut iced::Renderer, x: f32, y: f32, width: f32, height: f32, color| {
            r.fill_quad(Quad { bounds: Rectangle { x, y, width, height }, snap: false, ..Quad::default() }, color);
        };
        let run = |content: String, width: f32| text::Text {
            shaping: if content.is_ascii() { text::Shaping::Basic } else { text::Shaping::Advanced },
            content,
            bounds: Size::new(width, m.row_h),
            size: Pixels(m.font_size),
            line_height: text::LineHeight::Absolute(Pixels(m.row_h)),
            font: MONO,
            align_x: text::Alignment::Left,
            align_y: Vertical::Top,
            wrapping: text::Wrapping::None,
        };
        let number =
            |n: Option<u32>| n.map_or_else(|| " ".repeat(self.digits), |n| format!("{n:>w$}", w = self.digits));
        for (i, line) in self.lines.iter().enumerate().skip(first) {
            let start = starts[i];
            if start >= last_row {
                break;
            }
            let y = bounds.y + start as f32 * m.row_h;
            let height = (starts[i + 1] - start) as f32 * m.row_h;
            let (fill, sign, sign_color) = match line.kind {
                LineKind::Added => (Some(colors.added), "+", colors.added_sign),
                LineKind::Removed => (Some(colors.removed), "-", colors.removed_sign),
                LineKind::Context => (None, " ", colors.number),
            };
            if let Some(fill) = fill {
                quad(renderer, bounds.x, y, bounds.width, height, fill);
            }
            if self.chosen.is_some_and(|c| c.contains(&i)) {
                quad(renderer, bounds.x, y, bounds.width, height, colors.chosen);
                quad(renderer, bounds.x, y, BAR, height, colors.chosen_bar);
            }
            let gutter = format!("{} {}", number(line.old), number(line.new));
            let x = bounds.x + PADDING;
            renderer.fill_text(run(gutter, text_x - x), Point::new(x, y), colors.number, visible);
            let sign_x = text_x - 2.0 * m.cell_w;
            renderer.fill_text(run(sign.to_owned(), m.cell_w), Point::new(sign_x, y), sign_color, visible);
            let shown = display(&line.text);
            let chars: Vec<char> = shown.chars().collect();
            for (row, piece) in chars.chunks(cols).enumerate() {
                let row_y = y + row as f32 * m.row_h;
                if row_y > visible.y + visible.height {
                    break;
                }
                let piece: String = piece.iter().collect();
                let width = bounds.x + bounds.width - text_x;
                renderer.fill_text(run(piece, width), Point::new(text_x, row_y), colors.text, visible);
            }
        }
    }
}

impl<'a, M: Clone + 'a> From<DiffLines<'a, M>> for Element<'a, M> {
    fn from(w: DiffLines<'a, M>) -> Self {
        Element::new(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> DiffLine {
        DiffLine { kind: LineKind::Context, text: text.into(), old: Some(1), new: Some(1), no_newline: false }
    }

    #[test]
    fn tabs_go_to_the_next_stop() {
        assert_eq!(display("\tx"), "    x");
        assert_eq!(display("ab\tc"), "ab  c");
        assert_eq!(display("abcd\tef\r"), "abcd    ef");
        assert_eq!(width("ab\tc\r"), 5);
        assert_eq!(width(""), 0);
    }

    #[test]
    fn long_lines_take_more_rows() {
        let lines = [line(""), line("0123456789"), line("01234567890"), line("ü")];
        assert_eq!(count_rows(&lines, 10), vec![0, 1, 2, 4, 5], "an empty line takes a row too");
        let state = State { rows: Rows { key: (0, 0, 0, 0), starts: count_rows(&lines, 10) }, ..State::default() };
        let m = Metrics { font_size: 10.0, cell_w: 6.0, row_h: 10.0 };
        let at = |y| DiffLines::<()>::line_at(&state, &m, y);
        assert_eq!(
            (at(0.0), at(9.9), at(10.0), at(25.0), at(35.0), at(45.0), at(50.0)),
            (Some(0), Some(0), Some(1), Some(2), Some(2), Some(3), None)
        );
    }

    #[test]
    fn digits_of_line_numbers() {
        assert_eq!((digits(0), digits(9), digits(10), digits(99), digits(100), digits(12_345)), (1, 1, 2, 2, 3, 5));
    }
}
