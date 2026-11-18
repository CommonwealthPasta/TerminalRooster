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

/// The line between the two sides of a split, which can be dragged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divider {
    /// The split in the layout tree: `false` goes to `a`, `true` to `b`.
    pub path: Vec<bool>,
    pub axis: Axis,
    /// The area of the whole split.
    pub split: Rect,
    /// The divider's column (vertical) or the lower title row (horizontal).
    pub at: u16,
}

impl Divider {
    /// Whether a press at this cell grabs the divider.
    pub fn hit(&self, x: u16, y: u16) -> bool {
        let s = self.split;
        match self.axis {
            Axis::Vertical => x == self.at && (s.y..s.bottom()).contains(&y),
            Axis::Horizontal => y == self.at && (s.x..s.right()).contains(&x),
        }
    }

    /// The split ratio that puts the divider at this cell.
    pub fn ratio_at(&self, x: u16, y: u16) -> f32 {
        let s = self.split;
        let r = match self.axis {
            Axis::Vertical => (x.saturating_sub(s.x)) as f32 / s.width.saturating_sub(1).max(1) as f32,
            Axis::Horizontal => (y.saturating_sub(s.y)) as f32 / s.height.max(1) as f32,
        };
        r.clamp(0.05, 0.95)
    }
}

#[derive(Debug, Default)]
pub struct Arranged {
    pub panes: Vec<Placed>,
    pub dividers: Vec<Divider>,
}

pub fn arrange(layout: &Layout, area: Rect) -> Arranged {
    let mut out = Arranged::default();
    place(layout, area, &mut Vec::new(), &mut out);
    out
}

fn place(layout: &Layout, area: Rect, path: &mut Vec<bool>, out: &mut Arranged) {
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
            out.dividers.push(Divider { path: path.clone(), axis: *axis, split: area, at });
            path.push(false);
            place(a, ra, path, out);
            path.pop();
            path.push(true);
            place(b, rb, path, out);
            path.pop();
        }
    }
}

/// `ratio` of `total`, leaving each side at least one cell when there are two.
fn share(total: u16, ratio: f32) -> u16 {
    let n = (total as f32 * ratio).round() as u16;
    if total >= 2 { n.clamp(1, total - 1) } else { n.min(total) }
}

/// Sets the ratio of the split at `path`.
pub fn set_ratio(layout: &mut Layout, path: &[bool], new: f32) {
    match (layout, path.split_first()) {
        (Layout::Split { ratio, .. }, None) => *ratio = new,
        (Layout::Split { a, b, .. }, Some((side, rest))) => set_ratio(if *side { b } else { a }, rest, new),
        (Layout::Pane(_), _) => {}
    }
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
        assert_eq!(out.dividers[1].path, vec![true]);
        assert!(out.dividers[1].hit(50, 10));
        assert_eq!(out.panes[2].content(), Rect::new(42, 11, 39, 9));
    }

    #[test]
    fn dragging_sets_the_ratio() {
        let mut l = Layout::Pane(p(1));
        l.split(p(1), Axis::Vertical, p(2));
        l.split(p(2), Axis::Horizontal, p(3));
        let out = arrange(&l, Rect::new(0, 0, 81, 20));
        let d = &out.dividers[1];
        set_ratio(&mut l, &d.path, d.ratio_at(50, 5));
        let out = arrange(&l, Rect::new(0, 0, 81, 20));
        assert_eq!(out.panes[2].area.y, 5);
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
