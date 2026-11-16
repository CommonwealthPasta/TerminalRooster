//! Files opened from the Files tab. Each session has its own open files, and
//! one of them can be shown in place of the session's terminals.

mod keys;
mod text;
mod view;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::Task;
use iced::widget::operation;
use iced::widget::text_editor::{self, Action, Cursor, Edit, Position};
use termroost_core::TabId;

use crate::app::{App, Dialog, Message};
pub use text::{Decoded, Format, Indent};

/// Widget id of the file editor, for focusing it.
pub const FILE_EDITOR: &str = "file-editor";

/// An open file, unique across sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocId(u64);

/// The files a session has open.
#[derive(Debug, Default)]
pub struct SessionFiles {
    pub open: Vec<OpenFile>,
    /// The file shown in place of the terminals; `None` shows the terminals.
    pub shown: Option<DocId>,
}

#[derive(Debug)]
pub struct OpenFile {
    pub id: DocId,
    pub path: PathBuf,
    pub kind: DocKind,
}

impl OpenFile {
    /// It has changes not written to disk yet.
    pub fn modified(&self) -> bool {
        matches!(&self.kind, DocKind::Text(t) if t.modified())
    }
}

#[derive(Debug)]
pub enum DocKind {
    /// Being read.
    Loading,
    Text(Box<TextDoc>),
}

/// A text file in the built-in editor.
pub struct TextDoc {
    pub content: text_editor::Content,
    pub format: Format,
    pub indent: Indent,
    /// Changes the text so far, as a number unique to this state of it.
    pub version: u64,
    /// The `version` on disk.
    pub saved_version: u64,
    /// Being written now.
    pub saving: bool,
    /// Close it once it is written (Save in the close dialog).
    close_after_save: bool,
}

impl TextDoc {
    pub fn modified(&self) -> bool {
        self.version != self.saved_version
    }
}

impl std::fmt::Debug for TextDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the text: logs must not show what is in people's files.
        f.debug_struct("TextDoc")
            .field("lines", &self.content.line_count())
            .field("format", &self.format)
            .field("version", &self.version)
            .field("saved_version", &self.saved_version)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum EditorMsg {
    /// Show one of the visible session's files, or its terminals (`None`).
    Show(Option<DocId>),
    /// Close a file, asking first if it has unsaved changes.
    Close(DocId),
    /// Close a file without saving its changes.
    Discard(DocId),
    /// A file was read (or can't be edited, and why).
    Loaded(DocId, Result<Decoded, String>),
    /// Something done in the editor: typing, moving, selecting, scrolling.
    Action(DocId, text_editor::Action),
    /// Tab (`false`) or Shift+Tab (`true`).
    Indent(DocId, bool),
    Save(DocId),
    /// A file was written as it was at that version.
    Saved(DocId, u64, Result<(), String>),
}

impl EditorMsg {
    /// Whether the message may be logged: some carry what is in a file or
    /// what the user types.
    pub fn loggable(&self) -> bool {
        !matches!(self, EditorMsg::Loaded(..) | EditorMsg::Action(..))
    }
}

impl App {
    /// The open files of the visible session.
    pub(crate) fn session_files(&self) -> Option<&SessionFiles> {
        self.editors.get(&self.active_tab_id()?)
    }

    fn session_files_mut(&mut self) -> Option<&mut SessionFiles> {
        let tab = self.active_tab_id()?;
        self.editors.get_mut(&tab)
    }

    /// The file shown in place of the visible session's terminals.
    pub(crate) fn shown_doc(&self) -> Option<&OpenFile> {
        let files = self.session_files()?;
        let id = files.shown?;
        files.open.iter().find(|d| d.id == id)
    }

    /// How a file of session `tab` is named to the user: its path inside the
    /// session's folder, else its whole path.
    pub(crate) fn file_label(&self, tab: Option<TabId>, path: &Path) -> String {
        let s = self.core.state();
        let root = tab.and_then(|t| s.tab(t)).map(|t| s.tab_working_dir(t));
        match root.and_then(|root| path.strip_prefix(root).ok().map(Path::to_path_buf)) {
            Some(rel) => rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
            None => crate::sidebar::display_dir(path),
        }
    }

    /// [`App::file_label`] for a file of the visible session.
    pub(crate) fn doc_label(&self, path: &Path) -> String {
        self.file_label(self.active_tab_id(), path)
    }

