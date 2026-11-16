//! Reading text files for the editor: what counts as text, and how its
//! lines end, so saving can write it back the same way.

use std::path::Path;

/// Larger files are not opened: the editor keeps whole copies for undo.
pub const MAX_SIZE: u64 = 5 * 1024 * 1024;
/// How much of the start is looked at for NUL bytes, as git does to spot binaries.
const BINARY_PROBE: usize = 8000;
const BOM: char = '\u{feff}';

/// How the lines of a file end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
        }
    }
}

/// What Tab inserts in a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Tabs,
    /// This many spaces per level.
    Spaces(usize),
}

impl Indent {
    pub fn label(self) -> String {
        match self {
            Indent::Tabs => "Tabs".into(),
            Indent::Spaces(n) => format!("Spaces: {n}"),
        }
    }

    /// What Tab inserts at character column `column`: a tab, or the spaces up
    /// to the next indent stop.
    pub fn unit(self, column: usize) -> String {
        match self {
            Indent::Tabs => "\t".into(),
            Indent::Spaces(n) => " ".repeat(n - column % n),
        }
    }

    /// How many bytes at the start of `line` one Shift+Tab removes: a tab, or
    /// up to one level of spaces.
    pub fn outdent_len(self, line: &str) -> usize {
        if line.starts_with('\t') {
            return 1;
        }
        let level = match self {
            Indent::Tabs => 4,
            Indent::Spaces(n) => n,
        };
        line.bytes().take_while(|b| *b == b' ').count().min(level)
    }
}

/// How a file is indented: tabs if more lines start with one than with
/// spaces, else the smallest indent of two or more spaces (4 if none).
pub fn detect_indent(text: &str) -> Indent {
    let (mut tabs, mut spaced, mut width) = (0, 0, usize::MAX);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if line.starts_with('\t') {
            tabs += 1;
        } else if line.starts_with(' ') {
            spaced += 1;
            // One space is alignment (` * ` in comments), not indent.
            let n = line.len() - line.trim_start_matches(' ').len();
            if n >= 2 {
                width = width.min(n);
            }
        }
    }
    if tabs > spaced {
        Indent::Tabs
    } else if (2..=8).contains(&width) {
        Indent::Spaces(width)
    } else {
        Indent::Spaces(4)
    }
}

/// What to keep of a file's bytes besides its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// It starts with a UTF-8 byte order mark.
    pub bom: bool,
    /// Its usual line ending (the one most of its lines use).
    pub eol: Eol,
}

/// A file read as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    /// The text without the byte order mark.
    pub text: String,
    pub format: Format,
}

/// Reads `path` as text, or says why it can't be edited.
pub fn read_file(path: &Path) -> Result<Decoded, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("it is not a file".into());
    }
    if meta.len() > MAX_SIZE {
        return Err(too_large());
    }
    decode(std::fs::read(path).map_err(|e| e.to_string())?)
}

fn too_large() -> String {
    format!("it is larger than {} MB", MAX_SIZE / (1024 * 1024))
}

/// Checks that `bytes` are text the editor can write back unchanged.
pub fn decode(bytes: Vec<u8>) -> Result<Decoded, String> {
    if bytes.len() as u64 > MAX_SIZE {
        return Err(too_large());
    }
    if bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) {
        return Err("it looks like a binary file".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_owned())?;
    let (bom, text) = match text.strip_prefix(BOM) {
        Some(rest) => (true, rest.to_owned()),
        None => (false, text),
    };
    // The editor would take a lone CR for a line break and save it as one.
    let bytes = text.as_bytes();
    let lone_cr = bytes.iter().enumerate().any(|(i, b)| *b == b'\r' && bytes.get(i + 1) != Some(&b'\n'));
    if lone_cr {
        return Err("some of its lines end with a lone CR (old Mac line breaks)".into());
    }
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    let eol = if crlf > lf { Eol::Crlf } else { Eol::Lf };
    Ok(Decoded { text, format: Format { bom, eol } })
}

/// The bytes to write for `lines`: each line ended the file's usual way
/// (also lines added in the editor, which ends them with LF), and the byte
/// order mark if the file had one. The editor's last line is what follows
/// the last line break, so a final newline is kept as it was.
pub fn encode<S: AsRef<str>>(lines: impl IntoIterator<Item = S>, format: Format) -> Vec<u8> {
    let mut out = String::new();
    if format.bom {
        out.push(BOM);
    }
    for (i, line) in lines.into_iter().enumerate() {
        if i > 0 {
            out.push_str(format.eol.as_str());
        }
        out.push_str(line.as_ref());
    }
    out.into_bytes()
}

