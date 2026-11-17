//! The settings page: appearance (themes), terminal options and shortcuts.

use iced::widget::{
    Column, button, checkbox, column, container, pick_list, row, scrollable, slider, space, text, text_input,
};
use iced::{Alignment, Border, Color, Element, Length};
use termroost_core::session::shell::{self, ShellOption};
use termroost_core::session::{bundled_bash, bundled_nvim};

use crate::app::{App, Message, SettingsState};
use crate::color_picker;
use crate::editor::FileEditor;
use crate::keymap::{Action, Combo};
use crate::notify::{Category, Level};
use crate::sidebar::muted_text;
use crate::theme::{self, BOLD, MONO};
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
            self.editor_section(st),
            self.notifications_section(),
            self.integrations_section(),
            self.shortcuts_section(st),
            muted_text(format!(
                "Everything is stored in {}. Logs are in {}.",
                self.core.data_dir().join(termroost_core::paths::DB_FILE).display(),
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

    fn notifications_section(&self) -> Element<'_, Message> {
        let level_row = |category: Category, label: &'static str, about: &'static str| -> Element<'_, Message> {
            let current = self.prefs.level(category);
            let choices = row(Level::ALL.into_iter().map(|level| {
                let selected = level == current;
                // Always pressable: a disabled button would make the choice look faded.
                button(text(level.label()).size(12))
                    .padding([4, 12])
                    .style(if selected { button::primary } else { button::secondary })
                    .on_press(Message::NotifyLevel(category, level))
                    .into()
            }))
            .spacing(4);
            row![column![text(label).size(13), muted_text(about)].spacing(2).width(Length::Fill), choices]
                .spacing(16)
                .align_y(Alignment::Center)
                .into()
        };
        let integration = self.core.state().settings.shell_integration;
        section(
            "Notifications",
            column![
                level_row(
                    Category::Agent,
                    "Coding agents",
                    "Claude Code, Codex or Grok finishes its turn or waits for you. Programs that ask for a \
                     notification count too.",
                ),
                level_row(
                    Category::Error,
                    "Failed commands",
                    "A command exits with an error status, or a terminal's shell does.",
                ),
                muted_text(
                    "Subtle marks the terminal, its session and its project until you look at the terminal. \
                     Prominent also pops up a note when the terminal is out of view, and a desktop notification with \
                     a flashing taskbar entry while Terminal Rooster is in the background. The terminal you are \
                     looking at never notifies.",
                ),
                row![
                    button(text("Send a test notification").size(12))
                        .padding([4, 10])
                        .style(button::secondary)
                        .on_press(Message::TestNotification),
                ],
                checkbox(integration)
                    .label("Shell integration for bash and PowerShell")
                    .on_toggle(Message::ShellIntegration)
                    .size(16)
                    .text_size(13),
                muted_text(
                    "Lets Terminal Rooster see how each command ends, which failed commands need. It is loaded when a \
                     terminal starts, from Terminal Rooster's own folder; your startup files are not changed. Other \
                     shells can report a command's status themselves with OSC 133;D. Applies to terminals started \
                     from now on.",
                ),
                checkbox(self.prefs.confirm_quit)
                    .label("Ask before quitting while terminals run programs")
                    .on_toggle(Message::ConfirmQuitPref)
                    .size(16)
                    .text_size(13),
            ]
            .spacing(12),
        )
    }

    /// What files from the Files tab open in, and the terminal editor's command.
    fn editor_section<'a>(&'a self, st: &'a SettingsState) -> Element<'a, Message> {
        let current = self.prefs.file_editor;
        let choice = |label: &'a str, value: FileEditor| {
            // Always pressable: a disabled button would make the choice look faded.
            button(text(label).size(12))
                .padding([4, 12])
                .style(if current == value { button::primary } else { button::secondary })
                .on_press(Message::FileEditor(value))
        };
        let choices =
            row![choice("Built-in editor", FileEditor::Builtin), choice("Neovim or Vim", FileEditor::Terminal)]
                .spacing(4);
        let placeholder = match &st.editor_found {
            Some(p) => format!("Empty: {}", p.display()),
            None => "Empty: Neovim or Vim (none found)".to_owned(),
        };
        section(
            "Editor",
            column![
                row![
                    column![
                        text("Edit files with").size(13),
                        muted_text("Files you open from the Files tab show in place of the session's terminals."),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    choices,
                ]
                .spacing(16)
                .align_y(Alignment::Center),
                crate::ui::field(
                    "Terminal editor command",
                    text_input(&placeholder, &self.prefs.editor_command).on_input(Message::EditorCommand).padding(6),
                ),
                self.neovim_row(st),
                muted_text(
                    "Neovim or Vim runs in a terminal, with your own configuration; quitting it (:q) closes the \
                     file. The file's path is added to the command. Leave it empty to use Neovim, or Vim when there \
                     is no Neovim. Ctrl+Shift shortcuts and Alt+arrows stay with Terminal Rooster; every other key \
                     goes to the editor.",
                ),
            ]
            .spacing(12),
        )
    }

    /// Getting Neovim: on Windows Terminal Rooster can install it for itself;
    /// elsewhere the system's package manager does. Nothing when the user's
    /// own Neovim is on PATH.
    fn neovim_row<'a>(&'a self, st: &'a SettingsState) -> Element<'a, Message> {
        use crate::app::InstallState;

        if let Some(path) = &st.nvim_on_path {
            return muted_text(format!("Neovim: {}", path.display())).into();
        }
        if !bundled_nvim::SUPPORTED {
            let how = if cfg!(target_os = "macos") {
                "brew install neovim"
            } else {
                "your package manager, e.g. sudo apt install neovim, sudo dnf install neovim or sudo pacman -S neovim"
            };
            return muted_text(format!("Neovim isn't installed. Install it with {how}, then reopen Settings.")).into();
        }
        let mb = bundled_nvim::download_size().div_ceil(1_000_000);
        let version = bundled_nvim::NVIM_VERSION;
        let install = |label: &'a str| button(text(label).size(13)).padding([4, 10]).on_press(Message::InstallNvim);
        let (action, status): (Option<Element<'a, Message>>, Element<'a, Message>) =
            match (&self.nvim_install, bundled_nvim::installed()) {
                (InstallState::Running(progress), _) => (None, text(progress.to_string()).size(13).into()),
                (_, Some(exe)) => {
                    (None, muted_text(format!("Neovim {version} is installed in {}.", exe.display())).into())
                }
                (InstallState::Failed(e), None) => (
                    Some(install("Try again").into()),
                    text(format!("Could not install Neovim: {e}")).size(13).style(text::danger).into(),
                ),
                (InstallState::Idle, None) => (
                    Some(install("Install and use").into()),
                    muted_text(format!(
                        "Neovim {version}: a {mb} MB download, unpacked into Terminal Rooster's own folder. Nothing \
                         else on the system changes."
                    ))
                    .into(),
                ),
            };
        row![text("Neovim").size(13).width(150)]
            .push(action)
            .push(container(status).width(Length::Fill))
            .spacing(12)
            .align_y(Alignment::Center)
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
                // The trust entry may be for an earlier version of the hook (e.g.
                // TermRoost's), so offer to trust it again until a report shows it runs.
                (true, None) => (
                    "Added and trusted.".to_owned(),
                    vec![
                        small("Trust again…").style(button::secondary).on_press(Message::TrustCodexHook).into(),
                        remove,
                    ],
                ),
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
                muted_text("Lets Terminal Rooster resume the right Codex conversation in each terminal."),
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
                    text("Font size").size(13).width(150),
                    slider(8.0_f32..=32.0, self.prefs.font_size, Message::FontSize).step(1.0_f32).width(260),
                    text(format!("{} px", self.prefs.font_size)).size(13),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                row![
                    text("Default shell").size(13).width(150),
                    text_input(&format!("platform default: {platform}"), &st.shell_input)
                        .on_input(Message::ShellInput)
                        .on_submit(Message::ShellCommit)
                        .size(13)
                        .padding(5)
                        .width(Length::Fill),
                    self.shell_picker(&st.shell_input, &platform, Message::PickDefaultShell),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                muted_text(
                    "Pick an installed shell, or type any command line. Used for new terminals in projects without \
                     their own shell. Typed changes are saved when you press Enter or leave settings.",
                ),
            ]
            .push(bundled_bash::SUPPORTED.then(|| self.bundled_bash_row(&st.shell_input, &platform)))
            .spacing(12),
        )
    }

    /// Dropdown of the installed shells, showing the one `input` (or, when it
    /// is empty, `fallback`) runs. Hidden until detection has found some.
    pub(crate) fn shell_picker<'a>(
        &'a self,
        input: &str,
        fallback: &str,
        on_pick: fn(ShellOption) -> Message,
    ) -> Option<Element<'a, Message>> {
        if self.shells.is_empty() {
            return None;
        }
        let current = if input.trim().is_empty() { fallback } else { input };
        let picker = pick_list(self.shells.as_slice(), crate::app::shell_matching(&self.shells, current), on_pick)
            .placeholder("Installed shells…")
            .text_size(13)
            .padding(5)
            .width(190);
        Some(picker.into())
    }

    /// Windows: download Terminal Rooster's own bash, or say where it is.
    fn bundled_bash_row<'a>(&'a self, input: &str, platform: &str) -> Element<'a, Message> {
        use crate::app::{InstallState, shell_matching};

        let mb = bundled_bash::download_size() / 1_000_000;
        let version = bundled_bash::GIT_VERSION;
        let install = |label: &'a str| button(text(label).size(13)).padding([4, 10]).on_press(Message::InstallBash);
        let (action, status): (Option<Element<'a, Message>>, Element<'a, Message>) =
            match (&self.bash_install, bundled_bash::installed()) {
                (InstallState::Running(progress), _) => (None, text(progress.to_string()).size(13).into()),
                (_, Some(_)) => {
                    let current = if input.trim().is_empty() { platform } else { input };
                    let in_use = shell_matching(&self.shells, current).is_some_and(|o| o.name == bundled_bash::NAME);
                    let dir = bundled_bash::install_dir().map(|d| d.display().to_string()).unwrap_or_default();
                    let note = if in_use {
                        format!("Installed in {dir} and used for new terminals.")
                    } else {
                        format!("Installed in {dir}. Pick “{}” above to use it.", bundled_bash::NAME)
                    };
                    (None, muted_text(note).into())
                }
                (InstallState::Failed(e), None) => (
                    Some(install("Try again").into()),
                    text(format!("Could not install bash: {e}")).size(13).style(text::danger).into(),
                ),
                (InstallState::Idle, None) => (
                    Some(install("Install and use").into()),
                    muted_text(format!(
                        "Bash with coreutils, vim, less, ssh and git, from Git for Windows {version}: a {mb} MB \
                         download, a few hundred MB unpacked into Terminal Rooster's own folder. Nothing else on the \
                         system changes."
                    ))
                    .into(),
                ),
            };
        row![text(bundled_bash::NAME).size(13).width(150)]
            .push(action)
            .push(container(status).width(Length::Fill))
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
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

