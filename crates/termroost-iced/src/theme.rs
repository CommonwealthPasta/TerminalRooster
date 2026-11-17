//! Fonts, colors and widget styles.
//!
//! Every part of the window follows the same few rules, so the parts look
//! like they belong together:
//! - All colors come from the theme's six, so a custom theme gets the same
//!   look. Lines, fills under the pointer and quiet surfaces are the text
//!   color at a low opacity over what is behind them.
//! - The accent marks what is selected, focused or switched on. Filled
//!   buttons are drawn in the text color, so they don't compete with it.
//! - Corners are [`RADIUS_SM`] on small controls, and [`RADIUS`] on
//!   buttons, fields and list items.
//! - What is raised (cards, menus, the selected list item) is lighter than
//!   what it sits on, and casts a soft shadow.

use std::sync::LazyLock;

use iced::border;
use iced::overlay::menu;
use iced::widget::{button, checkbox, container, pane_grid, pick_list, scrollable, text_editor, text_input};
use iced::{Background, Border, Color, Font, Shadow, Theme, Vector, font};

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

/// The interface font. Named on Windows, so its bold and semibold weights are found too.
pub const UI: Font = if cfg!(windows) { Font::with_name("Segoe UI") } else { Font::DEFAULT };
/// Titles, headings and names that lead a line.
pub const SEMIBOLD: Font = Font { weight: font::Weight::Semibold, ..UI };
pub const BOLD: Font = Font { weight: font::Weight::Bold, ..UI };

/// Text sizes: a page's title, body text and small print.
pub const TEXT_PAGE: f32 = 22.0;
pub const TEXT: f32 = 13.0;
pub const TEXT_SM: f32 = 12.0;

/// Corners of small controls such as icon buttons.
pub const RADIUS_SM: f32 = 6.0;
/// Corners of buttons, fields, menus and list items.
pub const RADIUS: f32 = 8.0;
/// Corners of cards, dialogs and pop-ups.
pub const RADIUS_LG: f32 = 12.0;
/// Fully round ends.
const PILL: f32 = 999.0;

