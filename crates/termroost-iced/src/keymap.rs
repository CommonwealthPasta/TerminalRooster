//! Keyboard shortcuts (configurable in Settings), plus iced keyboard events ->
//! core key input.
//!
//! The default shortcuts only use Ctrl+Shift, Ctrl+Tab/PageUp/PageDown,
//! Alt+Arrows and Shift/Ctrl+Insert, so plain Ctrl+letter combinations keep
//! reaching the programs in terminals.

use std::collections::{BTreeMap, HashMap};

use iced::keyboard::key::{Named, Physical};
use iced::keyboard::{Key, Modifiers};
use termroost_core as core;

/// Something a shortcut does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    NewProject,
    NewSession,
    NextSession,
    PrevSession,
    NextProject,
    PrevProject,
    SearchProjects,
    MoveSessionUp,
    MoveSessionDown,
    MoveProjectUp,
    MoveProjectDown,
    ToggleTasks,
    SplitRight,
    SplitDown,
    CloseTerminal,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    Copy,
    Paste,
    ScrollPageUp,
    ScrollPageDown,
    ScrollTop,
    ScrollBottom,
    FontBigger,
    FontSmaller,
    FontReset,
}

/// Static description of an action.
struct Info {
    /// Stable name in the saved preferences; never change it.
    id: &'static str,
    label: &'static str,
    group: &'static str,
    defaults: &'static [&'static str],
}

impl Action {
    /// In the order Settings lists them.
    pub const ALL: [Action; 28] = [
        Action::NewProject,
        Action::NewSession,
        Action::NextSession,
        Action::PrevSession,
        Action::NextProject,
        Action::PrevProject,
        Action::SearchProjects,
        Action::MoveSessionUp,
        Action::MoveSessionDown,
        Action::MoveProjectUp,
        Action::MoveProjectDown,
        Action::ToggleTasks,
        Action::SplitRight,
        Action::SplitDown,
        Action::CloseTerminal,
        Action::FocusLeft,
        Action::FocusRight,
        Action::FocusUp,
        Action::FocusDown,
        Action::Copy,
        Action::Paste,
        Action::ScrollPageUp,
        Action::ScrollPageDown,
        Action::ScrollTop,
        Action::ScrollBottom,
        Action::FontBigger,
        Action::FontSmaller,
        Action::FontReset,
    ];

