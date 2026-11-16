//! Notifications about terminals: failed commands and coding agents that
//! finished or need the user, as quiet or as loud as chosen in Settings.
//!
//! The core turns what programs send into [`Alert`]s; this decides how to
//! show them. Every alert for a terminal the user isn't looking at leaves a
//! mark on the terminal, its session and its project until the user looks at
//! it ("subtle"). "Prominent" alerts also pop up inside the window when the
//! terminal is out of view, and become desktop notifications with a flashing
//! taskbar entry when TerminalRooster is in the background.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use termroost_core::{AgentEvent, Alert, PaneId};

/// How loudly to tell the user about one kind of alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Off,
    Subtle,
    Prominent,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Off, Level::Subtle, Level::Prominent];

    pub fn label(self) -> &'static str {
        match self {
            Level::Off => "Off",
            Level::Subtle => "Subtle",
            Level::Prominent => "Prominent",
        }
    }
}

/// The two kinds of alerts, each with its own [`Level`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    /// A command (or a terminal's shell) exited with an error.
    Error,
    /// A coding agent finished or needs the user; or a program asked for a notification.
    Agent,
}

/// Alerts of one category closer together than this, in one terminal, are
/// one event (an agent may report its turn's end in several ways at once).
const REPEAT: Duration = Duration::from_secs(5);

/// Pop-ups inside the window go away on their own after this.
pub const TOAST_LIFETIME: Duration = Duration::from_secs(10);

/// An alert the user has not looked at yet.
#[derive(Clone, Debug)]
pub struct Notice {
    pub category: Category,
    /// What happened, e.g. "Codex finished" or "cargo test failed".
    pub headline: String,
    /// More about it: the exit status, or what the agent said.
    pub detail: Option<String>,
    pub at: Instant,
}

/// Shortens a command line for a headline.
fn short(s: &str, max: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

impl Notice {
    pub fn from_alert(alert: &Alert) -> Notice {
        let (category, headline, detail) = match alert {
            Alert::CommandFailed { code, command } => (
                Category::Error,
                match command {
                    Some(c) => format!("{} failed", short(c, 48)),
                    None => "A command failed".to_owned(),
                },
                Some(format!("Exit status {code}")),
            ),
            Alert::Agent { agent, event, message } => {
                let name = agent.map(|a| a.label()).unwrap_or("The agent");
                let headline = match event {
                    AgentEvent::Done => format!("{name} is waiting for you"),
                    AgentEvent::Failed => format!("{name} stopped with an error"),
                    AgentEvent::Approval => format!("{name} needs your approval"),
                    AgentEvent::Question => format!("{name} has a question"),
                    AgentEvent::SignIn => format!("{name} needs you to sign in"),
                };
                (Category::Agent, headline, message.clone())
            }
            Alert::Notification { title, body } => match title {
                Some(t) => (Category::Agent, t.clone(), Some(body.clone()).filter(|b| !b.is_empty())),
                None => (Category::Agent, short(body, 80), None),
            },
        };
        Notice { category, headline, detail, at: Instant::now() }
    }

    /// The terminal's shell itself exited with an error.
    pub fn shell_exited(code: i32) -> Notice {
        Notice {
            category: Category::Error,
            headline: "The shell exited with an error".into(),
            detail: Some(format!("Exit status {code} · press Enter in the terminal to restart it")),
            at: Instant::now(),
        }
    }

    pub fn glyph(&self) -> &'static str {
        match self.category {
            Category::Error => "✗",
            Category::Agent => "✓",
        }
    }
}

/// The unseen notices of one terminal: the latest of each category.
#[derive(Clone, Debug, Default)]
pub struct PaneNotices {
    pub error: Option<Notice>,
    pub agent: Option<Notice>,
}

impl PaneNotices {
    /// Records a notice. Returns `false` when it only repeats the previous
    /// one of its category (which then takes over any new detail).
    pub fn add(&mut self, notice: Notice) -> bool {
        let slot = match notice.category {
            Category::Error => &mut self.error,
            Category::Agent => &mut self.agent,
        };
        if let Some(old) = slot.as_mut().filter(|n| n.at.elapsed() < REPEAT && n.category == Category::Agent) {
            if notice.detail.is_some() {
                old.detail = notice.detail;
            }
            return false;
        }
        *slot = Some(notice);
        true
    }

