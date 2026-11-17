//! The color picker in the theme editor: a palette of ready-made colors, the
//! colors the theme already uses, and hue/saturation/lightness sliders.

use std::sync::LazyLock;

use iced::widget::{Column, button, column, container, row, slider, space, text};
use iced::{Alignment, Border, Element, Length};

use crate::app::Message;
use crate::sidebar::muted_text;
use crate::theme::{self, BOLD, MONO};
use crate::themes::{ColorField, Hex, ThemeDef};

/// Side of one palette swatch, in pixels.
const CHIP: f32 = 18.0;
/// Number of hues in the palette, evenly spaced around the color wheel.
const HUES: usize = 24;
/// Lightness (OKLCH) of each colored palette row, lightest first.
const ROWS: [f64; 10] = [0.95, 0.89, 0.82, 0.74, 0.66, 0.58, 0.50, 0.42, 0.33, 0.24];

/// The open picker: which color it changes and the slider positions.
#[derive(Clone, Copy, Debug)]
pub struct ColorPicker {
    pub field: ColorField,
    /// The color when the picker opened, for "Revert".
    pub original: Hex,
    /// Kept separately from the color so a slider doesn't jump when rounding
    /// to `#rrggbb` loses precision (or the hue of a gray).
    pub hsl: Hsl,
}

impl ColorPicker {
    pub fn new(field: ColorField, color: Hex) -> ColorPicker {
        ColorPicker { field, original: color, hsl: Hsl::of(color) }
    }
}

/// Hue in degrees (`0..360`), saturation and lightness in percent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsl {
    pub h: f32,
    pub s: f32,
    pub l: f32,
}

impl Hsl {
    pub fn of(c: Hex) -> Hsl {
        let [r, g, b] = [c.0, c.1, c.2].map(|v| v as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d == 0.0 {
            return Hsl { h: 0.0, s: 0.0, l: l * 100.0 };
        }
        let s = (d / (1.0 - (2.0 * l - 1.0).abs())).min(1.0);
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        Hsl { h: h * 60.0, s: s * 100.0, l: l * 100.0 }
    }

    pub fn to_hex(self) -> Hex {
        let s = self.s.clamp(0.0, 100.0) / 100.0;
        let l = self.l.clamp(0.0, 100.0) / 100.0;
        let h = self.h.rem_euclid(360.0) / 60.0;
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Hex(byte(r), byte(g), byte(b))
    }
}

/// sRGB (`0..=1` per channel) of an OKLCH color, or `None` when sRGB can't show it.
fn oklch(l: f64, c: f64, h: f64) -> Option<[f64; 3]> {
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548 * b;
    let (l3, m3, s3) = (l_.powi(3), m_.powi(3), s_.powi(3));
    let linear = [
        4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3,
        -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701 * s3,
    ];
    const EPS: f64 = 1e-4;
    linear.iter().all(|v| (-EPS..=1.0 + EPS).contains(v)).then(|| {
        linear.map(|v| {
            let v = v.clamp(0.0, 1.0);
            if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
        })
    })
}

/// The OKLCH color with lightness `l` and hue `h`, as colorful as sRGB allows up to chroma `c`.
fn oklch_hex(l: f64, c: f64, h: f64) -> Hex {
    let (mut lo, mut hi) = (0.0, c);
    if oklch(l, c, h).is_some() {
        lo = c;
    } else {
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if oklch(l, mid, h).is_some() { lo = mid } else { hi = mid }
        }
    }
    let [r, g, b] = oklch(l, lo, h).unwrap_or([l; 3]).map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8);
    Hex(r, g, b)
}

/// A row of grays from white to black, then rows of hues from light to dark.
/// Built in OKLCH so each row looks evenly bright across hues.
static PALETTE: LazyLock<Vec<Vec<Hex>>> = LazyLock::new(|| {
    let grays = (0..HUES).map(|i| oklch_hex(1.0 - i as f64 / (HUES - 1) as f64, 0.0, 0.0)).collect();
    let hues = ROWS.iter().map(|&l| {
        // Most colorful in the middle rows, softer towards white and black.
        let chroma = (0.19 * (1.0 - ((l - 0.62) / 0.5).powi(2))).max(0.03);
        (0..HUES).map(|i| oklch_hex(l, chroma, 20.0 + 360.0 * i as f64 / HUES as f64)).collect()
    });
    std::iter::once(grays).chain(hues).collect()
});

