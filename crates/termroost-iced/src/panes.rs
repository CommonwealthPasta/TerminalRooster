//! The terminal area: header plus the split pane grid of the visible tab.

use iced::widget::{button, center, column, container, pane_grid, pick_list, row, space, text, tooltip};
use iced::{Alignment, Element, Length, Padding};
use termroost_core::{AgentKind, Axis, PaneId, Tab};

use crate::agent_icon::agent_icon;
use crate::app::{App, Message};
use crate::keymap::Action;
use crate::leave_room::leave_room;
use crate::notify::Category;
use crate::side_panel::SideTab;
use crate::sidebar::{BOLD, ICON_BUTTON_WIDTH, branch_label, icon_button, muted_text, short_dir};
use crate::terminal_view::TerminalView;
use crate::theme;
use crate::worktree::WorktreeMsg;

/// Space between the buttons of a terminal's title bar.
const CONTROL_SPACING: f32 = 2.0;
/// Width of the two buttons narrow terminals get, which the title leaves room for.
const COMPACT_CONTROLS_WIDTH: f32 = 2.0 * ICON_BUTTON_WIDTH + CONTROL_SPACING;
/// Space between a terminal's title and its buttons.
const TITLE_GAP: f32 = 8.0;

impl App {
    pub(crate) fn view_main(&self) -> Element<'_, Message> {
        let s = self.core.state();
        let Some(project) = s.selected_project.and_then(|p| s.project(p)) else {
            return center(
                column![
                    text(crate::DISPLAY_NAME).size(30).font(BOLD),
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
        let git = self.git_of(tab);
        // While the right panel is hidden, buttons bring it back on any of its
        // tabs (Git only for a session in a repository).
        let panel_buttons: Element<'_, Message> = if self.prefs.panel_open {
            space::horizontal().width(0).into()
        } else {
            let show = |label: String, tab: SideTab| {
                button(text(label).size(12)).padding([3, 10]).style(button::secondary).on_press(Message::ShowPanel(tab))
            };
            let mut buttons = row![show(format!("Tasks ({open})"), SideTab::Tasks)].spacing(6);
            if git.is_some() {
                buttons = buttons.push(show("Git".into(), SideTab::Git));
            }
            buttons.push(show("Files".into(), SideTab::Files)).into()
        };
        let mut header = row![text(&tab.title).size(14).font(BOLD)];
        if let Some(head) = git.and_then(|g| g.head.as_ref()) {
            header = header.push(branch_label(head, 12.0, 40));
        }
        header = header.push(muted_text(short_dir(&s.tab_working_dir(tab), 70)).wrapping(text::Wrapping::None));
        header = header.push(space::horizontal());
        if git.is_some() {
            let worktrees = button(text("Worktrees").size(12))
                .padding([3, 10])
                .style(button::secondary)
                .on_press(Message::Worktree(WorktreeMsg::Open));
            header = header.push(tooltip(
                worktrees,
                container(text("Switch this session to another git worktree, or make a new one").size(12))
                    .padding([4, 8])
                    .style(container::rounded_box),
                tooltip::Position::Bottom,
            ));
        }
        let header = header.push(panel_buttons).spacing(10).align_y(Alignment::Center).padding([4, 10]);

        let mut banners = column![];
        let messages = [
            (&self.error, Message::DismissError, theme::error_banner as fn(&iced::Theme) -> container::Style),
            (&self.notice, Message::DismissNotice, theme::notice_banner),
        ];
        for (message, dismiss, style) in messages {
            if let Some(message) = message {
                banners = banners.push(
                    container(
                        row![text(message).size(13).width(Length::Fill), icon_button("✕", dismiss, "Dismiss")]
                            .align_y(Alignment::Center),
                    )
                    .padding([4, 10])
                    .style(style),
                );
            }
        }
        let body: Element<'_, Message> = match (self.shown_doc(), self.grids.get(&tab.id)) {
            (Some(doc), _) => self.view_doc(doc),
            (None, Some(grid)) => pane_grid(grid, move |_, pane, _| self.pane_content(*pane, tab))
                .spacing(3)
                .on_click(Message::PaneClicked)
                .on_drag(Message::PaneDragged)
                .on_resize(8, Message::PaneResized)
                .style(theme::pane_grid_style)
                .into(),
            (None, None) => center(text("…")).into(),
        };
        // Each part keeps its place in the column whether it shows anything or
        // not: widgets are matched to their state by position, so a banner
        // coming and going would otherwise reset the editor and the terminals.
        column![header, self.view_file_strip(), banners, container(body).padding(3).height(Length::Fill)].into()
    }