    /// The notice to show first: errors before agents.
    pub fn first(&self) -> Option<&Notice> {
        self.error.as_ref().or(self.agent.as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Notice> {
        self.error.iter().chain(self.agent.iter())
    }
}

/// A pop-up inside the window about a terminal that is out of view.
#[derive(Clone, Debug)]
pub struct Toast {
    pub pane: PaneId,
    pub notice: Notice,
    /// Where the terminal is: "Session 2 · my-service".
    pub place: String,
    pub shown: Instant,
}

/// Shows a desktop notification. `clicked` gets the pane when the user
/// clicks it. Runs on its own thread: the system calls can block.
pub fn desktop(title: String, body: String, pane: PaneId, clicked: async_channel::Sender<PaneId>) {
    std::thread::Builder::new()
        .name("terminalrooster-notification".into())
        .spawn(move || {
            if let Err(e) = platform::show(&title, &body, pane, clicked) {
                tracing::warn!(error = %e, "desktop notification failed");
            }
        })
        .ok();
}

/// Prepares desktop notifications (on Windows: registers the app's name and icon).
pub fn init(data_dir: &std::path::Path) {
    let data_dir = data_dir.to_path_buf();
    std::thread::spawn(move || {
        if let Err(e) = platform::init(&data_dir) {
            tracing::warn!(error = %e, "could not set up desktop notifications");
        }
    });
}

#[cfg(windows)]
mod platform {
    use std::path::Path;

    use tauri_winrt_notification::Toast;
    use termroost_core::PaneId;

    const ICON: &[u8] = include_bytes!("../../../packaging/icons/terminalrooster-256.png");

    /// Windows shows toasts of an app that isn't packaged only when its id is
    /// registered (`HKCU\Software\Classes\AppUserModelId\<id>`), with the name
    /// and icon to show. The uninstaller removes the key again.
    pub fn init(data_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let icon = data_dir.join("notification-icon.png");
        if std::fs::read(&icon).ok().as_deref() != Some(ICON) {
            std::fs::write(&icon, ICON)?;
        }
        let key =
            windows_registry::CURRENT_USER.create(format!(r"Software\Classes\AppUserModelId\{}", crate::APP_ID))?;
        key.set_string("DisplayName", crate::DISPLAY_NAME)?;
        key.set_string("IconUri", icon.display().to_string())?;
        Ok(())
    }

    pub fn show(
        title: &str,
        body: &str,
        pane: PaneId,
        clicked: async_channel::Sender<PaneId>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Toast::new(crate::APP_ID)
            .title(title)
            .text1(body)
            .on_activated(move |_| {
                let _ = clicked.send_blocking(pane);
                Ok(())
            })
            .show()?;
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use notify_rust::{Hint, Notification};
    use termroost_core::PaneId;

    pub fn init(_data_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    pub fn show(
        title: &str,
        body: &str,
        pane: PaneId,
        clicked: async_channel::Sender<PaneId>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let handle = Notification::new()
            .appname(crate::DISPLAY_NAME)
            .summary(title)
            .body(body)
            .icon(crate::APP_ID)
            .hint(Hint::DesktopEntry(crate::APP_ID.to_owned()))
            .action("default", "Show")
            .show()?;
        // Blocks this notification's thread until it is clicked or closed.
        handle.wait_for_action(|action| {
            if action == "default" {
                let _ = clicked.send_blocking(pane);
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use termroost_core::AgentKind;

    use super::*;

    #[test]
    fn headlines() {
        let n = Notice::from_alert(&Alert::CommandFailed { code: 101, command: Some("cargo test".into()) });
        assert_eq!(
            (n.category, n.headline.as_str(), n.detail.as_deref()),
            (Category::Error, "cargo test failed", Some("Exit status 101"))
        );
        let n = Notice::from_alert(&Alert::Agent {
            agent: Some(AgentKind::Codex),
            event: AgentEvent::Approval,
            message: None,
        });
        assert_eq!((n.category, n.headline.as_str()), (Category::Agent, "Codex needs your approval"));
    }

    #[test]
    fn repeats_merge() {
        let mut p = PaneNotices::default();
        let done = |message: Option<&str>| {
            Notice::from_alert(&Alert::Agent {
                agent: Some(AgentKind::Codex),
                event: AgentEvent::Done,
                message: message.map(Into::into),
            })
        };
        assert!(p.add(done(None)));
        assert!(!p.add(done(Some("All tests pass"))), "the bell after the state change");
        assert_eq!(p.agent.as_ref().unwrap().detail.as_deref(), Some("All tests pass"));
        // Errors are each worth telling: two failing commands in a row.
        let fail = || Notice::from_alert(&Alert::CommandFailed { code: 1, command: None });
        assert!(p.add(fail()));
        assert!(p.add(fail()));
        assert_eq!(p.first().unwrap().category, Category::Error);
    }
}
