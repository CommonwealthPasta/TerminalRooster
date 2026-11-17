//! Pop-ups in the top right corner about terminals that are out of view.

use iced::widget::{Column, column, container, row, text};
use iced::{Alignment, Element, Length};

use crate::app::{App, Message};
use crate::notify::Toast;
use crate::sidebar::muted_text;
use crate::theme::{self, BOLD, SEMIBOLD, Tone};
use crate::ui;

fn toast(t: &Toast) -> Element<'_, Message> {
    let n = &t.notice;
    let category = n.category;
    // The alert's mark in a tinted circle.
    let mark =
        container(text(n.glyph()).size(13).font(BOLD)).center_x(26).center_y(26).style(theme::toast_mark(category));
    let mut body = column![text(&n.headline).size(14).font(SEMIBOLD)].spacing(2);
    if let Some(detail) = &n.detail {
        body = body.push(text(detail).size(12));
    }
    body = body.push(muted_text(&t.place));
    let buttons = row![
        ui::small("Dismiss", theme::button_link(Tone::Neutral)).on_press(Message::DismissToast(t.pane)),
        ui::small("Show", theme::button_secondary).on_press(Message::ShowPane(t.pane)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let content = column![
        row![mark, body.width(Length::Fill)].spacing(12).align_y(Alignment::Start),
        container(buttons).align_right(Length::Fill),
    ]
    .spacing(8);
    container(content).padding([12, 14]).width(360).style(theme::toast).into()
}

impl App {
    /// The pop-ups, newest on top. Everything around them stays clickable.
    pub(crate) fn view_toasts(&self) -> Element<'_, Message> {
        let list = Column::with_children(self.toasts.iter().rev().map(toast)).spacing(10);
        container(list)
            .padding(iced::Padding { top: 52.0, right: 16.0, ..Default::default() })
            .align_right(Length::Fill)
            .into()
    }
}
