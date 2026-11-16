//! Project and tab columns. Both lists can be reordered by dragging, and a
//! search box filters the projects.

use std::path::Path;

use iced::widget::{button, column, container, mouse_area, row, scrollable, space, stack, text, text_input, tooltip};
use iced::{Alignment, Element, Font, Length, Padding, font};
use termroost_core::git::Head;
use termroost_core::{AgentKind, PaneId, Project, ProjectGroup, Tab, TabId};

use crate::agent_icon::{agent_icon, terminal_icon};
use crate::app::{App, DropTarget, Message, PROJECT_SEARCH, RENAME_INPUT, RenameTarget};
use crate::groups::{self, Row};
use crate::keymap::Action;
use crate::notify::{Category, Notice};
use crate::on_escape::on_escape;
use crate::reorder::reorder;
use crate::theme;

/// Width of the edit/delete button column next to the selected list item.
const ACTIONS_WIDTH: f32 = 26.0;
/// Space between the items of the project and session lists.
const LIST_SPACING: f32 = 2.0;
/// How far right a group's projects are.
const GROUP_INDENT: f32 = 12.0;

pub const BOLD: Font = Font { weight: font::Weight::Bold, ..Font::DEFAULT };

/// Powerline's branch symbol, which the bundled JetBrains Mono has.
const BRANCH_SYMBOL: &str = "\u{E0A0}";

/// A git branch (or detached commit) with the branch symbol, cut to `max` characters.
pub fn branch_label<'a>(head: &Head, size: f32, max: usize) -> Element<'a, Message> {
    let name = head.to_string();
    let name = if name.chars().count() > max {
        format!("{}…", name.chars().take(max.saturating_sub(1)).collect::<String>())
    } else {
        name
    };
    row![
        text(BRANCH_SYMBOL)
            .font(theme::MONO)
            .size(size)
            .style(|t: &iced::Theme| text::Style { color: Some(theme::accent(t)) }),
        text(name).size(size).wrapping(text::Wrapping::None),
    ]
    .spacing(4)
    .align_y(Alignment::Center)
    .into()
}

/// Width of the glyph in an [`icon_button`], and the padding on either side of it.
const ICON_GLYPH_WIDTH: f32 = 18.0;
const ICON_PADDING: f32 = 4.0;
/// Width of an [`icon_button`].
pub const ICON_BUTTON_WIDTH: f32 = ICON_GLYPH_WIDTH + 2.0 * ICON_PADDING;

pub fn icon_button<'a>(label: &'a str, msg: Message, tip: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    tooltip(
        button(text(label).size(14).align_x(Alignment::Center).width(ICON_GLYPH_WIDTH))
            .padding([2.0, ICON_PADDING])
            .style(theme::icon_button)
            .on_press(msg),
        container(text(tip).size(12)).padding([4, 8]).style(container::rounded_box),
        tooltip::Position::Bottom,
    )
    .into()
}

pub fn section_title(label: &str) -> iced::widget::Text<'_> {
    text(label).size(11).font(BOLD).style(|t: &iced::Theme| text::Style { color: Some(theme::muted(t)) })
}

pub fn muted_text<'a>(s: impl text::IntoFragment<'a>) -> iced::widget::Text<'a> {
    text(s).size(11).style(|t: &iced::Theme| text::Style { color: Some(theme::muted(t)) })
}

/// Shows a path with the home directory abbreviated to `~`.
pub fn display_dir(path: &std::path::Path) -> String {
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    path.display().to_string()
}

/// Like [`display_dir`], but keeps only the trailing components that fit in `max` characters.
pub fn short_dir(path: &std::path::Path, max: usize) -> String {
    let full = display_dir(path);
    if full.chars().count() <= max {
        return full;
    }
    let parts: Vec<&str> = full.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let mut out = String::new();
    for part in parts.iter().rev() {
        let candidate = if out.is_empty() { part.to_string() } else { format!("{part}/{out}") };
        if candidate.chars().count() + 2 > max && !out.is_empty() {
            break;
        }
        out = candidate;
    }
    format!("…/{out}")
}

/// Whether a project matches the projects search: each word of `query` is in
/// its name, its group's name or its folder's name, ignoring case. An empty
/// query matches all.
pub fn matches_search(name: &str, group: Option<&str>, dir: &Path, query: &str) -> bool {
    let mut words = query.split_whitespace().map(str::to_lowercase).peekable();
    if words.peek().is_none() {
        return true;
    }
    let name = name.to_lowercase();
    let group = group.unwrap_or_default().to_lowercase();
    // Not the whole path: its first folders are often shared by every project.
    let folder = dir.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
    words.all(|w| name.contains(&w) || group.contains(&w) || folder.contains(&w))
}

