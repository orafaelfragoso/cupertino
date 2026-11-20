//! Session consent authority (`consent` role, `cupertino-prompts`, `docs/architecture/shell-host.md#consent-and-capture`).

use cupertino_kit::core::WindowKey;

use crate::context::{self, ShellRequest};

/// Opaque identifier of one consent run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Run(u32);

impl Run {
    pub(crate) const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub(crate) const fn raw(self) -> u32 {
        self.0
    }
}

/// Application requesting consent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub desktop_entry: bool,
    pub identity: String,
    pub name: String,
    pub services: u32,
    pub remembered: bool,
}

/// A consent prompt requested by the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub run: Run,
    pub capabilities: u32,
    pub output: String,
    pub resuming: bool,
    pub app: Option<App>,
}

/// User decision on a consent prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Allow,
    Deny,
    Dismiss,
}

/// Consent authority events delivered as [`crate::Event::Consent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Requested(Request),
    Changed { run: Run, phase: u32 },
}

/// Bind as the session's consent authority (`consent` role).
pub fn bind() {
    context::push_request(ShellRequest::ConsentBind);
}

/// Identify the window on which the person answers consent prompts.
pub fn set_window(window: &WindowKey) {
    context::push_request(ShellRequest::ConsentSetWindow {
        window: window.clone(),
    });
}

/// Send the user's answer for `run`.
pub fn answer(run: Run, answer: Answer) {
    context::push_request(ShellRequest::ConsentAnswer { run, answer });
}

/// Stop an active consent run.
pub fn stop(run: Run) {
    context::push_request(ShellRequest::ConsentStop { run });
}