/// `a` moved toward `b` by `t` (0 is `a`, 1 is `b`), as `b` laid over `a` at that opacity.
fn mix(a: Color, b: Color, t: f32) -> Color {
    Color { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}

fn is_dark(t: &Theme) -> bool {
    t.extended_palette().is_dark
}

/// One amount for light themes, another for dark ones, where the same
/// opacity shows less.
fn by_tone(t: &Theme, light: f32, dark: f32) -> f32 {
    if is_dark(t) { dark } else { light }
}

fn background(t: &Theme) -> Color {
    t.extended_palette().background.base.color
}

/// The text color.
pub fn ink(t: &Theme) -> Color {
    t.extended_palette().background.base.text
}

pub fn accent(t: &Theme) -> Color {
    t.extended_palette().primary.base.color
}

/// Secondary text: the text color at reduced opacity, readable on light and dark backgrounds.
pub fn muted(t: &Theme) -> Color {
    ink(t).scale_alpha(0.62)
}

/// Placeholders and what can't be used right now.
pub fn faint(t: &Theme) -> Color {
    ink(t).scale_alpha(0.42)
}

/// Hairlines: dividers, and the edges of cards and panes.
pub fn line(t: &Theme) -> Color {
    ink(t).scale_alpha(by_tone(t, 0.10, 0.12))
}

/// The edge of a field or an outlined button.
fn edge(t: &Theme) -> Color {
    ink(t).scale_alpha(by_tone(t, 0.22, 0.18))
}

/// The fill of something under the pointer.
fn hover_fill(t: &Theme) -> Color {
    ink(t).scale_alpha(by_tone(t, 0.06, 0.08))
}

/// The fill of something being pressed.
fn press_fill(t: &Theme) -> Color {
    ink(t).scale_alpha(by_tone(t, 0.10, 0.13))
}

/// Cards, menus and dialogs.
pub fn raised(t: &Theme) -> Color {
    if is_dark(t) { mix(background(t), ink(t), 0.07) } else { mix(background(t), Color::WHITE, 0.85) }
}

/// The chosen one of several: a selected tab, or the selected item of a list.
fn chosen(t: &Theme) -> Color {
    if is_dark(t) { mix(background(t), ink(t), 0.14) } else { Color::WHITE }
}

/// The side columns, a step further from the text color than the terminals.
fn side(t: &Theme) -> Color {
    if is_dark(t) { mix(background(t), Color::BLACK, 0.2) } else { mix(background(t), ink(t), 0.03) }
}

/// What text is typed on: set into dark themes, white in light ones.
fn field(t: &Theme) -> Color {
    if is_dark(t) { mix(background(t), Color::BLACK, 0.12) } else { Color::WHITE }
}

/// The top of a terminal: its title and buttons.
fn title_fill(t: &Theme) -> Color {
    mix(background(t), ink(t), by_tone(t, 0.04, 0.05))
}

fn shadow(t: &Theme, light: f32, dark: f32, y: f32, blur: f32) -> Shadow {
    Shadow {
        color: Color { a: by_tone(t, light, dark), ..Color::BLACK },
        offset: Vector::new(0.0, y),
        blur_radius: blur,
    }
}

/// Under buttons and the selected item.
fn shadow_sm(t: &Theme) -> Shadow {
    shadow(t, 0.07, 0.3, 1.0, 2.0)
}

/// Under menus, tooltips and pop-ups.
fn shadow_md(t: &Theme) -> Shadow {
    shadow(t, 0.12, 0.45, 4.0, 14.0)
}

/// The kinds of message a note or badge can carry, each in its own color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

fn tone_color(t: &Theme, tone: Tone) -> Color {
    let p = t.extended_palette();
    match tone {
        Tone::Neutral => ink(t),
        Tone::Accent => p.primary.base.color,
        Tone::Success => p.success.base.color,
        Tone::Warning => p.warning.base.color,
        Tone::Danger => p.danger.base.color,
    }
}

/// Text in a tone's color, nudged toward the text color so it reads on a tint of itself.
fn tone_text(t: &Theme, tone: Tone) -> Color {
    let c = tone_color(t, tone);
    if tone == Tone::Neutral { c } else { mix(c, ink(t), by_tone(t, 0.25, 0.15)) }
}

// Buttons

fn button_style(background: Option<Color>, text_color: Color, radius: f32) -> button::Style {
    button::Style {
        background: background.map(Background::Color),
        text_color,
        border: border::rounded(radius),
        ..button::Style::default()
    }
}

/// A filled button, darker (or lighter) under the pointer. Disabled, it fades into the background.
fn filled(t: &Theme, status: button::Status, fill: Color, on: Color, toward: Color) -> button::Style {
    let (background, text_color) = match status {
        button::Status::Active => (fill, on),
        button::Status::Hovered => (mix(fill, toward, 0.14), on),
        button::Status::Pressed => (mix(fill, toward, 0.24), on),
        button::Status::Disabled => (press_fill(t), faint(t)),
    };
    let shadow = if status == button::Status::Active || status == button::Status::Hovered {
        shadow_sm(t)
    } else {
        Shadow::default()
    };
    button::Style { shadow, ..button_style(Some(background), text_color, RADIUS) }
}

/// The main action of a dialog or form, filled with the text color.
pub fn button_primary(t: &Theme, status: button::Status) -> button::Style {
    filled(t, status, ink(t), background(t), background(t))
}

/// Any other action: outlined, on a raised fill.
pub fn button_secondary(t: &Theme, status: button::Status) -> button::Style {
    let fill = raised(t);
    let (background, text_color, edge_color, shadow) = match status {
        button::Status::Active => (fill, ink(t), edge(t), shadow_sm(t)),
        button::Status::Hovered => (mix(fill, ink(t), 0.05), ink(t), edge(t), shadow_sm(t)),
        button::Status::Pressed => (mix(fill, ink(t), 0.09), ink(t), edge(t), Shadow::default()),
        button::Status::Disabled => (fill, faint(t), line(t), Shadow::default()),
    };
    button::Style {
        background: Some(background.into()),
        text_color,
        border: Border { width: 1.0, color: edge_color, radius: RADIUS.into() },
        shadow,
        snap: true,
    }
}

/// Icon buttons, and other buttons that show only a fill under the pointer.
pub fn button_ghost(t: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Active => button_style(None, ink(t).scale_alpha(0.78), RADIUS_SM),
        button::Status::Hovered => button_style(Some(hover_fill(t)), ink(t), RADIUS_SM),
        button::Status::Pressed => button_style(Some(press_fill(t)), ink(t), RADIUS_SM),
        button::Status::Disabled => button_style(None, faint(t), RADIUS_SM),
    }
}

