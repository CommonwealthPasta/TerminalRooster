//! A vertical list whose items can be dragged into a new order.
//!
//! Items keep working as usual: a press still reaches them, so clicking,
//! double-clicking and their buttons are unchanged. Once the pointer moves a
//! few pixels with the button held, the item follows the pointer and a line
//! shows where it will land. Dropping it beside the list (e.g. on a terminal)
//! cancels the move.
//!
//! Items can be nested ([`Reorder::nested`]): an item deeper than the one above
//! it belongs to it and moves with it. A list can also take items dropped onto
//! others ([`Reorder::on_drop`]).

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
/// The middle part of an item, as fractions of its height, where a drop goes
/// onto it (if the list takes that) rather than above or below it.
const ONTO: std::ops::Range<f32> = 0.3..0.7;

/// Where a dragged item would land. Indices are of the whole list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Between two items (`None` past an end of the list), at this depth.
    Between { above: Option<usize>, below: Option<usize>, depth: u8 },
    /// Onto an item.
    Onto(usize),
}

type OnDrop<'a, Message> = Box<dyn Fn(usize, Target) -> Option<Message> + 'a>;

pub struct Reorder<'a, Message> {
    items: Vec<Element<'a, Message, Theme, iced::Renderer>>,
    spacing: f32,
    /// Depth of each item; missing ones are 0.
    depths: Vec<u8>,
    /// How far right each level of depth is drawn.
    indent: f32,
    on_drop: Option<OnDrop<'a, Message>>,
}

/// A list of `items` that can be reordered by dragging; see [`Reorder::on_move`].
pub fn reorder<'a, Message>(items: impl IntoIterator<Item = Element<'a, Message>>) -> Reorder<'a, Message> {
    Reorder { items: items.into_iter().collect(), spacing: 0.0, depths: Vec::new(), indent: 0.0, on_drop: None }
}

