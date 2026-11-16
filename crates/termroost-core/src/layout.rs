//! Binary split tree describing how the panes of a tab are arranged.

use serde::{Deserialize, Serialize};

use crate::model::PaneId;

/// Orientation of the divider of a split (same meaning as iced's `pane_grid::Axis`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// The divider is horizontal: `a` is above `b`.
    Horizontal,
    /// The divider is vertical: `a` is left of `b`.
    Vertical,
}

/// A layout tree whose leaves are panes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Pane(PaneId),
    Split {
        axis: Axis,
        /// Fraction of the space given to `a`, in `0.0..=1.0`.
        ratio: f32,
        a: Box<Layout>,
        b: Box<Layout>,
    },
}

impl Layout {
    /// All panes in the tree, left/top first.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<PaneId>) {
        match self {
            Layout::Pane(p) => out.push(*p),
            Layout::Split { a, b, .. } => {
                a.collect(out);
                b.collect(out);
            }
        }
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            Layout::Pane(p) => *p == pane,
            Layout::Split { a, b, .. } => a.contains(pane) || b.contains(pane),
        }
    }

    pub fn first_pane(&self) -> PaneId {
        match self {
            Layout::Pane(p) => *p,
            Layout::Split { a, .. } => a.first_pane(),
        }
    }

    /// Splits `target` in two, placing `new` after it (right or below).
    /// Returns `false` if `target` is not in the tree.
    pub fn split(&mut self, target: PaneId, axis: Axis, new: PaneId) -> bool {
        match self {
            Layout::Pane(p) if *p == target => {
                *self = Layout::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(Layout::Pane(target)),
                    b: Box::new(Layout::Pane(new)),
                };
                true
            }
            Layout::Pane(_) => false,
            Layout::Split { a, b, .. } => a.split(target, axis, new) || b.split(target, axis, new),
        }
    }

    /// Removes `target`, replacing its parent split with the sibling.
    /// Returns `false` if `target` is absent or is the only pane.
    pub fn remove(&mut self, target: PaneId) -> bool {
        match self {
            Layout::Pane(_) => false,
            Layout::Split { a, b, .. } => {
                if matches!(**a, Layout::Pane(p) if p == target) {
                    *self = std::mem::replace(&mut **b, Layout::Pane(target));
                    true
                } else if matches!(**b, Layout::Pane(p) if p == target) {
                    *self = std::mem::replace(&mut **a, Layout::Pane(target));
                    true
                } else {
                    a.remove(target) || b.remove(target)
                }
            }
        }
    }

    /// Builds a layout from a list of panes (used to repair broken trees).
    pub fn chain(panes: &[PaneId]) -> Option<Layout> {
        let (first, rest) = panes.split_first()?;
        let mut layout = Layout::Pane(*first);
        let mut last = *first;
        for p in rest {
            layout.split(last, Axis::Vertical, *p);
            last = *p;
        }
        Some(layout)
    }

    /// Clamps ratios into a sane range.
    pub fn sanitize(&mut self) {
        if let Layout::Split { ratio, a, b, .. } = self {
            if !ratio.is_finite() {
                *ratio = 0.5;
            }
            *ratio = ratio.clamp(0.05, 0.95);
            a.sanitize();
            b.sanitize();
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("layout serializes")
    }

    pub fn from_json(s: &str) -> Option<Layout> {
        serde_json::from_str(s).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(i: i64) -> PaneId {
        PaneId(i)
    }

    #[test]
    fn split_and_remove() {
        let mut l = Layout::Pane(p(1));
        assert!(l.split(p(1), Axis::Vertical, p(2)));
        assert!(l.split(p(2), Axis::Horizontal, p(3)));
        assert_eq!(l.panes(), vec![p(1), p(2), p(3)]);
        assert!(!l.split(p(9), Axis::Vertical, p(4)));

        assert!(l.remove(p(2)));
        assert_eq!(l.panes(), vec![p(1), p(3)]);
        match &l {
            Layout::Split { axis, b, .. } => {
                assert_eq!(*axis, Axis::Vertical);
                assert_eq!(**b, Layout::Pane(p(3)));
            }
            _ => panic!("expected split"),
        }
        assert!(l.remove(p(1)));
        assert_eq!(l, Layout::Pane(p(3)));
        assert!(!l.remove(p(3)), "last pane cannot be removed");
    }

    #[test]
    fn json_round_trip() {
        let mut l = Layout::Pane(p(1));
        l.split(p(1), Axis::Horizontal, p(2));
        if let Layout::Split { ratio, .. } = &mut l {
            *ratio = 0.3;
        }
        let json = l.to_json();
        assert_eq!(Layout::from_json(&json), Some(l));
        assert_eq!(Layout::from_json("garbage"), None);
    }

    #[test]
    fn chain_and_sanitize() {
        let mut l = Layout::chain(&[p(1), p(2), p(3)]).unwrap();
        assert_eq!(l.panes(), vec![p(1), p(2), p(3)]);
        if let Layout::Split { ratio, .. } = &mut l {
            *ratio = f32::NAN;
        }
        l.sanitize();
        if let Layout::Split { ratio, .. } = &l {
            assert_eq!(*ratio, 0.5);
        }
        assert!(Layout::chain(&[]).is_none());
    }
}
