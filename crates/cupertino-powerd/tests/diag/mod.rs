//! Diagnostic events in tests: a journal socket of the test's own, and the process of its own a fresh emitter needs
//! (one emitter per process).

#![allow(dead_code)]

use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use cupertino_diagnostic_events::{journal, Record, Severity};

/// Set in the child process: the test named runs its body.
const CHILD: &str = "DIAG_TEST_CHILD";

/// What a child prints when its test is skipped (`cupertino_test_bus::missing_tool`).
const SKIPPED: &str = "SKIPPED:";

/// Whether this process is the child of [`child`].
pub fn is_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

/// Runs the test `name` of this binary again in a process of its own whose events go to `journal`, and returns what it
/// did; `None` when the child skipped its test.
pub fn child(name: &str, journal: &Journal) -> Option<Output> {
    let state = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .args(["--exact", name, "--nocapture", "--test-threads", "1"])
        .env(CHILD, "1")
        .env("XDG_STATE_HOME", state.path())
        .env("HOME", state.path().join("home"))
        .env_remove("XDG_SESSION_ID")
        .env_remove("STATE_DIRECTORY")
        .env(journal::SOCKET_ENV, journal.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the child failed: {stderr}");
    if stderr.contains(SKIPPED) {
        eprintln!("{stderr}");
        return None;
    }
    Some(output)
}

/// A journal socket bound in a temporary directory, which [`journal::SOCKET_ENV`] names to a process.
pub struct Journal {
    _dir: tempfile::TempDir,
    path: PathBuf,
    socket: UnixDatagram,
}

impl Journal {
    pub fn new() -> Journal {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.socket");
        let socket = UnixDatagram::bind(&path).unwrap();
        socket.set_nonblocking(true).unwrap();
        Journal {
            _dir: dir,
            path,
            socket,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The datagrams sent so far.
    pub fn read(&self) -> Vec<Datagram> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; 1 << 16];
        while let Ok(n) = self.socket.recv(&mut buf) {
            let bytes = buf[..n].to_vec();
            let fields = journal::parse(&bytes).expect("a datagram of the native protocol");
            let json = fields
                .iter()
                .find(|(k, _)| k == "CUPERTINO_DIAG_JSON")
                .map(|(_, v)| String::from_utf8(v.clone()).unwrap())
                .expect("a datagram with the record");
            let record = Record::parse(&json).unwrap();
            out.push(Datagram {
                bytes,
                fields,
                record,
            });
        }
        out
    }

    /// The one datagram of `id` sent so far.
    pub fn one(&self, id: &str) -> Datagram {
        let mut all: Vec<Datagram> = self
            .read()
            .into_iter()
            .filter(|d| d.record.id == id)
            .collect();
        assert_eq!(all.len(), 1, "one event {id}");
        all.remove(0)
    }
}

/// One datagram: its bytes, its fields and the record it carries.
pub struct Datagram {
    pub bytes: Vec<u8>,
    pub fields: Vec<(String, Vec<u8>)>,
    pub record: Record,
}

impl Datagram {
    /// A field of the journal entry.
    pub fn field(&self, key: &str) -> &str {
        let value = &self
            .fields
            .iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("no {key}"))
            .1;
        std::str::from_utf8(value).unwrap()
    }

    /// A field of the event, as text.
    pub fn text(&self, key: &str) -> &str {
        self.record
            .fields
            .get(key)
            .unwrap_or_else(|| panic!("no field {key} in {}", self.record.to_json()))
            .as_str()
            .unwrap()
    }

    /// Checks the event's id, severity and the journal's priority for it, and that `secrets` appear nowhere in the
    /// datagram.
    pub fn check(&self, id: &str, severity: Severity, secrets: &[&str]) {
        assert_eq!(self.record.id, id);
        assert_eq!(self.record.severity, severity);
        assert_eq!(self.field("CUPERTINO_DIAG_ID"), id);
        assert_eq!(self.field("CUPERTINO_DIAG_SCHEMA"), "1");
        assert_eq!(self.field("PRIORITY"), severity.priority().to_string());
        for secret in secrets {
            assert!(
                !self
                    .bytes
                    .windows(secret.len())
                    .any(|w| w == secret.as_bytes()),
                "{secret:?} is in the datagram: {}",
                String::from_utf8_lossy(&self.bytes)
            );
        }
    }
}
