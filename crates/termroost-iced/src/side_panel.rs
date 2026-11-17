//! The right panel: the project's tasks, the git log of the session on
//! screen, or the files of the session's folder.

use iced::widget::{column, container, row, space};
use iced::{Alignment, Element, Length};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message};
use crate::icons::Icon;
use crate::keymap::Action;
use crate::theme;
use crate::ui::{self, icon_button};

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
        let tabs = SideTab::ALL
            .into_iter()
            .filter(|tab| *tab != SideTab::Git || self.in_repo())
            .map(|tab| (tab.label().to_owned(), tab == current, Message::ShowPanel(tab)));
        let hide = match current.action() {
            Some(action) => self.keymap.tip("Hide panel", action),
            None => "Hide panel".to_owned(),
        };
        let header =
            row![ui::segmented(tabs), space::horizontal(), icon_button(Icon::Close, Message::ClosePanel, hide)]
                .align_y(Alignment::Center);

        let body = match current {
            SideTab::Tasks => self.view_tasks(),
            SideTab::Git => self.view_git_log(),
            SideTab::Files => self.view_files(),
        };
        container(column![header, body].spacing(12).padding([10, 12]))
            .width(300)
            .height(Length::Fill)
            .style(theme::side_column)
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
