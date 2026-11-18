//! Colors: the interface's few named colors.

use ratatui::style::{Color, Modifier, Style};

/// Interface colors. Named ANSI colors and modifiers only, so the interface
/// follows the terminal's theme.
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