    fn info(self) -> Info {
        const GENERAL: &str = "Projects and sessions";
        const TERMINALS: &str = "Terminals";
        const TEXT: &str = "Text size";
        let (id, label, group, defaults): (_, _, _, &'static [&'static str]) = match self {
            Action::NewProject => ("new_project", "New project", GENERAL, &["Ctrl+Shift+N"]),
            Action::NewSession => ("new_session", "New session", GENERAL, &["Ctrl+Shift+T"]),
            Action::NextSession => ("next_session", "Next session", GENERAL, &["Ctrl+Tab", "Ctrl+PageDown"]),
            Action::PrevSession => {
                ("previous_session", "Previous session", GENERAL, &["Ctrl+Shift+Tab", "Ctrl+PageUp"])
            }
            Action::NextProject => ("next_project", "Next project", GENERAL, &["Ctrl+Shift+PageDown"]),
            Action::PrevProject => ("previous_project", "Previous project", GENERAL, &["Ctrl+Shift+PageUp"]),
            Action::SearchProjects => ("search_projects", "Search projects", GENERAL, &["Ctrl+Shift+P"]),
            Action::MoveSessionUp => ("move_session_up", "Move session up", GENERAL, &["Ctrl+Alt+PageUp"]),
            Action::MoveSessionDown => ("move_session_down", "Move session down", GENERAL, &["Ctrl+Alt+PageDown"]),
            Action::MoveProjectUp => ("move_project_up", "Move project up", GENERAL, &["Ctrl+Shift+Alt+PageUp"]),
            Action::MoveProjectDown => {
                ("move_project_down", "Move project down", GENERAL, &["Ctrl+Shift+Alt+PageDown"])
            }
            Action::ToggleTasks => ("toggle_tasks", "Show / hide tasks", GENERAL, &["Ctrl+Shift+K"]),
            Action::SplitRight => ("split_right", "Split right", TERMINALS, &["Ctrl+Shift+E"]),
            Action::SplitDown => ("split_down", "Split down", TERMINALS, &["Ctrl+Shift+O"]),
            Action::CloseTerminal => ("close_terminal", "Close terminal", TERMINALS, &["Ctrl+Shift+W"]),
            Action::FocusLeft => ("focus_left", "Go to the terminal on the left", TERMINALS, &["Alt+Left"]),
            Action::FocusRight => ("focus_right", "Go to the terminal on the right", TERMINALS, &["Alt+Right"]),
            Action::FocusUp => ("focus_up", "Go to the terminal above", TERMINALS, &["Alt+Up"]),
            Action::FocusDown => ("focus_down", "Go to the terminal below", TERMINALS, &["Alt+Down"]),
            Action::Copy => ("copy", "Copy", TERMINALS, &["Ctrl+Shift+C", "Ctrl+Insert"]),
            Action::Paste => ("paste", "Paste", TERMINALS, &["Ctrl+Shift+V", "Shift+Insert"]),
            Action::ScrollPageUp => ("scroll_page_up", "Scroll up a page", TERMINALS, &["Shift+PageUp"]),
            Action::ScrollPageDown => ("scroll_page_down", "Scroll down a page", TERMINALS, &["Shift+PageDown"]),
            Action::ScrollTop => ("scroll_top", "Scroll to the top", TERMINALS, &["Shift+Home"]),
            Action::ScrollBottom => ("scroll_bottom", "Scroll to the bottom", TERMINALS, &["Shift+End"]),
            Action::FontBigger => ("font_bigger", "Bigger", TEXT, &["Ctrl+Shift+="]),
            Action::FontSmaller => ("font_smaller", "Smaller", TEXT, &["Ctrl+Shift+-"]),
            Action::FontReset => ("font_reset", "Default size", TEXT, &["Ctrl+Shift+0"]),
        };
        Info { id, label, group, defaults }
    }

    pub fn id(self) -> &'static str {
        self.info().id
    }

    pub fn label(self) -> &'static str {
        self.info().label
    }

    /// Heading the action is listed under in Settings.
    pub fn group(self) -> &'static str {
        self.info().group
    }

    pub fn defaults(self) -> Vec<Combo> {
        self.info().defaults.iter().map(|s| Combo::parse(s).expect("valid default shortcut")).collect()
    }

    /// Handled by the focused terminal rather than by the app.
    pub fn in_terminal(self) -> bool {
        matches!(
            self,
            Action::Copy
                | Action::Paste
                | Action::ScrollPageUp
                | Action::ScrollPageDown
                | Action::ScrollTop
                | Action::ScrollBottom
        )
    }
}

/// The key of a shortcut, without modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComboKey {
    /// A lowercase character of the unshifted key, e.g. `t` or `=`.
    Char(char),
    Named(Named),
}

/// Keys without text that shortcuts can use, and their names. Escape is
/// missing on purpose: it cancels recording a shortcut.
const NAMED: [(Named, &str); 26] = [
    (Named::Tab, "Tab"),
    (Named::Enter, "Enter"),
    (Named::Space, "Space"),
    (Named::Backspace, "Backspace"),
    (Named::Delete, "Delete"),
    (Named::Insert, "Insert"),
    (Named::Home, "Home"),
    (Named::End, "End"),
    (Named::PageUp, "PageUp"),
    (Named::PageDown, "PageDown"),
    (Named::ArrowLeft, "Left"),
    (Named::ArrowRight, "Right"),
    (Named::ArrowUp, "Up"),
    (Named::ArrowDown, "Down"),
    (Named::F1, "F1"),
    (Named::F2, "F2"),
    (Named::F3, "F3"),
    (Named::F4, "F4"),
    (Named::F5, "F5"),
    (Named::F6, "F6"),
    (Named::F7, "F7"),
    (Named::F8, "F8"),
    (Named::F9, "F9"),
    (Named::F10, "F10"),
    (Named::F11, "F11"),
    (Named::F12, "F12"),
];

/// Name of the logo key on this platform.
pub const LOGO: &str = if cfg!(target_os = "macos") {
    "Cmd"
} else if cfg!(windows) {
    "Win"
} else {
    "Super"
};

/// A key plus the exact modifiers held with it, e.g. Ctrl+Shift+T.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Combo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
    pub key: ComboKey,
}

