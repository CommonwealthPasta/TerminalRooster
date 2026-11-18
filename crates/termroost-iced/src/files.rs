//! The Files tab of the right panel: a tree of the visible session's folder.
//!
//! Folders are read when they are opened, on a thread (the folder may be on a
//! slow network drive), and kept by their full path, so sessions in the same
//! folder share what was read.
//!
//! While the tab is on screen, the folders shown are watched (see
//! [`folder_watch`]) and read again when they change, since agents add and
//! remove files all the time. Folders that can't be watched are read again
//! every few seconds instead.
//!
//! Right-clicking an entry (or the folder at the top) opens a small menu
//! under it, to show the entry in the system's file manager.
//!
//! What git leaves out (`.gitignore`, `.git/info/exclude`, the global
//! excludes file) is dimmed, along with everything inside it.
//!
//! Each entry has an icon for its kind of file (see [`file_icons`]).

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use iced::widget::{button, column, container, mouse_area, operation, row, space, text};
use iced::{Alignment, Element, Length, Task};

use crate::app::{App, Message};
use crate::file_icons::{self, Hues};
use crate::folder_watch::{self, Changes, FolderWatch};
use crate::icons::{Icon, icon};
use crate::reveal;
use crate::side_panel::SideTab;
use crate::sidebar::{muted_text, short_dir};
use crate::theme::{self, Tone};
use crate::ui::{self, icon_button};

/// Entries shown per folder; the rest are counted.
const MAX_ENTRIES: usize = 1000;
/// Indent per folder level, in pixels.
const INDENT: f32 = 14.0;
/// The width of a folder's chevron, and the space files leave for it.
const CHEVRON: f32 = 12.0;
/// The size of an entry's icon.
const ICON: f32 = 14.0;
/// The space between a chevron, an icon and a name.
const GAP: f32 = 4.0;
/// How often the folders shown are read again when they can't be watched.
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
    /// Folders that changed while being read: read again once that's done.
    again: HashSet<PathBuf>,
    /// The entry whose menu is open (it was right-clicked).
    menu: Option<PathBuf>,
    /// A file being named, shown at the top of its folder.
    new_file: Option<NewFile>,
    /// Hears of changes in the folders shown.
    pub(crate) watch: FolderWatch,
}

/// A new file, until it has a name.
#[derive(Debug, PartialEq, Eq)]
pub struct NewFile {
    /// The folder it goes in.
    dir: PathBuf,
    name: String,
    /// Why it could not be created.
    error: Option<String>,
    /// It is being created.
    creating: bool,
}

#[derive(Clone, Debug)]
pub enum FilesMsg {
    /// Open or close a folder.
    Toggle(PathBuf),
    /// Open a file in place of the terminals.
    Open(PathBuf),
    /// Folders were read.
    Listed(Vec<(PathBuf, Result<Vec<Entry>, String>)>),
    /// Something changed in the folders watched.
    Changed(Changes),
    /// Time to read the folders shown again (they can't all be watched).
    Poll,
    /// An entry was right-clicked: open its menu (or close it, if open).
    Menu(PathBuf),
    /// Show a file, or open a folder, in the system's file manager.
    Reveal {
        path: PathBuf,
        is_dir: bool,
    },
    /// The file manager was started, or why not.
    Revealed(Result<(), String>),
    /// Name a new file in this folder.
    NewFile(PathBuf),
    NewFileName(String),
    /// Enter in the name.
    CreateFile,
    /// The file was created in `dir` (and opens), or why not.
    Created {
        dir: PathBuf,
        path: PathBuf,
        result: Result<(), String>,
    },
}

/// The name field of a new file.
const NEW_FILE_INPUT: &str = "new-file-input";

/// Where a file named `name` goes in `dir`. The name may go through folders
/// (`docs/notes.md`), but not out of `dir`.
pub fn new_file_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    let plain = Path::new(name).components().all(|c| matches!(c, Component::Normal(_)));
    if name.is_empty() || !plain || name.ends_with(std::path::is_separator) {
        return Err("Not a file name".into());
    }
    Ok(dir.join(name))
}