/// The theme's distinct colors, in the order the editor lists them.
fn theme_colors(t: &ThemeDef) -> Vec<Hex> {
    let fields =
        ColorField::UI.iter().chain(ColorField::TERMINAL.iter()).map(|(f, _)| *f).chain((0..16).map(ColorField::Ansi));
    let mut out: Vec<Hex> = Vec::new();
    for c in fields.map(|f| t.get(f)) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Black or white, whichever reads better on `c`.
fn contrast(c: Hex) -> Hex {
    if c.luminance() > 0.4 { Hex(0, 0, 0) } else { Hex(255, 255, 255) }
}

/// A clickable color square; the current color has a check mark.
fn chip<'a>(c: Hex, selected: bool, msg: Message) -> Element<'a, Message> {
    let mark: Element<'a, Message> = if selected {
        text("✓").font(MONO).size(12).color(contrast(c).color()).center().width(Length::Fill).into()
    } else {
        space().into()
    };
    button(mark)
        .width(CHIP)
        .height(CHIP)
        .padding(0)
        .on_press(msg)
        .style(move |t: &iced::Theme, status| {
            let p = t.extended_palette();
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(c.color().into()),
                border: Border {
                    width: if hovered { 2.0 } else { 1.0 },
                    color: if hovered { p.background.base.text } else { theme::muted(t).scale_alpha(0.5) },
                    radius: 3.0.into(),
                },
                ..button::Style::default()
            }
        })
        .into()
}

/// The square next to a color field; opens the picker for that field. While
/// open it gets an accent ring plus an inner black or white one, so it stands
/// out even when the color is the accent itself.
pub fn field_swatch<'a>(c: Hex, open: bool, msg: Message) -> Element<'a, Message> {
    let inner: Element<'a, Message> = if open {
        container(space())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_: &iced::Theme| container::Style {
                border: Border { width: 1.0, color: contrast(c).color(), radius: 2.0.into() },
                ..container::Style::default()
            })
            .into()
    } else {
        space().into()
    };
    button(inner)
        .width(22)
        .height(22)
        .padding(0)
        .on_press(msg)
        .style(move |t: &iced::Theme, status| {
            let p = t.extended_palette();
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(c.color().into()),
                border: Border {
                    width: if open || hovered { 2.0 } else { 1.0 },
                    color: if open {
                        p.primary.base.color
                    } else if hovered {
                        p.background.base.text
                    } else {
                        theme::muted(t)
                    },
                    radius: 3.0.into(),
                },
                ..button::Style::default()
            }
        })
        .into()
}

