//! Files opened from the Files tab. Each session has its own open files, and
//! one of them can be shown in place of the session's terminals.

mod text;
mod view;

use std::path::{Path, PathBuf};

use iced::Task;
use iced::widget::{operation, text_editor};

use crate::app::{App, Message};
pub use text::{Decoded, Format};

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
}

impl std::fmt::Debug for TextDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the text: logs must not show what is in people's files.
        f.debug_struct("TextDoc").field("lines", &self.content.line_count()).field("format", &self.format).finish()
    }
}

#[derive(Clone, Debug)]
pub enum EditorMsg {
    /// Show one of the visible session's files, or its terminals (`None`).
    Show(Option<DocId>),
    Close(DocId),
    /// A file was read (or can't be edited, and why).
    Loaded(DocId, Result<Decoded, String>),
    /// Something done in the editor: typing, moving, selecting, scrolling.
    Action(DocId, text_editor::Action),
}

impl EditorMsg {
    /// Whether the message may be logged: some carry what is in a file or
    /// what the user types.
    pub fn loggable(&self) -> bool {
        matches!(self, EditorMsg::Show(_) | EditorMsg::Close(_))
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

    /// How a file is named to the user: its path inside the visible session's
    /// folder, else its whole path.
    pub(crate) fn doc_label(&self, path: &Path) -> String {
        match self.files_root().and_then(|root| path.strip_prefix(root).ok().map(Path::to_path_buf)) {
            Some(rel) => rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
            None => crate::sidebar::display_dir(path),
        }
    }

    /// Shows the visible session's terminals instead of a file.
    pub(crate) fn show_terminals(&mut self) {
        if let Some(files) = self.session_files_mut() {
            files.shown = None;
        }
    }

    fn doc_mut(&mut self, id: DocId) -> Option<&mut OpenFile> {
        self.editors.values_mut().flat_map(|f| f.open.iter_mut()).find(|d| d.id == id)
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

    pub(crate) fn update_editor(&mut self, msg: EditorMsg) -> Task<Message> {
        match msg {
            EditorMsg::Show(id) => {
                if let Some(files) = self.session_files_mut() {
                    files.shown = id.filter(|id| files.open.iter().any(|d| d.id == *id));
                }
                self.focus_active()
            }
            EditorMsg::Close(id) => {
                self.close_doc(id);
                self.focus_active()
            }
            EditorMsg::Loaded(id, Ok(decoded)) => {
                if let Some(doc) = self.doc_mut(id) {
                    let mut content = text_editor::Content::with_text(&decoded.text);
                    // It starts at the end of the text; files are read from the top.
                    content.perform(text_editor::Action::Move(text_editor::Motion::DocumentStart));
                    doc.kind = DocKind::Text(Box::new(TextDoc { content, format: decoded.format }));
                }
                self.focus_active()
            }
            EditorMsg::Loaded(id, Err(e)) => {
                let path = self.doc_mut(id).map(|d| d.path.clone());
                let name = path.map(|p| self.doc_label(&p));
                self.close_doc(id);
                if let Some(name) = name {
                    self.error = Some(format!("Can't open {name}: {e}."));
                }
                self.focus_active()
            }
            EditorMsg::Action(id, action) => {
                // Only looking for now: no changes.
                if !action.is_edit()
                    && let Some(OpenFile { kind: DocKind::Text(doc), .. }) = self.doc_mut(id)
                {
                    doc.content.perform(action);
                }
                Task::none()
            }
        }
    }
}
