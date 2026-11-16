//! The per-project task panel. A task can be dragged onto a session (in the
//! sessions list, or the terminals on screen) to link it to that session.

use iced::widget::{
    checkbox, column, container, mouse_area, pick_list, pin, row, scrollable, space, stack, text, text_input,
};
use iced::{Alignment, Element, Length, Point, mouse};
use termroost_core::Task;

use crate::app::{App, DropTarget, Message, TASK_EDIT_INPUT, TASK_INPUT, TabChoice};
use crate::keymap::Action;
use crate::sidebar::{BOLD, icon_button, muted_text};
use crate::theme;

/// Width of the label that follows the pointer while a task is dragged.
const DRAG_LABEL_WIDTH: f32 = 240.0;

impl App {
    /// The label next to the pointer while a task is dragged: the task, and
    /// which session it would be linked to.
    pub(crate) fn view_task_drag(&self) -> Element<'_, Message> {
        let s = self.core.state();
        let Some((drag, cursor, task)) =
            self.task_drag.filter(|d| d.moving).and_then(|d| Some((d, d.cursor?, s.task(d.task)?)))
        else {
            return space::horizontal().width(0).into();
        };
        let session = drag.over.and_then(|t| self.drop_tab(t)).and_then(|t| s.tab(t));
        let hint = match session {
            Some(tab) if task.tab == Some(tab.id) => format!("Already linked to {}", tab.title),
            Some(tab) => format!("Link to {}", tab.title),
            None => "Drop on a session to link it".to_owned(),
        };
        let label = column![
            text(&task.title).size(13).font(BOLD).wrapping(text::Wrapping::None),
            muted_text(hint).wrapping(text::Wrapping::None),
        ]
        .spacing(2);
        // Right of the pointer, or left of it near the window's right edge.
        let right_edge = self.prefs.window.map_or(f32::MAX, |w| w.width);
        let x = if cursor.x + 16.0 + DRAG_LABEL_WIDTH > right_edge {
            (cursor.x - 16.0 - DRAG_LABEL_WIDTH).max(0.0)
        } else {
            cursor.x + 16.0
        };
        pin(container(label).padding([6, 10]).width(DRAG_LABEL_WIDTH).clip(true).style(theme::drag_label))
            .position(Point::new(x, cursor.y + 12.0))
            .into()
    }

    /// The terminals area: a task dropped on it is linked to the session on screen.
    pub(crate) fn drop_zone<'a>(&'a self, content: Element<'a, Message>) -> Element<'a, Message> {
        let target = DropTarget::Terminals;
        let s = self.core.state();
        let hint = if self.is_drop_target(target)
            && let Some(task) = self.task_drag.and_then(|d| s.task(d.task))
            && let Some(tab) = s.active_tab()
        {
            Some(if task.tab == Some(tab.id) {
                format!("“{}” is already linked to {}", task.title, tab.title)
            } else {
                format!("Link “{}” to {}", task.title, tab.title)
            })
        } else {
            None
        };
        // The same widgets with or without a drag, so the terminals keep their state.
        let (label, style): (Element<'_, Message>, fn(&iced::Theme) -> container::Style) = match hint {
            Some(hint) => (
                container(text(hint).size(15).font(BOLD)).padding([8, 14]).style(theme::drag_label).into(),
                theme::drop_target,
            ),
            None => (space::horizontal().width(0).into(), |_| container::Style::default()),
        };
        // While a task is dragged, the cover takes the pointer so the terminals don't react to it.
        let dragging = self.task_drag.is_some_and(|d| d.moving);
        let cover = mouse_area(container(label).center(Length::Fill).style(style)).interaction(if dragging {
            mouse::Interaction::Grabbing
        } else {
            mouse::Interaction::None
        });
        let mut area = mouse_area(stack![content, cover]);
        if self.task_drag.is_some() {
            area = area
                .on_enter(Message::TaskDragOver(target, true))
                .on_exit(Message::TaskDragOver(target, false))
                .on_release(Message::TaskDrop(target));
        }
        area.into()
    }

    /// The side panel: the project's tasks, or the git log of the session on screen.
    pub(crate) fn view_tasks(&self) -> Element<'_, Message> {
        let header = row![
            self.side_tabs(),
            space::horizontal(),
            icon_button("✕", Message::ToggleTasks, self.keymap.hint("Hide panel", Action::ToggleTasks)),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        let body = if self.git_tab_shown() { self.view_git_log() } else { self.view_task_list() };
        container(column![header, body].spacing(10).padding([10, 10]))
            .width(300)
            .height(Length::Fill)
            .style(theme::sidebar)
            .into()
    }

    fn view_task_list(&self) -> Element<'_, Message> {
        let s = self.core.state();
        let Some(project) = s.selected_project else { return space::horizontal().width(0).into() };
        let active_tab = self.active_tab_id();

        let mut choices = vec![TabChoice { id: None, label: "no session".into() }];
        choices.extend(s.tabs_of(project).map(|t| TabChoice { id: Some(t.id), label: t.title.clone() }));

        let (open, done) = s.tasks_of(project).fold((0, 0), |(o, d), t| if t.done { (o, d + 1) } else { (o + 1, d) });
        let counts = muted_text(format!("{open} open · {done} done"));

        let filters = row![
            checkbox(self.prefs.show_done).label("Show done").on_toggle(Message::ShowDone).size(14).text_size(12),
            checkbox(self.prefs.tasks_this_tab_only)
                .label("This session only")
                .on_toggle(Message::ThisTabOnly)
                .size(14)
                .text_size(12),
        ]
        .spacing(12);

        let input = text_input(
            if self.prefs.tasks_this_tab_only { "Add a task to this session…" } else { "Add a task…" },
            &self.new_task,
        )
        .id(TASK_INPUT)
        .on_input(Message::NewTaskInput)
        .on_submit(Message::AddTask)
        .size(13)
        .padding(6);

        let visible = s
            .tasks_of(project)
            .filter(|t| (self.prefs.show_done || !t.done) && (!self.prefs.tasks_this_tab_only || t.tab == active_tab));
        let list = column(visible.map(|t| self.task_item(t, &choices, active_tab))).spacing(6);

        column![counts, filters, input, scrollable(list).height(Length::Fill)].spacing(10).into()
    }

    fn task_item<'a>(
        &'a self,
        task: &'a Task,
        choices: &[TabChoice],
        active_tab: Option<termroost_core::TabId>,
    ) -> Element<'a, Message> {
        let id = task.id;
        let title: Element<'_, Message> = match &self.task_edit {
            Some((edit, value)) if *edit == id => text_input("Task", value)
                .id(TASK_EDIT_INPUT)
                .on_input(Message::TaskEditInput)
                .on_submit(Message::TaskEditCommit)
                .size(13)
                .padding(4)
                .into(),
            _ => {
                let done = task.done;
                mouse_area(
                    text(&task.title)
                        .size(13)
                        .width(Length::Fill)
                        .style(move |t: &iced::Theme| text::Style { color: done.then(|| theme::muted(t)) }),
                )
                .on_press(Message::TaskGrab(id))
                .on_double_click(Message::EditTask(id))
                .into()
            }
        };
        let selected = choices.iter().find(|c| c.id == task.tab).cloned();
        let mut link = row![
            muted_text("session"),
            pick_list(choices.to_vec(), selected, move |c| Message::TaskTab(id, c)).text_size(11).padding([1, 6]),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if let Some(tab) = task.tab.filter(|t| Some(*t) != active_tab) {
            link = link.push(icon_button("↗", Message::SelectTab(tab), "Go to session"));
        }

        let linked_here = task.tab.is_some() && task.tab == active_tab;
        // A press anywhere on the card that its controls don't take (the title
        // included) picks the task up.
        let card = container(
            column![
                row![
                    checkbox(task.done).on_toggle(move |d| Message::TaskDone(id, d)).size(16),
                    title,
                    icon_button("✎", Message::EditTask(id), "Edit"),
                    icon_button("✕", Message::DeleteTask(id), "Delete task"),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                link,
            ]
            .spacing(4),
        )
        .padding([6, 8])
        .width(Length::Fill)
        .style(move |t: &iced::Theme| {
            let p = t.extended_palette();
            container::Style {
                background: Some(iced::Background::Color(p.background.base.color)),
                border: iced::Border {
                    width: 1.0,
                    color: if linked_here { p.primary.base.color } else { p.background.strong.color },
                    radius: 6.0.into(),
                },
                ..container::Style::default()
            }
        });
        mouse_area(card)
            .on_press(Message::TaskGrab(id))
            // A click puts it down again here. This doesn't wait for the app-wide
            // listener, which only starts after the press and can miss a quick tap.
            .on_release(Message::TaskDragEnd)
            .interaction(mouse::Interaction::Grab)
            .into()
    }
}
