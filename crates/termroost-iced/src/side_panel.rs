//! The right panel: the project's tasks, the git log of the session on
//! screen, or the files of the session's folder.

use iced::widget::{button, column, container, row, space, text};
use iced::{Alignment, Element, Length};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message};
use crate::keymap::Action;
use crate::sidebar::icon_button;
use crate::theme::{self, BOLD};

/// Which tab the right panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SideTab {
    // Stored as "Tasks" and "Git", as before the Files tab came; a build of
    // the Files tab alone stored lowercase names.
    #[default]
    #[serde(alias = "tasks")]
    Tasks,
    Git,
    #[serde(alias = "files")]
    Files,
}

impl SideTab {
    pub const ALL: [SideTab; 3] = [SideTab::Tasks, SideTab::Git, SideTab::Files];

    pub fn label(self) -> &'static str {
        match self {
            SideTab::Tasks => "Tasks",
            SideTab::Git => "Git",
            SideTab::Files => "Files",
        }
    }

    /// The shortcut that shows or hides the panel on this tab, if it has one.
    pub fn action(self) -> Option<Action> {
        match self {
            SideTab::Tasks => Some(Action::ToggleTasks),
            SideTab::Git => None,
            SideTab::Files => Some(Action::ToggleFiles),
        }
    }
}

impl App {
    /// The tab on screen. The Git tab needs a session in a repository;
    /// without one the panel shows the tasks.
    pub(crate) fn shown_side_tab(&self) -> SideTab {
        match self.prefs.side_tab {
            SideTab::Git if !self.in_repo() => SideTab::Tasks,
            tab => tab,
        }
    }

    pub(crate) fn view_side_panel(&self) -> Element<'_, Message> {
        let current = self.shown_side_tab();
        let mut tabs = row![].spacing(2);
        for tab in SideTab::ALL {
            if tab == SideTab::Git && !self.in_repo() {
                continue;
            }
            let selected = tab == current;
            let label = text(tab.label()).size(13).font(if selected { BOLD } else { iced::Font::DEFAULT });
            tabs = tabs.push(
                button(label).padding([3, 10]).style(theme::panel_tab(selected)).on_press(Message::ShowPanel(tab)),
            );
        }
        let hide = match current.action() {
            Some(action) => self.keymap.hint("Hide panel", action),
            None => "Hide panel".to_owned(),
        };
        let header =
            row![tabs, space::horizontal(), icon_button("✕", Message::ClosePanel, hide)].align_y(Alignment::Center);

        let body = match current {
            SideTab::Tasks => self.view_tasks(),
            SideTab::Git => self.view_git_log(),
            SideTab::Files => self.view_files(),
        };
        container(column![header, body].spacing(10).padding([10, 10]))
            .width(300)
            .height(Length::Fill)
            .style(theme::sidebar)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::SideTab;

    #[test]
    fn stored_tabs_still_read() {
        // A tab that doesn't read would reset every preference to its default.
        for (stored, tab) in [
            ("\"Tasks\"", SideTab::Tasks),
            ("\"Git\"", SideTab::Git),
            ("\"Files\"", SideTab::Files),
            ("\"tasks\"", SideTab::Tasks),
            ("\"files\"", SideTab::Files),
        ] {
            assert_eq!(serde_json::from_str::<SideTab>(stored).unwrap(), tab, "{stored}");
        }
    }
}