/// The picker for `picker.field` of theme `t`, shown under that field's group.
pub fn panel<'a>(picker: &ColorPicker, t: &ThemeDef) -> Element<'a, Message> {
    let field = picker.field;
    let current = t.get(field);
    let pick = move |c: Hex| Message::ThemeColor(field, c.to_string());
    let chips = |colors: &[Hex]| row(colors.iter().map(|&c| chip(c, c == current, pick(c)))).spacing(2);

    let big = container(space()).width(36).height(36).style(move |th: &iced::Theme| container::Style {
        background: Some(current.color().into()),
        border: Border { width: 1.0, color: theme::muted(th), radius: 4.0.into() },
        ..container::Style::default()
    });
    let mut value = row![text(current.to_string()).font(MONO).size(12)].spacing(8);
    if current != picker.original {
        value = value.push(muted_text(format!("was {}", picker.original)).font(MONO));
    }
    let header = row![
        big,
        column![text(field.label()).size(14).font(BOLD), value].spacing(2).width(Length::Fill),
        button(text("Revert").size(12))
            .padding([4, 10])
            .style(button::secondary)
            .on_press_maybe((current != picker.original).then(|| pick(picker.original))),
        button(text("Done").size(12)).padding([4, 12]).style(button::primary).on_press(Message::PickColor(None)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let (grays, hues) = PALETTE.split_first().expect("palette has rows");
    let grid = column![chips(grays), Column::with_children(hues.iter().map(|r| chips(r).into())).spacing(2)].spacing(6);

    let hsl = picker.hsl;
    let adjust = |label: &'a str, max: f32, value: f32, unit: &'a str, set: fn(Hsl, f32) -> Hsl| {
        row![
            text(label).size(12).width(74),
            slider(0.0..=max, value, move |v| Message::PickerHsl(set(hsl, v))).step(1.0_f32),
            text(format!("{value:.0}{unit}")).font(MONO).size(12).width(40),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    };
    let side = column![
        muted_text("In this theme"),
        chips(&theme_colors(t)).wrap().vertical_spacing(2),
        space().height(6),
        muted_text("Adjust"),
        adjust("Hue", 360.0, hsl.h, "°", |c, h| Hsl { h, ..c }),
        adjust("Saturation", 100.0, hsl.s, "%", |c, s| Hsl { s, ..c }),
        adjust("Lightness", 100.0, hsl.l, "%", |c, l| Hsl { l, ..c }),
    ]
    .spacing(6)
    .width(300);

    container(
        column![
            header,
            row![column![muted_text("Palette"), grid].spacing(6), side].spacing(24).wrap().vertical_spacing(16),
        ]
        .spacing(14),
    )
    .padding(12)
    .width(Length::Fill)
    .style(|th: &iced::Theme| {
        let p = th.extended_palette();
        container::Style {
            background: Some(p.background.base.color.into()),
            border: Border { width: 1.0, color: p.primary.base.color, radius: 6.0.into() },
            ..container::Style::default()
        }
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trip() {
        for c in [Hex(0x2a, 0xc3, 0xde), Hex(0xf7, 0x76, 0x8e), Hex(0, 0, 0), Hex(255, 255, 255), Hex(0x80, 0x80, 0x80)]
        {
            assert_eq!(Hsl::of(c).to_hex(), c, "{c}");
        }
        assert_eq!(Hsl { h: 0.0, s: 100.0, l: 50.0 }.to_hex(), Hex(255, 0, 0));
        assert_eq!(Hsl { h: 120.0, s: 100.0, l: 50.0 }.to_hex(), Hex(0, 255, 0));
        assert_eq!(Hsl { h: 240.0, s: 100.0, l: 25.0 }.to_hex(), Hex(0, 0, 128));
        assert_eq!(Hsl { h: 360.0, s: 100.0, l: 50.0 }.to_hex(), Hex(255, 0, 0));
        let h = Hsl::of(Hex(255, 0, 255));
        assert!((h.h - 300.0).abs() < 0.01 && (h.s - 100.0).abs() < 0.01 && (h.l - 50.0).abs() < 0.01);
    }

    #[test]
    fn oklch_reference_colors() {
        assert_eq!(oklch_hex(1.0, 0.0, 0.0), Hex(255, 255, 255));
        assert_eq!(oklch_hex(0.0, 0.0, 0.0), Hex(0, 0, 0));
        // sRGB red is oklch(0.62796 0.25768 29.23); allow for those digits being rounded.
        let red = oklch_hex(0.627_96, 0.257_68, 29.23);
        assert!(red.0 >= 254 && red.1 <= 1 && red.2 <= 1, "got {red}");
        // Chroma beyond sRGB is reduced, not clipped to another hue or lightness.
        assert!(oklch(0.6, 0.4, 200.0).is_none());
        let c = oklch_hex(0.6, 0.4, 200.0);
        assert!(c.1 > c.0 && c.2 > c.0, "a cyan, got {c}");
    }

    #[test]
    fn palette_shape() {
        assert_eq!(PALETTE.len(), 1 + ROWS.len());
        assert!(PALETTE.iter().all(|r| r.len() == HUES));
        assert_eq!(PALETTE[0][0], Hex(255, 255, 255));
        assert_eq!(PALETTE[0][HUES - 1], Hex(0, 0, 0));
        // Rows get darker from top to bottom.
        let lum = |r: &[Hex]| r.iter().map(|c| c.luminance()).sum::<f32>();
        assert!(PALETTE[1..].windows(2).all(|w| lum(&w[0]) > lum(&w[1])));
    }

    #[test]
    fn theme_colors_are_distinct() {
        let colors = theme_colors(&ThemeDef::dark());
        assert!(colors.len() > 10);
        assert!(colors.iter().enumerate().all(|(i, c)| !colors[..i].contains(c)));
        // The dark theme's background is used twice (interface and terminal), listed once.
        assert_eq!(colors.iter().filter(|c| **c == Hex::new(0x1a1b26)).count(), 1);
    }
}