    /// Shows the visible session's terminals instead of a file.
    pub(crate) fn show_terminals(&mut self) {
        if let Some(files) = self.session_files_mut() {
            files.shown = None;
        }
    }

    fn doc(&self, id: DocId) -> Option<(TabId, &OpenFile)> {
        self.editors.iter().find_map(|(tab, f)| Some((*tab, f.open.iter().find(|d| d.id == id)?)))
    }

    fn doc_mut(&mut self, id: DocId) -> Option<&mut OpenFile> {
        self.editors.values_mut().flat_map(|f| f.open.iter_mut()).find(|d| d.id == id)
    }

    fn text_mut(&mut self, id: DocId) -> Option<&mut TextDoc> {
        match &mut self.doc_mut(id)?.kind {
            DocKind::Text(t) => Some(t),
            DocKind::Loading => None,
        }
    }

    /// Files with unsaved changes, named for the quit dialog: "src/main.rs (agents)".
    pub(crate) fn unsaved_files(&self) -> Vec<String> {
        let s = self.core.state();
        // In the order of the sessions list.
        s.tabs
            .iter()
            .filter_map(|t| Some((t, self.editors.get(&t.id)?)))
            .flat_map(|(t, files)| {
                files
                    .open
                    .iter()
                    .filter(|d| d.modified())
                    .map(|d| format!("{} ({})", self.file_label(Some(t.id), &d.path), t.title))
            })
            .collect()
    }

    /// How many files of session `tab` have unsaved changes.
    pub(crate) fn unsaved_in(&self, tab: TabId) -> usize {
        self.editors.get(&tab).map_or(0, |f| f.open.iter().filter(|d| d.modified()).count())
    }

    /// Moves the keyboard to the shown file, if there is one. `None` when
    /// the terminals are shown.
    pub(crate) fn focus_doc(&self) -> Option<Task<Message>> {
        Some(match self.shown_doc()?.kind {
            DocKind::Text(_) => operation::focus(FILE_EDITOR),
            DocKind::Loading => Task::none(),
        })
    }

