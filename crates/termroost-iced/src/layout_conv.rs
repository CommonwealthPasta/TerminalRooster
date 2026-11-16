//! Conversion between the core layout tree and iced's pane grid.

use iced::widget::pane_grid::{self, Configuration, Node};
use termroost_core::{Axis, Layout, PaneId};

fn to_iced_axis(a: Axis) -> pane_grid::Axis {
    match a {
        Axis::Horizontal => pane_grid::Axis::Horizontal,
        Axis::Vertical => pane_grid::Axis::Vertical,
    }
}

fn from_iced_axis(a: pane_grid::Axis) -> Axis {
    match a {
        pane_grid::Axis::Horizontal => Axis::Horizontal,
        pane_grid::Axis::Vertical => Axis::Vertical,
    }
}

fn configuration(layout: &Layout) -> Configuration<PaneId> {
    match layout {
        Layout::Pane(p) => Configuration::Pane(*p),
        Layout::Split { axis, ratio, a, b } => Configuration::Split {
            axis: to_iced_axis(*axis),
            ratio: *ratio,
            a: Box::new(configuration(a)),
            b: Box::new(configuration(b)),
        },
    }
}

pub fn grid_state(layout: &Layout) -> pane_grid::State<PaneId> {
    pane_grid::State::with_configuration(configuration(layout))
}

pub fn layout_of(state: &pane_grid::State<PaneId>) -> Option<Layout> {
    fn walk(node: &Node, state: &pane_grid::State<PaneId>) -> Option<Layout> {
        Some(match node {
            Node::Pane(p) => Layout::Pane(*state.get(*p)?),
            Node::Split { axis, ratio, a, b, .. } => Layout::Split {
                axis: from_iced_axis(*axis),
                ratio: *ratio,
                a: Box::new(walk(a, state)?),
                b: Box::new(walk(b, state)?),
            },
        })
    }
    walk(state.layout(), state)
}

/// Finds the grid pane showing a core pane.
pub fn find(state: &pane_grid::State<PaneId>, pane: PaneId) -> Option<pane_grid::Pane> {
    state.iter().find(|(_, p)| **p == pane).map(|(g, _)| *g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut l = Layout::Pane(PaneId(1));
        l.split(PaneId(1), Axis::Vertical, PaneId(2));
        l.split(PaneId(2), Axis::Horizontal, PaneId(3));
        let state = grid_state(&l);
        assert_eq!(layout_of(&state), Some(l));
        assert!(find(&state, PaneId(3)).is_some());
        assert!(find(&state, PaneId(9)).is_none());
    }
}
