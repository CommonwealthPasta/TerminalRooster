//! A menu that floats under the widget it belongs to, over whatever is below
//! it, instead of taking room in the layout and pushing that down.
//!
//! Pressing a mouse button anywhere else, or turning the wheel, closes it;
//! the press still does what it does there.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Clipboard, Shell, overlay};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector, mouse};

/// Space between the widget and its menu.
const GAP: f32 = 2.0;

pub struct ContextMenu<'a, Message> {
    content: Element<'a, Message, Theme, iced::Renderer>,
    menu: Option<Element<'a, Message, Theme, iced::Renderer>>,
    indent: f32,
    on_close: Message,
}

/// `content`, with `menu` floating under it while there is one. `on_close`
/// is sent when the menu should close.
pub fn context_menu<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    menu: Option<Element<'a, Message>>,
    on_close: Message,
) -> ContextMenu<'a, Message> {
    ContextMenu { content: content.into(), menu, indent: 0.0, on_close }
}

impl<Message> ContextMenu<'_, Message> {
    /// How far right of the content's left edge the menu starts.
    pub fn indent(mut self, indent: f32) -> Self {
        self.indent = indent;
        self
    }
}

impl<Message: Clone> Widget<Message, Theme, iced::Renderer> for ContextMenu<'_, Message> {
    fn children(&self) -> Vec<Tree> {
        std::iter::once(&self.content).chain(&self.menu).map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        match &self.menu {
            Some(menu) => tree.diff_children(&[&self.content, menu]),
            None => tree.diff_children(std::slice::from_ref(&self.content)),
        }
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        let content = self.content.as_widget_mut().layout(&mut tree.children[0], renderer, limits);
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
        let content = layout.children().next().unwrap();
        self.content.as_widget().draw(&tree.children[0], renderer, theme, style, content, cursor, viewport);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        let (content_tree, menu_tree) = tree.children.split_first_mut()?;
        match (&mut self.menu, menu_tree.first_mut()) {
            (Some(menu), Some(tree)) => Some(overlay::Element::new(Box::new(MenuOverlay {
                menu,
                tree,
                anchor: layout.bounds() + translation,
                indent: self.indent,
                on_close: self.on_close.clone(),
            }))),
            _ => {
                let content = layout.children().next().unwrap();
                self.content.as_widget_mut().overlay(content_tree, content, renderer, viewport, translation)
            }
        }
    }
}

impl<'a, Message: Clone + 'a> From<ContextMenu<'a, Message>> for Element<'a, Message> {
    fn from(w: ContextMenu<'a, Message>) -> Self {
        Element::new(w)
    }
}

struct MenuOverlay<'a, 'b, Message> {
    menu: &'a mut Element<'b, Message, Theme, iced::Renderer>,
    tree: &'a mut Tree,
    /// Where the widget the menu belongs to is in the window.
    anchor: Rectangle,
    indent: f32,
    on_close: Message,
}

impl<Message: Clone> overlay::Overlay<Message, Theme, iced::Renderer> for MenuOverlay<'_, '_, Message> {
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let node = self.menu.as_widget_mut().layout(self.tree, renderer, &layout::Limits::new(Size::ZERO, bounds));
        let size = node.size();
        // Under the widget, or over it when there is no room below; in the window either way.
        let below = self.anchor.y + self.anchor.height + GAP;
        let above = self.anchor.y - GAP - size.height;
        let y = if below + size.height <= bounds.height || above < 0.0 { below } else { above };
        let x = (self.anchor.x + self.indent).min(bounds.width - size.width).max(0.0);
        node.move_to(Point::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.menu.as_widget().draw(self.tree, renderer, theme, style, layout, cursor, &layout.bounds());
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let press = matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::WheelScrolled { .. }));
        let over = cursor.is_over(layout.bounds());
        if press && !over {
            // Not captured: the press still reaches what it's on.
            shell.publish(self.on_close.clone());
            return;
        }
        self.menu.as_widget_mut().update(
            self.tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &layout.bounds(),
        );
        if press {
            // A press on the menu is the menu's, not the tree's under it.
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if !cursor.is_over(layout.bounds()) {
            return mouse::Interaction::None;
        }
        // Never None over the menu: that tells the window the pointer is on
        // the menu, so what is under it doesn't light up.
        match self.menu.as_widget().mouse_interaction(self.tree, layout, cursor, &layout.bounds(), renderer) {
            mouse::Interaction::None => mouse::Interaction::Idle,
            interaction => interaction,
        }
    }
}