/// The column of byte offset `byte` in `line`, counted in characters.
pub fn char_column(line: &str, byte: usize) -> usize {
    let mut end = byte.min(line.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format_of(text: &str) -> Format {
        decode(text.as_bytes().to_vec()).unwrap().format
    }

    #[test]
    fn line_endings_follow_most_lines() {
        assert_eq!(format_of("a\nb\n").eol, Eol::Lf);
        assert_eq!(format_of("a\r\nb\r\n").eol, Eol::Crlf);
        assert_eq!(format_of("a\r\nb\r\nc\n").eol, Eol::Crlf, "mixed: the usual one");
        assert_eq!(format_of("one line").eol, Eol::Lf);
        assert_eq!(format_of("").eol, Eol::Lf);
    }

    #[test]
    fn a_byte_order_mark_is_kept_aside() {
        let d = decode("\u{feff}x\r\n".as_bytes().to_vec()).unwrap();
        assert_eq!(d.text, "x\r\n");
        assert_eq!(d.format, Format { bom: true, eol: Eol::Crlf });
        assert!(!format_of("x").bom);
    }

    #[test]
    fn what_is_not_editable_text_is_refused() {
        assert!(decode(b"ab\0cd".to_vec()).unwrap_err().contains("binary"));
        assert!(decode(vec![b'a', 0xff, 0xfe]).unwrap_err().contains("UTF-8"));
        assert!(decode(b"a\rb\r\n".to_vec()).unwrap_err().contains("lone CR"));
        assert!(decode(b"ends with\r".to_vec()).is_err());
        assert!(decode(vec![b'a'; MAX_SIZE as usize + 1]).unwrap_err().contains("larger"));
        // A NUL past the part looked at isn't noticed (git does the same).
        let mut late = vec![b'a'; BINARY_PROBE + 10];
        late[BINARY_PROBE + 5] = 0;
        assert!(decode(late).is_ok());
    }

    #[test]
    fn reading_checks_the_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_file(dir.path()).unwrap_err().contains("not a file"));
        assert!(read_file(&dir.path().join("gone")).is_err());
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "hi\n").unwrap();
        assert_eq!(read_file(&file).unwrap().text, "hi\n");
    }

    #[test]
    fn encoding_ends_every_line_the_files_way() {
        let lf = Format { bom: false, eol: Eol::Lf };
        assert_eq!(encode(["a", "b", ""], lf), b"a\nb\n");
        assert_eq!(encode(["a", "b"], lf), b"a\nb", "no final newline: none added");
        assert_eq!(encode([""], lf), b"");
        let crlf_bom = Format { bom: true, eol: Eol::Crlf };
        assert_eq!(encode(["x", ""], crlf_bom), "\u{feff}x\r\n".as_bytes());
    }

    #[test]
    fn saving_writes_back_what_was_read() {
        use iced::widget::text_editor::{Action, Content, Edit, Motion};
        let lines = |c: &Content| c.lines().map(|l| l.text.into_owned()).collect::<Vec<_>>();
        for original in ["fn main() {\r\n    x\r\n}\r\n", "a\nb", "\u{feff}bom\n", ""] {
            let d = decode(original.as_bytes().to_vec()).unwrap();
            let content = Content::with_text(&d.text);
            assert_eq!(encode(lines(&content), d.format), original.as_bytes(), "{original:?}");
        }
        // A line added in a CRLF file ends with CRLF too.
        let d = decode(b"a\r\nb\r\n".to_vec()).unwrap();
        let mut content = Content::with_text(&d.text);
        content.perform(Action::Move(Motion::DocumentStart));
        content.perform(Action::Move(Motion::End));
        content.perform(Action::Edit(Edit::Enter));
        content.perform(Action::Edit(Edit::Insert('n')));
        assert_eq!(encode(lines(&content), d.format), b"a\r\nn\r\nb\r\n");
    }

    #[test]
    fn indent_follows_the_file() {
        assert_eq!(detect_indent("fn a() {\n    x;\n        y;\n}\n"), Indent::Spaces(4));
        assert_eq!(detect_indent("a:\n  b:\n    c\n"), Indent::Spaces(2));
        assert_eq!(detect_indent("{\n\tx\n\t\ty\n}\n"), Indent::Tabs);
        assert_eq!(detect_indent("/**\n * doc\n */\n"), Indent::Spaces(4), "one space is alignment");
        assert_eq!(detect_indent("plain\n"), Indent::Spaces(4));
    }

    #[test]
    fn tab_goes_to_the_next_stop_and_shift_tab_back_one_level() {
        assert_eq!(Indent::Spaces(4).unit(0), "    ");
        assert_eq!(Indent::Spaces(4).unit(6), "  ");
        assert_eq!(Indent::Tabs.unit(3), "\t");
        assert_eq!(Indent::Spaces(4).outdent_len("      x"), 4);
        assert_eq!(Indent::Spaces(4).outdent_len("  x"), 2);
        assert_eq!(Indent::Spaces(2).outdent_len("\tx"), 1);
        assert_eq!(Indent::Tabs.outdent_len("x"), 0);
    }

    #[test]
    fn columns_count_characters() {
        assert_eq!(char_column("héllo", 3), 2, "é is two bytes");
        assert_eq!(char_column("héllo", 2), 1, "inside é: before it");
        assert_eq!(char_column("abc", 99), 3);
    }
}