/// A toolbar button: a light fill of the text color, so a row of them stays quiet.
pub fn button_tonal(t: &Theme, status: button::Status) -> button::Style {
    let fill = |light, dark| Some(ink(t).scale_alpha(by_tone(t, light, dark)));
    match status {
        button::Status::Active => button_style(fill(0.06, 0.08), ink(t), RADIUS),
        button::Status::Hovered => button_style(fill(0.10, 0.13), ink(t), RADIUS),
        button::Status::Pressed => button_style(fill(0.14, 0.17), ink(t), RADIUS),
        button::Status::Disabled => button_style(fill(0.04, 0.05), faint(t), RADIUS),
    }
}

/// One of the choices of a segmented control, or a tab: the chosen one is
/// raised out of the [`segment_tray`].
pub fn segment(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |t, status| {
        if selected {
            return button::Style { shadow: shadow_sm(t), ..button_style(Some(chosen(t)), ink(t), RADIUS_SM) };
        }
        match status {
            button::Status::Active => button_style(None, muted(t), RADIUS_SM),
            button::Status::Hovered => button_style(Some(hover_fill(t)), ink(t), RADIUS_SM),
            button::Status::Pressed => button_style(Some(press_fill(t)), ink(t), RADIUS_SM),
            button::Status::Disabled => button_style(None, faint(t), RADIUS_SM),
        }
    }
}

/// The tray that holds a segmented control's choices together.
pub fn segment_tray(t: &Theme) -> container::Style {
    container::Style {
        background: Some(ink(t).scale_alpha(by_tone(t, 0.06, 0.08)).into()),
        border: border::rounded(RADIUS),
        ..container::Style::default()
    }
}

/// A line of a list or menu that takes clicks, such as a file in the Files tab.
pub fn row_button(t: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Active | button::Status::Disabled => button_style(None, ink(t), RADIUS_SM),
        button::Status::Hovered => button_style(Some(hover_fill(t)), ink(t), RADIUS_SM),
        button::Status::Pressed => button_style(Some(press_fill(t)), ink(t), RADIUS_SM),
    }
}

/// "New project" and "New session" under their lists: quiet until hovered.
pub fn add_item_button(t: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Active | button::Status::Disabled => button_style(None, muted(t), RADIUS),
        button::Status::Hovered => button_style(Some(hover_fill(t)), ink(t), RADIUS),
        button::Status::Pressed => button_style(Some(press_fill(t)), ink(t), RADIUS),
    }
}

// Fields and other controls

pub fn input(t: &Theme, status: text_input::Status) -> text_input::Style {
    let edge_color = match status {
        text_input::Status::Active => edge(t),
        text_input::Status::Hovered => ink(t).scale_alpha(0.32),
        text_input::Status::Focused { .. } => accent(t),
        text_input::Status::Disabled => line(t),
    };
    text_input::Style {
        background: field(t).into(),
        border: Border { width: 1.0, color: edge_color, radius: RADIUS.into() },
        icon: muted(t),
        placeholder: faint(t),
        value: ink(t),
        selection: accent(t).scale_alpha(0.35),
    }
}

/// A dropdown's button, framed like a field.
pub fn dropdown(t: &Theme, status: pick_list::Status) -> pick_list::Style {
    let edge_color = match status {
        pick_list::Status::Active => edge(t),
        pick_list::Status::Hovered => ink(t).scale_alpha(0.32),
        pick_list::Status::Opened { .. } => accent(t),
    };
    pick_list::Style {
        text_color: ink(t),
        placeholder_color: muted(t),
        handle_color: muted(t),
        background: field(t).into(),
        border: Border { width: 1.0, color: edge_color, radius: RADIUS.into() },
    }
}

/// A dropdown's list of choices.
pub fn dropdown_menu(t: &Theme) -> menu::Style {
    menu::Style {
        background: raised(t).into(),
        border: Border { width: 1.0, color: line(t), radius: RADIUS.into() },
        text_color: ink(t),
        selected_text_color: ink(t),
        selected_background: hover_fill(t).into(),
        shadow: shadow_md(t),
    }
}