/// Creates an empty file at `path`, and the folders it goes in. A file
/// already there is left alone.
pub fn create_file(path: &Path) -> Result<(), String> {
    let exists = || format!("{} already exists", path.file_name().unwrap_or_default().to_string_lossy());
    // Checked first: on Windows, a folder in the way is "access denied".
    if path.symlink_metadata().is_ok() {
        return Err(exists());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(exists()),
        Err(e) => Err(e.to_string()),
    }
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
    /// The menu of the entry above it.
    Menu { entry: &'a Entry, depth: usize },
    /// The name of a new file, at the top of its folder.
    NewFile { new: &'a NewFile, depth: usize },
}

/// The tree under `root` as lines: each open folder's listing below it.
pub fn rows<'a>(root: &Path, state: &'a FilesState) -> Vec<Row<'a>> {
    let mut out = Vec::new();
    push_rows(root, 0, false, state, &mut out);
    out
}

/// `dir` and the open folders shown below it, which are what a refresh reads.
pub fn shown_dirs(dir: &Path, state: &FilesState) -> Vec<PathBuf> {
    let mut out = Vec::new();
    push_shown(dir, state, &mut out);
    out
}

fn push_shown(dir: &Path, state: &FilesState, out: &mut Vec<PathBuf>) {
    out.push(dir.to_path_buf());
    if let Some(Listing::Ready(entries)) = state.listings.get(dir) {
        for entry in entries.iter().take(MAX_ENTRIES) {
            if entry.is_dir && state.expanded.contains(&entry.path) {
                push_shown(&entry.path, state, out);
            }
        }
    }
}

/// The open folders in `new` that `old` (the folder's listing before) didn't
/// have: what they hold was never read, or is from before they went away.
fn appeared(old: Option<&Listing>, new: &[Entry], expanded: &HashSet<PathBuf>) -> Vec<PathBuf> {
    let before = match old {
        Some(Listing::Ready(entries)) => entries.as_slice(),
        _ => &[],
    };
    new.iter()
        .filter(|e| e.is_dir && expanded.contains(&e.path) && !before.iter().any(|b| b.path == e.path))
        .map(|e| e.path.clone())
        .collect()
}

