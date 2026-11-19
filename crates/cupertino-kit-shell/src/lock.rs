//! Session lock (`ext-session-lock`) API for the lock screen (`docs/architecture/shell-host.md#surfaces-and-roles`).

use crate::context::{self, ShellRequest};

/// Session-lock lifecycle events delivered as [`crate::Event::Lock`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The server has hidden the session and shows only windows with [`crate::Role::Lock`].
    Locked,
    /// The server ended or refused the session lock.
    Finished,
}

/// Ask the server to lock the session (`session_lock` role).
pub fn lock() {
    context::push_request(ShellRequest::LockRequest);
}

/// End an active session lock (`session_lock` role).
pub fn unlock() {
    context::push_request(ShellRequest::LockUnlock);
}
