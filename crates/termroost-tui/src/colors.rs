//! Colors: the interface's few named colors, and how terminal cells map onto
//! the colors of the terminal this app runs in.
//!
//! The core resolves every cell to RGB through a [`Palette`]. Ours holds
//! marker values: its default foreground and background, and its 16 ANSI
//! colors, are mapped back here to "default" and to indexed colors. So the
//! programs inside show in the user's own terminal theme, light or dark,
//! instead of in a theme chosen for them. Colors that programs give as RGB
//! stay RGB.

use ratatui::style::{Color, Modifier, Style};
use termroost_core::{Palette, Rgb};

/// Tokyo Night, so that colors blended from these (dimmed text) look right.
const ANSI: [u32; 16] = [
    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, //
    0x414868, 0xff899d, 0xb9f27c, 0xff9e64, 0x8db0ff, 0xc7a9ff, 0xa4daff, 0xc0caf5,
];
/// One off ANSI 15, so the two stay apart.
const FG: Rgb = hex(0xc0caf4);
const BG: Rgb = hex(0x1a1b26);

const fn hex(v: u32) -> Rgb {
    Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// The palette handed to the core.
pub fn palette() -> Palette {
    Palette::new(ANSI.map(hex), FG, BG, FG, hex(0x33467c))
}

/// Interface colors. Named ANSI colors and modifiers only, so the interface
/// follows the terminal's theme too.
pub mod ui {
    use super::*;

    pub const ACCENT: Color = Color::Cyan;
    pub fn accent() -> Style {
        Style::new().fg(ACCENT)
    }
    pub fn strong() -> Style {
        Style::new().add_modifier(Modifier::BOLD)
    }
    pub fn dim() -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }
    pub fn good() -> Style {
        Style::new().fg(Color::Green)
    }
    pub fn warn() -> Style {
        Style::new().fg(Color::Yellow)
    }
    pub fn bad() -> Style {
        Style::new().fg(Color::Red)
    }
}

/// How to draw a cell with these resolved colors.
pub fn cell_style(fg: Rgb, bg: Rgb) -> Style {
    // A default color in the other role (inverse video, say) has no name of
    // its own; reversing the cell expresses it.
    let (fg_slot, bg_slot, reversed) =
        if fg == BG || bg == FG { (color(bg, FG), color(fg, BG), true) } else { (color(fg, FG), color(bg, BG), false) };
    let style = Style::new().fg(fg_slot).bg(bg_slot);
    if reversed { style.add_modifier(Modifier::REVERSED) } else { style }
}

/// `c` as a terminal color; `default` is the slot's own default color.
fn color(c: Rgb, default: Rgb) -> Color {
    if c == default {
        return Color::Reset;
    }
    if let Some(i) = ANSI.iter().position(|a| hex(*a) == c) {
        return Color::Indexed(i as u8);
    }
    indexed(c).map(Color::Indexed).unwrap_or(Color::Rgb(c.r, c.g, c.b))
}

/// The xterm 256-color index of `c`, if it is one of the color cube's or the
/// gray ramp's (as [`Palette::new`] computes them).
fn indexed(c: Rgb) -> Option<u8> {
    let level = |v: u8| match v {
        0 => Some(0),
        v if v >= 95 && (v - 55).is_multiple_of(40) => Some((v - 55) / 40),
        _ => None,
    };
    if let (Some(r), Some(g), Some(b)) = (level(c.r), level(c.g), level(c.b)) {
        return Some(16 + r * 36 + g * 6 + b);
    }
    let gray = c.r == c.g && c.g == c.b && c.r >= 8 && (c.r - 8).is_multiple_of(10) && c.r <= 238;
    gray.then(|| 232 + (c.r - 8) / 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(i: usize) -> Rgb {
        palette().get(i)
    }

    #[test]
    fn defaults_follow_the_outer_terminal() {
        assert_eq!(cell_style(FG, BG), Style::new().fg(Color::Reset).bg(Color::Reset));
    }

    #[test]
    fn inverse_video_reverses_the_cell() {
        let s = cell_style(BG, FG);
        assert_eq!((s.fg, s.bg), (Some(Color::Reset), Some(Color::Reset)));
        assert!(s.add_modifier.contains(Modifier::REVERSED));
        // Red text in inverse video: a red background behind default-background text.
        let red = rgb(1);
        let s = cell_style(BG, red);
        assert_eq!((s.fg, s.bg), (Some(Color::Indexed(1)), Some(Color::Reset)));
        assert!(s.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn palette_colors_map_back_to_indices() {
        for i in 0..256 {
            assert_eq!(color(rgb(i), FG), Color::Indexed(i as u8), "index {i}");
        }
        assert_eq!(color(Rgb::new(1, 2, 3), FG), Color::Rgb(1, 2, 3));
    }
}
