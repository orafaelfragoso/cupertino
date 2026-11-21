//! The consent questions the portal asks the person on an application's behalf: whether the application may take a
//! screenshot, and whether it may use keyboard shortcuts while another application has the keyboard. The portal
//! (`cupertino-portal`) decides what to ask and keeps the answers; this program only shows the question and says what
//! the person answered, over `org.cupertino.Prompts1` ([`service`]). The questions wait in a pure model
//! ([`model`]).
//!
//! The portal words every row of the shortcuts it lists, so nothing here knows the portal's types.

pub mod model;
pub mod service;

use std::time::{Duration, Instant};

/// How long a consent alert waits after it first showed before a key, a click or a held key's repeats can grant. A
/// key typed for the application just before the alert took the keyboard lands in this time and grants nothing.
pub const ARMING: Duration = Duration::from_millis(500);

/// The responses of `AskScreenshot`.
pub const SCREENSHOT_GRANTED: u32 = 0;
pub const SCREENSHOT_CANCELLED: u32 = 1;
/// The responses of `AskShortcuts`: Allow and Don't Allow are remembered by the portal, a dismissal is not.
pub const SHORTCUTS_ALLOWED: u32 = 0;
pub const SHORTCUTS_DENIED: u32 = 1;
pub const SHORTCUTS_DISMISSED: u32 = 2;

/// The application that asks, as the portal names it from its desktop entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct App {
    /// The app id the portal was given; empty when it has none.
    pub id: String,
    /// The name of its desktop entry, else its id; `None` when the id is empty.
    pub name: Option<String>,
    /// The `Icon=` name of its desktop entry, when that is an icon name.
    pub icon: Option<String>,
}

/// One shortcut of the list, worded by the portal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// What the shortcut does.
    pub description: String,
    /// Its keys, as the person reads them (`⌃⌥T`), or that it has none.
    pub keys: String,
    /// Why it gets no key (`not available: …`); empty when it gets its key.
    pub note: String,
}

/// What the person is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Question {
    /// May `app` take a screenshot of the screen?
    Screenshot { app: App },
    /// May `app` use `rows`, also while the person works in other apps? `allowed` of its shortcuts were allowed
    /// before and are not listed.
    Shortcuts {
        app: App,
        rows: Vec<Row>,
        allowed: u32,
    },
}

impl Question {
    pub fn app(&self) -> &App {
        match self {
            Question::Screenshot { app } | Question::Shortcuts { app, .. } => app,
        }
    }

    /// The response of `choice` to this question.
    pub fn response(&self, choice: Choice) -> u32 {
        match (self, choice) {
            (Question::Screenshot { .. }, Choice::Allow) => SCREENSHOT_GRANTED,
            (Question::Screenshot { .. }, Choice::Deny | Choice::Dismiss) => SCREENSHOT_CANCELLED,
            (Question::Shortcuts { .. }, Choice::Allow) => SHORTCUTS_ALLOWED,
            (Question::Shortcuts { .. }, Choice::Deny) => SHORTCUTS_DENIED,
            (Question::Shortcuts { .. }, Choice::Dismiss) => SHORTCUTS_DISMISSED,
        }
    }
}

/// How the person answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Take Screenshot, or Allow.
    Allow,
    /// Cancel, or Don't Allow.
    Deny,
    /// Escape, the alert closed, the question taken away: nothing is granted and nothing remembered.
    Dismiss,
}

/// When a consent alert may grant: [`ARMING`] after it first showed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arming {
    shown: Instant,
}

impl Arming {
    pub fn new(shown: Instant) -> Self {
        Self { shown }
    }

    /// Whether an answer that grants is taken at `now`.
    pub fn armed(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.shown) >= ARMING
    }

    /// How long until it is armed, from `now`.
    pub fn remaining(&self, now: Instant) -> Duration {
        ARMING.saturating_sub(now.saturating_duration_since(self.shown))
    }
}

/// A call of the interface for the program's loop. Dropping its reply unanswered answers the call as dismissed.
#[derive(Clone, Debug)]
pub enum Request {
    /// Ask `question` for the portal's request at `handle`; the response goes to `reply`.
    Ask {
        handle: String,
        question: Question,
        reply: async_channel::Sender<u32>,
    },
    /// The portal's request at `handle` was closed: take its question down without an answer.
    Close { handle: String },
}