/// Text size of a list item's name.
const NAME_SIZE: f32 = 14.0;

/// `content` centered on the first line of a list item's name, where it
/// stays when the name wraps.
fn on_first_line<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    let line = text::LineHeight::default().to_absolute(NAME_SIZE.into());
    container(content).height(line).align_y(Alignment::Center).into()
}

/// The text color of a list item.
fn item_text(t: &iced::Theme, selected: bool) -> iced::Color {
    let p = t.extended_palette();
    if selected { p.primary.weak.text } else { p.background.base.text }
}

/// A list item; `drop_target` outlines it while a task is held over it.
fn item_style(selected: bool, drop_target: bool) -> impl Fn(&iced::Theme) -> container::Style {
    move |t| {
        let p = t.extended_palette();
        let border = iced::Border::default().rounded(4);
        container::Style {
            background: selected.then_some(iced::Background::Color(p.primary.weak.color)),
            text_color: Some(item_text(t, selected)),
            border: if drop_target { border.width(2.0).color(p.primary.base.color) } else { border },
            ..container::Style::default()
        }
    }
}

/// The line down the left of a group's projects, as wide as they are indented.
fn group_rail<'a>() -> Element<'a, Message> {
    let line = container(space::horizontal()).width(2).height(Length::Fill).style(|t: &iced::Theme| container::Style {
        background: Some(iced::Background::Color(iced::Color { a: 0.5, ..theme::muted(t) })),
        ..container::Style::default()
    });
    container(line).padding(Padding { left: 5.0, ..Padding::ZERO }).width(GROUP_INDENT).height(Length::Fill).into()
}

/// A "+" under the last item of a list, as wide as the list.
fn add_item<'a>(msg: Message, tip: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    tooltip(
        button(text("+").size(18).width(Length::Fill).align_x(Alignment::Center))
            .width(Length::Fill)
            .padding([2, 8])
            .style(theme::add_item_button)
            .on_press(msg),
        container(text(tip).size(12)).padding([4, 8]).style(container::rounded_box),
        tooltip::Position::Bottom,
    )
    .into()
}

fn activity_dot<'a>(on: bool) -> Element<'a, Message> {
    if on {
        text("●").size(10).style(|t: &iced::Theme| text::Style { color: Some(theme::accent(t)) }).into()
    } else {
        space::horizontal().width(0).into()
    }
}

/// What a list item shows next to its name: the mark of an unseen alert
/// (✗ a failure, ✓ an agent), else a dot for new output.
fn marker<'a>(notice: Option<&Notice>, activity: bool) -> Element<'a, Message> {
    let Some(n) = notice else { return activity_dot(activity) };
    let category = n.category;
    tooltip(
        text(n.glyph())
            .size(13)
            .font(BOLD)
            .style(move |t: &iced::Theme| text::Style { color: Some(theme::notice_color(t, category)) }),
        container(text(n.headline.clone()).size(12)).padding([4, 8]).style(container::rounded_box),
        tooltip::Position::Bottom,
    )
    .into()
}

impl App {
    /// The alert to show for a set of terminals: a failure before an agent.
    fn notice_of(&self, panes: impl IntoIterator<Item = PaneId>) -> Option<&Notice> {
        let found: Vec<&Notice> = panes.into_iter().filter_map(|p| self.notices.get(&p)?.first()).collect();
        found.iter().find(|n| n.category == Category::Error).or(found.first()).copied()
    }

    fn tab_notice(&self, tab: &Tab) -> Option<&Notice> {
        self.notice_of(tab.layout.panes())
    }