/// `dim`: `dir` is ignored. Git's rules don't say so of the things inside an
/// ignored folder (`target/` doesn't match `target/debug`), so they inherit it.
fn push_rows<'a>(dir: &Path, depth: usize, dim: bool, state: &'a FilesState, out: &mut Vec<Row<'a>>) {
    let new = state.new_file.as_ref().filter(|n| n.dir == dir);
    if let Some(new) = new {
        out.push(Row::NewFile { new, depth });
    }
    match state.listings.get(dir) {
        None => out.push(Row::Note { text: "Loading…".into(), depth }),
        Some(Listing::Failed(e)) => out.push(Row::Note { text: format!("Can't read this folder: {e}"), depth }),
        Some(Listing::Ready(entries)) => {
            if entries.is_empty() && depth == 0 && new.is_none() {
                out.push(Row::Note { text: "This folder is empty.".into(), depth });
            }
            for entry in entries.iter().take(MAX_ENTRIES) {
                let open = entry.is_dir && state.expanded.contains(&entry.path);
                let dim = dim || entry.ignored;
                out.push(Row::Entry { entry, depth, open, dim });
                if state.menu.as_ref() == Some(&entry.path) {
                    out.push(Row::Menu { entry, depth });
                }
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

    /// Closes the menu of an entry, if one is open.
    pub(crate) fn close_files_menu(&mut self) -> bool {
        self.files.menu.take().is_some()
    }

    /// Reads the folders the Files tab shows again, if it is on screen, and
    /// tries again to watch those that couldn't be.
    pub(crate) fn refresh_files(&mut self) -> Task<Message> {
        self.files.watch.retry();
        match self.files_root().filter(|_| self.files_shown()) {
            Some(root) => self.read_dirs(shown_dirs(&root, &self.files)),
            None => Task::none(),
        }
    }

    /// Watches what the Files tab shows, or nothing when it is not on screen.
    pub(crate) fn watch_files(&mut self) {
        let wanted = match self.files_root().filter(|_| self.files_shown()) {
            Some(root) if folder_watch::WHOLE_TREE => HashSet::from([root]),
            Some(root) => shown_dirs(&root, &self.files).into_iter().collect(),
            None => HashSet::new(),
        };
        self.files.watch.set(wanted);
    }

    /// Reads folders on a thread. Those being read already are read again
    /// once that's done, as they may have changed since it started.
    fn read_dirs(&mut self, dirs: Vec<PathBuf>) -> Task<Message> {
        // Watched before they're read, so no change falls in between.
        self.watch_files();
        let (dirs, busy): (Vec<PathBuf>, Vec<PathBuf>) =
            dirs.into_iter().partition(|d| self.files.reading.insert(d.clone()));
        self.files.again.extend(busy);
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
                self.files.menu = None;
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
                let mut next = Vec::new();
                for (dir, result) in listed {
                    self.files.reading.remove(&dir);
                    if self.files.again.remove(&dir) {
                        next.push(dir.clone());
                    }
                    let listing = match result {
                        Ok(entries) => {
                            next.extend(appeared(self.files.listings.get(&dir), &entries, &self.files.expanded));
                            Listing::Ready(entries)
                        }
                        Err(e) => Listing::Failed(e),
                    };
                    self.files.listings.insert(dir, listing);
                }
                self.read_dirs(next)
            }
            FilesMsg::Changed(changes) => match self.files_root().filter(|_| self.files_shown()) {
                Some(root) => {
                    let dirs = shown_dirs(&root, &self.files).into_iter().filter(|d| changes.touches(d)).collect();
                    self.read_dirs(dirs)
                }
                None => Task::none(),
            },
            FilesMsg::Open(path) => {
                self.files.menu = None;
                self.open_file(path)
            }
            FilesMsg::Poll => self.refresh_files(),
            FilesMsg::Menu(path) => {
                self.files.menu = if self.files.menu.as_ref() == Some(&path) { None } else { Some(path) };
                Task::none()
            }
            FilesMsg::Reveal { path, is_dir } => {
                self.files.menu = None;
                let revealed = reveal::in_background(path, is_dir).map(|r| Message::Files(FilesMsg::Revealed(r)));
                Task::batch([revealed, self.focus_active()])
            }
            FilesMsg::Revealed(Ok(())) => Task::none(),
            FilesMsg::Revealed(Err(e)) => {
                self.error = Some(format!("Could not open the file manager: {e}"));
                Task::none()
            }
            FilesMsg::NewFile(dir) => {
                self.files.menu = None;
                // The folder opens, so the name is typed where the file will show.
                let read = match self.files_root() {
                    Some(root) if dir != root && self.files.expanded.insert(dir.clone()) => {
                        self.read_dirs(shown_dirs(&dir, &self.files))
                    }
                    _ => Task::none(),
                };
                self.files.new_file = Some(NewFile { dir, name: String::new(), error: None, creating: false });
                Task::batch([read, operation::focus(NEW_FILE_INPUT)])
            }
            FilesMsg::NewFileName(name) => {
                if let Some(new) = &mut self.files.new_file {
                    new.name = name;
                    new.error = None;
                }
                Task::none()
            }
            FilesMsg::CreateFile => {
                let Some(new) = self.files.new_file.as_mut().filter(|n| !n.creating && !n.name.trim().is_empty())
                else {
                    return Task::none();
                };
                let path = match new_file_path(&new.dir, &new.name) {
                    Ok(path) => path,
                    Err(e) => {
                        new.error = Some(e);
                        return Task::none();
                    }
                };
                new.creating = true;
                let dir = new.dir.clone();
                // On a thread: the folder may be on a slow network drive.
                let (tx, rx) = async_channel::bounded(1);
                let target = path.clone();
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(create_file(&target));
                });
                Task::perform(
                    async move {
                        let result = rx.recv().await.unwrap_or_else(|_| Err("creating it stopped".into()));
                        (dir, path, result)
                    },
                    |(dir, path, result)| Message::Files(FilesMsg::Created { dir, path, result }),
                )
            }
            FilesMsg::Created { dir, path, result: Ok(()) } => {
                self.files.new_file = None;
                // Folders made on the way to it (`docs/` of `docs/notes.md`) open too.
                for folder in path.ancestors().skip(1).take_while(|f| *f != dir) {
                    self.files.expanded.insert(folder.to_path_buf());
                }
                let read = self.read_dirs(vec![dir]);
                Task::batch([read, self.open_file(path)])
            }
            FilesMsg::Created { result: Err(e), .. } => {
                if let Some(new) = &mut self.files.new_file {
                    new.creating = false;
                    new.error = Some(e);
                }
                Task::none()
            }
        }
    }

    /// Stops naming a new file, if one is being named.
    pub(crate) fn cancel_new_file(&mut self) -> bool {
        self.files.new_file.take().is_some()
    }

    /// The Files tab of the right panel.
    pub(crate) fn view_files(&self) -> Element<'_, Message> {
        let Some(root) = self.files_root() else { return space::horizontal().width(0).into() };
        let hues = Hues::of(&self.active_theme());
        let lines = rows(&root, &self.files).into_iter().map(|r| file_row(r, hues));
        let path = mouse_area(muted_text(short_dir(&root, 40)).wrapping(text::Wrapping::None).width(Length::Fill))
            .on_right_press(Message::Files(FilesMsg::Menu(root.clone())));
        let top = row![path, icon_button(Icon::Plus, Message::Files(FilesMsg::NewFile(root.clone())), "New file")]
            .align_y(Alignment::Center);
        // The folder's own menu, under its name; always there, so the tree keeps its place.
        let root_menu: Element<'_, Message> = match &self.files.menu {
            Some(m) if *m == root => file_menu(root.clone(), true, 0.0),
            _ => space::vertical().height(0).into(),
        };
        column![top, root_menu, ui::scroll(column(lines).width(Length::Fill)).height(Length::Fill)].spacing(8).into()
    }
}

