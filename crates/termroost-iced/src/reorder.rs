//! A vertical list whose items can be dragged into a new order.
//!
//! Items keep working as usual: a press still reaches them, so clicking,
//! double-clicking and their buttons are unchanged. Once the pointer moves a
//! few pixels with the button held, the item follows the pointer and a line
//! shows where it will land. Dropping it beside the list (e.g. on a terminal)
//! cancels the move.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad, Renderer as _};
use iced::advanced::widget::{self, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::{
    Alignment, Border, Color, Element, Event, Length, Padding, Point, Rectangle, Shadow, Size, Theme, Vector, mouse,
    window,
};

use crate::theme;

/// How far the pointer moves with the button held before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 5.0;
/// How far beside the list a drop still counts.
const DROP_MARGIN: f32 = 8.0;
/// Thickness of the line that shows where the item will land.
const LINE: f32 = 2.0;

type OnMove<'a, Message> = Box<dyn Fn(usize, usize) -> Message + 'a>;

pub struct Reorder<'a, Message> {
    items: Vec<Element<'a, Message, Theme, iced::Renderer>>,
    spacing: f32,
    on_move: Option<OnMove<'a, Message>>,
}

/// A list of `items` that can be reordered by dragging; see [`Reorder::on_move`].
pub fn reorder<'a, Message>(items: impl IntoIterator<Item = Element<'a, Message>>) -> Reorder<'a, Message> {
    Reorder { items: items.into_iter().collect(), spacing: 0.0, on_move: None }
}

impl<'a, Message> Reorder<'a, Message> {
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Called with `(from, to)` when an item is dropped somewhere else: the
    /// item at `from` goes to index `to` of the list without it. Without this,
    /// nothing can be dragged.
    pub fn on_move(mut self, f: impl Fn(usize, usize) -> Message + 'a) -> Self {
        self.on_move = Some(Box::new(f));
        self
    }

    /// Like [`Reorder::on_move`], but only when `enabled`.
    pub fn on_move_if(self, enabled: bool, f: impl Fn(usize, usize) -> Message + 'a) -> Self {
        if enabled { self.on_move(f) } else { self }
    }
}

#[derive(Default)]
struct State {
    drag: Option<Drag>,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    /// The item being dragged.
    index: usize,
    /// Where the button went down.
    origin: Point,
    /// Where the pointer is now. Kept here because the list is drawn without
    /// a cursor while the pointer is outside its scrollable.
    cursor: Point,
    /// Past the threshold: the item follows the pointer.
    moving: bool,
}

impl Drag {
    /// The index the item would get if dropped now; `None` cancels the move.
    fn target(&self, layout: Layout<'_>) -> Option<usize> {
        let b = layout.bounds();
        if self.cursor.x < b.x - DROP_MARGIN || self.cursor.x > b.x + b.width + DROP_MARGIN {
            return None;
        }
        let above = layout
            .children()
            .enumerate()
            .filter(|(i, l)| *i != self.index && l.bounds().center_y() < self.cursor.y)
            .count();
        Some(above)
    }
}

