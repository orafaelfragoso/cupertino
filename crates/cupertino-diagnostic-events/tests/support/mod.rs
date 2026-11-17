//! What a test that installs the process's emitter needs: a state directory, a home and a session of its own, and no
//! journal unless the test names one. Every test is a process of its own under nextest, so the environment it sets and the
//! emitter it installs reach no other test.

use std::path::Path;

/// Point the process at `state`: its state directory, its home, no session, and a journal socket that is not there.
pub fn isolate(state: &Path) {
    std::env::set_var("XDG_STATE_HOME", state);
    std::env::set_var("HOME", state.join("home"));
    std::env::remove_var("XDG_SESSION_ID");
    std::env::set_var(
        cupertino_diagnostic_events::journal::SOCKET_ENV,
        state.join("no-journal"),
    );
}
