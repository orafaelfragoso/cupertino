//! What the session observer last said: appearance, display options and lock state.

use crate::{Error, Session, Shell, ShellEvent};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_protocol_app::AppEvent;
use cupertino_wayland_client::{Connection, Event, ExtensionEvent};
use std::time::Duration;

/// The session as of the last event of its observer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionState {
    pub dark: bool,
    pub display: DisplayOptions,
    pub locked: bool,
}

/// One event of the session's state, as [`SessionState::apply`] took it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SessionChange {
    Appearance { dark: bool },
    DisplayOptions(DisplayOptions),
    Lock { locked: bool },
}

impl SessionChange {
    fn of_app(event: &AppEvent) -> Option<SessionChange> {
        match *event {
            AppEvent::Appearance { dark } => Some(SessionChange::Appearance { dark }),
            AppEvent::DisplayOptions(options) => Some(SessionChange::DisplayOptions(options)),
            AppEvent::SessionLock { locked } => Some(SessionChange::Lock { locked }),
            AppEvent::ScrollCount { .. }
            | AppEvent::ScrollBars { .. }
            | AppEvent::SteadyCaret { .. }
            | AppEvent::MenuBar { .. }
            | AppEvent::MenuReader { .. }
            | AppEvent::BackdropTone { .. }
            | AppEvent::BuffersLost
            | AppEvent::LayerFinished { .. }
            | AppEvent::LayerReplaced { .. }
            | AppEvent::LayerSettled { .. } => None,
        }
    }

    fn of_shell(event: &ShellEvent) -> Option<SessionChange> {
        match *event {
            ShellEvent::SessionLock { locked } => Some(SessionChange::Lock { locked }),
            _ => None,
        }
    }
}

impl SessionState {
    /// Take `event` when it is one of the session's state. What an application's [`cupertino_protocol_app::Appearance`]
    /// tells counts as well.
    pub fn apply(&mut self, event: &ExtensionEvent) -> Option<SessionChange> {
        let change = event
            .downcast_ref::<AppEvent>()
            .and_then(SessionChange::of_app)
            .or_else(|| {
                event
                    .downcast_ref::<ShellEvent>()
                    .and_then(SessionChange::of_shell)
            })?;
        self.set(change);
        Some(change)
    }

    fn set(&mut self, change: SessionChange) {
        match change {
            SessionChange::Appearance { dark } => self.dark = dark,
            SessionChange::DisplayOptions(options) => self.display = options,
            SessionChange::Lock { locked } => self.locked = locked,
        }
    }
}

impl Shell {
    /// [`Shell::session`], with the session's state at connect read before it returns, so the process draws nothing
    /// on a guess. The other events that arrived meanwhile are returned in order, for the caller's loop.
    pub fn observe(
        &self,
        conn: &mut Connection,
    ) -> Result<(Session, SessionState, Vec<Event>), Error> {
        let session = self.session(conn);
        conn.roundtrip()?;
        let mut state = SessionState::default();
        let mut rest = Vec::new();
        for event in conn.wait(Some(Duration::ZERO))? {
            match event {
                Event::Extension(ref e) if state.apply(e).is_some() => {}
                event => rest.push(event),
            }
        }
        Ok((session, state, rest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_takes_each_change_of_the_session() {
        let options = DisplayOptions {
            reduce_motion: true,
            ..DisplayOptions::default()
        };
        let mut state = SessionState::default();
        for event in [
            SessionChange::of_app(&AppEvent::Appearance { dark: true }),
            SessionChange::of_app(&AppEvent::DisplayOptions(options)),
            SessionChange::of_shell(&ShellEvent::SessionLock { locked: true }),
        ] {
            state.set(event.expect("a change of the session"));
        }
        assert_eq!(
            state,
            SessionState {
                dark: true,
                display: options,
                locked: true
            }
        );
        state.set(SessionChange::Lock { locked: false });
        assert!(!state.locked);
    }

    #[test]
    fn requests_and_scroll_counts_are_not_the_session_state() {
        assert_eq!(
            SessionChange::of_shell(&ShellEvent::LockRequested { explicit: true }),
            None
        );
        assert_eq!(SessionChange::of_shell(&ShellEvent::LogOutRequested), None);
        assert_eq!(
            SessionChange::of_app(&AppEvent::ScrollCount {
                vertical: true,
                count: 1
            }),
            None
        );
    }
}