/// The base latin character of a key (layout independent for letters).
fn latin(key: &Key, physical: Physical) -> Option<char> {
    key.to_latin(physical).map(|c| c.to_ascii_lowercase())
}

/// The unshifted key for a shifted symbol on a US layout, so a shortcut
/// recorded or matched as Ctrl+Shift++ is the same as Ctrl+Shift+=.
fn unshift(c: char) -> char {
    const PAIRS: [(char, char); 21] = [
        ('!', '1'),
        ('@', '2'),
        ('#', '3'),
        ('$', '4'),
        ('%', '5'),
        ('^', '6'),
        ('&', '7'),
        ('*', '8'),
        ('(', '9'),
        (')', '0'),
        ('_', '-'),
        ('+', '='),
        ('{', '['),
        ('}', ']'),
        ('|', '\\'),
        (':', ';'),
        ('"', '\''),
        ('<', ','),
        ('>', '.'),
        ('?', '/'),
        ('~', '`'),
    ];
    let c = c.to_ascii_lowercase();
    PAIRS.iter().find(|(shifted, _)| *shifted == c).map_or(c, |(_, base)| *base)
}

impl Combo {
    /// The combination of a key press; `None` for a lone modifier or a key
    /// shortcuts can't use.
    pub fn from_event(key: &Key, physical: Physical, mods: Modifiers) -> Option<Combo> {
        let key = match key {
            Key::Named(n) => ComboKey::Named(NAMED.iter().find(|(k, _)| k == n)?.0),
            Key::Character(s) => {
                let c = latin(key, physical).or_else(|| {
                    let mut chars = s.chars();
                    chars.next().filter(|_| chars.next().is_none())
                })?;
                if c.is_whitespace() || c.is_control() {
                    return None;
                }
                ComboKey::Char(unshift(c))
            }
            Key::Unidentified => return None,
        };
        Some(Combo { ctrl: mods.control(), shift: mods.shift(), alt: mods.alt(), logo: mods.logo(), key })
    }

    /// Parses the [`Display`](std::fmt::Display) form, e.g. `Ctrl+Shift+T` or `Alt+Left`.
    pub fn parse(s: &str) -> Option<Combo> {
        let s = s.trim();
        // The key itself may be "+".
        let (mods, key) = match s.strip_suffix("++") {
            Some(mods) => (mods, "+"),
            None => s.rsplit_once('+').unwrap_or(("", s)),
        };
        let mut combo = Combo { ctrl: false, shift: false, alt: false, logo: false, key: ComboKey::Char(' ') };
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => combo.ctrl = true,
                "shift" => combo.shift = true,
                "alt" | "option" => combo.alt = true,
                "win" | "super" | "cmd" | "meta" | "logo" => combo.logo = true,
                _ => return None,
            }
        }
        let key = key.trim();
        combo.key = match NAMED.iter().find(|(_, name)| name.eq_ignore_ascii_case(key)) {
            Some((named, _)) => ComboKey::Named(*named),
            None => {
                let mut chars = key.chars();
                let c = chars.next().filter(|c| !c.is_whitespace() && chars.next().is_none())?;
                ComboKey::Char(unshift(c))
            }
        };
        Some(combo)
    }

    /// Whether the shortcut leaves normal typing alone: letters and symbols
    /// need Ctrl, Alt or the logo key, and keys like Enter or the arrows need
    /// some modifier. Function keys are fine on their own.
    pub fn leaves_typing_alone(&self) -> bool {
        let strong = self.ctrl || self.alt || self.logo;
        match self.key {
            ComboKey::Char(_) => strong,
            ComboKey::Named(
                Named::F1
                | Named::F2
                | Named::F3
                | Named::F4
                | Named::F5
                | Named::F6
                | Named::F7
                | Named::F8
                | Named::F9
                | Named::F10
                | Named::F11
                | Named::F12,
            ) => true,
            ComboKey::Named(_) => strong || self.shift,
        }
    }
}

impl std::fmt::Display for Combo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (on, name) in [(self.ctrl, "Ctrl"), (self.shift, "Shift"), (self.alt, "Alt"), (self.logo, LOGO)] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            ComboKey::Char(c) => write!(f, "{}", c.to_ascii_uppercase()),
            ComboKey::Named(n) => f.write_str(NAMED.iter().find(|(k, _)| *k == n).map_or("?", |(_, name)| name)),
        }
    }
}

