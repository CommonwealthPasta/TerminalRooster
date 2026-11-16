//! The Files tab of the right panel: a tree of the visible session's folder.
//!
//! Folders are read when they are opened, on a thread (the folder may be on a
//! slow network drive), and kept by their full path, so sessions in the same
//! folder share what was read.
//!
//! While the tab is on screen, the folders shown are read again every few
//! seconds, since agents add and remove files all the time.
//!
//! What git leaves out (`.gitignore`, `.git/info/exclude`, the global
//! excludes file) is dimmed, along with everything inside it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use iced::widget::{button, column, row, scrollable, space, text};
use iced::{Alignment, Element, Length, Task};

use crate::app::{App, Message};
use crate::side_panel::SideTab;
use crate::sidebar::{icon_button, muted_text, short_dir};
use crate::theme;

/// Entries shown per folder; the rest are counted.
const MAX_ENTRIES: usize = 1000;
/// Indent per folder level, in pixels.
const INDENT: f32 = 14.0;
/// How often the folders shown are read again.
pub const POLL: Duration = Duration::from_secs(3);

/// One file or folder in a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    /// Git's ignore rules leave it out.
    pub ignored: bool,
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
    /// Open a file in place of the terminals.
    Open(PathBuf),
    /// Folders were read.
    Listed(Vec<(PathBuf, Result<Vec<Entry>, String>)>),
    /// Time to read the folders shown again.
    Poll,
    /// The refresh button.
    Refresh,
}

/// The files and folders in `dir`: folders first, then by name ignoring case.
/// `.git` (a folder, or a file in a linked worktree) is left out.
pub fn list_dir(dir: &Path) -> Result<Vec<Entry>, String> {
    let read = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    // What a walk that follows git's rules (as git does: only inside a
    // repository) still lists; the rest is ignored. Hidden files count too.
    let kept: HashSet<PathBuf> = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .ignore(false)
        .parents(true)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .map(ignore::DirEntry::into_path)
        .collect();
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
            let ignored = !kept.contains(&path);
            Entry { name: e.file_name().to_string_lossy().into_owned(), path, is_dir, ignored }
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
        /// Ignored by git, itself or as part of an ignored folder.
        dim: bool,
    },
    /// "Loading…", an error, or how many entries are not shown.
    Note { text: String, depth: usize },
}

/// The tree under `root` as lines: each open folder's listing below it.
pub fn rows<'a>(root: &Path, state: &'a FilesState) -> Vec<Row<'a>> {
    let mut out = Vec::new();
    push_rows(root, 0, false, state, &mut out);
    out
}

/// `dir` and the open folders shown below it, which are what a refresh reads.
pub fn shown_dirs(dir: &Path, state: &FilesState) -> Vec<PathBuf> {
    let open = rows(dir, state).into_iter().filter_map(|r| match r {
        Row::Entry { entry, open: true, .. } => Some(entry.path.clone()),
        _ => None,
    });
    std::iter::once(dir.to_path_buf()).chain(open).collect()
}

