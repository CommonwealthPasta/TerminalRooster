//! Building blocks the views share, each in the look `theme` describes: so
//! that a button, a field or a note looks the same wherever it is.

use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::{Column, Scrollable, TextInput, button, container, scrollable, space, text, text_input, tooltip};
use iced::{Element, Length, Theme};

use crate::app::Message;
use crate::icons::{Icon, icon};
use crate::theme::{self, SEMIBOLD, TEXT, TEXT_SM, Tone};

/// Size of the icon in an [`icon_button`], and the padding around it.
const ICON_SIZE: f32 = 14.0;
const ICON_PADDING: f32 = 4.0;
/// Width (and height) of an [`icon_button`].
pub const ICON_BUTTON_WIDTH: f32 = ICON_SIZE + 2.0 * ICON_PADDING;

/// `content` with a tooltip. Each line break in `label` starts a smaller,
/// dimmer line under the first, such as for a keyboard shortcut.
pub fn tip<'a>(
    content: impl Into<Element<'a, Message>>,
    label: impl Into<String>,
    position: tooltip::Position,
) -> Element<'a, Message> {
    let label: String = label.into();
    let mut lines = label.lines();
    let first = text(lines.next().unwrap_or_default().to_owned()).size(TEXT_SM);
    let rest = lines.map(|l| {
        text(l.to_owned()).size(11).style(|t: &Theme| text::Style { color: Some(theme::tooltip_hint(t)) }).into()
    });
    let body = Column::with_children(std::iter::once(first.into()).chain(rest)).spacing(1);
    tooltip(content, container(body).padding([5, 9]).style(theme::tooltip), position).gap(6).into()
}

/// A small borderless button showing `glyph`, with a tooltip.
pub fn icon_button<'a>(glyph: Icon, msg: Message, label: impl Into<String>) -> Element<'a, Message> {
    let ink = |t: &Theme| theme::ink(t).scale_alpha(0.78);
    tip(
        button(icon(glyph, ICON_SIZE, ink)).padding(ICON_PADDING).style(theme::button_ghost).on_press(msg),
        label,
        tooltip::Position::Bottom,
    )
}

/// A text field.
pub fn input<'a>(placeholder: &str, value: &str) -> TextInput<'a, Message> {
    text_input(placeholder, value).size(TEXT).padding([6, 10]).style(theme::input)
}

/// A vertical scrollable with thin scrollbars over the content's edge.
pub fn scroll<'a>(content: impl Into<Element<'a, Message>>) -> Scrollable<'a, Message> {
    scrollable(content)
        .direction(Direction::Vertical(Scrollbar::new().width(6).scroller_width(6).margin(2)))
        .style(theme::scrollbars)
}

/// A hairline down the height it is given.
pub fn vertical_divider<'a>() -> Element<'a, Message> {
    container(space()).width(1).height(Length::Fill).style(theme::divider).into()
}

/// A small rounded label.
pub fn badge<'a>(label: impl text::IntoFragment<'a>, tone: Tone) -> Element<'a, Message> {
    container(text(label).size(11).wrapping(text::Wrapping::None)).padding([1, 7]).style(theme::badge(tone)).into()
}

/// Text in the muted color, at its default size.
pub fn muted(t: &Theme) -> text::Style {
    text::Style { color: Some(theme::muted(t)) }
}

/// A small heading over a group of things, such as a column's list.
pub fn heading<'a>(label: &'a str) -> text::Text<'a> {
    text(label).size(TEXT_SM).font(SEMIBOLD).style(muted)
}
