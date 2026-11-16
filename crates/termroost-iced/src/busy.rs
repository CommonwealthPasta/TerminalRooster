//! What goes on in the terminals of a list item (a session, a project or a
//! closed group): a spinner while a coding agent is in the middle of a turn.

use std::sync::LazyLock;
use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Clipboard, Shell, renderer, svg};
use iced::time::Instant;
use iced::widget::{container, space, text, tooltip};
use iced::{Color, Element, Event, Length, Radians, Rectangle, Size, Theme, mouse, window};
use termroost_core::PaneId;

use crate::app::{App, Message};

/// What goes on in some terminals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Busy {
    /// The coding agents in the middle of a turn, by name.
    pub working: Vec<String>,
}

impl App {
    /// What goes on in these terminals.
    pub(crate) fn busy_in(&self, panes: impl IntoIterator<Item = PaneId>) -> Busy {
        let s = self.core.state();
        let mut busy = Busy::default();
        for pane in panes {
            let Some(activity) = self.busy.get(&pane) else { continue };
            if activity.agent_working {
                // The agent the scan found, else the one last recorded for the terminal.
                let kind =
                    activity.programs.iter().find_map(|p| p.agent).or_else(|| Some(s.pane(pane)?.agent.as_ref()?.kind));
                let name = kind.map_or("Agent", |k| k.label()).to_owned();
                if !busy.working.contains(&name) {
                    busy.working.push(name);
                }
            }
        }
        busy
    }
}

/// Size of the mark, in pixels.
const MARK_SIZE: f32 = 12.0;

/// A spinner while an agent works, with a tooltip saying which; nothing
/// otherwise. `ink` is its color.
pub fn busy_mark<'a>(busy: Busy, ink: impl Fn(&Theme) -> Color + 'a) -> Element<'a, Message> {
    if busy.working.is_empty() {
        return space::horizontal().width(0).into();
    }
    let tip = format!("Working: {}", busy.working.join(", "));
    tooltip(
        Element::new(Spinner { ink: Box::new(ink) }),
        container(text(tip).size(12)).padding([4, 8]).style(container::rounded_box),
        tooltip::Position::Bottom,
    )
    .into()
}

/// A faint ring with a quarter of it drawn solid; it turns.
static SPINNER: LazyLock<svg::Handle> = LazyLock::new(|| {
    svg::Handle::from_memory(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="none" stroke="#000" stroke-width="2.5" opacity="0.3"/><path d="M8 2a6 6 0 0 1 6 6" fill="none" stroke="#000" stroke-width="2.5" stroke-linecap="round"/></svg>"##
            .as_slice(),
    )
});

/// How long a frame of the spinner shows, and how many frames make a turn.
/// Few, as each one redraws the window.
const FRAME: Duration = Duration::from_millis(100);
const FRAMES: u32 = 10;

/// All spinners turn together, counted from this moment.
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Which frame shows at `now`.
fn frame_at(now: Instant) -> u32 {
    let since = now.saturating_duration_since(*EPOCH);
    (since.as_millis() / FRAME.as_millis()) as u32
}

/// The spinner. It asks for its own redraws, so it turns without the app
/// building its view again.
struct Spinner<'a> {
    ink: Box<dyn Fn(&Theme) -> Color + 'a>,
}

impl<M> Widget<M, Theme, iced::Renderer> for Spinner<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(MARK_SIZE), Length::Fixed(MARK_SIZE))
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &iced::Renderer, _limits: &layout::Limits) -> layout::Node {
        layout::Node::new(Size::new(MARK_SIZE, MARK_SIZE))
    }

    fn update(
        &mut self,
        _tree: &mut Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            shell.request_redraw_at(*EPOCH + FRAME * (frame_at(*now) + 1));
        }
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let turn = (frame_at(Instant::now()) % FRAMES) as f32 / FRAMES as f32;
        let spinner = svg::Svg::new(LazyLock::force(&SPINNER).clone())
            .color((self.ink)(theme))
            .rotation(Radians(turn * std::f32::consts::TAU));
        svg::Renderer::draw_svg(renderer, spinner, layout.bounds(), layout.bounds());
    }
}