/// `dim`: `dir` is ignored. Git's rules don't say so of the things inside an
/// ignored folder (`target/` doesn't match `target/debug`), so they inherit it.
fn push_rows<'a>(dir: &Path, depth: usize, dim: bool, state: &'a FilesState, out: &mut Vec<Row<'a>>) {
    match state.listings.get(dir) {
        None => out.push(Row::Note { text: "Loading…".into(), depth }),
        Some(Listing::Failed(e)) => out.push(Row::Note { text: format!("Can't read this folder: {e}"), depth }),
        Some(Listing::Ready(entries)) => {
            if entries.is_empty() && depth == 0 {
                out.push(Row::Note { text: "This folder is empty.".into(), depth });
            }
            for entry in entries.iter().take(MAX_ENTRIES) {
                let open = entry.is_dir && state.expanded.contains(&entry.path);
                let dim = dim || entry.ignored;
                out.push(Row::Entry { entry, depth, open, dim });
                if open {
                    push_rows(&entry.path, depth + 1, dim, state, out);
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

    /// The Files tab is on screen.
    pub(crate) fn files_shown(&self) -> bool {
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

    /// Reads the folders the Files tab shows again, if it is on screen.
    pub(crate) fn refresh_files(&mut self) -> Task<Message> {
        match self.files_root().filter(|_| self.files_shown()) {
            Some(root) => self.read_dirs(shown_dirs(&root, &self.files)),
            None => Task::none(),
        }
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
                    // What was read before may be old: read it again, with the
                    // folders inside it that are still open.
                    self.read_dirs(shown_dirs(&dir, &self.files))
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
            FilesMsg::Open(path) => self.open_file(path),
            FilesMsg::Poll => self.refresh_files(),
            FilesMsg::Refresh => Task::batch([self.refresh_files(), self.focus_active()]),
        }
    }

    /// The Files tab of the right panel.
    pub(crate) fn view_files(&self) -> Element<'_, Message> {
        let Some(root) = self.files_root() else { return space::horizontal().width(0).into() };
        let lines = rows(&root, &self.files).into_iter().map(file_row);
        let top = row![
            muted_text(short_dir(&root, 40)).wrapping(text::Wrapping::None).width(Length::Fill),
            icon_button("↻", Message::Files(FilesMsg::Refresh), "Read the folders again"),
        ]
        .align_y(Alignment::Center);
        column![top, scrollable(column(lines).width(Length::Fill)).height(Length::Fill),].spacing(8).into()
    }
}

fn file_row(r: Row<'_>) -> Element<'_, Message> {
    match r {
        Row::Entry { entry, depth, open, dim } => {
            let arrow = match (entry.is_dir, open) {
                (true, true) => "▾",
                (true, false) => "▸",
                (false, _) => "",
            };
            let style = move |t: &iced::Theme| text::Style { color: dim.then(|| theme::muted(t)) };
            let label = row![
                space::horizontal().width(depth as f32 * INDENT),
                text(arrow).size(11).width(12).style(style),
                text(&entry.name).size(13).wrapping(text::Wrapping::None).style(style),
            ]
            .spacing(2)
            .align_y(Alignment::Center);
            let press =
                if entry.is_dir { FilesMsg::Toggle(entry.path.clone()) } else { FilesMsg::Open(entry.path.clone()) };
            button(label)
                .width(Length::Fill)
                .padding([2, 4])
                .style(theme::tree_row)
                .on_press(Message::Files(press))
                .into()
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

    /// What `list_dir` says of each entry: its name, with `~` when it is ignored.
    fn marked(dir: &Path) -> Vec<String> {
        list_dir(dir).unwrap().iter().map(|e| format!("{}{}", e.name, if e.ignored { "~" } else { "" })).collect()
    }

    #[test]
    fn what_git_ignores_is_marked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        for f in ["main.rs", "build.log", ".env"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join("src/gen")).unwrap();
        std::fs::write(root.join("src/.gitignore"), "gen/\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();

        assert_eq!(marked(root), ["src", "target~", ".env", ".gitignore", "build.log~", "main.rs"]);
        assert_eq!(marked(&root.join("src")), ["gen~", ".gitignore", "lib.rs"], "rules of the folder itself");
        // Rules from folders above count too; the inside of an ignored folder
        // isn't matched by them (rows() dims it).
        std::fs::write(root.join("src/notes.log"), "").unwrap();
        assert_eq!(marked(&root.join("src")), ["gen~", ".gitignore", "lib.rs", "notes.log~"]);
        assert_eq!(marked(&root.join("target")), ["debug"]);
    }

    #[test]
    fn outside_a_repository_gitignore_does_not_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(dir.path().join("a.log"), "").unwrap();
        assert_eq!(marked(dir.path()), [".gitignore", "a.log"], "as in git");
    }

    fn entry(path: &str, is_dir: bool) -> Entry {
        let path = PathBuf::from(path);
        Entry { name: path.file_name().unwrap().to_string_lossy().into_owned(), path, is_dir, ignored: false }
    }

    /// Each line as `depth:text`, `▸`/`▾` marking closed and open folders and
    /// `~` dimmed entries.
    fn lines(root: &str, state: &FilesState) -> Vec<String> {
        rows(Path::new(root), state)
            .into_iter()
            .map(|r| match r {
                Row::Entry { entry, depth, open, dim } => {
                    let mark = match (entry.is_dir, open) {
                        (true, true) => "▾",
                        (true, false) => "▸",
                        _ => "",
                    };
                    format!("{depth}:{mark}{}{}", entry.name, if dim { "~" } else { "" })
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
    fn a_refresh_reads_the_open_folders_on_screen() {
        let mut state = FilesState::default();
        state.listings.insert("/r".into(), Listing::Ready(vec![entry("/r/a", true), entry("/r/b", true)]));
        state.listings.insert("/r/a".into(), Listing::Ready(vec![entry("/r/a/x", true)]));
        state.listings.insert("/r/b".into(), Listing::Ready(vec![entry("/r/b/y", true)]));
        // b/y is open but b isn't, so it isn't on screen; a/x is, but not read yet.
        state.expanded.extend(["/r/a".into(), "/r/a/x".into(), "/r/b/y".into()]);
        let dirs: Vec<PathBuf> = ["/r", "/r/a", "/r/a/x"].into_iter().map(PathBuf::from).collect();
        assert_eq!(shown_dirs(Path::new("/r"), &state), dirs);
        assert_eq!(shown_dirs(Path::new("/r/a"), &state), dirs[1..]);
    }

    #[test]
    fn the_inside_of_an_ignored_folder_is_dimmed_too() {
        let mut state = FilesState::default();
        let ignored = Entry { ignored: true, ..entry("/r/target", true) };
        state.listings.insert("/r".into(), Listing::Ready(vec![entry("/r/src", true), ignored]));
        state.listings.insert("/r/target".into(), Listing::Ready(vec![entry("/r/target/debug", true)]));
        state.listings.insert("/r/target/debug".into(), Listing::Ready(vec![entry("/r/target/debug/app", false)]));
        state.expanded.extend(["/r/target".into(), "/r/target/debug".into()]);
        assert_eq!(lines("/r", &state), ["0:▸src", "0:▾target~", "1:▾debug~", "2:app~"]);
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
