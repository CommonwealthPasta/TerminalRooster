//! Pop-ups in the top right corner about terminals that are out of view.

use iced::widget::{Column, button, column, container, row, text};
use iced::{Alignment, Element, Length};

use crate::app::{App, Message};
use crate::notify::Toast;
use crate::sidebar::muted_text;
use crate::theme::{self, BOLD};

fn toast(t: &Toast) -> Element<'_, Message> {
    let n = &t.notice;
    let category = n.category;
    let glyph = text(n.glyph())
        .size(16)
        .font(BOLD)
        .style(move |th: &iced::Theme| text::Style { color: Some(theme::notice_color(th, category)) });
    let mut body = column![text(&n.headline).size(14).font(BOLD)].spacing(2);
    if let Some(detail) = &n.detail {
        body = body.push(text(detail).size(12));
    }
    body = body.push(muted_text(&t.place));
    let buttons = column![
        button(text("Show").size(12)).padding([3, 10]).style(button::primary).on_press(Message::ShowPane(t.pane)),
        button(text("Dismiss").size(12)).padding([3, 10]).style(button::text).on_press(Message::DismissToast(t.pane)),
    ]
    .spacing(4)
    .align_x(Alignment::End);
    container(row![glyph, body.width(Length::Fill), buttons].spacing(10).align_y(Alignment::Start))
        .padding([10, 12])
        .width(360)
        .style(theme::toast(category))
        .into()
}

impl App {
    /// The pop-ups, newest on top. Everything around them stays clickable.
    pub(crate) fn view_toasts(&self) -> Element<'_, Message> {
        let list = Column::with_children(self.toasts.iter().rev().map(toast)).spacing(8);
        container(list)
            .padding(iced::Padding { top: 44.0, right: 16.0, ..Default::default() })
            .align_right(Length::Fill)
            .into()
    }
}