    /// Opens a file in the visible session (or shows it, if it is open
    /// already), reading it on a thread.
    pub(crate) fn open_file(&mut self, path: PathBuf) -> Task<Message> {
        let Some(tab) = self.active_tab_id() else {
            self.error = Some("Files open in place of a session's terminals: start a session first.".into());
            return Task::none();
        };
        let files = self.editors.entry(tab).or_default();
        if let Some(doc) = files.open.iter().find(|d| d.path == path) {
            files.shown = Some(doc.id);
            return self.focus_active();
        }
        self.next_doc += 1;
        let id = DocId(self.next_doc);
        files.open.push(OpenFile { id, path: path.clone(), kind: DocKind::Loading });
        files.shown = Some(id);
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(text::read_file(&path));
        });
        Task::perform(async move { rx.recv().await.unwrap_or_else(|_| Err("reading it stopped".into())) }, move |r| {
            Message::Editor(EditorMsg::Loaded(id, r))
        })
    }

    /// Closes a file; its session shows the file next to it, else its terminals.
    fn close_doc(&mut self, id: DocId) {
        for files in self.editors.values_mut() {
            let Some(i) = files.open.iter().position(|d| d.id == id) else { continue };
            files.open.remove(i);
            if files.shown == Some(id) {
                files.shown = files.open.get(i).or_else(|| files.open.last()).map(|d| d.id);
            }
            return;
        }
    }

    /// Writes a file with changes on a thread; `then_close` closes it after.
    pub(crate) fn save_doc(&mut self, id: DocId, then_close: bool) -> Task<Message> {
        let Some(OpenFile { path, kind: DocKind::Text(doc), .. }) = self.doc_mut(id) else { return Task::none() };
        doc.close_after_save |= then_close;
        if doc.saving {
            return Task::none();
        }
        if !doc.modified() {
            if doc.close_after_save {
                self.close_doc(id);
                return self.focus_active();
            }
            return Task::none();
        }
        doc.saving = true;
        let bytes = text::encode(doc.content.lines().map(|l| l.text), doc.format);
        let version = doc.version;
        let path = path.clone();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(std::fs::write(&path, bytes).map_err(|e| e.to_string()));
        });
        Task::perform(async move { rx.recv().await.unwrap_or_else(|_| Err("writing it stopped".into())) }, move |r| {
            Message::Editor(EditorMsg::Saved(id, version, r))
        })
    }

    fn next_version(&mut self) -> u64 {
        self.next_version += 1;
        self.next_version
    }

    pub(crate) fn update_editor(&mut self, msg: EditorMsg) -> Task<Message> {
        match msg {
            EditorMsg::Show(id) => {
                if let Some(files) = self.session_files_mut() {
                    files.shown = id.filter(|id| files.open.iter().any(|d| d.id == *id));
                }
                self.focus_active()
            }
            EditorMsg::Close(id) => {
                if let Some((tab, doc)) = self.doc(id).filter(|(_, d)| d.modified()) {
                    let name = self.file_label(Some(tab), &doc.path);
                    self.dialog = Some(Dialog::CloseFile { doc: id, name });
                    return Task::none();
                }
                self.close_doc(id);
                self.focus_active()
            }
            EditorMsg::Discard(id) => {
                self.dialog = None;
                self.close_doc(id);
                self.focus_active()
            }
            EditorMsg::Loaded(id, Ok(decoded)) => {
                let version = self.next_version();
                if let Some(doc) = self.doc_mut(id) {
                    let mut content = text_editor::Content::with_text(&decoded.text);
                    // It starts at the end of the text; files are read from the top.
                    content.perform(Action::Move(text_editor::Motion::DocumentStart));
                    doc.kind = DocKind::Text(Box::new(TextDoc {
                        content,
                        format: decoded.format,
                        indent: text::detect_indent(&decoded.text),
                        version,
                        saved_version: version,
                        saving: false,
                        close_after_save: false,
                    }));
                }
                self.focus_active()
            }
            EditorMsg::Loaded(id, Err(e)) => {
                let name = self.doc(id).map(|(tab, d)| self.file_label(Some(tab), &d.path));
                self.close_doc(id);
                if let Some(name) = name {
                    self.error = Some(format!("Can't open {name}: {e}."));
                }
                self.focus_active()
            }
            EditorMsg::Action(id, action) => {
                let version = if action.is_edit() { Some(self.next_version()) } else { None };
                if let Some(doc) = self.text_mut(id) {
                    doc.content.perform(action);
                    if let Some(v) = version {
                        doc.version = v;
                    }
                }
                Task::none()
            }
            EditorMsg::Indent(id, back) => {
                let version = self.next_version();
                if let Some(doc) = self.text_mut(id)
                    && indent(doc, back)
                {
                    doc.version = version;
                }
                Task::none()
            }
            EditorMsg::Save(id) => self.save_doc(id, false),
            EditorMsg::Saved(id, version, result) => {
                let name = self.doc(id).map(|(tab, d)| self.file_label(Some(tab), &d.path));
                let Some(doc) = self.text_mut(id) else { return Task::none() };
                doc.saving = false;
                match result {
                    Ok(()) => {
                        doc.saved_version = version;
                        if doc.close_after_save {
                            self.close_doc(id);
                            return self.focus_active();
                        }
                    }
                    Err(e) => {
                        doc.close_after_save = false;
                        self.error = Some(format!("Could not save {}: {e}", name.unwrap_or_default()));
                    }
                }
                Task::none()
            }
        }
    }
}

/// Tab inserts the file's indent at the cursor (replacing a selection);
/// Shift+Tab takes one level off the start of the cursor's line. Whether
/// the text changed.
fn indent(doc: &mut TextDoc, back: bool) -> bool {
    let at = doc.content.cursor().position;
    let line = doc.content.line(at.line).map(|l| l.text.into_owned()).unwrap_or_default();
    if !back {
        let unit = doc.indent.unit(text::char_column(&line, at.column));
        doc.content.perform(Action::Edit(Edit::Paste(Arc::new(unit))));
        return true;
    }
    let n = doc.indent.outdent_len(&line);
    if n == 0 {
        return false;
    }
    let start = Position { line: at.line, column: 0 };
    doc.content.move_to(Cursor { position: start, selection: Some(Position { line: at.line, column: n }) });
    doc.content.perform(Action::Edit(Edit::Delete));
    doc.content
        .move_to(Cursor { position: Position { line: at.line, column: at.column.saturating_sub(n) }, selection: None });
    true
}
