//! GUI-neutral snapshot of the visible part of a terminal.
//!
//! Colors are fully resolved to RGB (palette, OSC overrides, inverse and dim
//! applied), so a front end only has to paint cells.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Term, TermMode, point_to_viewport};
use alacritty_terminal::vte::ansi::{Color, CursorShape as ACursorShape, NamedColor, Rgb as ARgb};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    const fn hex(v: u32) -> Self {
        Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    fn blend(self, other: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 * (1.0 - t) + b as f32 * t).round() as u8;
        Rgb::new(m(self.r, other.r), m(self.g, other.g), m(self.b, other.b))
    }
}

impl From<ARgb> for Rgb {
    fn from(c: ARgb) -> Self {
        Rgb::new(c.r, c.g, c.b)
    }
}

/// Cell attributes relevant for drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderCell {
    pub ch: char,
    /// Combining characters drawn on top of `ch`.
    pub zerowidth: Option<Box<[char]>>,
    pub fg: Rgb,
    pub bg: Rgb,
    pub style: CellStyle,
    /// First half of a double-width character.
    pub wide: bool,
    /// Second half of a double-width character (draw nothing).
    pub spacer: bool,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorInfo {
    pub col: usize,
    pub row: usize,
    pub shape: CursorShape,
}

/// Visible terminal content, row-major.
#[derive(Clone, Debug, Default)]
pub struct RenderContent {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<RenderCell>,
    pub cursor: Option<CursorInfo>,
    pub cursor_color: Rgb,
    pub default_fg: Rgb,
    pub default_bg: Rgb,
    pub selection_bg: Rgb,
    /// Lines scrolled back from the bottom (0 = following output).
    pub display_offset: usize,
    pub history_size: usize,
    pub alt_screen: bool,
    pub mouse_mode: bool,
    pub app_cursor: bool,
}

impl RenderContent {
    pub fn cell(&self, col: usize, row: usize) -> Option<&RenderCell> {
        (col < self.cols && row < self.rows).then(|| &self.cells[row * self.cols + col])
    }

    pub fn row(&self, row: usize) -> &[RenderCell] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    /// Text of the visible screen (rows joined with `\n`), mainly for tests.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for r in 0..self.rows {
            let line: String = self.row(r).iter().filter(|c| !c.spacer).map(|c| c.ch).collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    pub(crate) fn rebuild<T: EventListener>(&mut self, term: &Term<T>, palette: &Palette) {
        let content = term.renderable_content();
        let cols = term.columns();
        let rows = term.screen_lines();
        let overrides = content.colors;
        let default_fg = palette.resolve(Color::Named(NamedColor::Foreground), overrides);
        let default_bg = palette.resolve(Color::Named(NamedColor::Background), overrides);

        self.cols = cols;
        self.rows = rows;
        self.default_fg = default_fg;
        self.default_bg = default_bg;
        self.selection_bg = palette.selection_bg;
        self.cursor_color = palette.resolve(Color::Named(NamedColor::Cursor), overrides);
        self.display_offset = content.display_offset;
        self.history_size = term.grid().history_size();
        self.alt_screen = content.mode.contains(TermMode::ALT_SCREEN);
        self.mouse_mode = content.mode.intersects(TermMode::MOUSE_MODE);
        self.app_cursor = content.mode.contains(TermMode::APP_CURSOR);

        let blank = RenderCell {
            ch: ' ',
            zerowidth: None,
            fg: default_fg,
            bg: default_bg,
            style: CellStyle::default(),
            wide: false,
            spacer: false,
            selected: false,
        };
        self.cells.clear();
        self.cells.resize(cols * rows, blank);

        let offset = content.display_offset as i32;
        let selection = content.selection;
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            let col = indexed.point.column.0;
            if row < 0 || row as usize >= rows || col >= cols {
                continue;
            }
            let cell = indexed.cell;
            let flags = cell.flags;
            let mut fg = palette.resolve(cell.fg, overrides);
            let mut bg = palette.resolve(cell.bg, overrides);
            if flags.contains(Flags::DIM) {
                fg = fg.blend(bg, 0.34);
            }
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let out = &mut self.cells[row as usize * cols + col];
            out.ch = if cell.c == '\t' { ' ' } else { cell.c };
            out.zerowidth = cell.zerowidth().filter(|z| !z.is_empty()).map(Into::into);
            out.fg = fg;
            out.bg = bg;
            out.style = CellStyle {
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                strikeout: flags.contains(Flags::STRIKEOUT),
            };
            out.wide = flags.contains(Flags::WIDE_CHAR);
            out.spacer = flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
            out.selected = selection.is_some_and(|s| s.contains(indexed.point));
        }

