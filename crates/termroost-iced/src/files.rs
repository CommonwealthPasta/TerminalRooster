//! The Files tab of the right panel: a tree of the visible session's folder.
//!
//! Folders are read when they are opened, on a thread (the folder may be on a
//! slow network drive), and kept by their full path, so sessions in the same
//! folder share what was read.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use iced::widget::{button, column, row, scrollable, space, text};
use iced::{Alignment, Element, Length, Task};

use crate::app::{App, Message};
use crate::side_panel::SideTab;
use crate::sidebar::{muted_text, short_dir};
use crate::theme;

/// Entries shown per folder; the rest are counted.
const MAX_ENTRIES: usize = 1000;
/// Indent per folder level, in pixels.
const INDENT: f32 = 14.0;

/// One file or folder in a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    Ready(Vec<Entry>),
    Failed(String),
}

#[derive(Debug, Default)]
pub struct FilesState {
    /// Folders shown open, by full path. The root is always open.
    expanded: HashSet<PathBuf>,
    /// What was read of each folder.
    listings: HashMap<PathBuf, Listing>,
    /// Folders being read right now.
    reading: HashSet<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum FilesMsg {
    /// Open or close a folder.
    Toggle(PathBuf),
    /// Folders were read.
    Listed(Vec<(PathBuf, Result<Vec<Entry>, String>)>),
}

/// The files and folders in `dir`: folders first, then by name ignoring case.
/// `.git` (a folder, or a file in a linked worktree) is left out.
pub fn list_dir(dir: &Path) -> Result<Vec<Entry>, String> {
    let read = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut entries: Vec<Entry> = read
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != ".git")
        .map(|e| {
            let path = e.path();
            // The listing knows the type for free; only links need a look at their target.
            let is_dir = match e.file_type() {
                Ok(t) if t.is_symlink() => path.is_dir(),
                Ok(t) => t.is_dir(),
                Err(_) => false,
            };
            Entry { name: e.file_name().to_string_lossy().into_owned(), path, is_dir }
        })
        .collect();
    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase(), e.name.clone()));
    Ok(entries)
}

/// A line of the tree as shown.
#[derive(Debug, PartialEq, Eq)]
pub enum Row<'a> {
    Entry {
        entry: &'a Entry,
        depth: usize,
        open: bool,
    },
    /// "Loading…", an error, or how many entries are not shown.
    Note {
        text: String,
        depth: usize,
    },
}

/// The tree under `root` as lines: each open folder's listing below it.
pub fn rows<'a>(root: &Path, state: &'a FilesState) -> Vec<Row<'a>> {
    let mut out = Vec::new();
    push_rows(root, 0, state, &mut out);
    out
}

fn push_rows<'a>(dir: &Path, depth: usize, state: &'a FilesState, out: &mut Vec<Row<'a>>) {
    match state.listings.get(dir) {
        None => out.push(Row::Note { text: "Loading…".into(), depth }),
        Some(Listing::Failed(e)) => out.push(Row::Note { text: format!("Can't read this folder: {e}"), depth }),
        Some(Listing::Ready(entries)) => {
            if entries.is_empty() && depth == 0 {
                out.push(Row::Note { text: "This folder is empty.".into(), depth });
            }
            for entry in entries.iter().take(MAX_ENTRIES) {
                let open = entry.is_dir && state.expanded.contains(&entry.path);
                out.push(Row::Entry { entry, depth, open });
                if open {
                    push_rows(&entry.path, depth + 1, state, out);
                }
            }
            if entries.len() > MAX_ENTRIES {
                out.push(Row::Note { text: format!("… {} more", entries.len() - MAX_ENTRIES), depth });
            }
        }
    }
}

impl App {
    /// The folder the Files tab shows: the visible session's.
    pub(crate) fn files_root(&self) -> Option<PathBuf> {
        let s = self.core.state();
        match s.active_tab() {
            Some(tab) => Some(s.tab_working_dir(tab)),
            None => s.selected_project.and_then(|p| s.project(p)).map(|p| p.working_dir.clone()),
        }
    }

    fn files_shown(&self) -> bool {
        self.prefs.panel_open && self.prefs.side_tab == SideTab::Files && self.settings.is_none()
    }

    /// Reads the shown root folder if it hasn't been read yet (a new session,
    /// or one that moved to another folder).
    pub(crate) fn ensure_files_root(&mut self) -> Option<Task<Message>> {
        if !self.files_shown() {
            return None;
        }
        let root = self.files_root()?;
        if self.files.listings.contains_key(&root) || self.files.reading.contains(&root) {
            return None;
        }
        Some(self.read_dirs(vec![root]))
    }

