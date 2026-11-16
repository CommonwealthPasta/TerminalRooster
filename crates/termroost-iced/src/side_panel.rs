//! The right panel: the project's tasks, or the files of the session's folder.

use iced::widget::{button, column, container, row, space, text};
use iced::{Alignment, Element, Length};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message};
use crate::keymap::Action;
use crate::sidebar::{BOLD, icon_button};
use crate::theme;

/// Which tab the right panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideTab {
    #[default]
    Tasks,
    Files,
}

impl SideTab {
    pub const ALL: [SideTab; 2] = [SideTab::Tasks, SideTab::Files];

    pub fn label(self) -> &'static str {
        match self {
            SideTab::Tasks => "Tasks",
            SideTab::Files => "Files",
        }
    }

    /// The shortcut that shows or hides the panel on this tab.
    pub fn action(self) -> Action {
        match self {
            SideTab::Tasks => Action::ToggleTasks,
            SideTab::Files => Action::ToggleFiles,
        }
    }
}

impl App {
    pub(crate) fn view_side_panel(&self) -> Element<'_, Message> {
        let current = self.prefs.side_tab;
        let mut tabs = row![].spacing(2);
        for tab in SideTab::ALL {
            let selected = tab == current;
            let label = text(tab.label()).size(13).font(if selected { BOLD } else { iced::Font::DEFAULT });
            tabs = tabs.push(
                button(label).padding([3, 10]).style(theme::panel_tab(selected)).on_press(Message::ShowPanel(tab)),
            );
        }
        let header = row![
            tabs,
            space::horizontal(),
            icon_button("✕", Message::ClosePanel, self.keymap.hint("Hide panel", current.action())),
        ]
        .align_y(Alignment::Center);

        let body = match current {
            SideTab::Tasks => self.view_tasks(),
            SideTab::Files => self.view_files(),
        };
        container(column![header, body].spacing(10).padding([10, 10]))
            .width(300)
            .height(Length::Fill)
            .style(theme::sidebar)
            .into()
    }
}
