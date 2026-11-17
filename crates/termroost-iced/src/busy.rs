//! What goes on in the terminals of a list item (a session, a project or a
//! closed group): a spinner while a coding agent is in the middle of a turn,
//! else a pulse while programs run (a dev server, a build, an editor,
//! commands an agent left running). The tooltip says which.

use std::sync::LazyLock;
use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Clipboard, Shell, renderer, svg};
use iced::time::Instant;
use iced::widget::{container, space, svg as svg_widget, text, tooltip};
use iced::{Color, Element, Event, Length, Rectangle, Size, Theme, mouse, window};
use termroost_core::PaneId;

use crate::app::{App, Message};

/// What goes on in some terminals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Busy {
    /// The coding agents in the middle of a turn, by name.
    pub working: Vec<String>,
    /// The programs running, by name; those an agent started say so.
    pub running: Vec<String>,
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
            for p in activity.busy_programs() {
                let name = match p.started_by {
                    Some(agent) => format!("{} (started by {agent})", p.name),
                    None => p.name.clone(),
                };
                if !busy.running.contains(&name) {
                    busy.running.push(name);
                }
            }
        }
        busy
    }
}

/// Size of the mark, in pixels.
const MARK_SIZE: f32 = 12.0;

/// A spinner while an agent works, else a pulse while programs run, with a
/// tooltip saying which; nothing when all is quiet. `ink` is its color.
pub fn busy_mark<'a>(busy: Busy, ink: impl Fn(&Theme) -> Color + 'a) -> Element<'a, Message> {
    let mark: Element<'a, Message> = if !busy.working.is_empty() {
        Element::new(Spinner { ink: Box::new(ink) })
    } else if !busy.running.is_empty() {
        svg_widget(LazyLock::force(&PULSE).clone())
            .width(MARK_SIZE)
            .height(MARK_SIZE)
            .style(move |t, _| svg_widget::Style { color: Some(ink(t)) })
            .into()
    } else {
        return space::horizontal().width(0).into();
    };
    let mut tip = Vec::new();
    if !busy.working.is_empty() {
        tip.push(format!("Working: {}", busy.working.join(", ")));
    }
    if !busy.running.is_empty() {
        tip.push(format!("Running: {}", busy.running.join(", ")));
    }
    tooltip(
        mark,
        container(text(tip.join("\n")).size(12)).padding([5, 9]).style(crate::theme::tooltip),
        tooltip::Position::Bottom,
    )
    .into()
}

/// A pulse line, as on a heart monitor: something is alive in there.
static PULSE: LazyLock<svg::Handle> = LazyLock::new(|| {
    svg::Handle::from_memory(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><polyline points="1,8.5 4.5,8.5 6.5,3 9.5,13 11.5,8.5 15,8.5" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##
            .as_slice(),
    )
});

/// The spinner's frames: a faint ring with a quarter of it drawn solid, the
/// quarter a step further round in each. Each frame is a picture of its own
/// rather than one picture turned: iced draws an SVG as a bitmap the size of
/// the mark, and turns that bitmap with nearest-neighbour sampling, which
/// showed its pixels.
static SPINNER: LazyLock<Vec<svg::Handle>> = LazyLock::new(|| (0..FRAMES).map(spinner_frame).collect());

/// Frame `frame` of the spinner (see [`SPINNER`]).
fn spinner_frame(frame: u32) -> svg::Handle {
    // Clockwise from twelve o'clock, on a ring of radius 6 around (8, 8).
    let point = |turn: f32| {
        let (sin, cos) = (turn * std::f32::consts::TAU).sin_cos();
        (8.0 + 6.0 * sin, 8.0 - 6.0 * cos)
    };
    let start = frame as f32 / FRAMES as f32;
    let (x0, y0) = point(start);
    let (x1, y1) = point(start + 0.25);
    svg::Handle::from_memory(
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="none" stroke="#000" stroke-width="2.5" opacity="0.3"/><path d="M{x0:.3} {y0:.3}A6 6 0 0 1 {x1:.3} {y1:.3}" fill="none" stroke="#000" stroke-width="2.5" stroke-linecap="round"/></svg>"##
        )
        .into_bytes(),
    )
}

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
        let frame = &SPINNER[(frame_at(Instant::now()) % FRAMES) as usize];
        let spinner = svg::Svg::new(frame.clone()).color((self.ink)(theme));
        svg::Renderer::draw_svg(renderer, spinner, layout.bounds(), layout.bounds());
    }
}