fn file_row(r: Row<'_>, hues: Hues) -> Element<'_, Message> {
    match r {
        Row::Entry { entry, depth, open, dim } => {
            let chevron: Element<'_, Message> = match (entry.is_dir, open) {
                (true, true) => icon(Icon::ChevronDown, CHEVRON, theme::muted).into(),
                (true, false) => icon(Icon::ChevronRight, CHEVRON, theme::muted).into(),
                (false, _) => space::horizontal().width(CHEVRON).into(),
            };
            let look = if entry.is_dir { file_icons::folder(open) } else { file_icons::file(&entry.name) };
            let style = move |t: &iced::Theme| text::Style { color: dim.then(|| theme::muted(t)) };
            let label = row![
                space::horizontal().width(depth as f32 * INDENT),
                chevron,
                file_icons::icon(look, hues, dim, ICON),
                text(&entry.name).size(13).wrapping(text::Wrapping::None).style(style),
            ]
            .spacing(GAP)
            .align_y(Alignment::Center);
            let press =
                if entry.is_dir { FilesMsg::Toggle(entry.path.clone()) } else { FilesMsg::Open(entry.path.clone()) };
            let line = button(label)
                .width(Length::Fill)
                .padding([3, 6])
                .style(theme::row_button)
                .on_press(Message::Files(press));
            mouse_area(line).on_right_press(Message::Files(FilesMsg::Menu(entry.path.clone()))).into()
        }
        // Both line up with the icons of the entries around them.
        Row::Menu { entry, depth } => {
            file_menu(entry.path.clone(), entry.is_dir, depth as f32 * INDENT + CHEVRON + GAP)
        }
        Row::Note { text: note, depth } => {
            row![space::horizontal().width(depth as f32 * INDENT + CHEVRON + GAP), muted_text(note)]
                .padding([3, 6])
                .into()
        }
        Row::NewFile { new, depth } => {
            let indent = depth as f32 * INDENT;
            let name = ui::input("File name", &new.name)
                .id(NEW_FILE_INPUT)
                .size(13)
                .padding([2, 6])
                .on_input(|name| Message::Files(FilesMsg::NewFileName(name)))
                .on_submit(Message::Files(FilesMsg::CreateFile));
            // Where the file will be, with the icon its name gives it.
            let line = row![
                space::horizontal().width(indent),
                space::horizontal().width(CHEVRON),
                file_icons::icon(file_icons::file(&new.name), hues, false, ICON),
                name,
            ]
            .spacing(GAP)
            .align_y(Alignment::Center);
            let mut lines = column![line].spacing(4);
            if let Some(e) = &new.error {
                let under_name = indent + CHEVRON + ICON + 3.0 * GAP;
                lines = lines.push(row![space::horizontal().width(under_name), ui::note_text(Tone::Danger, e)]);
            }
            container(lines).padding([2, 6]).into()
        }
    }
}

