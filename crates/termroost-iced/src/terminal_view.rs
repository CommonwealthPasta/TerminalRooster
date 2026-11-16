//! iced widget that draws a core terminal and feeds it keyboard/mouse input.
//!
//! The widget talks to the [`TerminalHandle`] directly for I/O (keys, paste,
//! selection, scrolling, resize), so typing does not go through app messages.

use std::sync::Arc;

use iced::advanced::clipboard::Kind as ClipboardKind;
use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse::{self, ScrollDelta};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::text::{self, Paragraph as _, Renderer as _};
use iced::advanced::widget::{self, Widget, operation, tree};
use iced::advanced::{Clipboard, Shell};
use iced::alignment::Vertical;
use iced::font::{Style as FontStyle, Weight};
use iced::keyboard::{self, Key, key::Named};
use iced::{Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme};
use termroost_core::{
    self as core, CursorShape, GridSize, MouseButton as CoreButton, MouseEventKind, MouseInput, Rgb, SelectionKind,
    TerminalHandle,
};

use crate::keymap::{self, Action, Keymap};
use crate::theme::{self, MONO};

const PADDING: f32 = 4.0;
const LINE_FACTOR: f32 = 1.25;

pub struct TerminalView<M> {
    handle: TerminalHandle,
    keymap: Arc<Keymap>,
    id: widget::Id,
    /// This is the focused pane of the visible tab.
    active: bool,
    font_size: f32,
    on_focus: M,
    on_restart: M,
}

impl<M: Clone> TerminalView<M> {
    pub fn new(
        handle: TerminalHandle,
        keymap: Arc<Keymap>,
        active: bool,
        font_size: f32,
        on_focus: M,
        on_restart: M,
    ) -> Self {
        let id = widget_id(handle.pane());
        Self { handle, keymap, id, active, font_size, on_focus, on_restart }
    }
}

/// Widget id of a pane's terminal, for focus operations.
pub fn widget_id(pane: core::PaneId) -> widget::Id {
    widget::Id::from(format!("terminal-{}", pane.0))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Metrics {
    font_size: f32,
    cell_w: f32,
    cell_h: f32,
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
        Metrics { font_size, cell_w, cell_h: (font_size * LINE_FACTOR).ceil() }
    }
}

#[derive(Default)]
struct State {
    focused: bool,
    metrics: Option<Metrics>,
    selecting: bool,
    mouse_held: Option<CoreButton>,
    last_click: Option<mouse::Click>,
    last_motion_cell: Option<(usize, usize)>,
    scroll_px: f32,
    modifiers: keyboard::Modifiers,
}

impl operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }
    fn focus(&mut self) {
        self.focused = true;
    }
    fn unfocus(&mut self) {
        self.focused = false;
    }
}

fn color(c: Rgb) -> Color {
    Color::from_rgb8(c.r, c.g, c.b)
}

fn font_for(style: core::CellStyle) -> Font {
    Font {
        weight: if style.bold { Weight::Bold } else { Weight::Normal },
        style: if style.italic { FontStyle::Italic } else { FontStyle::Normal },
        ..MONO
    }
}

impl<M: Clone> TerminalView<M> {
    fn metrics(&self, state: &State) -> Metrics {
        state.metrics.filter(|m| m.font_size == self.font_size).unwrap_or_else(|| Metrics::measure(self.font_size))
    }

    /// Cell under a point relative to the widget, and whether it is the right half.
    fn cell_at(m: &Metrics, p: Point) -> (usize, usize, bool) {
        let x = (p.x - PADDING).max(0.0);
        let y = (p.y - PADDING).max(0.0);
        let col = (x / m.cell_w) as usize;
        let row = (y / m.cell_h) as usize;
        let right = (x % m.cell_w) > m.cell_w / 2.0;
        (col, row, right)
    }

    fn copy_selection(&self, clipboard: &mut dyn Clipboard) -> bool {
        match self.handle.selection_text() {
            Some(t) => {
                clipboard.write(ClipboardKind::Standard, t);
                true
            }
            None => false,
        }
    }

    fn paste_from(&self, clipboard: &mut dyn Clipboard, kind: ClipboardKind) {
        if let Some(t) = clipboard.read(kind).filter(|t| !t.is_empty()) {
            self.handle.paste(&t);
        }
    }

    fn mouse_report(&self, state: &State, kind: MouseEventKind, col: usize, row: usize) -> bool {
        self.handle.mouse(&MouseInput { kind, col, row, mods: keymap::modifiers(state.modifiers) })
    }
}