pub fn check(t: &Theme, status: checkbox::Status) -> checkbox::Style {
    let (checked, hovered, disabled) = match status {
        checkbox::Status::Active { is_checked } => (is_checked, false, false),
        checkbox::Status::Hovered { is_checked } => (is_checked, true, false),
        checkbox::Status::Disabled { is_checked } => (is_checked, false, true),
    };
    let p = t.extended_palette();
    let (fill, edge_color) = match (checked, disabled) {
        (_, true) => (press_fill(t), line(t)),
        (true, false) => {
            let fill = if hovered { mix(accent(t), ink(t), 0.12) } else { accent(t) };
            (fill, fill)
        }
        (false, false) => (field(t), if hovered { ink(t).scale_alpha(0.4) } else { edge(t) }),
    };
    checkbox::Style {
        background: fill.into(),
        icon_color: p.primary.base.text,
        border: Border { width: 1.0, color: edge_color, radius: 4.0.into() },
        text_color: None,
    }
}

/// Thin rounded scrollbars with no track, darker while used.
pub fn scrollbars(t: &Theme, status: scrollable::Status) -> scrollable::Style {
    let (hovered, dragged) = match status {
        scrollable::Status::Active { .. } => (false, false),
        scrollable::Status::Hovered { is_vertical_scrollbar_hovered, is_horizontal_scrollbar_hovered, .. } => {
            (is_vertical_scrollbar_hovered || is_horizontal_scrollbar_hovered, false)
        }
        scrollable::Status::Dragged { is_vertical_scrollbar_dragged, is_horizontal_scrollbar_dragged, .. } => {
            (false, is_vertical_scrollbar_dragged || is_horizontal_scrollbar_dragged)
        }
    };
    let alpha = if dragged {
        0.5
    } else if hovered {
        0.38
    } else {
        0.22
    };
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller { background: ink(t).scale_alpha(alpha).into(), border: border::rounded(PILL) },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::default(t, status).auto_scroll,
    }
}

/// The file editor: square, framed like a terminal, in the accent color while it has the keyboard.
pub fn file_editor(t: &Theme, status: text_editor::Status) -> text_editor::Style {
    let focused = matches!(status, text_editor::Status::Focused { .. });
    text_editor::Style {
        background: background(t).into(),
        border: Border { width: 1.0, color: if focused { accent(t) } else { line(t) }, radius: 0.0.into() },
        placeholder: faint(t),
        value: ink(t),
        selection: accent(t).scale_alpha(0.3),
    }
}

// Surfaces

/// The side columns: projects, sessions and the right panel.
pub fn side_column(t: &Theme) -> container::Style {
    container::Style { background: Some(side(t).into()), text_color: Some(ink(t)), ..container::Style::default() }
}

/// A hairline between two parts of the window.
pub fn divider(t: &Theme) -> container::Style {
    container::Style { background: Some(line(t).into()), ..container::Style::default() }
}

/// A task in the Tasks tab; one `linked` to the session on screen is outlined in the accent.
pub fn task_card(linked: bool) -> impl Fn(&Theme) -> container::Style {
    move |t| container::Style {
        background: Some(raised(t).into()),
        text_color: Some(ink(t)),
        border: Border {
            width: 1.0,
            color: if linked { accent(t).scale_alpha(0.7) } else { line(t) },
            radius: RADIUS.into(),
        },
        shadow: shadow_sm(t),
        ..container::Style::default()
    }
}

/// A small menu, such as an entry's in the Files tab.
pub fn menu(t: &Theme) -> container::Style {
    container::Style {
        background: Some(raised(t).into()),
        text_color: Some(ink(t)),
        border: Border { width: 1.0, color: line(t), radius: RADIUS.into() },
        shadow: shadow_md(t),
        ..container::Style::default()
    }
}

/// A tooltip, in the text color so it stands out from whatever it covers.
pub fn tooltip(t: &Theme) -> container::Style {
    container::Style {
        background: Some(ink(t).into()),
        text_color: Some(background(t)),
        border: border::rounded(RADIUS_SM),
        shadow: shadow_md(t),
        ..container::Style::default()
    }
}

/// The smaller lines under a tooltip's first, such as a keyboard shortcut.
pub fn tooltip_hint(t: &Theme) -> Color {
    background(t).scale_alpha(0.7)
}

/// A list item; the selected one is raised, and `drop_target` outlines it
/// while a task is held over it.
pub fn list_item(selected: bool, drop_target: bool) -> impl Fn(&Theme) -> container::Style {
    move |t| {
        let mut style = container::Style {
            text_color: Some(ink(t)),
            border: border::rounded(RADIUS),
            ..container::Style::default()
        };
        if selected {
            style.background = Some(chosen(t).into());
            style.shadow = shadow_sm(t);
        }
        if drop_target {
            style.background = Some(accent(t).scale_alpha(0.14).into());
            style.border = style.border.width(1.5).color(accent(t));
        }
        style
    }
}

