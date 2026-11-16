//! Tells the app when Escape is pressed in a text field.
//!
//! iced's text field lets go of the keyboard on Escape and keeps the key to
//! itself, so by the time the app hears of the key it can no longer tell
//! where it was pressed. This wrapper asks its content whether it has the
//! keyboard before passing the key on.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Operation, Tree, Widget, operation};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::keyboard::{self, key::Named};
use iced::{Element, Event, Length, Rectangle, Size, Theme, Vector, mouse};

pub struct OnEscape<'a, Message> {
    content: Element<'a, Message>,
    message: Message,
}

/// Publishes `message` when Escape is pressed while `content` (or something
/// in it) has the keyboard.
pub fn on_escape<'a, Message>(content: impl Into<Element<'a, Message>>, message: Message) -> OnEscape<'a, Message> {
    OnEscape { content: content.into(), message }
}

/// Finds out whether any widget it visits has the keyboard.
struct HasKeyboard(bool);

impl Operation for HasKeyboard {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, state: &mut dyn operation::Focusable) {
        self.0 |= state.is_focused();
    }
}

impl<Message: Clone> Widget<Message, Theme, iced::Renderer> for OnEscape<'_, Message> {
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
        self.content.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
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
        if let Event::Keyboard(keyboard::Event::KeyPressed { key: keyboard::Key::Named(Named::Escape), .. }) = event {
            // Before the content sees the key: a text field lets go of the keyboard on it.
            let mut focus = HasKeyboard(false);
            self.content.as_widget_mut().operate(&mut tree.children[0], layout, renderer, &mut focus);
            if focus.0 {
                shell.publish(self.message.clone());
            }
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
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
        self.content.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
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
        self.content.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation)
    }
}

impl<'a, Message: Clone + 'a> From<OnEscape<'a, Message>> for Element<'a, Message> {
    fn from(widget: OnEscape<'a, Message>) -> Self {
        Element::new(widget)
    }
}
