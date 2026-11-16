//! The settings page: appearance (themes), terminal options and shortcuts.

use iced::widget::{Column, button, checkbox, column, container, row, scrollable, slider, space, text, text_input};
use iced::{Alignment, Border, Color, Element, Length};
use termroost_core::session::shell;

use crate::app::{App, Message, SettingsState};
use crate::color_picker;
use crate::sidebar::{BOLD, muted_text};
use crate::theme::{self, MONO};
use crate::themes::{ANSI_NAMES, ColorField, Hex, ThemeDef};

fn swatch<'a>(hex: Hex, size: f32) -> Element<'a, Message> {
    container(space())
        .width(size)
        .height(size)
        .style(move |t: &iced::Theme| container::Style {
            background: Some(hex.color().into()),
            border: Border { width: 1.0, color: theme::muted(t), radius: 3.0.into() },
            ..container::Style::default()
        })
        .into()
}

fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding(16)
        .width(Length::Fill)
        .style(|t: &iced::Theme| {
            let p = t.extended_palette();
            container::Style {
                background: Some(p.background.weak.color.into()),
                border: Border { width: 1.0, color: p.background.strong.color, radius: 8.0.into() },
                ..container::Style::default()
            }
        })
        .into()
}

fn section<'a>(title: &'a str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![text(title).size(17).font(BOLD), card(content)].spacing(8).into()
}

/// A row of small swatches summarizing a theme.
fn theme_strip<'a>(t: &ThemeDef) -> Element<'a, Message> {
    let colors = [
        t.ui.background,
        t.ui.text,
        t.ui.primary,
        t.terminal.background,
        t.terminal.foreground,
        t.terminal.ansi[1],
        t.terminal.ansi[2],
        t.terminal.ansi[3],
        t.terminal.ansi[4],
        t.terminal.ansi[5],
    ];
    row(colors.into_iter().map(|c| swatch(c, 14.0))).spacing(2).into()
}

impl App {
    pub(crate) fn view_settings(&self) -> Element<'_, Message> {
        let Some(st) = &self.settings else { return space().into() };
        let header = row![
            button(text("← Back").size(14)).padding([6, 12]).style(button::secondary).on_press(Message::CloseSettings),
            text("Settings").size(24).font(BOLD),
        ]
        .spacing(16)
        .align_y(Alignment::Center);

        let body = column![
            self.appearance_section(st),
            self.terminal_section(st),
            self.integrations_section(),
            shortcuts_section(),
            muted_text(format!(
                "Everything is stored in {}. Logs are in {}.",
                self.core.data_dir().join("termroost.db").display(),
                crate::logging::log_dir(self.core.data_dir()).display()
            )),
        ]
        .spacing(28)
        .max_width(960);

        container(
            column![header, scrollable(container(body).padding(iced::Padding { right: 14.0, ..Default::default() }))]
                .spacing(20),
        )
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn appearance_section<'a>(&'a self, st: &'a SettingsState) -> Element<'a, Message> {
        let follow = self.prefs.follow_system;
        let tone = if self.system_is_dark() { "dark" } else { "light" };
        let note = if follow {
            format!(
                "Your system is {tone}. Light mode uses “{}” and dark mode uses “{}”. Using a theme makes it the \
                 choice for its mode.",
                self.prefs.light_theme, self.prefs.dark_theme
            )
        } else {
            "The theme you use stays on regardless of the system setting.".to_owned()
        };
        let active = self.active_theme().name;
        let list = Column::with_children(self.themes.all().map(|t| self.theme_row(t, st, &active))).spacing(6);

        let mut col = column![
            checkbox(follow)
                .label("Match the system's light/dark setting")
                .on_toggle(Message::FollowSystem)
                .size(16)
                .text_size(14),
            muted_text(note),
            text("Interface theme").size(14).font(BOLD),
            list,
            muted_text("Built-in themes can't be changed. Duplicate one to make your own, then edit its colors."),
        ]
        .spacing(10);
        if let Some(t) = st.editing.as_deref().and_then(|n| self.themes.find(n)) {
            col = col.push(self.theme_editor(st, t));
        }
        col = col.push(self.terminal_colors());
        section("Appearance", col)
    }