    /// Reads folders on a thread; those already being read are skipped.
    fn read_dirs(&mut self, dirs: Vec<PathBuf>) -> Task<Message> {
        let dirs: Vec<PathBuf> = dirs.into_iter().filter(|d| self.files.reading.insert(d.clone())).collect();
        if dirs.is_empty() {
            return Task::none();
        }
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let listed = dirs.into_iter().map(|d| {
                let listing = list_dir(&d);
                (d, listing)
            });
            let _ = tx.send_blocking(listed.collect());
        });
        Task::perform(async move { rx.recv().await.unwrap_or_default() }, |l| Message::Files(FilesMsg::Listed(l)))
    }

    pub(crate) fn update_files(&mut self, msg: FilesMsg) -> Task<Message> {
        match msg {
            FilesMsg::Toggle(dir) => {
                let read = if self.files.expanded.remove(&dir) {
                    Task::none()
                } else {
                    self.files.expanded.insert(dir.clone());
                    if self.files.listings.contains_key(&dir) { Task::none() } else { self.read_dirs(vec![dir]) }
                };
                // The click took the keyboard from the terminal.
                Task::batch([read, self.focus_active()])
            }
            FilesMsg::Listed(listed) => {
                for (dir, result) in listed {
                    self.files.reading.remove(&dir);
                    let listing = match result {
                        Ok(entries) => Listing::Ready(entries),
                        Err(e) => Listing::Failed(e),
                    };
                    self.files.listings.insert(dir, listing);
                }
                Task::none()
            }
        }
    }

    /// The Files tab of the right panel.
    pub(crate) fn view_files(&self) -> Element<'_, Message> {
        let Some(root) = self.files_root() else { return space::horizontal().width(0).into() };
        let lines = rows(&root, &self.files).into_iter().map(file_row);
        column![
            muted_text(short_dir(&root, 44)).wrapping(text::Wrapping::None),
            scrollable(column(lines).width(Length::Fill)).height(Length::Fill),
        ]
        .spacing(8)
        .into()
    }
}

fn file_row(r: Row<'_>) -> Element<'_, Message> {
    match r {
        Row::Entry { entry, depth, open } => {
            let arrow = match (entry.is_dir, open) {
                (true, true) => "▾",
                (true, false) => "▸",
                (false, _) => "",
            };
            let label = row![
                space::horizontal().width(depth as f32 * INDENT),
                text(arrow).size(11).width(12),
                text(&entry.name).size(13).wrapping(text::Wrapping::None),
            ]
            .spacing(2)
            .align_y(Alignment::Center);
            let toggle = entry.is_dir.then(|| Message::Files(FilesMsg::Toggle(entry.path.clone())));
            button(label).width(Length::Fill).padding([2, 4]).style(theme::tree_row).on_press_maybe(toggle).into()
        }
        Row::Note { text: note, depth } => {
            row![space::horizontal().width(depth as f32 * INDENT + 18.0), muted_text(note)].padding([2, 4]).into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn folders_come_first_then_names_ignoring_case() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["b.txt", "A.txt", ".gitignore"] {
            std::fs::write(dir.path().join(f), "").unwrap();
        }
        for d in ["zeta", "Alpha", ".git"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        let entries = list_dir(dir.path()).unwrap();
        assert_eq!(names(&entries), ["Alpha", "zeta", ".gitignore", "A.txt", "b.txt"], ".git is left out");
        assert!(entries[0].is_dir && !entries[2].is_dir);
        assert_eq!(entries[0].path, dir.path().join("Alpha"));
    }

    #[test]
    fn a_git_file_of_a_linked_worktree_is_left_out() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".git"), "gitdir: elsewhere").unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        assert_eq!(names(&list_dir(dir.path()).unwrap()), ["main.rs"]);
    }

    #[test]
    fn a_missing_folder_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_dir(&dir.path().join("gone")).is_err());
    }

    fn entry(path: &str, is_dir: bool) -> Entry {
        let path = PathBuf::from(path);
        Entry { name: path.file_name().unwrap().to_string_lossy().into_owned(), path, is_dir }
    }

    /// Each line as `depth:text`, `▸`/`▾` marking closed and open folders.
    fn lines(root: &str, state: &FilesState) -> Vec<String> {
        rows(Path::new(root), state)
            .into_iter()
            .map(|r| match r {
                Row::Entry { entry, depth, open } => {
                    let mark = match (entry.is_dir, open) {
                        (true, true) => "▾",
                        (true, false) => "▸",
                        _ => "",
                    };
                    format!("{depth}:{mark}{}", entry.name)
                }
                Row::Note { text, depth } => format!("{depth}:({text})"),
            })
            .collect()
    }

    #[test]
    fn open_folders_show_their_listing_below_them() {
        let mut state = FilesState::default();
        state.listings.insert(
            "/r".into(),
            Listing::Ready(vec![entry("/r/src", true), entry("/r/docs", true), entry("/r/x.md", false)]),
        );
        state.listings.insert("/r/src".into(), Listing::Ready(vec![entry("/r/src/lib", true)]));
        state.expanded.insert("/r/src".into());
        state.expanded.insert("/r/src/lib".into());
        state.expanded.insert("/r/x.md".into());
        assert_eq!(
            lines("/r", &state),
            ["0:▾src", "1:▾lib", "2:(Loading…)", "0:▸docs", "0:x.md"],
            "an open folder not read yet says so; files never open"
        );
    }

    #[test]
    fn errors_empty_folders_and_long_listings_are_noted() {
        let mut state = FilesState::default();
        assert_eq!(lines("/r", &state), ["0:(Loading…)"]);
        state.listings.insert("/r".into(), Listing::Failed("denied".into()));
        assert_eq!(lines("/r", &state), ["0:(Can't read this folder: denied)"]);
        state.listings.insert("/r".into(), Listing::Ready(Vec::new()));
        assert_eq!(lines("/r", &state), ["0:(This folder is empty.)"]);
        let many = (0..MAX_ENTRIES + 5).map(|i| entry(&format!("/r/f{i}"), false)).collect();
        state.listings.insert("/r".into(), Listing::Ready(many));
        let shown = lines("/r", &state);
        assert_eq!(shown.len(), MAX_ENTRIES + 1);
        assert_eq!(shown.last().unwrap(), "0:(… 5 more)");
    }
}
