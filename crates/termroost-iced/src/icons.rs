//! The interface's icons: line drawings on a 16 px grid, drawn for Terminal
//! Rooster (see `icons/LICENSES.md`). They take the color they're given.

use std::sync::LazyLock;

use iced::widget::{Svg, svg};
use iced::{Color, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Plus,
    Close,
    Edit,
    Trash,
    Settings,
    SplitRight,
    SplitDown,
    Refresh,
    Open,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    ArrowDown,
    ArrowUp,
    Search,
    Copy,
}

/// The files, in the order of [`Icon`].
const SOURCES: [&[u8]; 16] = [
    include_bytes!("../icons/ui/plus.svg"),
    include_bytes!("../icons/ui/close.svg"),
    include_bytes!("../icons/ui/edit.svg"),
    include_bytes!("../icons/ui/trash.svg"),
    include_bytes!("../icons/ui/settings.svg"),
    include_bytes!("../icons/ui/split_right.svg"),
    include_bytes!("../icons/ui/split_down.svg"),
    include_bytes!("../icons/ui/refresh.svg"),
    include_bytes!("../icons/ui/open.svg"),
    include_bytes!("../icons/ui/chevron_down.svg"),
    include_bytes!("../icons/ui/chevron_right.svg"),
    include_bytes!("../icons/ui/chevron_left.svg"),
    include_bytes!("../icons/ui/arrow_down.svg"),
    include_bytes!("../icons/ui/arrow_up.svg"),
    include_bytes!("../icons/ui/search.svg"),
    include_bytes!("../icons/ui/copy.svg"),
];

// Built once so every view hands iced the same handles, and their rasterized
// copies are reused instead of drawn again.
static HANDLES: LazyLock<Vec<svg::Handle>> =
    LazyLock::new(|| SOURCES.iter().map(|bytes| svg::Handle::from_memory(*bytes)).collect());

/// `icon`, `size` pixels square, in the color `ink` gives.
pub fn icon<'a>(icon: Icon, size: f32, ink: impl Fn(&Theme) -> Color + 'a) -> Svg<'a> {
    svg(HANDLES[icon as usize].clone()).width(size).height(size).style(move |t, _| svg::Style { color: Some(ink(t)) })
}