/// The shortcuts in effect: each action's defaults, or the user's own list
/// for the actions they changed.
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    by_combo: HashMap<Combo, Action>,
    by_action: HashMap<Action, Vec<Combo>>,
}

impl Keymap {
    /// `custom` maps action ids to the user's shortcuts for them (as text).
    /// Unknown actions and unreadable shortcuts are skipped; when two actions
    /// claim the same keys, the user's own setting wins over a default.
    pub fn new(custom: &BTreeMap<String, Vec<String>>) -> Keymap {
        let mut map = Keymap::default();
        let custom_of = |a: Action| custom.get(a.id()).map(|list| list.iter().filter_map(|s| Combo::parse(s)));
        for a in Action::ALL {
            if let Some(list) = custom_of(a) {
                list.for_each(|c| map.add(a, c));
            }
        }
        for a in Action::ALL {
            if custom_of(a).is_none() {
                a.defaults().into_iter().for_each(|c| map.add(a, c));
            }
        }
        map
    }

    fn add(&mut self, action: Action, combo: Combo) {
        if let std::collections::hash_map::Entry::Vacant(slot) = self.by_combo.entry(combo) {
            slot.insert(action);
            self.by_action.entry(action).or_default().push(combo);
        }
    }

    /// The action of a key press, if it is a shortcut.
    pub fn action(&self, key: &Key, physical: Physical, mods: Modifiers) -> Option<Action> {
        self.owner(&Combo::from_event(key, physical, mods)?)
    }

    pub fn owner(&self, combo: &Combo) -> Option<Action> {
        self.by_combo.get(combo).copied()
    }

    pub fn combos(&self, action: Action) -> &[Combo] {
        self.by_action.get(&action).map_or(&[], Vec::as_slice)
    }

    /// Changes the user's settings (`custom`, as for [`Keymap::new`]) so
    /// `action` has exactly `combos`, taking any of them away from other
    /// actions. Returns each shortcut taken away and the action that had it.
    pub fn assign(
        &self,
        custom: &mut BTreeMap<String, Vec<String>>,
        action: Action,
        combos: Vec<Combo>,
    ) -> Vec<(Combo, Action)> {
        let mut taken = Vec::new();
        for other in Action::ALL.into_iter().filter(|a| *a != action) {
            let theirs = self.combos(other);
            if theirs.iter().any(|c| combos.contains(c)) {
                taken.extend(theirs.iter().filter(|c| combos.contains(c)).map(|c| (*c, other)));
                store(custom, other, theirs.iter().copied().filter(|c| !combos.contains(c)).collect());
            }
        }
        store(custom, action, combos);
        taken
    }

    /// `"label (first shortcut)"` for tooltips.
    pub fn hint(&self, label: &str, action: Action) -> String {
        match self.combos(action).first() {
            Some(c) => format!("{label} ({c})"),
            None => label.to_owned(),
        }
    }
}

/// Saves an action's shortcuts in the user's settings, or forgets them when
/// they are its defaults (so later changes to the defaults reach the user).
fn store(custom: &mut BTreeMap<String, Vec<String>>, action: Action, combos: Vec<Combo>) {
    let defaults = action.defaults();
    if combos.len() == defaults.len() && combos.iter().all(|c| defaults.contains(c)) {
        custom.remove(action.id());
    } else {
        custom.insert(action.id().to_owned(), combos.iter().map(Combo::to_string).collect());
    }
}

pub fn modifiers(m: Modifiers) -> core::Modifiers {
    core::Modifiers { shift: m.shift(), ctrl: m.control(), alt: m.alt(), logo: m.logo() }
}

