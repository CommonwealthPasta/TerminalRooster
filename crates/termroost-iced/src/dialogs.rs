//! Modal dialogs.

use std::sync::Arc;

use iced::widget::{
    Column, button, center, checkbox, column, container, mouse_area, opaque, row, scrollable, space, text, text_input,
};
use iced::{Alignment, Element, Length};
use termroost_core::AgentKind;
use termroost_core::session::shell;

use crate::app::{App, DIALOG_FIRST_INPUT, Dialog, Message};
use crate::keymap::Keymap;
use crate::sidebar::{BOLD, muted_text};
use crate::terminal_view::{KeyboardFocus, TerminalView};
use crate::theme;

pub(crate) fn field<'a>(label: &'a str, input: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![text(label).size(12), input.into()].spacing(4).into()
}

fn actions<'a>(confirm: &'a str, danger: bool) -> Element<'a, Message> {
    row![
        space::horizontal(),
        button(text("Cancel")).padding([6, 14]).style(button::secondary).on_press(Message::DialogCancel),
        button(text(confirm))
            .padding([6, 14])
            .style(if danger { button::danger } else { button::primary })
            .on_press(Message::DialogSubmit),
    ]
    .spacing(8)
    .into()
}

fn modal<'a>(content: Element<'a, Message>, width: f32) -> Element<'a, Message> {
    opaque(
        mouse_area(
            center(opaque(container(content).padding(20).width(width).style(theme::modal_card)))
                .style(theme::modal_backdrop),
        )
        .on_press(Message::DialogCancel),
    )
}

/// The small Codex window for trusting the hook. Clicking outside does not
/// close it (it would end Codex); Done does.
fn codex_window<'a>(
    handle: &'a termroost_core::TerminalHandle,
    keymap: &Arc<Keymap>,
    focus: &KeyboardFocus,
    font_size: f32,
) -> Element<'a, Message> {
    let term =
        TerminalView::new(handle.clone(), keymap.clone(), focus.clone(), true, font_size, Message::Noop, Message::Noop);
    let content = column![
        row![
            text("Type /hooks and trust the TerminalRooster hook.").size(14).width(Length::Fill),
            button(text("Done")).padding([5, 14]).style(button::primary).on_press(Message::DialogCancel),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
        container(term).height(420).style(theme::pane(true)),
    ]
    .spacing(10);
    opaque(
        center(opaque(container(content).padding(14).width(820).style(theme::modal_card))).style(theme::modal_backdrop),
    )
}