/// The bar at the left edge of the selected list item; nothing on the others.
pub fn selection_bar(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |t| container::Style {
        background: selected.then(|| accent(t).into()),
        border: border::rounded(PILL),
        ..container::Style::default()
    }
}

/// A tinted box with a message, such as an error.
pub fn note(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |t| {
        let c = tone_color(t, tone);
        container::Style {
            background: Some(c.scale_alpha(by_tone(t, 0.08, 0.12)).into()),
            text_color: Some(tone_text(t, tone)),
            border: Border { width: 1.0, color: c.scale_alpha(0.3), radius: RADIUS.into() },
            ..container::Style::default()
        }
    }
}

/// A small rounded label, such as a branch in the git log.
pub fn badge(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |t| {
        let (fill, text_color) = match tone {
            Tone::Neutral => (ink(t).scale_alpha(by_tone(t, 0.07, 0.1)), ink(t).scale_alpha(0.85)),
            _ => (tone_color(t, tone).scale_alpha(by_tone(t, 0.12, 0.16)), tone_text(t, tone)),
        };
        container::Style {
            background: Some(fill.into()),
            text_color: Some(text_color),
            border: border::rounded(PILL),
            ..container::Style::default()
        }
    }
}

// Terminals

/// The frame around a terminal and its title: the accent while it has the keyboard.
pub fn pane_frame(active: bool) -> impl Fn(&Theme) -> container::Style {
    move |t| container::Style {
        background: Some(title_fill(t).into()),
        border: Border { width: 1.0, color: if active { accent(t) } else { line(t) }, radius: 0.0.into() },
        ..container::Style::default()
    }
}

/// A terminal's title bar, on the frame's fill: plain text while the terminal has the keyboard, else muted.
pub fn title_bar(active: bool) -> impl Fn(&Theme) -> container::Style {
    move |t| container::Style {
        text_color: Some(if active { ink(t) } else { muted(t) }),
        ..container::Style::default()
    }
}

/// A terminal shown on its own, such as Neovim editing a file: framed like a pane.
pub fn terminal_frame(t: &Theme) -> container::Style {
    container::Style {
        border: Border { width: 1.0, color: line(t), radius: 0.0.into() },
        ..container::Style::default()
    }
}

pub fn pane_grid_style(t: &Theme) -> pane_grid::Style {
    let mut style = pane_grid::default(t);
    style.hovered_split.color = accent(t);
    style.picked_split.color = accent(t);
    style.hovered_region.background = accent(t).scale_alpha(0.15).into();
    style.hovered_region.border = Border { width: 1.5, color: accent(t), radius: RADIUS.into() };
    style
}

// Dialogs and pop-ups

/// The terminals of the shown session while a task is held over them.
pub fn drop_target(t: &Theme) -> container::Style {
    container::Style {
        background: Some(accent(t).scale_alpha(0.12).into()),
        text_color: Some(ink(t)),
        border: Border { width: 1.5, color: accent(t), radius: RADIUS_LG.into() },
        ..container::Style::default()
    }
}

/// The label that follows the pointer while a task is dragged.
pub fn drag_label(t: &Theme) -> container::Style {
    container::Style {
        background: Some(raised(t).into()),
        text_color: Some(ink(t)),
        border: Border { width: 1.0, color: accent(t).scale_alpha(0.6), radius: RADIUS.into() },
        shadow: shadow_md(t),
        ..container::Style::default()
    }
}

/// The color of an alert: danger for failures, success for agents.
pub fn notice_color(t: &Theme, category: crate::notify::Category) -> Color {
    tone_color(t, notice_tone(category))
}

pub fn notice_tone(category: crate::notify::Category) -> Tone {
    match category {
        crate::notify::Category::Error => Tone::Danger,
        crate::notify::Category::Agent => Tone::Success,
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

/// A pop-up about a terminal that is out of view.
pub fn toast(category: crate::notify::Category) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let p = theme.extended_palette();
        container::Style {
            background: Some(Background::Color(p.background.weak.color)),
            text_color: Some(p.background.weak.text),
            border: Border { width: 1.5, color: notice_color(theme, category), radius: 8.0.into() },
            shadow: iced::Shadow {
                color: Color { a: 0.35, ..Color::BLACK },
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 12.0,
            },
            ..container::Style::default()
        }
    }
}