impl<Message> Reorder<'_, Message> {
    /// The drag to draw, if an item is being moved.
    fn moving(&self, tree: &Tree) -> Option<Drag> {
        let drag = tree.state.downcast_ref::<State>().drag?;
        (drag.moving && drag.index < self.items.len()).then_some(drag)
    }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Reorder<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        self.items.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.items);
    }

    fn size(&self) -> Size<Length> {
        Size { width: Length::Fill, height: Length::Shrink }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        layout::flex::resolve(
            layout::flex::Axis::Vertical,
            renderer,
            limits,
            Length::Fill,
            Length::Shrink,
            Padding::ZERO,
            self.spacing,
            Alignment::Start,
            &mut self.items,
            &mut tree.children,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            for ((child, state), layout) in self.items.iter_mut().zip(&mut tree.children).zip(layout.children()) {
                child.as_widget_mut().operate(state, layout, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        // While an item is moving, the items don't see the pointer: nothing
        // under it lights up, and a button pressed to start the drag doesn't
        // fire when it is let go.
        let mut item_cursor = cursor;
        match (&self.on_move, event) {
            (None, _) => state.drag = None,
            (Some(_), Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))) => {
                state.drag = cursor.position().and_then(|p| {
                    let index = layout.children().position(|l| l.bounds().contains(p))?;
                    Some(Drag { index, origin: p, cursor: p, moving: false })
                });
            }
            (Some(_), Event::Mouse(mouse::Event::CursorMoved { .. })) => {
                if let Some(drag) = &mut state.drag {
                    // Outside the scrollable the cursor "levitates"; it is still where the pointer is.
                    if let Some(p) = cursor.land().position() {
                        drag.cursor = p;
                        drag.moving |= drag.origin.distance(p) >= DRAG_THRESHOLD;
                    }
                    if drag.moving {
                        item_cursor = mouse::Cursor::Unavailable;
                        shell.request_redraw();
                    }
                }
            }
            (Some(on_move), Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))) => {
                if let Some(drag) = state.drag.take().filter(|d| d.moving) {
                    item_cursor = mouse::Cursor::Unavailable;
                    if let Some(to) = drag.target(layout)
                        && to != drag.index
                        && drag.index < self.items.len()
                    {
                        shell.publish(on_move(drag.index, to));
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            (Some(_), Event::Window(window::Event::Unfocused)) => {
                // The button may be let go where the window never hears of it.
                state.drag = None;
                shell.request_redraw();
            }
            _ => {}
        }

        for ((child, state), layout) in self.items.iter_mut().zip(&mut tree.children).zip(layout.children()) {
            child.as_widget_mut().update(state, event, layout, item_cursor, renderer, clipboard, shell, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if self.moving(tree).is_some() {
            return mouse::Interaction::Grabbing;
        }
        self.items
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, state), layout)| {
                child.as_widget().mouse_interaction(state, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let drag = self.moving(tree);
        let item_cursor = if drag.is_some() { mouse::Cursor::Unavailable } else { cursor };
        let rounded = Border::default().rounded(4);

        for (i, ((child, state), layout)) in self.items.iter().zip(&tree.children).zip(layout.children()).enumerate() {
            let b = layout.bounds();
            if !b.intersects(viewport) {
                continue;
            }
            if drag.is_some_and(|d| d.index == i) {
                // The empty place the item was lifted from.
                renderer.fill_quad(
                    Quad { bounds: b, border: rounded.color(theme::muted(theme)).width(1.0), ..Quad::default() },
                    Color::TRANSPARENT,
                );
            } else {
                child.as_widget().draw(state, renderer, theme, style, layout, item_cursor, viewport);
            }
        }

        let Some(drag) = drag else { return };
        let items: Vec<Rectangle> = layout.children().map(|l| l.bounds()).collect();
        let lifted = items[drag.index];

        // Where it will land: a line in the gap it would go into.
        if let Some(to) = drag.target(layout).filter(|to| *to != drag.index) {
            let others: Vec<&Rectangle> =
                items.iter().enumerate().filter(|(i, _)| *i != drag.index).map(|(_, b)| b).collect();
            let gap = self.spacing / 2.0;
            let y = match (to.checked_sub(1).and_then(|i| others.get(i)), others.get(to)) {
                (Some(above), Some(below)) => (above.y + above.height + below.y) / 2.0,
                (Some(above), None) => above.y + above.height + gap,
                (None, Some(below)) => below.y - gap,
                (None, None) => lifted.center_y(),
            };
            renderer.fill_quad(
                Quad {
                    bounds: Rectangle { x: lifted.x, y: y - LINE / 2.0, width: lifted.width, height: LINE },
                    border: Border::default().rounded(LINE / 2.0),
                    ..Quad::default()
                },
                theme::accent(theme),
            );
        }

        // The item itself, following the pointer up and down, kept in view.
        let list = layout.bounds();
        let top = list.y.max(viewport.y);
        let bottom = (list.y + list.height).min(viewport.y + viewport.height) - lifted.height;
        let y = (lifted.y + drag.cursor.y - drag.origin.y).min(bottom).max(top);
        let ghost = Rectangle { y, ..lifted };
        let dy = Vector::new(0.0, y - lifted.y);
        let p = theme.extended_palette();
        let (child, state) = (&self.items[drag.index], &tree.children[drag.index]);
        let item = layout.children().nth(drag.index).expect("index checked");
        renderer.with_layer(ghost.expand(12.0), |r| {
            r.fill_quad(
                Quad {
                    bounds: ghost,
                    border: rounded.color(theme::accent(theme)).width(1.0),
                    shadow: Shadow {
                        color: Color { a: 0.35, ..Color::BLACK },
                        offset: Vector::new(0.0, 2.0),
                        blur_radius: 8.0,
                    },
                    ..Quad::default()
                },
                p.background.weak.color,
            );
            r.with_translation(dy, |r| {
                child.as_widget().draw(state, r, theme, style, item, mouse::Cursor::Unavailable, &lifted);
            });
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        // No tooltips while an item is moving.
        if self.moving(tree).is_some() {
            return None;
        }
        overlay::from_children(&mut self.items, tree, layout, renderer, viewport, translation)
    }
}

impl<'a, Message: 'a> From<Reorder<'a, Message>> for Element<'a, Message> {
    fn from(list: Reorder<'a, Message>) -> Self {
        Element::new(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where an item dragged from `index` would land with the pointer at
    /// (`x`, `y`), in a list 100 wide of three items 40 high with 2 between
    /// them. Their middles are at 20, 62 and 104.
    fn target(index: usize, x: f32, y: f32) -> Option<usize> {
        let items = (0..3)
            .map(|i| layout::Node::new(Size::new(100.0, 40.0)).move_to(Point::new(0.0, i as f32 * 42.0)))
            .collect();
        let list = layout::Node::with_children(Size::new(100.0, 124.0), items);
        let drag = Drag { index, origin: Point::ORIGIN, cursor: Point::new(x, y), moving: true };
        drag.target(Layout::new(&list))
    }

    #[test]
    fn items_land_where_the_pointer_is() {
        assert_eq!(target(0, 50.0, 10.0), Some(0), "still in its place");
        assert_eq!(target(0, 50.0, 61.0), Some(0), "not yet past the middle of the next item");
        assert_eq!(target(0, 50.0, 70.0), Some(1), "past the middle of the next item");
        assert_eq!(target(0, 50.0, 500.0), Some(2), "below the list: last");
        assert_eq!(target(2, 50.0, -30.0), Some(0), "above the list: first");
        assert_eq!(target(2, 50.0, 50.0), Some(1));
        assert_eq!(target(1, 105.0, 10.0), Some(0), "a little beside the list still counts");
        assert_eq!(target(1, 300.0, 10.0), None, "dropped on something else: cancelled");
        assert_eq!(target(1, -20.0, 10.0), None);
    }
}