/// Converts a key press into the core's GUI-neutral key input.
pub fn key_input(key: &Key, physical: Physical, mods: Modifiers, text: Option<&str>) -> Option<core::KeyInput> {
    let named = |k| Some(core::Key::Named(k));
    let k = match key {
        Key::Named(n) => match n {
            Named::Enter => named(core::NamedKey::Enter),
            Named::Tab => named(core::NamedKey::Tab),
            Named::Backspace => named(core::NamedKey::Backspace),
            Named::Escape => named(core::NamedKey::Escape),
            Named::Space => named(core::NamedKey::Space),
            Named::ArrowUp => named(core::NamedKey::Up),
            Named::ArrowDown => named(core::NamedKey::Down),
            Named::ArrowLeft => named(core::NamedKey::Left),
            Named::ArrowRight => named(core::NamedKey::Right),
            Named::Home => named(core::NamedKey::Home),
            Named::End => named(core::NamedKey::End),
            Named::PageUp => named(core::NamedKey::PageUp),
            Named::PageDown => named(core::NamedKey::PageDown),
            Named::Insert => named(core::NamedKey::Insert),
            Named::Delete => named(core::NamedKey::Delete),
            Named::F1 => named(core::NamedKey::F(1)),
            Named::F2 => named(core::NamedKey::F(2)),
            Named::F3 => named(core::NamedKey::F(3)),
            Named::F4 => named(core::NamedKey::F(4)),
            Named::F5 => named(core::NamedKey::F(5)),
            Named::F6 => named(core::NamedKey::F(6)),
            Named::F7 => named(core::NamedKey::F(7)),
            Named::F8 => named(core::NamedKey::F(8)),
            Named::F9 => named(core::NamedKey::F(9)),
            Named::F10 => named(core::NamedKey::F(10)),
            Named::F11 => named(core::NamedKey::F(11)),
            Named::F12 => named(core::NamedKey::F(12)),
            _ => None,
        },
        Key::Character(c) => {
            // For Ctrl/Alt combos use the latin base key so shortcuts work on any layout.
            let base = if mods.control() || mods.alt() {
                latin(key, physical).map(String::from).unwrap_or_else(|| c.to_string())
            } else {
                c.to_string()
            };
            Some(core::Key::Char(base))
        }
        Key::Unidentified => None,
    }?;
    Some(core::KeyInput { key: k, mods: modifiers(mods), text: text.map(str::to_owned) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(s: &str) -> Combo {
        Combo::parse(s).unwrap_or_else(|| panic!("parse {s}"))
    }

    #[test]
    fn combos_parse_and_print() {
        for s in ["Ctrl+Shift+T", "Ctrl+Tab", "Alt+Left", "Shift+PageUp", "Ctrl+Shift+=", "F5", "Ctrl+Alt+Delete"] {
            assert_eq!(combo(s).to_string(), s);
        }
        assert_eq!(combo("ctrl+shift+t"), combo("Ctrl+Shift+T"));
        assert_eq!(combo("Ctrl+Shift++"), combo("Ctrl+Shift+="), "shifted symbols mean their key");
        assert_eq!(combo("Ctrl+Shift+)"), combo("Ctrl+Shift+0"));
        assert_eq!(combo("Super+K"), combo("Win+K"));
        assert_eq!(Combo::parse("Hyper+T"), None);
        assert_eq!(Combo::parse("Ctrl+Shift+"), None);
        assert_eq!(Combo::parse("Ctrl+Escape"), None, "Escape cancels recording");
    }

    #[test]
    fn key_events_become_combos() {
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let t = Key::Character("t".into());
        assert_eq!(
            Combo::from_event(&t, Physical::Code(iced::keyboard::key::Code::KeyT), ctrl_shift),
            Some(combo("Ctrl+Shift+T"))
        );
        let tab = Key::Named(Named::Tab);
        assert_eq!(
            Combo::from_event(&tab, Physical::Code(iced::keyboard::key::Code::Tab), Modifiers::CTRL),
            Some(combo("Ctrl+Tab"))
        );
        let shift = Key::Named(Named::Shift);
        assert_eq!(Combo::from_event(&shift, Physical::Code(iced::keyboard::key::Code::ShiftLeft), ctrl_shift), None);
    }

    #[test]
    fn typing_stays_safe() {
        assert!(combo("Ctrl+Shift+T").leaves_typing_alone());
        assert!(combo("Alt+Left").leaves_typing_alone());
        assert!(combo("Shift+PageUp").leaves_typing_alone());
        assert!(combo("F5").leaves_typing_alone());
        assert!(!combo("T").leaves_typing_alone());
        assert!(!combo("Shift+T").leaves_typing_alone());
        assert!(!combo("Enter").leaves_typing_alone());
    }

    #[test]
    fn defaults_are_valid_and_distinct() {
        let map = Keymap::new(&BTreeMap::new());
        let mut seen = std::collections::HashSet::new();
        for a in Action::ALL {
            assert!(!a.defaults().is_empty(), "{a:?} has a default");
            for c in a.defaults() {
                assert!(seen.insert(c), "{c} is the default of two actions");
                assert!(c.leaves_typing_alone(), "{c}");
                assert_eq!(map.owner(&c), Some(a));
            }
        }
        let ids: std::collections::HashSet<_> = Action::ALL.iter().map(|a| a.id()).collect();
        assert_eq!(ids.len(), Action::ALL.len(), "ids are unique");
    }

    /// Applies `assign` like Settings does and returns the new keymap.
    fn assign(
        custom: &mut BTreeMap<String, Vec<String>>,
        action: Action,
        combos: Vec<Combo>,
    ) -> (Keymap, Vec<(Combo, Action)>) {
        let taken = Keymap::new(custom).assign(custom, action, combos);
        (Keymap::new(custom), taken)
    }

    #[test]
    fn assigning_moves_shortcuts_between_actions() {
        let mut custom = BTreeMap::new();
        let f2 = combo("F2");

        // Add F2 to "New project".
        let (map, taken) = assign(&mut custom, Action::NewProject, vec![combo("Ctrl+Shift+N"), f2]);
        assert!(taken.is_empty());
        assert_eq!(map.owner(&f2), Some(Action::NewProject));

        // Giving F2 to "Show / hide tasks" takes it from "New project", which is back to its default.
        let (map, taken) = assign(&mut custom, Action::ToggleTasks, vec![combo("Ctrl+Shift+K"), f2]);
        assert_eq!(taken, [(f2, Action::NewProject)]);
        assert_eq!(map.owner(&f2), Some(Action::ToggleTasks));
        assert_eq!(map.combos(Action::NewProject), [combo("Ctrl+Shift+N")]);
        assert!(!custom.contains_key("new_project"), "defaults again, so not stored");

        // Taking a default from another action leaves that action without it, even after a restart.
        let (map, taken) = assign(&mut custom, Action::NewProject, vec![combo("Ctrl+Shift+N"), combo("Ctrl+Shift+T")]);
        assert_eq!(taken, [(combo("Ctrl+Shift+T"), Action::NewSession)]);
        assert!(map.combos(Action::NewSession).is_empty());
        assert_eq!(custom.get("new_session"), Some(&vec![]));

        // Removing one (✕) keeps the rest.
        let (map, _) = assign(&mut custom, Action::ToggleTasks, vec![combo("Ctrl+Shift+K")]);
        assert_eq!(map.owner(&f2), None);
        assert!(!custom.contains_key("toggle_tasks"));

        // Reset takes the defaults back from whoever has them now.
        let (map, taken) = assign(&mut custom, Action::NewSession, Action::NewSession.defaults());
        assert_eq!(taken, [(combo("Ctrl+Shift+T"), Action::NewProject)]);
        assert_eq!(map.owner(&combo("Ctrl+Shift+T")), Some(Action::NewSession));
        assert!(custom.is_empty(), "everything is back to the defaults: {custom:?}");
    }

    #[test]
    fn custom_shortcuts_replace_defaults_and_win_conflicts() {
        let custom = BTreeMap::from([
            ("new_project".to_owned(), vec!["Ctrl+Shift+T".to_owned(), "nonsense+".to_owned()]),
            ("toggle_tasks".to_owned(), vec![]),
            ("no_such_action".to_owned(), vec!["Ctrl+Shift+Q".to_owned()]),
        ]);
        let map = Keymap::new(&custom);
        assert_eq!(map.combos(Action::NewProject), [combo("Ctrl+Shift+T")]);
        assert_eq!(map.owner(&combo("Ctrl+Shift+N")), None, "the old default is gone");
        assert!(map.combos(Action::NewSession).is_empty(), "its default was taken");
        assert!(map.combos(Action::ToggleTasks).is_empty(), "explicitly no shortcut");
        assert_eq!(map.combos(Action::Copy).len(), 2, "untouched actions keep their defaults");
        assert_eq!(map.hint("Tasks", Action::ToggleTasks), "Tasks");
        assert_eq!(map.hint("New project", Action::NewProject), "New project (Ctrl+Shift+T)");
    }
}