    fn pane_content<'a>(&'a self, pane: PaneId, tab: &'a Tab) -> pane_grid::Content<'a, Message> {
        let s = self.core.state();
        let active = tab.focused_pane == Some(pane);
        let handle = self.core.terminal(pane);

        let body: Element<'_, Message> = match handle.clone() {
            Some(h) => TerminalView::new(
                h,
                self.keymap.clone(),
                self.keyboard_focus.clone(),
                active && self.dialog.is_none(),
                self.prefs.font_size,
                Message::FocusPane(pane),
                Message::RestartPane(pane),
            )
            .into(),
            None => center(text("starting…")).into(),
        };

        let title = handle.as_ref().and_then(|h| h.title()).unwrap_or_else(|| "Terminal".to_owned());
        let agent = s.pane(pane).and_then(|p| p.agent.as_ref()).map(|a| {
            let label = match &a.session_id {
                Some(id) => format!("{} · {}", a.kind.label(), &id[..id.len().min(8)]),
                None => format!("{} · new session", a.kind.label()),
            };
            (a.kind, label)
        });
        // Nothing wraps; when space runs out the last item is cut off first, so
        // they go in order of importance. The gap on the right keeps the title
        // off the buttons.
        let mut title_row = row![].spacing(8).align_y(Alignment::Center).padding(Padding::ZERO.right(TITLE_GAP));
        // An alert the user hasn't seen. It goes away when the user focuses this terminal.
        let notice = self.notices.get(&pane).and_then(|n| n.first());
        if let Some(n) = notice {
            let label = match &n.detail {
                Some(d) if n.category == Category::Error => format!("{} {} · {d}", n.glyph(), n.headline),
                _ => format!("{} {}", n.glyph(), n.headline),
            };
            title_row = title_row.push(
                container(text(label).size(11).wrapping(text::Wrapping::None))
                    .padding([1, 6])
                    .style(theme::notice_chip(n.category))
                    .clip(true),
            );
        }
        if let Some((kind, label)) = agent {
            // Drawn on `theme::badge`, so one-color logos take its text color.
            let icon = agent_icon(kind, 12.0, |t| t.extended_palette().secondary.base.text);
            // The label is cut inside the badge's padding, so a short badge keeps its rounded end.
            let label = container(text(label).size(11).wrapping(text::Wrapping::None)).clip(true);
            let badge = row![icon, label].spacing(4).align_y(Alignment::Center);
            title_row = title_row.push(container(badge).padding([1, 6]).style(theme::badge).clip(true));
        }
        title_row = title_row.push(container(text(title).size(12).wrapping(text::Wrapping::None)).clip(true));

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
        .spacing(CONTROL_SPACING)
        .align_y(Alignment::Center);
        // Narrow panes get only the essentials. Keep `COMPACT_CONTROLS_WIDTH` in step.
        let compact = row![
            icon_button("⚙", Message::PaneSettings(pane), "Terminal settings, agents & resume command"),
            icon_button("✕", Message::ClosePane(pane), k.hint("Close terminal", Action::CloseTerminal)),
        ]
        .spacing(CONTROL_SPACING)
        .align_y(Alignment::Center);

        let title_bar = pane_grid::TitleBar::new(leave_room(title_row, COMPACT_CONTROLS_WIDTH))
            .controls(pane_grid::Controls::dynamic(full, compact))
            .always_show_controls()
            .padding([2, 6])
            .style(theme::title_bar(active));

        pane_grid::Content::new(container(body).width(Length::Fill).height(Length::Fill).style(theme::pane(active)))
            .title_bar(title_bar)
    }
}
