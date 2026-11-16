//! A wrapper that keeps its content out of a strip on its right.
//!
//! A pane's title bar lays its title out as wide as the whole bar, then draws
//! the title over its buttons when the two don't fit side by side. Wrapped in
//! this, the title ends before the buttons and is cut off there instead.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::{Element, Event, Length, Rectangle, Size, Theme, Vector, mouse};

pub struct LeaveRoom<'a, Message> {
    content: Element<'a, Message, Theme, iced::Renderer>,
    room: f32,
}

/// `content`, at most as wide as the space it gets less `room` pixels.
pub fn leave_room<'a, Message>(content: impl Into<Element<'a, Message>>, room: f32) -> LeaveRoom<'a, Message> {
    LeaveRoom { content: content.into(), room }
}

impl<Message> Widget<Message, Theme, iced::Renderer> for LeaveRoom<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        let limits = limits.max_width((limits.max().width - self.room).max(0.0));
        let content = self.content.as_widget_mut().layout(&mut tree.children[0], renderer, &limits);
        layout::Node::with_children(content.size(), vec![content])
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let content = layout.children().next().unwrap();
        self.content.as_widget_mut().operate(&mut tree.children[0], content, renderer, operation);
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
        let content = layout.children().next().unwrap();
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            content,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let content = layout.children().next().unwrap();
        self.content.as_widget().mouse_interaction(&tree.children[0], content, cursor, viewport, renderer)
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
        // Content that doesn't fit its bounds (unwrapped text) would still
        // draw past them, over the buttons.
        let Some(clip) = layout.bounds().intersection(viewport) else { return };
        let content = layout.children().next().unwrap();
        renderer.with_layer(clip, |renderer| {
            self.content.as_widget().draw(&tree.children[0], renderer, theme, style, content, cursor, &clip);
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
        let content = layout.children().next().unwrap();
        self.content.as_widget_mut().overlay(&mut tree.children[0], content, renderer, viewport, translation)
    }
}

impl<'a, Message: 'a> From<LeaveRoom<'a, Message>> for Element<'a, Message> {
    fn from(w: LeaveRoom<'a, Message>) -> Self {
        Element::new(w)
    }
}