    /// Terminal colors can follow the interface theme or come from any theme,
    /// e.g. dark terminals under a light interface.
    fn terminal_colors(&self) -> Element<'_, Message> {
        let chosen = self.prefs.terminal_theme.as_deref();
        let matching = self.active_theme();
        let mut cards = vec![terminal_card(
            "Match interface".to_owned(),
            &matching,
            chosen.is_none(),
            Message::TerminalTheme(None),
        )];
        cards.extend(self.themes.all().map(|t| {
            terminal_card(
                t.name.clone(),
                t,
                chosen == Some(t.name.as_str()),
                Message::TerminalTheme(Some(t.name.clone())),
            )
        }));
        column![
            text("Terminal colors").size(14).font(BOLD),
            muted_text("Terminals can use another theme's colors, for example dark terminals with a light interface."),
            row(cards).spacing(10).wrap(),
        ]
        .spacing(8)
        .padding(iced::Padding { top: 12.0, ..Default::default() })
        .into()
    }

    fn theme_row<'a>(&'a self, t: &'a ThemeDef, st: &'a SettingsState, active: &str) -> Element<'a, Message> {
        let builtin = self.themes.is_builtin(&t.name);
        let in_use = t.name == active;
        let mut tags = vec![if builtin { "built-in" } else { "custom" }, if t.is_dark() { "dark" } else { "light" }];
        if self.prefs.follow_system {
            if t.name == self.prefs.light_theme {
                tags.push("light mode");
            }
            if t.name == self.prefs.dark_theme {
                tags.push("dark mode");
            }
        }
        let editing = st.editing.as_deref() == Some(t.name.as_str());
        let small = |label: &'a str| text(label).size(12);
        let mut actions = row![
            button(small(if in_use { "In use" } else { "Use" }))
                .padding([4, 12])
                .style(button::primary)
                .on_press_maybe((!in_use).then(|| Message::UseTheme(t.name.clone()))),
        ]
        .spacing(6);
        if !builtin {
            actions = actions.push(
                button(small(if editing { "Editing" } else { "Edit" }))
                    .padding([4, 10])
                    .style(button::secondary)
                    .on_press_maybe((!editing).then(|| Message::EditTheme(t.name.clone()))),
            );
        }
        actions = actions.push(
            button(small("Duplicate"))
                .padding([4, 10])
                .style(button::secondary)
                .on_press(Message::DuplicateTheme(t.name.clone())),
        );
        if !builtin {
            actions = actions.push(
                button(small("Delete"))
                    .padding([4, 10])
                    .style(button::danger)
                    .on_press(Message::DeleteTheme(t.name.clone())),
            );
        }
        container(
            row![
                theme_strip(t),
                column![text(&t.name).size(14), muted_text(tags.join(" · "))].spacing(2).width(Length::Fill),
                actions,
            ]
            .spacing(14)
            .align_y(Alignment::Center),
        )
        .padding([8, 10])
        .style(move |th: &iced::Theme| {
            let p = th.extended_palette();
            container::Style {
                background: Some(p.background.base.color.into()),
                border: Border {
                    width: if in_use || editing { 2.0 } else { 1.0 },
                    color: if in_use || editing { p.primary.base.color } else { p.background.strong.color },
                    radius: 6.0.into(),
                },
                ..container::Style::default()
            }
        })
        .into()
    }

    fn color_input<'a>(&self, st: &'a SettingsState, t: &ThemeDef, field: ColorField) -> Element<'a, Message> {
        let value = st.inputs.get(&field).cloned().unwrap_or_else(|| t.get(field).to_string());
        let valid = Hex::parse(&value).is_some();
        let open = st.picker.is_some_and(|p| p.field == field);
        row![
            color_picker::field_swatch(t.get(field), open, Message::PickColor((!open).then_some(field))),
            text_input("#rrggbb", &value)
                .on_input(move |v| Message::ThemeColor(field, v))
                .font(MONO)
                .size(13)
                .padding(4)
                .width(92)
                .style(move |th: &iced::Theme, status| {
                    let mut s = text_input::default(th, status);
                    if !valid {
                        s.border.color = th.extended_palette().danger.base.color;
                    }
                    s
                }),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    }

    fn theme_editor<'a>(&'a self, st: &'a SettingsState, t: &'a ThemeDef) -> Element<'a, Message> {
        let labeled = |label: &'a str, field: ColorField| -> Element<'a, Message> {
            row![text(label).size(13).width(100), self.color_input(st, t, field)].align_y(Alignment::Center).into()
        };
        let ui = Column::with_children(ColorField::UI.iter().map(|(f, l)| labeled(l, *f))).spacing(6);
        let term = Column::with_children(ColorField::TERMINAL.iter().map(|(f, l)| labeled(l, *f))).spacing(6);
        let ansi_row = |bright: bool| -> Element<'a, Message> {
            row((0..8u8).map(|i| {
                let idx = i + if bright { 8 } else { 0 };
                column![muted_text(ANSI_NAMES[i as usize]), self.color_input(st, t, ColorField::Ansi(idx))]
                    .spacing(2)
                    .into()
            }))
            .spacing(8)
            .wrap()
            .into()
        };

        let mut name_col = column![
            row![
                text("Name").size(13).width(100),
                text_input("Theme name", &st.name_input)
                    .on_input(Message::ThemeNameInput)
                    .size(13)
                    .padding(5)
                    .width(260),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(4);
        if let Some(err) = &st.name_error {
            name_col = name_col.push(text(err).size(12).style(text::danger));
        }

        // The picker opens under the group holding the color being picked.
        let picker_after = |group: usize| -> Option<Element<'a, Message>> {
            let p = st.picker.as_ref()?;
            let at = match p.field {
                ColorField::Ansi(i) if i < 8 => 1,
                ColorField::Ansi(_) => 2,
                _ => 0,
            };
            (at == group).then(|| color_picker::panel(p, t))
        };

        column![
            text(format!("Editing “{}”", t.name)).size(15).font(BOLD),
            muted_text(
                "Changes apply immediately and are saved automatically. Type a #rrggbb color, or click a color's \
                 square to choose from a palette."
            ),
            name_col,
            row![
                column![text("Interface").size(13).font(BOLD), ui].spacing(8),
                column![text("Terminal").size(13).font(BOLD), term].spacing(8),
            ]
            .spacing(48),
        ]
        .push(picker_after(0))
        .push(text("Terminal colors (ANSI)").size(13).font(BOLD))
        .push(muted_text("Normal"))
        .push(ansi_row(false))
        .push(picker_after(1))
        .push(muted_text("Bright"))
        .push(ansi_row(true))
        .push(picker_after(2))
        .push(text("Preview").size(13).font(BOLD))
        .push(preview(t))
        .spacing(10)
        .padding(iced::Padding { top: 8.0, ..Default::default() })
        .into()
    }

    fn integrations_section(&self) -> Element<'_, Message> {
        let h = &self.codex_hook;
        let small = |label| button(text(label).size(13)).padding([5, 12]);
        let (status, actions): (String, Vec<Element<'_, Message>>) = if !h.codex_found {
            ("Codex isn't set up on this computer.".to_owned(), vec![])
        } else if h.installed_in.is_none() {
            (
                "Not added.".to_owned(),
                vec![small("Add hook").style(button::primary).on_press(Message::AddCodexHook).into()],
            )
        } else {
            let remove = small("Remove hook").style(button::secondary).on_press(Message::RemoveCodexHook).into();
            match (h.trusted, self.core.codex_hook_last_report()) {
                (_, Some(at)) => {
                    let at: chrono::DateTime<chrono::Local> = at.into();
                    (format!("Active · last report at {}.", at.format("%H:%M:%S")), vec![remove])
                }
                (true, None) => ("Added and trusted.".to_owned(), vec![remove]),
                (false, None) => (
                    "Added, but not trusted in Codex yet.".to_owned(),
                    vec![
                        small("Trust in Codex…").style(button::primary).on_press(Message::TrustCodexHook).into(),
                        remove,
                    ],
                ),
            }
        };
        section(
            "Integrations",
            column![
                text("Codex session hook").size(14).font(BOLD),
                muted_text("Lets TermRoost resume the right Codex conversation in each terminal."),
                text(status).size(13),
                row(actions).spacing(8),
            ]
            .spacing(8),
        )
    }

    fn terminal_section<'a>(&'a self, st: &'a SettingsState) -> Element<'a, Message> {
        let platform = {
            let sh = shell::default_shell();
            std::iter::once(sh.program).chain(sh.args).collect::<Vec<_>>().join(" ")
        };
        section(
            "Terminal",
            column![
                row![
                    text("Font size").size(13).width(120),
                    slider(8.0_f32..=32.0, self.prefs.font_size, Message::FontSize).step(1.0_f32).width(260),
                    text(format!("{} px", self.prefs.font_size)).size(13),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                row![
                    text("Default shell").size(13).width(120),
                    text_input(&format!("platform default: {platform}"), &st.shell_input)
                        .on_input(Message::ShellInput)
                        .on_submit(Message::ShellCommit)
                        .size(13)
                        .padding(5),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                muted_text(
                    "Used for new terminals in projects without their own shell. Saved when you press Enter or \
                     leave settings.",
                ),
            ]
            .spacing(12),
        )
    }
}

/// A clickable miniature terminal in a theme's colors.
fn terminal_card<'a>(label: String, t: &ThemeDef, selected: bool, msg: Message) -> Element<'a, Message> {
    let term = t.terminal.clone();
    let line = |s: &'a str, c: Hex| text(s).font(MONO).size(12).color(c.color());
    let dots = row(term.ansi[..8].iter().map(|c| line("●", *c).into())).spacing(1);
    let screen = container(
        column![row![line("~ $ ", term.ansi[2]), line("claude", term.foreground)], line("✓ ready", term.ansi[4]), dots]
            .spacing(1),
    )
    .padding(8)
    .width(150)
    .style(move |_: &iced::Theme| container::Style {
        background: Some(term.background.color().into()),
        border: Border::default().rounded(4),
        ..container::Style::default()
    });
    button(column![screen, text(label).size(12)].spacing(4).align_x(Alignment::Center))
        .padding(4)
        .on_press(msg)
        .style(move |th: &iced::Theme, status| {
            let p = th.extended_palette();
            button::Style {
                background: matches!(status, button::Status::Hovered).then(|| p.background.weak.color.into()),
                text_color: p.background.base.text,
                border: Border {
                    width: if selected { 2.0 } else { 1.0 },
                    color: if selected { p.primary.base.color } else { p.background.strong.color },
                    radius: 6.0.into(),
                },
                ..button::Style::default()
            }
        })
        .into()
}

/// Mock interface and terminal drawn with a theme's colors.
fn preview<'a>(t: &ThemeDef) -> Element<'a, Message> {
    let ui = t.ui.clone();
    let chip = |label: &'a str, bg: Hex, fg: Hex| -> Element<'a, Message> {
        container(text(label).size(12).color(fg.color()))
            .padding([3, 8])
            .style(move |_: &iced::Theme| container::Style {
                background: Some(bg.color().into()),
                border: Border::default().rounded(4),
                ..container::Style::default()
            })
            .into()
    };
    let on = |c: Hex| if c.luminance() > 0.4 { Hex(0, 0, 0) } else { Hex(255, 255, 255) };
    let ui_mock = container(
        column![
            text("Interface").size(13).color(ui.text.color()),
            text("Projects, sessions and tasks use these colors.").size(12).color(ui.text.color()),
            row![
                chip("Accent", ui.primary, on(ui.primary)),
                chip("Success", ui.success, on(ui.success)),
                chip("Warning", ui.warning, on(ui.warning)),
                chip("Danger", ui.danger, on(ui.danger)),
            ]
            .spacing(6),
        ]
        .spacing(8),
    )
    .padding(12)
    .width(Length::FillPortion(2))
    .style(move |_: &iced::Theme| container::Style {
        background: Some(ui.background.color().into()),
        border: Border { width: 1.0, color: Color { a: 0.3, ..ui.text.color() }, radius: 6.0.into() },
        ..container::Style::default()
    });

    let term = t.terminal.clone();
    let line = |s: &'a str, c: Hex| text(s).font(MONO).size(13).color(c.color());
    let ansi_dots = row(term.ansi.iter().map(|c| line("██", *c).into())).spacing(0);
    let selection = container(line("selected text", term.foreground)).style(move |_: &iced::Theme| container::Style {
        background: Some(term.selection.color().into()),
        ..container::Style::default()
    });
    let cursor = container(line(" ", term.background)).style(move |_: &iced::Theme| container::Style {
        background: Some(term.cursor.color().into()),
        ..container::Style::default()
    });
    let term_mock = container(
        column![
            row![line("~/project $ ", term.ansi[2]), line("cargo test", term.foreground)],
            line("test result: ok. 27 passed", term.ansi[10]),
            line("warning: unused variable `x`", term.ansi[3]),
            line("error[E0425]: cannot find value", term.ansi[1]),
            row![line("● ", term.ansi[4]), line("Reading src/main.rs", term.foreground)],
            row![selection, line(" ", term.foreground), cursor],
            ansi_dots,
        ]
        .spacing(2),
    )
    .padding(12)
    .width(Length::FillPortion(3))
    .style(move |_: &iced::Theme| container::Style {
        background: Some(term.background.color().into()),
        border: Border { width: 1.0, color: Color { a: 0.3, ..term.foreground.color() }, radius: 6.0.into() },
        ..container::Style::default()
    });
    row![ui_mock, term_mock].spacing(12).into()
}

fn shortcuts_section<'a>() -> Element<'a, Message> {
    const SHORTCUTS: [(&str, &str); 13] = [
        ("Ctrl+Shift+N", "New project"),
        ("Ctrl+Shift+T", "New session"),
        ("Ctrl+Shift+E / Ctrl+Shift+O", "Split right / split down"),
        ("Ctrl+Shift+W", "Close terminal"),
        ("Ctrl+Tab / Ctrl+Shift+Tab", "Next / previous session"),
        ("Ctrl+Shift+PgDn / PgUp", "Next / previous project"),
        ("Alt+Arrows", "Move between terminals"),
        ("Ctrl+Shift+C / Ctrl+Shift+V", "Copy / paste"),
        ("Right click", "Copy selection, or paste"),
        ("Shift+PgUp / Shift+PgDn", "Scroll history"),
        ("Ctrl+Shift+K", "Show / hide tasks"),
        ("Ctrl+Shift+= / - / 0", "Font size bigger / smaller / reset"),
        ("Shift+Enter", "Newline in Claude Code / Codex"),
    ];
    section(
        "Keyboard shortcuts",
        Column::with_children(
            SHORTCUTS.iter().map(|(k, d)| row![text(*k).font(MONO).size(13).width(260), text(*d).size(13)].into()),
        )
        .spacing(6),
    )
}