/// The menu of an entry: a new file in it (or next to it), or showing it in
/// the system's file manager.
fn file_menu<'a>(path: PathBuf, is_dir: bool, indent: f32) -> Element<'a, Message> {
    let item = |label: &'static str, msg: FilesMsg| {
        button(text(label).size(13))
            .width(Length::Fill)
            .padding([5, 10])
            .style(theme::row_button)
            .on_press(Message::Files(msg))
    };
    let dir = if is_dir { path.clone() } else { path.parent().map_or_else(|| path.clone(), Path::to_path_buf) };
    let items = column![
        item("New file", FilesMsg::NewFile(dir)),
        item(reveal::label(is_dir), FilesMsg::Reveal { path, is_dir }),
    ];
    row![space::horizontal().width(indent), container(items).padding(4).width(220).style(theme::menu)]
        .padding([2, 6])
        .into()
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
                Row::Menu { entry, depth } => format!("{depth}:[menu of {}]", entry.name),
                Row::NewFile { new, depth } => format!("{depth}:[new file {:?}]", new.name),
            })
            .collect()
    }

    fn naming(dir: &str, name: &str) -> Option<NewFile> {
        Some(NewFile { dir: dir.into(), name: name.into(), error: None, creating: false })
    }

    #[test]
    fn a_new_file_is_named_at_the_top_of_its_folder() {
        let mut state = FilesState::default();
        state.listings.insert("/r".into(), Listing::Ready(vec![entry("/r/src", true), entry("/r/x.md", false)]));
        state.listings.insert("/r/src".into(), Listing::Ready(vec![entry("/r/src/lib.rs", false)]));
        state.expanded.insert("/r/src".into());
        state.new_file = naming("/r/src", "main");
        assert_eq!(lines("/r", &state), ["0:▾src", "1:[new file \"main\"]", "1:lib.rs", "0:x.md"]);
        state.new_file = naming("/r", "");
        assert_eq!(lines("/r", &state), ["0:[new file \"\"]", "0:▾src", "1:lib.rs", "0:x.md"]);
        // Not "This folder is empty." under it.
        state.listings.insert("/r".into(), Listing::Ready(Vec::new()));
        assert_eq!(lines("/r", &state), ["0:[new file \"\"]"]);
    }

    #[test]
    fn new_file_names_stay_in_their_folder() {
        let dir = Path::new("/r");
        assert_eq!(new_file_path(dir, " notes.md "), Ok(dir.join("notes.md")));
        assert_eq!(new_file_path(dir, "docs/notes.md"), Ok(dir.join("docs/notes.md")));
        for bad in ["", "  ", "../x", "a/../../x", "/etc/x", ".", "docs/"] {
            assert!(new_file_path(dir, bad).is_err(), "{bad:?}");
        }
        #[cfg(windows)]
        for bad in [r"C:\x", r"\x", r"docs\"] {
            assert!(new_file_path(dir, bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn creating_a_file_makes_its_folders_and_keeps_what_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        create_file(&root.join("a.txt")).unwrap();
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"");
        create_file(&root.join("docs/deep/b.md")).unwrap();
        assert!(root.join("docs/deep/b.md").is_file());

        std::fs::write(root.join("a.txt"), "kept").unwrap();
        assert_eq!(create_file(&root.join("a.txt")), Err("a.txt already exists".into()));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "kept");
        assert_eq!(create_file(&root.join("docs")), Err("docs already exists".into()), "a folder");
        assert!(create_file(&root.join("a.txt/c.txt")).is_err(), "a file in the way of a folder");
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
    fn open_folders_new_to_a_listing_are_read() {
        let expanded: HashSet<PathBuf> = ["/r/a".into(), "/r/b".into(), "/r/x.md".into()].into();
        let old = Listing::Ready(vec![entry("/r/a", true), entry("/r/c", true)]);
        let new = [entry("/r/a", true), entry("/r/b", true), entry("/r/c", true), entry("/r/x.md", false)];
        assert_eq!(appeared(Some(&old), &new, &expanded), [PathBuf::from("/r/b")], "b came back; a was there");
        let all_open = [PathBuf::from("/r/a"), PathBuf::from("/r/b")];
        assert_eq!(appeared(None, &new, &expanded), all_open, "a first listing");
        assert_eq!(appeared(Some(&Listing::Failed("denied".into())), &new, &expanded), all_open);
    }

    #[test]
    fn a_menu_opens_under_its_entry() {
        let mut state = FilesState::default();
        state.listings.insert("/r".into(), Listing::Ready(vec![entry("/r/src", true), entry("/r/x.md", false)]));
        state.listings.insert("/r/src".into(), Listing::Ready(vec![entry("/r/src/lib.rs", false)]));
        state.expanded.insert("/r/src".into());
        state.menu = Some("/r/src".into());
        assert_eq!(lines("/r", &state), ["0:▾src", "0:[menu of src]", "1:lib.rs", "0:x.md"]);
        state.menu = Some("/r/src/lib.rs".into());
        assert_eq!(lines("/r", &state), ["0:▾src", "1:lib.rs", "1:[menu of lib.rs]", "0:x.md"]);
        assert_eq!(shown_dirs(Path::new("/r"), &state).len(), 2, "menus aren't folders to read");
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