    fn rename_input(&self, target: RenameTarget) -> Option<Element<'_, Message>> {
        let r = self.rename.as_ref().filter(|r| r.target == target)?;
        Some(
            text_input("Name", &r.value)
                .id(RENAME_INPUT)
                .on_input(Message::RenameInput)
                .on_submit(Message::RenameCommit)
                .size(13)
                .padding(5)
                .into(),
        )
    }

    pub(crate) fn view_projects(&self) -> Element<'_, Message> {
        let s = self.core.state();
        let header = row![
            section_title("PROJECTS"),
            space::horizontal(),
            icon_button("⚙", Message::OpenSettings, "Settings"),
            icon_button("+", Message::NewProject, self.keymap.hint("New project", Action::NewProject)),
        ]
        .align_y(Alignment::Center)
        .padding([8, 10]);

        let shown: Vec<&Project> = self.listed_projects().collect();
        // While searching, groups show the projects found in them.
        let searching = self.searching_projects();
        let open = |g| searching || s.group(g).is_some_and(|g| !g.collapsed);
        let (rows, depths): (Vec<Row>, Vec<u8>) =
            groups::rows(&groups::entries(shown.iter().copied()), open).into_iter().unzip();
        let rows_view = rows.iter().zip(&depths).map(|(row, depth)| {
            let item = match *row {
                Row::Group(g) => s.group(g).map(|g| self.group_item(g, searching)),
                Row::Project(p) => s.project(p).map(|p| self.project_item(p, *depth > 0)),
            };
            item.unwrap_or_else(|| space::horizontal().width(0).into())
        });
        // While searching, a place in the list isn't the same place among all projects.
        let all = groups::entries(&s.projects);
        let list = reorder(rows_view)
            .spacing(LIST_SPACING)
            .nested(depths, GROUP_INDENT)
            .on_drop_if(self.rename.is_none(), move |from, target| {
                groups::drop_command(&all, &rows, from, target).map(Message::ArrangeProjects)
            });
        let mut items = column![list].spacing(LIST_SPACING).padding([0, 6]);
        if shown.is_empty() && !s.projects.is_empty() {
            items = items.push(container(muted_text("No projects match")).padding([6, 8]));
        }
        let items = items.push(add_item(Message::NewProject, self.keymap.hint("New project", Action::NewProject)));
        container(column![header, self.project_search_box(), scrollable(items).height(Length::Fill)])
            .width(190)
            .height(Length::Fill)
            .style(theme::sidebar_deep)
            .into()
    }

    /// The projects the list shows: those matching the search, in their order.
    pub(crate) fn listed_projects(&self) -> impl Iterator<Item = &Project> {
        let s = self.core.state();
        let query = self.project_search.as_str();
        s.projects.iter().filter(move |p| {
            let group = p.group.and_then(|g| s.group(g)).map(|g| g.name.as_str());
            matches_search(&p.name, group, &p.working_dir, query)
        })
    }

    pub(crate) fn searching_projects(&self) -> bool {
        self.project_search.split_whitespace().next().is_some()
    }

    /// The box that filters the projects list, with a ✕ to empty it.
    fn project_search_box(&self) -> Element<'_, Message> {
        // Room on the right for the ✕, so the text doesn't run under it.
        let input = text_input(&self.keymap.hint("Search", Action::SearchProjects), &self.project_search)
            .id(PROJECT_SEARCH)
            .on_input(Message::ProjectSearch)
            .on_submit(Message::ProjectSearchSubmit)
            .size(13)
            .padding(Padding { right: 28.0, ..Padding::new(5.0) });
        // The input stays the first layer, so it keeps its focus when the ✕ comes and goes.
        let mut field = stack![on_escape(input, Message::ProjectSearchEscape)];
        if !self.project_search.is_empty() {
            let clear = icon_button("✕", Message::ProjectSearchClear, "Clear search");
            field = field.push(container(clear).padding([0, 2]).align_right(Length::Fill).center_y(Length::Fill));
        }
        container(field).padding(Padding { top: 0.0, right: 6.0, bottom: 6.0, left: 6.0 }).into()
    }

    /// A group's row: a click shows or hides its projects.
    fn group_item<'a>(&'a self, group: &'a ProjectGroup, searching: bool) -> Element<'a, Message> {
        if let Some(input) = self.rename_input(RenameTarget::Group(group.id)) {
            return input;
        }
        let s = self.core.state();
        let projects: Vec<&Project> = s.projects_in(group.id).collect();
        let open = searching || !group.collapsed;
        let arrow = text(if open { "▼" } else { "▶" }).font(theme::MONO).size(9).width(12);
        let mut label = row![arrow, text(&group.name).size(13).font(BOLD).width(Length::Fill)]
            .spacing(4)
            .align_y(Alignment::Center);
        let mut selected = false;
        if !open {
            // Closed, it stands for its projects: their alerts, and the selection.
            selected = projects.iter().any(|p| s.selected_project == Some(p.id));
            let tabs = || projects.iter().flat_map(|p| s.tabs_of(p.id));
            let notice = self.notice_of(tabs().flat_map(|t| t.layout.panes()));
            label = label.push(marker(notice, tabs().any(|t| self.activity.contains(&t.id))));
        }
        let label = label.push(muted_text(projects.len().to_string()));
        let mut body =
            mouse_area(container(label).padding([5, 8]).width(Length::Fill).style(item_style(selected, false)))
                .on_double_click(Message::GroupDoubleClicked(group.id));
        if !searching {
            // On release, so pressing it to drag it doesn't open or close it.
            body = body.on_release(Message::ToggleGroup(group.id));
        }
        let rename = icon_button("✎", Message::StartRename(RenameTarget::Group(group.id)), "Rename group");
        row![body, container(rename).width(ACTIONS_WIDTH)].align_y(Alignment::Center).into()
    }

    /// A project's row; `nested` in a group, it is drawn further right.
    fn project_item<'a>(&'a self, p: &'a Project, nested: bool) -> Element<'a, Message> {
        let item = self.project_row(p);
        if nested { row![group_rail(), item].into() } else { item }
    }

    fn project_row<'a>(&'a self, p: &'a Project) -> Element<'a, Message> {
        if let Some(input) = self.rename_input(RenameTarget::Project(p.id)) {
            return input;
        }
        let s = self.core.state();
        let selected = s.selected_project == Some(p.id);
        let busy = s.tabs_of(p.id).any(|t| self.activity.contains(&t.id));
        let notice = self.notice_of(s.tabs_of(p.id).flat_map(|t| t.layout.panes()));
        let label = column![
            row![text(&p.name).size(14).width(Length::Fill), marker(notice, busy)].align_y(Alignment::Center),
            muted_text(short_dir(&p.working_dir, 24)).wrapping(text::Wrapping::None),
        ]
        .spacing(1);
        let body = mouse_area(container(label).padding([6, 8]).width(Length::Fill).style(item_style(selected, false)))
            .on_press(Message::SelectProject(p.id))
            .on_double_click(Message::StartRename(RenameTarget::Project(p.id)));
        // Same widget structure whether selected or not, so the mouse area keeps
        // its click state and a double click on an unselected item still registers.
        let actions: Element<'_, Message> = if selected {
            column![
                icon_button("✎", Message::EditProject(p.id), "Edit project"),
                icon_button("✕", Message::AskDeleteProject(p.id), "Delete project"),
            ]
            .into()
        } else {
            space::horizontal().width(ACTIONS_WIDTH).into()
        };
        row![body, actions].align_y(Alignment::Center).into()
    }

    pub(crate) fn view_tabs(&self) -> Element<'_, Message> {
        let s = self.core.state();
        let Some(project) = s.selected_project.and_then(|p| s.project(p)) else {
            return container(space::horizontal()).width(210).height(Length::Fill).style(theme::sidebar).into();
        };
        let header = row![
            column![
                text(&project.name).size(15).font(BOLD),
                muted_text(short_dir(&project.working_dir, 26)).wrapping(text::Wrapping::None)
            ]
            .width(Length::Fill),
            icon_button("+", Message::NewTab, self.keymap.hint("New session", Action::NewSession)),
        ]
        .align_y(Alignment::Center)
        .padding([8, 10]);
        let ids: Vec<TabId> = s.tabs_of(project.id).map(|t| t.id).collect();
        let list = reorder(s.tabs_of(project.id).map(|t| self.tab_item(project, t)))
            .spacing(LIST_SPACING)
            .on_move_if(self.rename.is_none(), move |from, to| Message::MoveTab(ids[from], to));
        let items = column![list, add_item(Message::NewTab, self.keymap.hint("New session", Action::NewSession))]
            .spacing(LIST_SPACING)
            .padding([0, 6]);
        container(
            column![
                header,
                section_title("SESSIONS").width(Length::Fill).center(),
                scrollable(items).height(Length::Fill)
            ]
            .spacing(4),
        )
        .width(210)
        .height(Length::Fill)
        .style(theme::sidebar)
        .into()
    }

    fn tab_item<'a>(&'a self, project: &'a Project, tab: &'a Tab) -> Element<'a, Message> {
        if let Some(input) = self.rename_input(RenameTarget::Tab(tab.id)) {
            return input;
        }
        let s = self.core.state();
        let selected = project.active_tab == Some(tab.id);
        let panes = tab.layout.panes();
        let agents: Vec<&str> =
            panes.iter().filter_map(|p| s.pane(*p)?.agent.as_ref()).map(|a| a.kind.label()).collect();
        let mut subtitle = format!("{} terminal{}", panes.len(), if panes.len() == 1 { "" } else { "s" });
        if !agents.is_empty() {
            subtitle.push_str(" · ");
            subtitle.push_str(&agents.join(", "));
        }
        let open = s.open_task_count(tab.id);
        let badge: Element<'_, Message> = if open > 0 {
            container(text(open.to_string()).size(11)).padding([1, 6]).style(theme::badge).into()
        } else {
            space::horizontal().width(0).into()
        };
        let mut marks =
            row![marker(self.tab_notice(tab), self.activity.contains(&tab.id))].spacing(4).align_y(Alignment::Center);
        // One logo per kind of agent, in terminal order.
        let mut kinds: Vec<AgentKind> = Vec::new();
        for kind in panes.iter().filter_map(|p| Some(s.pane(*p)?.agent.as_ref()?.kind)) {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        // A session without an agent is plain terminals: TerminalRooster's own icon.
        if kinds.is_empty() {
            marks = marks.push(tooltip(
                terminal_icon(NAME_SIZE),
                container(text("Terminal").size(12)).padding([4, 8]).style(container::rounded_box),
                tooltip::Position::Bottom,
            ));
        }
        for kind in kinds {
            marks = marks.push(tooltip(
                agent_icon(kind, NAME_SIZE, move |t| item_text(t, selected)),
                container(text(kind.label()).size(12)).padding([4, 8]).style(container::rounded_box),
                tooltip::Position::Bottom,
            ));
        }
        // A long name wraps, breaking inside a word too when one doesn't fit.
        let name = text(&tab.title).size(NAME_SIZE).width(Length::Fill).wrapping(text::Wrapping::WordOrGlyph);
        let mut label = column![row![on_first_line(marks), name, on_first_line(badge)].spacing(4)].spacing(1);
        if let Some(head) = self.git_of(tab).and_then(|g| g.head.as_ref()) {
            label = label.push(branch_label(head, 11.0, 24));
        }
        let label = label.push(muted_text(subtitle));
        let target = DropTarget::Session(tab.id);
        let style = item_style(selected, self.is_drop_target(target));
        let body = mouse_area(container(label).padding([6, 8]).width(Length::Fill).style(style))
            .on_press(Message::SelectTab(tab.id))
            .on_double_click(Message::StartRename(RenameTarget::Tab(tab.id)));
        let actions: Element<'_, Message> = if selected {
            column![
                icon_button("✎", Message::StartRename(RenameTarget::Tab(tab.id)), "Rename session"),
                icon_button("✕", Message::AskDeleteTab(tab.id), "Delete session"),
            ]
            .into()
        } else {
            space::horizontal().width(ACTIONS_WIDTH).into()
        };
        // A task dragged from the task panel can be dropped here to link it to this session.
        let mut item = mouse_area(row![body, actions].align_y(Alignment::Center));
        if self.task_drag.is_some() {
            item = item
                .on_enter(Message::TaskDragOver(target, true))
                .on_exit(Message::TaskDragOver(target, false))
                .on_release(Message::TaskDrop(target));
        }
        item.into()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn dir(parts: &[&str]) -> PathBuf {
        parts.iter().collect()
    }

    #[test]
    fn search_matches_each_word_in_the_name_or_folder() {
        let worktree = dir(&["projects", "TermRoost-feature-search"]);
        let m = |query| matches_search("TerminalRooster", None, &worktree, query);
        assert!(m(""), "no search: every project");
        assert!(m("   "));
        assert!(m("rooster"), "in the name, any case");
        assert!(m("FEATURE"), "in the folder");
        assert!(m("search rooster"), "each word on its own, in any order");
        assert!(!m("rooster blog"), "every word has to match");
        assert!(!m("projects"), "the folders above the project's own don't count");
        assert!(!m("roosterterm"));
    }

    #[test]
    fn search_copes_with_folders_without_a_name() {
        assert!(matches_search("Root", None, Path::new("/"), "root"));
        assert!(!matches_search("Root", None, Path::new("/"), "home"));
    }

    #[test]
    fn search_finds_a_group_by_its_name() {
        let api = dir(&["work", "api"]);
        assert!(matches_search("api", Some("Client work"), &api, "client"), "every project of the group");
        assert!(matches_search("api", Some("Client work"), &api, "client api"), "each word anywhere");
        assert!(!matches_search("api", Some("Client work"), &api, "client web"));
        assert!(!matches_search("api", None, &api, "client"));
    }
}
