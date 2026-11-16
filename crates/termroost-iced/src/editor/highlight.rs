//! Syntax highlighting in the file editor, by the file's extension.
//!
//! iced's highlighter starts over only when its settings change, and one
//! editor shows every file in turn: switching between two `.rs` files would
//! keep the first one's state. So the settings name the file too.

use std::path::Path;

use iced::advanced::text::Highlighter;
use iced::highlighter;

use super::DocId;

/// iced's highlighter, started over for each file.
pub struct DocHighlighter(highlighter::Highlighter);

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    doc: DocId,
    inner: highlighter::Settings,
}

impl Settings {
    /// For file `path` (shown as `doc`), in colors for a dark or light interface.
    pub fn new(doc: DocId, path: &Path, dark: bool) -> Settings {
        let theme = if dark { highlighter::Theme::Base16Ocean } else { highlighter::Theme::InspiredGitHub };
        Settings { doc, inner: highlighter::Settings { theme, token: token(path) } }
    }
}

/// What the highlighter knows a language by: the file's extension, or its
/// name when it has none (`Makefile`, `Dockerfile`).
fn token(path: &Path) -> String {
    path.extension().or_else(|| path.file_name()).map(|t| t.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Highlighter for DocHighlighter {
    type Settings = Settings;
    type Highlight = highlighter::Highlight;
    type Iterator<'a> = <highlighter::Highlighter as Highlighter>::Iterator<'a>;

    fn new(settings: &Settings) -> Self {
        DocHighlighter(highlighter::Highlighter::new(&settings.inner))
    }

    fn update(&mut self, settings: &Settings) {
        // Starts over, also for another file of the same language.
        self.0.update(&settings.inner);
    }

    fn change_line(&mut self, line: usize) {
        self.0.change_line(line);
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        self.0.highlight_line(line)
    }

    fn current_line(&self) -> usize {
        self.0.current_line()
    }
}

/// The text format of a highlight.
pub fn format(
    highlight: &highlighter::Highlight,
    _theme: &iced::Theme,
) -> iced::advanced::text::highlighter::Format<iced::Font> {
    highlight.to_format()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_known_by_extension_or_name() {
        assert_eq!(token(Path::new("/w/src/main.rs")), "rs");
        assert_eq!(token(Path::new("/w/Makefile")), "Makefile");
        assert_eq!(token(Path::new("/w/.gitignore")), ".gitignore", "a dot file has no extension");
    }

    #[test]
    fn another_file_of_the_same_language_starts_over() {
        let a = Settings::new(DocId(1), Path::new("a.rs"), true);
        let b = Settings::new(DocId(2), Path::new("b.rs"), true);
        assert_ne!(a, b);
        assert_eq!(a, Settings::new(DocId(1), Path::new("a.rs"), true));
        assert_ne!(a, Settings::new(DocId(1), Path::new("a.rs"), false), "the theme changed");
    }
}