        self.cursor = match content.cursor.shape {
            ACursorShape::Hidden => None,
            shape => point_to_viewport(content.display_offset, content.cursor.point)
                .filter(|p| p.line < rows && p.column.0 < cols)
                .map(|p| CursorInfo {
                    col: p.column.0,
                    row: p.line,
                    shape: match shape {
                        ACursorShape::Underline => CursorShape::Underline,
                        ACursorShape::Beam => CursorShape::Beam,
                        ACursorShape::HollowBlock => CursorShape::HollowBlock,
                        _ => CursorShape::Block,
                    },
                }),
        };
    }
}

/// The palette shared by all terminals; replaced at runtime by [`crate::Core::set_palette`].
pub(crate) type SharedPalette = std::sync::Arc<std::sync::RwLock<Palette>>;

/// The 269-entry color table used to resolve terminal colors.
#[derive(Clone, Debug)]
pub struct Palette {
    colors: Vec<Rgb>,
    pub selection_bg: Rgb,
}

impl Default for Palette {
    /// A Tokyo Night inspired dark scheme.
    fn default() -> Self {
        let ansi: [u32; 16] = [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, //
            0x414868, 0xff899d, 0xb9f27c, 0xff9e64, 0x8db0ff, 0xc7a9ff, 0xa4daff, 0xc0caf5,
        ];
        let fg = Rgb::hex(0xc0caf5);
        let bg = Rgb::hex(0x1a1b26);
        Palette::new(ansi.map(Rgb::hex), fg, bg, fg, Rgb::hex(0x33467c))
    }
}

impl Palette {
    pub fn new(ansi: [Rgb; 16], fg: Rgb, bg: Rgb, cursor: Rgb, selection_bg: Rgb) -> Self {
        let mut colors = vec![Rgb::default(); alacritty_terminal::term::color::COUNT];
        colors[..16].copy_from_slice(&ansi);
        // 6x6x6 color cube.
        let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
        for r in 0..6 {
            for g in 0..6 {
                for b in 0..6 {
                    colors[16 + r * 36 + g * 6 + b] = Rgb::new(level(r), level(g), level(b));
                }
            }
        }
        // Grayscale ramp.
        for i in 0..24 {
            let v = (8 + i * 10) as u8;
            colors[232 + i] = Rgb::new(v, v, v);
        }
        colors[NamedColor::Foreground as usize] = fg;
        colors[NamedColor::Background as usize] = bg;
        colors[NamedColor::Cursor as usize] = cursor;
        for i in 0..8 {
            colors[NamedColor::DimBlack as usize + i] = ansi[i].blend(bg, 0.34);
        }
        colors[NamedColor::BrightForeground as usize] = ansi[15];
        colors[NamedColor::DimForeground as usize] = fg.blend(bg, 0.34);
        Self { colors, selection_bg }
    }

    pub fn get(&self, index: usize) -> Rgb {
        self.colors.get(index).copied().unwrap_or_default()
    }

    pub(crate) fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        let index = match color {
            Color::Spec(rgb) => return rgb.into(),
            Color::Indexed(i) => i as usize,
            Color::Named(n) => n as usize,
        };
        overrides[index].map(Rgb::from).unwrap_or_else(|| self.get(index))
    }
}
