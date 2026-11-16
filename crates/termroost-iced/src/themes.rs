//! Color themes: built-in Dark and Light plus user-defined themes.
//!
//! A theme colors both the interface (an iced palette) and the terminals (a
//! core [`Palette`]). User themes are stored as JSON in the core's opaque GUI
//! key/value storage, so they live in the same database as everything else.

use iced::Color;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use termroost_core::{Core, Palette, Rgb};

pub const THEMES_KEY: &str = "iced.themes";
pub const DARK: &str = "Dark";
pub const LIGHT: &str = "Light";

/// An RGB color, stored as `"#rrggbb"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hex(pub u8, pub u8, pub u8);

impl Hex {
    pub const fn new(v: u32) -> Hex {
        Hex((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Parses `#rgb`, `#rrggbb`, `rgb` or `rrggbb`.
    pub fn parse(s: &str) -> Option<Hex> {
        let s = s.trim().trim_start_matches('#');
        let digits: Vec<u8> = s.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect::<Option<_>>()?;
        match digits.as_slice() {
            [r, g, b] => Some(Hex(r * 17, g * 17, b * 17)),
            [r1, r2, g1, g2, b1, b2] => Some(Hex(r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2)),
            _ => None,
        }
    }

    pub fn color(self) -> Color {
        Color::from_rgb8(self.0, self.1, self.2)
    }

    pub fn rgb(self) -> Rgb {
        Rgb::new(self.0, self.1, self.2)
    }

    /// Relative luminance in `0.0..=1.0`.
    pub fn luminance(self) -> f32 {
        let lin = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(self.0) + 0.7152 * lin(self.1) + 0.0722 * lin(self.2)
    }
}

impl std::fmt::Display for Hex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

impl Serialize for Hex {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hex {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Hex::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color {s:?}")))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiColors {
    pub background: Hex,
    pub text: Hex,
    pub primary: Hex,
    pub success: Hex,
    pub warning: Hex,
    pub danger: Hex,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TermColors {
    pub foreground: Hex,
    pub background: Hex,
    pub cursor: Hex,
    pub selection: Hex,
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the bright variants.
    pub ansi: [Hex; 16],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeDef {
    pub name: String,
    pub ui: UiColors,
    pub terminal: TermColors,
}

/// One editable color of a theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorField {
    UiBackground,
    UiText,
    UiPrimary,
    UiSuccess,
    UiWarning,
    UiDanger,
    TermForeground,
    TermBackground,
    TermCursor,
    TermSelection,
    Ansi(u8),
}

impl ColorField {
    pub const UI: [(ColorField, &'static str); 6] = [
        (ColorField::UiBackground, "Background"),
        (ColorField::UiText, "Text"),
        (ColorField::UiPrimary, "Accent"),
        (ColorField::UiSuccess, "Success"),
        (ColorField::UiWarning, "Warning"),
        (ColorField::UiDanger, "Danger"),
    ];
    pub const TERMINAL: [(ColorField, &'static str); 4] = [
        (ColorField::TermForeground, "Foreground"),
        (ColorField::TermBackground, "Background"),
        (ColorField::TermCursor, "Cursor"),
        (ColorField::TermSelection, "Selection"),
    ];
}

pub const ANSI_NAMES: [&str; 8] = ["Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White"];

impl ThemeDef {
    pub fn get(&self, field: ColorField) -> Hex {
        match field {
            ColorField::UiBackground => self.ui.background,
            ColorField::UiText => self.ui.text,
            ColorField::UiPrimary => self.ui.primary,
            ColorField::UiSuccess => self.ui.success,
            ColorField::UiWarning => self.ui.warning,
            ColorField::UiDanger => self.ui.danger,
            ColorField::TermForeground => self.terminal.foreground,
            ColorField::TermBackground => self.terminal.background,
            ColorField::TermCursor => self.terminal.cursor,
            ColorField::TermSelection => self.terminal.selection,
            ColorField::Ansi(i) => self.terminal.ansi[i as usize % 16],
        }
    }

    pub fn set(&mut self, field: ColorField, value: Hex) {
        let slot = match field {
            ColorField::UiBackground => &mut self.ui.background,
            ColorField::UiText => &mut self.ui.text,
            ColorField::UiPrimary => &mut self.ui.primary,
            ColorField::UiSuccess => &mut self.ui.success,
            ColorField::UiWarning => &mut self.ui.warning,
            ColorField::UiDanger => &mut self.ui.danger,
            ColorField::TermForeground => &mut self.terminal.foreground,
            ColorField::TermBackground => &mut self.terminal.background,
            ColorField::TermCursor => &mut self.terminal.cursor,
            ColorField::TermSelection => &mut self.terminal.selection,
            ColorField::Ansi(i) => &mut self.terminal.ansi[i as usize % 16],
        };
        *slot = value;
    }

    pub fn is_dark(&self) -> bool {
        self.ui.background.luminance() < 0.4
    }

    pub fn iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(
            self.name.clone(),
            iced::theme::Palette {
                background: self.ui.background.color(),
                text: self.ui.text.color(),
                primary: self.ui.primary.color(),
                success: self.ui.success.color(),
                warning: self.ui.warning.color(),
                danger: self.ui.danger.color(),
            },
        )
    }

    /// The terminal palette for the core.
    pub fn palette(&self) -> Palette {
        let t = &self.terminal;
        Palette::new(t.ansi.map(Hex::rgb), t.foreground.rgb(), t.background.rgb(), t.cursor.rgb(), t.selection.rgb())
    }

    /// Tokyo Night inspired dark theme (the original look).
    pub fn dark() -> ThemeDef {
        ThemeDef {
            name: DARK.into(),
            ui: UiColors {
                background: Hex::new(0x1a1b26),
                text: Hex::new(0x9aa5ce),
                primary: Hex::new(0x2ac3de),
                success: Hex::new(0x9ece6a),
                warning: Hex::new(0xe0af68),
                danger: Hex::new(0xf7768e),
            },
            terminal: TermColors {
                foreground: Hex::new(0xc0caf5),
                background: Hex::new(0x1a1b26),
                cursor: Hex::new(0xc0caf5),
                selection: Hex::new(0x33467c),
                ansi: [
                    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, //
                    0x414868, 0xff899d, 0xb9f27c, 0xff9e64, 0x8db0ff, 0xc7a9ff, 0xa4daff, 0xc0caf5,
                ]
                .map(Hex::new),
            },
        }
    }

    /// GitHub-light inspired light theme.
    pub fn light() -> ThemeDef {
        ThemeDef {
            name: LIGHT.into(),
            ui: UiColors {
                background: Hex::new(0xf6f8fa),
                text: Hex::new(0x1f2328),
                primary: Hex::new(0x0969da),
                success: Hex::new(0x1a7f37),
                warning: Hex::new(0x9a6700),
                danger: Hex::new(0xcf222e),
            },
            terminal: TermColors {
                foreground: Hex::new(0x1f2328),
                background: Hex::new(0xffffff),
                cursor: Hex::new(0x0969da),
                selection: Hex::new(0xb6d7ff),
                ansi: [
                    0x24292f, 0xcf222e, 0x116329, 0x7d4e00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781, //
                    0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
                ]
                .map(Hex::new),
            },
        }
    }
}

/// Built-in themes plus the user's own (only the latter are persisted).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ThemeStore {
    pub custom: Vec<ThemeDef>,
    #[serde(skip)]
    builtin: Vec<ThemeDef>,
}

impl ThemeStore {
    pub fn load(core: &Core) -> ThemeStore {
        let mut store: ThemeStore =
            core.state().gui_value(THEMES_KEY).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        store.builtin = vec![ThemeDef::dark(), ThemeDef::light()];
        // A user theme can never shadow a built-in one.
        let builtin_names: Vec<String> = store.builtin.iter().map(|t| t.name.clone()).collect();
        store.custom.retain(|t| !builtin_names.contains(&t.name) && !t.name.trim().is_empty());
        store
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("themes serialize")
    }

    pub fn all(&self) -> impl Iterator<Item = &ThemeDef> {
        self.builtin.iter().chain(self.custom.iter())
    }

    pub fn find(&self, name: &str) -> Option<&ThemeDef> {
        self.all().find(|t| t.name == name)
    }

    pub fn custom_mut(&mut self, name: &str) -> Option<&mut ThemeDef> {
        self.custom.iter_mut().find(|t| t.name == name)
    }

    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtin.iter().any(|t| t.name == name)
    }

    /// `base`, or `base 2`, `base 3`… whichever is free.
    pub fn unique_name(&self, base: &str) -> String {
        if self.find(base).is_none() {
            return base.to_owned();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| self.find(n).is_none()).expect("free name")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(Hex::parse("#1a1B26"), Some(Hex(0x1a, 0x1b, 0x26)));
        assert_eq!(Hex::parse("fff"), Some(Hex(255, 255, 255)));
        assert_eq!(Hex::parse(" #000000 "), Some(Hex(0, 0, 0)));
        assert_eq!(Hex::parse("#12345"), None);
        assert_eq!(Hex::parse("#gg0000"), None);
        assert_eq!(Hex(0x0a, 0xbc, 0xff).to_string(), "#0abcff");
    }

    #[test]
    fn builtins_have_expected_tone() {
        assert!(ThemeDef::dark().is_dark());
        assert!(!ThemeDef::light().is_dark());
        assert!(ThemeDef::dark().iced_theme().extended_palette().is_dark);
        assert!(!ThemeDef::light().iced_theme().extended_palette().is_dark);
    }

    #[test]
    fn theme_json_round_trip() {
        let mut t = ThemeDef::light();
        t.name = "Mine".into();
        t.set(ColorField::Ansi(9), Hex(1, 2, 3));
        let store = ThemeStore { custom: vec![t.clone()], builtin: vec![] };
        let back: ThemeStore = serde_json::from_str(&store.to_json()).unwrap();
        assert_eq!(back.custom, vec![t]);
        assert_eq!(back.custom[0].get(ColorField::Ansi(9)), Hex(1, 2, 3));
    }
}
