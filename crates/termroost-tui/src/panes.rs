//! Where a session's terminals go on screen.
//!
//! Each terminal gets a title row and the rows below it, a column in from
//! the left. Side-by-side terminals have a one-column divider between them;
//! stacked ones need none, since the lower one's title row separates them.

use ratatui::layout::Rect;
use termroost_core::{Axis, Layout, PaneId};

/// A terminal's place: its title row is `area`'s first row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub pane: PaneId,
    pub area: Rect,
}

impl Placed {
    pub fn title(&self) -> Rect {
        Rect { height: self.area.height.min(1), ..self.area }
    }

    /// Below the title, a column in from the left edge.
    pub fn content(&self) -> Rect {
        let a = self.area;
        Rect { x: a.x + 1, y: a.y + 1, width: a.width.saturating_sub(1), height: a.height.saturating_sub(1) }
    }
}

/// The line between the two sides of a split.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divider {
    pub axis: Axis,
    /// The area of the whole split.
    pub split: Rect,
    /// The divider's column (vertical) or the lower title row (horizontal).
    pub at: u16,
}

#[derive(Debug, Default)]
pub struct Arranged {
    pub panes: Vec<Placed>,
    pub dividers: Vec<Divider>,
}

pub fn arrange(layout: &Layout, area: Rect) -> Arranged {
    let mut out = Arranged::default();
    place(layout, area, &mut out);
    out
}

fn place(layout: &Layout, area: Rect, out: &mut Arranged) {
    match layout {
        Layout::Pane(pane) => out.panes.push(Placed { pane: *pane, area }),
        Layout::Split { axis, ratio, a, b } => {
            let (ra, rb, at) = match axis {
                Axis::Vertical => {
                    let avail = area.width.saturating_sub(1);
                    let wa = share(avail, *ratio);
                    let ra = Rect { width: wa, ..area };
                    let rb = Rect { x: area.x + wa + 1, width: avail - wa, ..area };
                    (ra, rb, area.x + wa)
                }
                Axis::Horizontal => {
                    let ha = share(area.height, *ratio);
                    let ra = Rect { height: ha, ..area };
                    let rb = Rect { y: area.y + ha, height: area.height - ha, ..area };
                    (ra, rb, area.y + ha)
                }
            };
            out.dividers.push(Divider { axis: *axis, split: area, at });
            place(a, ra, out);
            place(b, rb, out);
        }
    }
}

/// `ratio` of `total`, leaving each side at least one cell when there are two.
fn share(total: u16, ratio: f32) -> u16 {
    let n = (total as f32 * ratio).round() as u16;
    if total >= 2 { n.clamp(1, total - 1) } else { n.min(total) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(i: i64) -> PaneId {
        PaneId(i)
    }

    #[test]
    fn side_by_side_and_stacked() {
        let mut l = Layout::Pane(p(1));
        l.split(p(1), Axis::Vertical, p(2));
        l.split(p(2), Axis::Horizontal, p(3));
        let out = arrange(&l, Rect::new(0, 0, 81, 20));
        assert_eq!(out.panes[0], Placed { pane: p(1), area: Rect::new(0, 0, 40, 20) });
        assert_eq!(out.panes[1], Placed { pane: p(2), area: Rect::new(41, 0, 40, 10) });
        assert_eq!(out.panes[2], Placed { pane: p(3), area: Rect::new(41, 10, 40, 10) });
        assert_eq!(out.dividers[0].at, 40);
        assert_eq!(out.dividers[1].at, 10);
        assert_eq!(out.panes[2].content(), Rect::new(42, 11, 39, 9));
    }

    #[test]
    fn tiny_areas_do_not_underflow() {
        let mut l = Layout::Pane(p(1));
        l.split(p(1), Axis::Vertical, p(2));
        let out = arrange(&l, Rect::new(0, 0, 1, 1));
        assert_eq!(out.panes.len(), 2);
        let out = arrange(&l, Rect::new(0, 0, 0, 0));
        assert_eq!(out.panes[1].area.width, 0);
    }
}
