//! What the tests share: a process isolated in a state directory of its own, the policy and origin of a record made by
//! hand, and reading back the user store. Every test is a process of its own under nextest, so the environment it sets
//! and the emitter it installs reach no other test.

use std::path::Path;

use cupertino_diagnostic_events::redact::{Policy, Salt};
use cupertino_diagnostic_events::{journal, store, Origin, Record};

/// Point the process at `state`: its state directory, its home, no session, and a journal socket that is not there.
pub fn isolate(state: &Path) {
    std::env::set_var("XDG_STATE_HOME", state);
    std::env::set_var("HOME", state.join("home"));
    std::env::remove_var("XDG_SESSION_ID");
    std::env::set_var(journal::SOCKET_ENV, state.join("no-journal"));
}

/// The policy of a process whose home is `/home/alex`, user `alex` and host `studio`, under a fixed salt.
pub fn policy() -> Policy {
    Policy::new(
        Some(Path::new("/home/alex")),
        Some("alex"),
        Some("studio"),
        Salt::from_bytes([7; 16]),
    )
}

/// Where a record made by hand comes from.
pub fn origin() -> Origin {
    Origin {
        time: "2026-11-09T14:03:27.512Z".into(),
        boot_id: Some("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0".into()),
        session: Some("3".into()),
        component: "cupertino-server".into(),
        version: "0.1.0".into(),
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        pid: 4242,
    }
}

/// The records of the user store under the state directory `state`, oldest first.
pub fn stored(state: &Path) -> Vec<Record> {
    store::read(&state.join("cupertino/events"))
        .unwrap()
        .records
        .into_iter()
        .map(|r| r.record)
        .collect()
}

/// Whether `needle` occurs in `haystack`.
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
