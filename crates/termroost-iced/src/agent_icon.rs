//! The logos of the coding agents, shown next to the sessions and terminals
//! that run one. Where the files come from: `icons/LICENSES.md`.

use std::sync::LazyLock;

use iced::widget::{Svg, svg};
use iced::{Color, Theme};
use termroost_core::AgentKind;

// Built once so every view hands iced the same handle, and its rasterized
// copy is reused instead of drawn again.
static CLAUDE: LazyLock<svg::Handle> =
    LazyLock::new(|| svg::Handle::from_memory(include_bytes!("../icons/claude.svg").as_slice()));
static CODEX: LazyLock<svg::Handle> =
    LazyLock::new(|| svg::Handle::from_memory(include_bytes!("../icons/codex.svg").as_slice()));
static GROK: LazyLock<svg::Handle> =
    LazyLock::new(|| svg::Handle::from_memory(include_bytes!("../icons/grok.svg").as_slice()));

/// The logo of `kind`, `size` pixels square. `ink` is the color of the text
/// around it, which one-color logos are drawn in.
pub fn agent_icon<'a>(kind: AgentKind, size: f32, ink: impl Fn(&Theme) -> Color + 'a) -> Svg<'a> {
    let handle = match kind {
        AgentKind::Claude => &CLAUDE,
        AgentKind::Codex => &CODEX,
        AgentKind::Grok => &GROK,
    };
    let icon = svg(LazyLock::force(handle).clone()).width(size).height(size);
    match kind {
        // Grok's logo is plain black or white, so it can't keep a color of its
        // own on both light and dark backgrounds.
        AgentKind::Grok => icon.style(move |t, _| svg::Style { color: Some(ink(t)) }),
        AgentKind::Claude | AgentKind::Codex => icon,
    }
}