/// A shortcut as a small key cap with a remove button.
fn key_chip<'a>(combo: Combo, remove: Message) -> Element<'a, Message> {
    key_cap(
        row![
            text(combo.to_string()).font(MONO).size(12),
            button(text("✕").size(10)).padding([0, 3]).style(theme::icon_button).on_press(remove),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
        3.0,
    )
}

/// The key-cap frame around a shortcut.
fn key_cap<'a>(content: impl Into<Element<'a, Message>>, right: f32) -> Element<'a, Message> {
    container(content)
        .padding(iced::Padding { top: 2.0, bottom: 2.0, left: 8.0, right })
        .style(|t: &iced::Theme| {
            let p = t.extended_palette();
            container::Style {
                background: Some(p.background.base.color.into()),
                border: Border { width: 1.0, color: p.background.strong.color, radius: 4.0.into() },
                ..container::Style::default()
            }
        })
        .into()
}

impl App {
    fn shortcuts_section<'a>(&'a self, st: &'a SettingsState) -> Element<'a, Message> {
        let changed = !self.prefs.shortcuts.is_empty();
        let mut col = column![
            row![
                muted_text(
                    "Click + and press the keys for a new shortcut, or ✕ to remove one. Programs in terminals use \
                     plain Ctrl+letter keys such as Ctrl+C, so Ctrl+Shift, Alt and the function keys work best."
                )
                .width(Length::Fill),
                button(text("Reset all").size(12))
                    .padding([4, 10])
                    .style(button::secondary)
                    .on_press_maybe(changed.then_some(Message::ResetShortcuts(None))),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
        ]
        .spacing(6);
        if let Some(note) = &st.shortcut_note {
            col = col.push(container(text(note).size(12)).padding([6, 10]).width(Length::Fill).style(
                |t: &iced::Theme| {
                    let p = t.extended_palette();
                    container::Style {
                        background: Some(p.primary.weak.color.into()),
                        text_color: Some(p.primary.weak.text),
                        border: Border::default().rounded(4),
                        ..container::Style::default()
                    }
                },
            ));
        }
        let mut group = "";
        for action in Action::ALL {
            if action.group() != group {
                group = action.group();
                col = col.push(text(group).size(13).font(BOLD));
            }
            col = col.push(self.shortcut_row(st, action));
        }
        col = col.push(text("Fixed").size(13).font(BOLD));
        for (keys, what) in
            [("Right click", "Copy the selection, or paste"), ("Shift+Enter", "New line in Claude Code / Codex")]
        {
            col = col.push(
                row![text(what).size(13).width(240), key_cap(text(keys).font(MONO).size(12), 8.0)]
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .height(28),
            );
        }
        section("Keyboard shortcuts", col)
    }

    fn shortcut_row<'a>(&'a self, st: &'a SettingsState, action: Action) -> Element<'a, Message> {
        let mut keys = row(self.keymap.combos(action).iter().map(|&c| key_chip(c, Message::RemoveShortcut(action, c))))
            .spacing(6)
            .align_y(Alignment::Center);
        keys = keys.push(if st.recording == Some(action) {
            button(text("Press the keys… (Esc cancels)").size(12))
                .padding([3, 10])
                .style(button::primary)
                .on_press(Message::RecordShortcut(None))
        } else {
            button(text("+").size(14).align_x(Alignment::Center).width(14))
                .padding([1, 6])
                .style(theme::icon_button)
                .on_press(Message::RecordShortcut(Some(action)))
        });
        let reset: Element<'a, Message> = if self.prefs.shortcuts.contains_key(action.id()) {
            button(text("Reset").size(12))
                .padding([3, 8])
                .style(button::text)
                .on_press(Message::ResetShortcuts(Some(action)))
                .into()
        } else {
            space().into()
        };
        row![text(action.label()).size(13).width(240), container(keys).width(Length::Fill), reset]
            .spacing(8)
            .align_y(Alignment::Center)
            .height(28)
            .into()
    }
}
