//! Session observation: lock notice, idle notice, lock/log-out requests, and `activate_app`.

use std::time::Duration;

use crate::context::{self, ShellRequest};

/// Outcome of [`activate_app`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    Activated,
    Resident,
    NotRunning,
}

/// Session-level changes delivered as [`crate::Event::Session`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The session is locked (`true`) or unlocked (`false`). Sent once at start.
    Locked(bool),
    /// The seat had no input for the watched time (`true`), or input resumed (`false`).
    Idle(bool),
    /// The server asks the lock screen to lock the session.
    LockRequested { explicit: bool },
    /// The server asks the session agent to confirm logging out.
    LogOutRequested,
    /// Answer to [`activate_app`].
    AppActivation { serial: u32, result: Activation },
}

/// Active idle watch; dropping it stops watching.
#[derive(Debug)]
pub struct IdleWatch {
    id: u64,
}

impl Drop for IdleWatch {
    fn drop(&mut self) {
        context::push_request(ShellRequest::SessionUnwatchIdle { id: self.id });
    }
}

/// Whether the session is currently locked.
pub fn locked() -> bool {
    context::with_ctx(|ctx| ctx.locked).unwrap_or(false)
}

/// Follow the seat's idleness after `after` without input; dropping the returned [`IdleWatch`] ends it.
pub fn watch_idle(after: Duration) -> IdleWatch {
    let id = context::with_ctx_mut(|ctx| {
        let id = ctx.next_watch_id;
        ctx.next_watch_id += 1;
        ctx.outgoing
            .push_back(ShellRequest::SessionWatchIdle { id, after });
        id
    })
    .unwrap_or(0);
    IdleWatch { id }
}

/// Bring a running application forward by its desktop file ID; the answer arrives as
/// [`Change::AppActivation`] with `serial`.
pub fn activate_app(serial: u32, desktop_id: &str) {
    context::push_request(ShellRequest::SessionActivateApp {
        serial,
        desktop_id: desktop_id.to_owned(),
    });
}