impl App {
    pub(crate) fn view_dialog<'a>(&'a self, dialog: &'a Dialog) -> Element<'a, Message> {
        let s = self.core.state();
        let platform_shell = {
            let sh = shell::default_shell();
            std::iter::once(sh.program).chain(sh.args).collect::<Vec<_>>().join(" ")
        };
        let content: Element<'_, Message> = match dialog {
            Dialog::Project { editing, name, dir, shell, error } => {
                let default_shell = s.settings.default_shell.clone().unwrap_or_else(|| platform_shell.clone());
                let mut col = column![
                    text(if editing.is_some() { "Edit project" } else { "New project" }).size(18).font(BOLD),
                    field(
                        "Name",
                        text_input("my-service", name)
                            .id(DIALOG_FIRST_INPUT)
                            .on_input(Message::DialogName)
                            .on_submit(Message::DialogSubmit)
                            .padding(6),
                    ),
                    field(
                        "Working directory",
                        row![
                            text_input("/path/to/project", dir)
                                .on_input(Message::DialogDir)
                                .on_submit(Message::DialogSubmit)
                                .padding(6),
                            button(text("Browse…"))
                                .padding([6, 10])
                                .style(button::secondary)
                                .on_press(Message::BrowseDir),
                        ]
                        .spacing(6),
                    ),
                    field(
                        "Shell (optional)",
                        row![
                            text_input(&format!("default: {default_shell}"), shell)
                                .on_input(Message::DialogShell)
                                .on_submit(Message::DialogSubmit)
                                .padding(6),
                            // Empty means "the default", so nothing is shown as picked.
                            self.shell_picker(shell, "", |o| Message::DialogShell(o.command)),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    ),
                ]
                .spacing(12);
                if let Some(e) = error {
                    col = col.push(text(e).size(13).style(text::danger));
                }
                col.push(actions(if editing.is_some() { "Save" } else { "Create" }, false)).into()
            }
            Dialog::ConfirmDeleteProject(p) => {
                let name = s.project(*p).map(|p| p.name.as_str()).unwrap_or("project");
                let tabs = s.tabs_of(*p).count();
                let terms: usize = s.tabs_of(*p).map(|t| t.layout.panes().len()).sum();
                let tasks = s.tasks_of(*p).count();
                column![
                    text(format!("Delete project “{name}”?")).size(18).font(BOLD),
                    text(format!(
                        "This closes {terms} terminal(s) in {tabs} session(s) and deletes {tasks} task(s) and all saved \
                         scrollback. The directory on disk is not touched. This cannot be undone."
                    ))
                    .size(13),
                    actions("Delete project", true),
                ]
                .spacing(14)
                .into()
            }
            Dialog::ConfirmDeleteTab(t) => {
                let tab = s.tab(*t);
                let name = tab.map(|t| t.title.as_str()).unwrap_or("session");
                let terms = tab.map(|t| t.layout.panes().len()).unwrap_or(0);
                let linked = s.tasks.iter().filter(|x| x.tab == Some(*t)).count();
                column![
                    text(format!("Delete session “{name}”?")).size(18).font(BOLD),
                    text(format!(
                        "This closes {terms} terminal(s) and their saved scrollback. {linked} linked task(s) are kept \
                         and unlinked."
                    ))
                    .size(13),
                    actions("Delete session", true),
                ]
                .spacing(14)
                .into()
            }
            Dialog::ConfirmQuit { running, dont_ask } => {
                let n = running.len();
                let list = Column::with_children(running.iter().map(|r| {
                    row![
                        text("•").size(13),
                        column![text(&r.programs).size(13), muted_text(&r.place)].spacing(1).width(Length::Fill),
                    ]
                    .spacing(8)
                    .into()
                }))
                .spacing(6);
                let mut col = column![
                    text("Quit TerminalRooster?").size(18).font(BOLD),
                    text(if n == 1 {
                        "A terminal is still running a program. Quitting stops it.".to_owned()
                    } else {
                        format!("{n} terminals are still running programs. Quitting stops them.")
                    })
                    .size(13),
                    container(scrollable(list)).max_height(320),
                ]
                .spacing(12);
                if running.iter().any(|r| r.agent) {
                    col = col.push(muted_text(
                        "Coding agents come back in the same conversation when TerminalRooster starts again, but a \
                         reply they are writing now is cut off.",
                    ));
                }
                col.push(
                    checkbox(*dont_ask)
                        .label("Don't ask again (Settings → Notifications)")
                        .on_toggle(Message::DialogDontAsk)
                        .size(14)
                        .text_size(12),
                )
                .push(actions("Quit", true))
                .into()
            }
            Dialog::CodexTrust(handle) => {
                return codex_window(handle, &self.keymap, &self.keyboard_focus, self.prefs.font_size);
            }
            Dialog::Worktree(form) => self.view_worktree(form),
            Dialog::RemoveWorktree(form) => self.view_remove(form),
            Dialog::CodexHook => column![
                text("Track Codex sessions?").size(18).font(BOLD),
                text("Add a small hook to Codex so TerminalRooster always resumes the right conversation in each terminal.",)
                    .size(13),
                muted_text("You can change this later in Settings."),
                row![
                    space::horizontal(),
                    button(text("Not now")).padding([6, 14]).style(button::secondary).on_press(Message::DialogCancel),
                    button(text("Add hook")).padding([6, 14]).style(button::primary).on_press(Message::DialogSubmit),
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into(),
            Dialog::Renamed(renamed) => {
                let mut body = column![text("TermRoost is now TerminalRooster").size(18).font(BOLD)].spacing(12);
                if let Some(from) = &renamed.moved_from {
                    body = body.push(
                        text(format!(
                            "Your projects, sessions, tasks, scrollback and settings were moved from {} to {}.",
                            from.display(),
                            self.core.data_dir().display()
                        ))
                        .size(13),
                    );
                }
                let buttons = if renamed.codex_hook_updated {
                    body = body
                        .push(
                            text(
                                "The Codex hook now reports to the new folder. Codex only runs hooks you trust, so \
                                 trust it again: type /hooks in the Codex window that opens.",
                            )
                            .size(13),
                        )
                        .push(muted_text("You can also do this later in Settings → Integrations."));
                    row![
                        space::horizontal(),
                        button(text("Later")).padding([6, 14]).style(button::secondary).on_press(Message::DialogCancel),
                        button(text("Trust in Codex…"))
                            .padding([6, 14])
                            .style(button::primary)
                            .on_press(Message::DialogSubmit),
                    ]
                } else {
                    row![
                        space::horizontal(),
                        button(text("OK")).padding([6, 14]).style(button::primary).on_press(Message::DialogSubmit),
                    ]
                };
                body.push(buttons.spacing(8)).into()
            }
            Dialog::Pane { pane, resume, args } => {
                let agent = s.pane(*pane).and_then(|p| p.agent.as_ref());
                let status: Element<'_, Message> = match agent {
                    Some(a) => {
                        let session = a.session_id.clone().unwrap_or_else(|| "(no conversation yet)".into());
                        let resume_line =
                            a.resume_command().unwrap_or_else(|| format!("{} (no conversation yet)", a.kind.as_str()));
                        column![
                            text(format!("{} is running here · session {session}", a.kind.label())).size(13),
                            muted_text(format!("On restore: {resume_line}")),
                            button(text("Don't resume it"))
                                .padding([4, 10])
                                .style(button::secondary)
                                .on_press(Message::DialogClearAgent),
                        ]
                        .spacing(6)
                        .into()
                    }
                    None => muted_text("No coding agent is running in this terminal.").into(),
                };
                let start = row(AgentKind::ALL.iter().map(|k| {
                    button(text(format!("Start {}", k.label())).size(13))
                        .padding([5, 10])
                        .on_press(Message::DialogStartAgent(*k))
                        .into()
                }))
                .spacing(6);
                column![
                    text("Terminal settings").size(18).font(BOLD),
                    status,
                    field(
                        "Agent arguments",
                        text_input("e.g. --model opus", args)
                            .id(DIALOG_FIRST_INPUT)
                            .on_input(Message::DialogArgs)
                            .padding(6),
                    ),
                    start,
                    muted_text(
                        "Claude Code, Codex and Grok are detected automatically, also when you start them yourself, \
                         and TerminalRooster follows the session they are in (including /clear). After a restart each \
                         terminal resumes its own conversation, not just the most recent one.",
                    ),
                    field(
                        "Resume command (used when no agent is running)",
                        text_input("e.g. npm run dev", resume)
                            .on_input(Message::DialogResume)
                            .on_submit(Message::DialogSubmit)
                            .padding(6),
                    ),
                    actions("Save", false),
                ]
                .spacing(12)
                .into()
            }
        };
        // The worktree list needs room for a path and its buttons.
        let width = if matches!(dialog, Dialog::Worktree(_)) { 600.0 } else { 480.0 };
        modal(container(content).width(Length::Fill).align_x(Alignment::Start).into(), width)
    }
}
