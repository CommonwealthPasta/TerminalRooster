//! Fonts, colors and widget styles.

use std::sync::LazyLock;

use iced::widget::{button, container, pane_grid};
use iced::{Background, Border, Color, Font, Theme};

/// Bundled terminal font (JetBrains Mono, SIL Open Font License 1.1).
pub const MONO: Font = Font::with_name("JetBrains Mono");

/// Bundled symbol font for glyphs JetBrains Mono lacks, such as the `⏺` and
/// `⎿` that coding agents print (Noto Sans Symbols 2, SIL OFL 1.1).
pub const SYMBOLS: Font = Font::with_name("Noto Sans Symbols 2");

const MONO_REGULAR: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
const SYMBOLS_REGULAR: &[u8] = include_bytes!("../fonts/NotoSansSymbols2-Regular.ttf");

pub const FONTS: [&[u8]; 5] = [
    MONO_REGULAR,
    include_bytes!("../fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../fonts/JetBrainsMono-BoldItalic.ttf"),
    SYMBOLS_REGULAR,
];

static MONO_FACE: LazyLock<Option<ttf_parser::Face<'static>>> =
    LazyLock::new(|| ttf_parser::Face::parse(MONO_REGULAR, 0).ok());
static SYMBOL_FACE: LazyLock<Option<ttf_parser::Face<'static>>> =
    LazyLock::new(|| ttf_parser::Face::parse(SYMBOLS_REGULAR, 0).ok());

fn has_glyph(face: &LazyLock<Option<ttf_parser::Face<'static>>>, c: char) -> bool {
    face.as_ref().is_some_and(|f| f.glyph_index(c).is_some())
}

/// Font to draw a single terminal character with: the terminal font when it
/// has the glyph, else the bundled symbol font when that has it, else the
/// terminal font (and the system fallback chain, e.g. for CJK and emoji).
pub fn glyph_font(c: char, styled: Font) -> Font {
    if has_glyph(&MONO_FACE, c) || !has_glyph(&SYMBOL_FACE, c) { styled } else { SYMBOLS }
}

pub const DEFAULT_FONT_SIZE: f32 = 14.0;

pub fn accent(theme: &Theme) -> Color {
    theme.extended_palette().primary.base.color
}

/// Secondary text: the text color at reduced opacity, readable on light and dark backgrounds.
pub fn muted(theme: &Theme) -> Color {
    theme.extended_palette().background.base.text.scale_alpha(0.62)
}

/// Background of the side columns.
pub fn sidebar(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.background.weak.color)),
        text_color: Some(p.background.weak.text),
        ..container::Style::default()
    }
}

pub fn sidebar_deep(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.background.weakest.color)),
        text_color: Some(p.background.weakest.text),
        ..container::Style::default()
    }
}

/// Small borderless icon buttons.
pub fn icon_button(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.extended_palette();
    button::Style {
        background: match status {
            button::Status::Hovered | button::Status::Pressed => Some(Background::Color(p.background.strong.color)),
            _ => None,
        },
        text_color: match status {
            button::Status::Disabled => p.background.strong.color,
            _ => p.background.base.text,
        },
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}

/// The "+" at the end of the project and tab lists: quiet until hovered.
pub fn add_item_button(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.extended_palette();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(p.background.strong.color)),
        text_color: if hovered { p.background.base.text } else { muted(theme) },
        border: Border::default().rounded(4),
        ..button::Style::default()
    }
}

pub fn badge(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.secondary.base.color)),
        text_color: Some(p.secondary.base.text),
        border: Border::default().rounded(8),
        ..container::Style::default()
    }
}

pub fn title_bar(active: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let p = theme.extended_palette();
        container::Style {
            background: Some(Background::Color(if active {
                p.background.strong.color
            } else {
                p.background.weak.color
            })),
            text_color: Some(if active { p.background.base.text } else { muted(theme) }),
            ..container::Style::default()
        }
    }
}

pub fn pane(active: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let p = theme.extended_palette();
        container::Style {
            border: Border {
                width: 1.0,
                color: if active { p.primary.base.color } else { p.background.strong.color },
                radius: 0.0.into(),
            },
            ..container::Style::default()
        }
    }
}

pub fn pane_grid_style(theme: &Theme) -> pane_grid::Style {
    let mut style = pane_grid::default(theme);
    style.hovered_split.color = accent(theme);
    style.picked_split.color = accent(theme);
    style
}

pub fn modal_backdrop(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: 0.6, ..Color::BLACK })),
        ..container::Style::default()
    }
}

pub fn modal_card(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.background.weak.color)),
        text_color: Some(p.background.weak.text),
        border: Border { width: 1.0, color: p.background.strong.color, radius: 8.0.into() },
        ..container::Style::default()
    }
}

pub fn error_banner(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.danger.weak.color)),
        text_color: Some(p.danger.weak.text),
        border: Border::default().rounded(4),
        ..container::Style::default()
    }
}

/// The terminals of the shown session while a task is held over them.
pub fn drop_target(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.primary.base.color.scale_alpha(0.18))),
        text_color: Some(p.background.base.text),
        border: Border { width: 2.0, color: p.primary.base.color, radius: 6.0.into() },
        ..container::Style::default()
    }
}

/// The label that follows the pointer while a task is dragged.
pub fn drag_label(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(p.background.base.color)),
        text_color: Some(p.background.base.text),
        border: Border { width: 1.0, color: p.primary.base.color, radius: 6.0.into() },
        shadow: iced::Shadow {
            color: Color { a: 0.35, ..Color::BLACK },
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
        ..container::Style::default()
    }
}
