//! Project and tab columns.

use iced::widget::{button, column, container, mouse_area, row, scrollable, space, text, text_input, tooltip};
use iced::{Alignment, Element, Font, Length, font};
use termroost_core::{Project, Tab};

use crate::app::{App, Message, RENAME_INPUT, RenameTarget};
use crate::keymap::Action;
use crate::theme;

/// Width of the edit/delete button column next to the selected list item.
const ACTIONS_WIDTH: f32 = 26.0;

pub const BOLD: Font = Font { weight: font::Weight::Bold, ..Font::DEFAULT };

pub fn icon_button<'a>(label: &'a str, msg: Message, tip: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    tooltip(
        button(text(label).size(14).align_x(Alignment::Center).width(18))
            .padding([2, 4])
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

fn item_style(selected: bool) -> impl Fn(&iced::Theme) -> container::Style {
    move |t| {
        let p = t.extended_palette();
        container::Style {
            background: selected.then_some(iced::Background::Color(p.primary.weak.color)),
            text_color: Some(if selected { p.primary.weak.text } else { p.background.base.text }),
            border: iced::Border::default().rounded(4),
            ..container::Style::default()
        }
    }
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

impl App {
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

        let items = column(s.projects.iter().map(|p| self.project_item(p)))
            .push(add_item(Message::NewProject, self.keymap.hint("New project", Action::NewProject)))
            .spacing(2)
            .padding([0, 6]);
        container(column![header, scrollable(items).height(Length::Fill)])
            .width(190)
            .height(Length::Fill)
            .style(theme::sidebar_deep)
            .into()
    }

    fn project_item<'a>(&'a self, p: &'a Project) -> Element<'a, Message> {
        if let Some(input) = self.rename_input(RenameTarget::Project(p.id)) {
            return input;
        }
        let s = self.core.state();
        let selected = s.selected_project == Some(p.id);
        let busy = s.tabs_of(p.id).any(|t| self.activity.contains(&t.id));
        let label = column![
            row![text(&p.name).size(14).width(Length::Fill), activity_dot(busy)].align_y(Alignment::Center),
            muted_text(short_dir(&p.working_dir, 24)).wrapping(text::Wrapping::None),
        ]
        .spacing(1);
        let body = mouse_area(container(label).padding([6, 8]).width(Length::Fill).style(item_style(selected)))
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
        let items = column(s.tabs_of(project.id).map(|t| self.tab_item(project, t)))
            .push(add_item(Message::NewTab, self.keymap.hint("New session", Action::NewSession)))
            .spacing(2)
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
        let label = column![
            row![activity_dot(self.activity.contains(&tab.id)), text(&tab.title).size(14).width(Length::Fill), badge]
                .spacing(4)
                .align_y(Alignment::Center),
            muted_text(subtitle),
        ]
        .spacing(1);
        let body = mouse_area(container(label).padding([6, 8]).width(Length::Fill).style(item_style(selected)))
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
        row![body, actions].align_y(Alignment::Center).into()
    }
}