impl<'a, Message: 'a> Reorder<'a, Message> {
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// The depth of each item, for lists of items that hold others: an item
    /// deeper than the one above belongs to it. `indent` is how far right
    /// the items show each level, for drawing where a dragged item lands.
    pub fn nested(mut self, depths: Vec<u8>, indent: f32) -> Self {
        self.depths = depths;
        self.indent = indent;
        self
    }

    /// Called with `(from, to)` when an item is dropped somewhere else: the
    /// item at `from` goes to index `to` of the list without it. Without this
    /// (or [`Reorder::on_drop`]), nothing can be dragged. For lists that
    /// aren't nested.
    pub fn on_move(self, f: impl Fn(usize, usize) -> Message + 'a) -> Self {
        self.on_drop(move |from, target| match target {
            Target::Between { above, .. } => {
                // The items above the gap, without the moved one.
                let to = above.map_or(0, |a| a + 1 - usize::from(a > from));
                (to != from).then(|| f(from, to))
            }
            Target::Onto(_) => None,
        })
    }

    /// Like [`Reorder::on_move`], but only when `enabled`.
    pub fn on_move_if(self, enabled: bool, f: impl Fn(usize, usize) -> Message + 'a) -> Self {
        if enabled { self.on_move(f) } else { self }
    }

    /// Asked what dropping the item at `from` (with the items it holds) on a
    /// target does: `None` if it can't go there, which shows no target and
    /// cancels a drop. Onto an item is only asked about over its middle.
    pub fn on_drop(mut self, f: impl Fn(usize, Target) -> Option<Message> + 'a) -> Self {
        self.on_drop = Some(Box::new(f));
        self
    }

    /// Like [`Reorder::on_drop`], but only when `enabled`.
    pub fn on_drop_if(self, enabled: bool, f: impl Fn(usize, Target) -> Option<Message> + 'a) -> Self {
        if enabled { self.on_drop(f) } else { self }
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

/// How many items move when the one at `index` is dragged: it and the deeper
/// ones right after it.
fn span(depths: &[u8], index: usize, len: usize) -> usize {
    let depth = |i: usize| depths.get(i).copied().unwrap_or(0);
    1 + (index + 1..len).take_while(|&i| depth(i) > depth(index)).count()
}

/// Where the items at `moved` would land with the pointer at `cursor`, given
/// the bounds of the list and of each item; `None` cancels the move.
/// `accepts` says whether the list takes a target.
fn find_target(
    cursor: Point,
    list: Rectangle,
    items: &[Rectangle],
    depths: &[u8],
    moved: std::ops::Range<usize>,
    accepts: impl Fn(Target) -> bool,
) -> Option<Target> {
    if cursor.x < list.x - DROP_MARGIN || cursor.x > list.x + list.width + DROP_MARGIN {
        return None;
    }
    let depth = |i: usize| depths.get(i).copied().unwrap_or(0);
    // Over the place it was lifted from: it stays.
    let first = items.get(moved.start)?;
    let last = items.get(moved.end - 1)?;
    if (first.y..last.y + last.height).contains(&cursor.y) {
        return None;
    }
    let others: Vec<usize> = (0..items.len()).filter(|i| !moved.contains(i)).collect();
    let distance = |i: usize| {
        let b = items[i];
        (b.y - cursor.y).max(cursor.y - (b.y + b.height)).max(0.0)
    };
    // The item under the pointer, or the nearest one.
    let pos = (0..others.len()).min_by(|&a, &b| distance(others[a]).total_cmp(&distance(others[b])))?;
    let near = others[pos];
    let b = items[near];
    let at = (cursor.y - b.y) / b.height;
    if ONTO.contains(&at) && accepts(Target::Onto(near)) {
        return Some(Target::Onto(near));
    }
    let target = if at < 0.5 {
        // Above it, at its depth.
        Target::Between { above: pos.checked_sub(1).map(|p| others[p]), below: Some(near), depth: depth(near) }
    } else {
        // Below it, still inside whatever it is in, except past the end of the list.
        let below = others.get(pos + 1).copied();
        let depth = match below {
            Some(below) => depth(near).max(depth(below)),
            None if at > 1.0 => 0,
            None => depth(near),
        };
        Target::Between { above: Some(near), below, depth }
    };
    accepts(target).then_some(target)
}

impl<Message> Reorder<'_, Message> {
    /// The drag to draw, if an item is being moved.
    fn moving(&self, tree: &Tree) -> Option<Drag> {
        let drag = tree.state.downcast_ref::<State>().drag?;
        (drag.moving && drag.index < self.items.len()).then_some(drag)
    }

    /// The items that move with a drag.
    fn moved(&self, drag: &Drag) -> std::ops::Range<usize> {
        drag.index..drag.index + span(&self.depths, drag.index, self.items.len())
    }

    /// Where the dragged items would land now.
    fn target(&self, drag: &Drag, layout: Layout<'_>) -> Option<Target> {
        let on_drop = self.on_drop.as_ref()?;
        let items: Vec<Rectangle> = layout.children().map(|l| l.bounds()).collect();
        find_target(drag.cursor, layout.bounds(), &items, &self.depths, self.moved(drag), |t| {
            on_drop(drag.index, t).is_some()
        })
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
        // While an item is moving, the items don't see the pointer: nothing
        // under it lights up, and a button pressed to start the drag doesn't
        // fire when it is let go.
        let mut item_cursor = cursor;
        let enabled = self.on_drop.is_some();
        match event {
            _ if !enabled => tree.state.downcast_mut::<State>().drag = None,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                tree.state.downcast_mut::<State>().drag = cursor.position().and_then(|p| {
                    let index = layout.children().position(|l| l.bounds().contains(p))?;
                    Some(Drag { index, origin: p, cursor: p, moving: false })
                });
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(drag) = &mut tree.state.downcast_mut::<State>().drag {
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
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let drag = tree.state.downcast_mut::<State>().drag.take();
                if let Some(drag) = drag.filter(|d| d.moving) {
                    item_cursor = mouse::Cursor::Unavailable;
                    if drag.index < self.items.len()
                        && let Some(target) = self.target(&drag, layout)
                        && let Some(message) = self.on_drop.as_ref().and_then(|f| f(drag.index, target))
                    {
                        shell.publish(message);
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Window(window::Event::Unfocused) => {
                // The button may be let go where the window never hears of it.
                tree.state.downcast_mut::<State>().drag = None;
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
        let moved = drag.map(|d| self.moved(&d)).unwrap_or_default();
        let item_cursor = if drag.is_some() { mouse::Cursor::Unavailable } else { cursor };
        let rounded = Border::default().rounded(theme::RADIUS);
        let items: Vec<Rectangle> = layout.children().map(|l| l.bounds()).collect();

        for (i, ((child, state), layout)) in self.items.iter().zip(&tree.children).zip(layout.children()).enumerate() {
            if !moved.contains(&i) && layout.bounds().intersects(viewport) {
                child.as_widget().draw(state, renderer, theme, style, layout, item_cursor, viewport);
            }
        }

        let Some(drag) = drag else { return };
        // The empty place the items were lifted from.
        let first = items[moved.start];
        let last = items[moved.end - 1];
        let lifted = Rectangle { height: last.y + last.height - first.y, ..first };
        renderer.fill_quad(
            Quad { bounds: lifted, border: rounded.color(theme::muted(theme)).width(1.0), ..Quad::default() },
            Color::TRANSPARENT,
        );

        let list = layout.bounds();
        match self.target(&drag, layout) {
            // Where it will land: a line in the gap it would go into, as far
            // right as the items at that depth.
            Some(Target::Between { above, below, depth }) => {
                let gap = self.spacing / 2.0;
                let y = match (above.map(|i| items[i]), below.map(|i| items[i])) {
                    (Some(a), Some(b)) => (a.y + a.height + b.y) / 2.0,
                    (Some(a), None) => a.y + a.height + gap,
                    (None, Some(b)) => b.y - gap,
                    (None, None) => lifted.center_y(),
                };
                let x = list.x + f32::from(depth) * self.indent;
                renderer.fill_quad(
                    Quad {
                        bounds: Rectangle { x, y: y - LINE / 2.0, width: list.x + list.width - x, height: LINE },
                        border: Border::default().rounded(LINE / 2.0),
                        ..Quad::default()
                    },
                    theme::accent(theme),
                );
            }
            // An outline around the item it would go onto.
            Some(Target::Onto(i)) => {
                renderer.fill_quad(
                    Quad {
                        bounds: items[i],
                        border: rounded.color(theme::accent(theme)).width(2.0),
                        ..Quad::default()
                    },
                    Color::TRANSPARENT,
                );
            }
            None => {}
        }

        // The items themselves, following the pointer up and down, kept in view.
        let top = list.y.max(viewport.y);
        let bottom = (list.y + list.height).min(viewport.y + viewport.height) - lifted.height;
        let y = (lifted.y + drag.cursor.y - drag.origin.y).min(bottom).max(top);
        let ghost = Rectangle { y, ..lifted };
        let dy = Vector::new(0.0, y - lifted.y);
        let p = theme.extended_palette();
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
                let lifted_items = self.items.iter().zip(&tree.children).zip(layout.children()).skip(moved.start);
                for ((child, state), item) in lifted_items.take(moved.len()) {
                    child.as_widget().draw(state, r, theme, style, item, mouse::Cursor::Unavailable, &lifted);
                }
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

    /// A list 100 wide of items 40 high with 2 between them: their tops are
    /// at 0, 42, 84… and their middles at 20, 62, 104…
    fn items(n: usize) -> (Rectangle, Vec<Rectangle>) {
        let items: Vec<Rectangle> =
            (0..n).map(|i| Rectangle { x: 0.0, y: i as f32 * 42.0, width: 100.0, height: 40.0 }).collect();
        (Rectangle { x: 0.0, y: 0.0, width: 100.0, height: n as f32 * 42.0 - 2.0 }, items)
    }

    /// Where the item at `index` of a flat list of three, dragged with the
    /// pointer at (`x`, `y`), would go: its index in the list without it.
    fn moved_to(index: usize, x: f32, y: f32) -> Option<usize> {
        let (list, items) = items(3);
        let target = find_target(Point::new(x, y), list, &items, &[], index..index + 1, |t| {
            matches!(t, Target::Between { .. })
        })?;
        let Target::Between { above, .. } = target else { panic!("not onto") };
        let to = above.map_or(0, |a| a + 1 - usize::from(a > index));
        (to != index).then_some(to)
    }

    #[test]
    fn items_land_where_the_pointer_is() {
        assert_eq!(moved_to(0, 50.0, 10.0), None, "still in its place");
        assert_eq!(moved_to(0, 50.0, 61.0), None, "not yet past the middle of the next item");
        assert_eq!(moved_to(0, 50.0, 70.0), Some(1), "past the middle of the next item");
        assert_eq!(moved_to(0, 50.0, 500.0), Some(2), "below the list: last");
        assert_eq!(moved_to(2, 50.0, -30.0), Some(0), "above the list: first");
        assert_eq!(moved_to(2, 50.0, 50.0), Some(1));
        assert_eq!(moved_to(2, 50.0, 41.0), Some(1), "between two items");
        assert_eq!(moved_to(1, 105.0, 10.0), Some(0), "a little beside the list still counts");
        assert_eq!(moved_to(1, 300.0, 10.0), None, "dropped on something else: cancelled");
        assert_eq!(moved_to(1, -20.0, 10.0), None);
    }

    #[test]
    fn deeper_items_move_with_the_one_above() {
        let depths = [0, 1, 1, 0, 0, 1];
        assert_eq!(span(&depths, 0, 6), 3);
        assert_eq!(span(&depths, 1, 6), 1);
        assert_eq!(span(&depths, 3, 6), 1, "nothing deeper after it");
        assert_eq!(span(&depths, 4, 6), 2, "to the end of the list");
        assert_eq!(span(&[], 0, 2), 1, "a flat list");
    }

    #[test]
    fn nested_items_land_inside_or_outside_by_the_item_under_the_pointer() {
        // A group (0) holding two items (1, 2), then two items at the top (3, 4).
        let (list, items) = items(5);
        let depths = [0, 1, 1, 0, 0];
        let target = |moved: std::ops::Range<usize>, y: f32| {
            find_target(Point::new(50.0, y), list, &items, &depths, moved, |_| true)
        };
        let between = |above, below, depth| Some(Target::Between { above, below, depth });
        // Item 4 dragged up.
        assert_eq!(target(4..5, 92.0), between(Some(1), Some(2), 1), "top of 2: inside the group");
        assert_eq!(target(4..5, 117.0), between(Some(2), Some(3), 1), "bottom of 2: still inside");
        assert_eq!(target(4..5, 128.0), between(Some(2), Some(3), 0), "top of 3: outside");
        assert_eq!(target(4..5, 37.0), between(Some(0), Some(1), 1), "bottom of the group: its first");
        assert_eq!(target(4..5, 104.0), Some(Target::Onto(2)), "the middle of 2");
        // The group dragged down, with its items.
        assert_eq!(target(0..3, 60.0), None, "over its own place");
        assert_eq!(target(0..3, 160.0), between(Some(3), Some(4), 0));
        assert_eq!(target(0..3, 500.0), between(Some(4), None, 0), "past the end");
        // Item 2 dragged down past the end: out of the group.
        let (list, items) = self::items(3);
        let target = find_target(Point::new(50.0, 500.0), list, &items, &[0, 1, 1], 2..3, |_| true);
        assert_eq!(target, between(Some(1), None, 0));
        let target = find_target(Point::new(50.0, 100.0), list, &items, &[0, 1, 1], 2..3, |_| true);
        assert_eq!(target, None, "its own place");
    }

    #[test]
    fn a_target_the_list_refuses_shows_nothing_or_what_is_beside_it() {
        let (list, items) = items(3);
        let no_onto = |t: Target| matches!(t, Target::Between { .. });
        let target = find_target(Point::new(50.0, 62.0), list, &items, &[], 0..1, no_onto);
        assert_eq!(target, Some(Target::Between { above: Some(1), below: Some(2), depth: 0 }), "not onto: beside");
        let target = find_target(Point::new(50.0, 62.0), list, &items, &[], 0..1, |_| false);
        assert_eq!(target, None);
    }
}