impl<M: Clone> Widget<M, Theme, iced::Renderer> for TerminalView<M> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn layout(&mut self, tree: &mut widget::Tree, _renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        let size = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        let state = tree.state.downcast_mut::<State>();
        let m = self.metrics(state);
        state.metrics = Some(m);
        let cols = ((size.width - 2.0 * PADDING) / m.cell_w).floor().max(2.0) as u16;
        let rows = ((size.height - 2.0 * PADDING) / m.cell_h).floor().max(1.0) as u16;
        if size.width > 0.0 && size.height > 0.0 {
            self.handle.resize(GridSize {
                cols,
                rows,
                cell_width: m.cell_w.round() as u16,
                cell_height: m.cell_h.round() as u16,
            });
        }
        layout::Node::new(size)
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        _renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let state = tree.state.downcast_mut::<State>();
        operation.focusable(Some(&self.id), layout.bounds(), state);
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let m = self.metrics(state);
        let handle = &self.handle;

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(mods)) => state.modifiers = *mods,
            Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, text, .. }) => {
                if !(state.focused && self.active) {
                    return;
                }
                if let Some(action) = self.keymap.action(key, *physical_key, *modifiers) {
                    // App shortcuts bubble up to the application subscription.
                    if !action.in_terminal() {
                        return;
                    }
                    match action {
                        Action::Copy => {
                            self.copy_selection(clipboard);
                        }
                        Action::Paste => self.paste_from(clipboard, ClipboardKind::Standard),
                        Action::ScrollPageUp => handle.scroll_page(true),
                        Action::ScrollPageDown => handle.scroll_page(false),
                        Action::ScrollTop => handle.scroll_lines(i32::MAX / 2, 0, 0),
                        Action::ScrollBottom => handle.scroll_to_bottom(),
                        _ => {}
                    }
                    shell.capture_event();
                    shell.request_redraw();
                    return;
                }
                if handle.exit_status().is_some() {
                    if matches!(key, Key::Named(Named::Enter)) {
                        shell.publish(self.on_restart.clone());
                        shell.capture_event();
                    }
                    return;
                }
                if let Some(input) = keymap::key_input(key, *physical_key, *modifiers, text.as_deref())
                    && handle.key(&input)
                {
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let Some(p) = cursor.position_in(bounds) else {
                    if state.focused {
                        state.focused = false;
                        handle.focus_changed(false);
                    }
                    return;
                };
                if !state.focused {
                    state.focused = true;
                    handle.focus_changed(true);
                }
                if !self.active {
                    shell.publish(self.on_focus.clone());
                }
                let (col, row, right) = Self::cell_at(&m, p);
                let report = handle.wants_mouse() && !state.modifiers.shift();
                let core_button = match button {
                    mouse::Button::Left => CoreButton::Left,
                    mouse::Button::Middle => CoreButton::Middle,
                    mouse::Button::Right => CoreButton::Right,
                    _ => return,
                };
                if report && self.mouse_report(state, MouseEventKind::Press(core_button), col, row) {
                    state.mouse_held = Some(core_button);
                } else {
                    match core_button {
                        CoreButton::Left => {
                            let click = mouse::Click::new(p, *button, state.last_click);
                            state.last_click = Some(click);
                            let kind = match click.kind() {
                                mouse::click::Kind::Single => SelectionKind::Simple,
                                mouse::click::Kind::Double => SelectionKind::Word,
                                mouse::click::Kind::Triple => SelectionKind::Line,
                            };
                            handle.select_start(col, row, right, kind);
                            state.selecting = true;
                        }
                        CoreButton::Middle => self.paste_from(clipboard, ClipboardKind::Primary),
                        CoreButton::Right => {
                            // Right click copies a selection, or pastes when there is none.
                            if self.copy_selection(clipboard) {
                                handle.clear_selection();
                            } else {
                                self.paste_from(clipboard, ClipboardKind::Standard);
                            }
                        }
                    }
                }
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                // A terminal can be laid out with no width or height (see `layout`);
                // `clamp` panics when its upper bound is below the lower one.
                let rel = Point::new(
                    (position.x - bounds.x).clamp(0.0, (bounds.width - 1.0).max(0.0)),
                    (position.y - bounds.y).clamp(0.0, (bounds.height - 1.0).max(0.0)),
                );
                let (col, row, right) = Self::cell_at(&m, rel);
                if state.selecting {
                    handle.select_update(col, row, right);
                    shell.request_redraw();
                } else if cursor.is_over(bounds) && handle.wants_mouse() && state.last_motion_cell != Some((col, row)) {
                    state.last_motion_cell = Some((col, row));
                    self.mouse_report(state, MouseEventKind::Move { held: state.mouse_held }, col, row);
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                if state.selecting && *button == mouse::Button::Left {
                    state.selecting = false;
                    if let Some(t) = handle.selection_text() {
                        clipboard.write(ClipboardKind::Primary, t);
                    }
                }
                if let Some(held) = state.mouse_held.take() {
                    let p = cursor.position_in(bounds).unwrap_or(Point::ORIGIN);
                    let (col, row, _) = Self::cell_at(&m, p);
                    self.mouse_report(state, MouseEventKind::Release(held), col, row);
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let Some(p) = cursor.position_in(bounds) else { return };
                let lines = match delta {
                    ScrollDelta::Lines { y, .. } => (y * 3.0).round() as i32,
                    ScrollDelta::Pixels { y, .. } => {
                        state.scroll_px += y;
                        let l = (state.scroll_px / m.cell_h).trunc();
                        state.scroll_px -= l * m.cell_h;
                        l as i32
                    }
                };
                let (col, row, _) = Self::cell_at(&m, p);
                handle.scroll_lines(lines, col, row);
                shell.capture_event();
                shell.request_redraw();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) && !self.handle.wants_mouse() {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let m = self.metrics(state);
        let bounds = layout.bounds();
        let focused = state.focused && self.active;
        let exited = self.handle.exit_status();

        self.handle.with_content(|c| {
            renderer.fill_quad(Quad { bounds, ..Quad::default() }, color(c.default_bg));
            renderer.with_layer(bounds, |r| draw_cells(r, c, &m, bounds, focused));
        });

        if let Some(code) = exited {
            let msg = match code {
                Some(code) => format!("process exited with code {code} · press Enter to restart"),
                None => "process exited · press Enter to restart".to_owned(),
            };
            let h = m.cell_h + 8.0;
            let bar = Rectangle { x: bounds.x, y: bounds.y + bounds.height - h, width: bounds.width, height: h };
            renderer.fill_quad(Quad { bounds: bar, ..Quad::default() }, Color::from_rgba8(224, 175, 104, 0.92));
            renderer.fill_text(
                text_run(msg, &m, bar.width, Font { weight: Weight::Bold, ..MONO }),
                Point::new(bar.x + 8.0, bar.y + 4.0),
                Color::from_rgb8(26, 27, 38),
                bar,
            );
        }
    }
}

fn text_run(content: String, m: &Metrics, width: f32, font: Font) -> text::Text<String, Font> {
    text::Text {
        content,
        bounds: Size::new(width, m.cell_h),
        size: Pixels(m.font_size),
        line_height: text::LineHeight::Absolute(Pixels(m.cell_h)),
        font,
        align_x: text::Alignment::Left,
        align_y: Vertical::Top,
        shaping: text::Shaping::Basic,
        wrapping: text::Wrapping::None,
    }
}

fn draw_cells(r: &mut iced::Renderer, c: &core::RenderContent, m: &Metrics, bounds: Rectangle, focused: bool) {
    let origin = Point::new(bounds.x + PADDING, bounds.y + PADDING);
    let cursor = c.cursor;
    let block_cursor_at = cursor.filter(|cur| focused && cur.shape == CursorShape::Block).map(|cur| (cur.col, cur.row));

    for row in 0..c.rows {
        let cells = c.row(row);
        let y = origin.y + row as f32 * m.cell_h;

        // Backgrounds, merged into runs of equal color.
        let mut col = 0;
        while col < c.cols {
            let bg_of = |cell: &core::RenderCell| if cell.selected { c.selection_bg } else { cell.bg };
            let bg = bg_of(&cells[col]);
            let start = col;
            while col < c.cols && bg_of(&cells[col]) == bg {
                col += 1;
            }
            if bg != c.default_bg {
                let rect = Rectangle {
                    x: origin.x + start as f32 * m.cell_w,
                    y,
                    width: (col - start) as f32 * m.cell_w,
                    height: m.cell_h,
                };
                r.fill_quad(Quad { bounds: rect, snap: false, ..Quad::default() }, color(bg));
            }
        }

        // Cursor block goes under the text.
        if let Some((cc, cr)) = block_cursor_at
            && cr == row
        {
            let w = if cells.get(cc).is_some_and(|x| x.wide) { 2.0 } else { 1.0 } * m.cell_w;
            let rect = Rectangle { x: origin.x + cc as f32 * m.cell_w, y, width: w, height: m.cell_h };
            r.fill_quad(Quad { bounds: rect, snap: false, ..Quad::default() }, color(c.cursor_color));
        }

        // Text: runs of ASCII with the same style are drawn together; anything
        // else is drawn per cell so fallback fonts cannot break alignment.
        let mut col = 0;
        while col < c.cols {
            let cell = &cells[col];
            if cell.spacer || (cell.ch == ' ' && cell.zerowidth.is_none()) {
                col += 1;
                continue;
            }
            let on_cursor = |x: usize| block_cursor_at == Some((x, row));
            let fg_of = |x: usize, cell: &core::RenderCell| if on_cursor(x) { c.default_bg } else { cell.fg };
            let x = origin.x + col as f32 * m.cell_w;
            let simple = |cell: &core::RenderCell| cell.ch.is_ascii_graphic() || cell.ch == ' ';

            if simple(cell) && cell.zerowidth.is_none() && !cell.wide {
                let fg = fg_of(col, cell);
                let style = cell.style;
                let start = col;
                let mut s = String::new();
                while col < c.cols {
                    let n = &cells[col];
                    let joins = simple(n)
                        && n.zerowidth.is_none()
                        && !n.wide
                        && n.style == style
                        && (n.ch == ' ' || fg_of(col, n) == fg);
                    if !joins {
                        break;
                    }
                    s.push(n.ch);
                    col += 1;
                }
                let trimmed = s.trim_end().len();
                s.truncate(trimmed);
                let width = (col - start) as f32 * m.cell_w + m.cell_w;
                r.fill_text(text_run(s, m, width, font_for(style)), Point::new(x, y), color(fg), bounds);
                decorate(r, style, x, y, (col - start) as f32 * m.cell_w, m, color(fg));
            } else {
                let mut s = String::from(cell.ch);
                if let Some(z) = &cell.zerowidth {
                    s.extend(z.iter());
                }
                let span = if cell.wide { 2.0 } else { 1.0 } * m.cell_w;
                let mut t = text_run(s, m, span, theme::glyph_font(cell.ch, font_for(cell.style)));
                t.align_x = text::Alignment::Center;
                t.shaping = text::Shaping::Advanced;
                let fg = fg_of(col, cell);
                r.fill_text(t, Point::new(x + span / 2.0, y), color(fg), bounds);
                decorate(r, cell.style, x, y, span, m, color(fg));
                col += 1;
            }
        }
    }

    // Non-block cursors (and the unfocused cursor outline).
    if let Some(cur) = cursor {
        let x = origin.x + cur.col as f32 * m.cell_w;
        let y = origin.y + cur.row as f32 * m.cell_h;
        let col = color(c.cursor_color);
        let thin = (m.font_size / 10.0).max(1.0);
        let rects: Vec<Rectangle> = match (focused, cur.shape) {
            (true, CursorShape::Block) => vec![],
            (true, CursorShape::Beam) => vec![Rectangle { x, y, width: thin, height: m.cell_h }],
            (true, CursorShape::Underline) => {
                vec![Rectangle { x, y: y + m.cell_h - thin, width: m.cell_w, height: thin }]
            }
            _ => vec![
                Rectangle { x, y, width: m.cell_w, height: 1.0 },
                Rectangle { x, y: y + m.cell_h - 1.0, width: m.cell_w, height: 1.0 },
                Rectangle { x, y, width: 1.0, height: m.cell_h },
                Rectangle { x: x + m.cell_w - 1.0, y, width: 1.0, height: m.cell_h },
            ],
        };
        for rect in rects {
            r.fill_quad(Quad { bounds: rect, snap: false, ..Quad::default() }, col);
        }
    }
}

fn decorate(r: &mut iced::Renderer, style: core::CellStyle, x: f32, y: f32, width: f32, m: &Metrics, fg: Color) {
    let thick = (m.font_size / 14.0).max(1.0);
    if style.underline {
        let rect = Rectangle { x, y: y + m.cell_h - thick - 1.0, width, height: thick };
        r.fill_quad(Quad { bounds: rect, snap: false, ..Quad::default() }, fg);
    }
    if style.strikeout {
        let rect = Rectangle { x, y: y + m.cell_h / 2.0, width, height: thick };
        r.fill_quad(Quad { bounds: rect, snap: false, ..Quad::default() }, fg);
    }
}

impl<'a, M: Clone + 'a> From<TerminalView<M>> for Element<'a, M, Theme, iced::Renderer> {
    fn from(w: TerminalView<M>) -> Self {
        Element::new(w)
    }
}
