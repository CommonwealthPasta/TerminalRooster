//! The terminal area: header plus the split pane grid of the visible tab.

use iced::widget::{button, center, column, container, pane_grid, pick_list, row, space, text};
use iced::{Alignment, Element, Length};
use termroost_core::{AgentKind, Axis, PaneId, Tab};

use crate::app::{App, Message};
use crate::keymap::Action;
use crate::sidebar::{BOLD, branch_label, icon_button, muted_text, short_dir};
use crate::terminal_view::TerminalView;
use crate::theme;

impl App {
    pub(crate) fn view_main(&self) -> Element<'_, Message> {
        let s = self.core.state();
        let Some(project) = s.selected_project.and_then(|p| s.project(p)) else {
            return center(
                column![
                    text("TerminalRooster").size(30).font(BOLD),
                    text("Projects keep their terminals, splits and tasks, even across restarts and crashes."),
                    button(text("New project")).padding([6, 14]).on_press(Message::NewProject),
                ]
                .spacing(14)
                .align_x(Alignment::Center),
            )
            .into();
        };
        let Some(tab) = s.active_tab() else {
            return center(
                column![
                    text(format!("{} has no sessions", project.name)),
                    button(text("New session")).padding([6, 14]).on_press(Message::NewTab),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .into();
        };

        let open: usize = s.tasks_of(project.id).filter(|t| !t.done).count();
        let tasks_toggle: Element<'_, Message> = if self.prefs.tasks_open {
            space::horizontal().width(0).into()
        } else {
            button(text(format!("Tasks ({open})")).size(12))
                .padding([3, 10])
                .style(button::secondary)
                .on_press(Message::ToggleTasks)
                .into()
        };
        let git = self.git_of(tab);
        let mut header = row![text(&tab.title).size(14).font(BOLD)];
        if let Some(head) = git.and_then(|g| g.head.as_ref()) {
            header = header.push(branch_label(head, 12.0, 40));
        }
        header = header.push(muted_text(short_dir(&s.tab_working_dir(tab), 70)).wrapping(text::Wrapping::None));
        header = header.push(space::horizontal());
        let header = header.push(tasks_toggle).spacing(10).align_y(Alignment::Center).padding([4, 10]);

        let mut content = column![header];
        if let Some(err) = &self.error {
            content = content.push(
                container(
                    row![text(err).size(13).width(Length::Fill), icon_button("✕", Message::DismissError, "Dismiss")]
                        .align_y(Alignment::Center),
                )
                .padding([4, 10])
                .style(theme::error_banner),
            );
        }
        let grid: Element<'_, Message> = match self.grids.get(&tab.id) {
            Some(grid) => pane_grid(grid, move |_, pane, _| self.pane_content(*pane, tab))
                .spacing(3)
                .on_click(Message::PaneClicked)
                .on_drag(Message::PaneDragged)
                .on_resize(8, Message::PaneResized)
                .style(theme::pane_grid_style)
                .into(),
            None => center(text("…")).into(),
        };
        content.push(container(grid).padding(3).height(Length::Fill)).into()
    }

    fn pane_content<'a>(&'a self, pane: PaneId, tab: &'a Tab) -> pane_grid::Content<'a, Message> {
        let s = self.core.state();
        let active = tab.focused_pane == Some(pane);
        let handle = self.core.terminal(pane);

        let body: Element<'_, Message> = match handle.clone() {
            Some(h) => TerminalView::new(
                h,
                self.keymap.clone(),
                active && self.dialog.is_none(),
                self.prefs.font_size,
                Message::FocusPane(pane),
                Message::RestartPane(pane),
            )
            .into(),
            None => center(text("starting…")).into(),
        };

        let title = handle.as_ref().and_then(|h| h.title()).unwrap_or_else(|| "Terminal".to_owned());
        let agent = s.pane(pane).and_then(|p| p.agent.as_ref()).map(|a| match &a.session_id {
            Some(id) => format!("{} · {}", a.kind.label(), &id[..id.len().min(8)]),
            None => format!("{} · new session", a.kind.label()),
        });
        let mut title_row =
            row![text(title).size(12).wrapping(text::Wrapping::None)].spacing(8).align_y(Alignment::Center);
        if let Some(agent) = agent {
            title_row = title_row.push(container(text(agent).size(11)).padding([1, 6]).style(theme::badge));
        }

        let k = &self.keymap;
        let full = row![
            pick_list(&AgentKind::ALL[..], None::<AgentKind>, move |k| Message::StartAgent(pane, k))
                .placeholder("Start agent")
                .text_size(12)
                .padding([2, 6]),
            icon_button("⚙", Message::PaneSettings(pane), "Terminal settings & resume command"),
            icon_button("◫", Message::Split(pane, Axis::Vertical), k.hint("Split right", Action::SplitRight)),
            icon_button("⊟", Message::Split(pane, Axis::Horizontal), k.hint("Split down", Action::SplitDown)),
            icon_button("✕", Message::ClosePane(pane), k.hint("Close terminal", Action::CloseTerminal)),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        // Narrow panes get only the essentials.
        let compact = row![
            icon_button("⚙", Message::PaneSettings(pane), "Terminal settings, agents & resume command"),
            icon_button("✕", Message::ClosePane(pane), k.hint("Close terminal", Action::CloseTerminal)),
        ]
        .spacing(2)
        .align_y(Alignment::Center);

        let title_bar = pane_grid::TitleBar::new(title_row)
            .controls(pane_grid::Controls::dynamic(full, compact))
            .always_show_controls()
            .padding([2, 6])
            .style(theme::title_bar(active));

        pane_grid::Content::new(container(body).width(Length::Fill).height(Length::Fill).style(theme::pane(active)))
            .title_bar(title_bar)
    }
}
