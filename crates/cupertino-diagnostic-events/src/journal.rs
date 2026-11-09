//! The journald sink of the system services: one datagram of the journal's native protocol per event, sent to
//! `/run/systemd/journal/socket` without libsystemd.
//!
//! A field is `KEY=value\n`, or, when the value holds a newline, `KEY\n`, the value's length as 64-bit little-endian, the
//! value and `\n`. The journal's own limits bound what it keeps.

use std::io;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

use crate::event::Record;

/// The journal's socket.
pub const SOCKET: &str = "/run/systemd/journal/socket";

/// Names another socket, for tests only: a `UnixDatagram` a test binds in a temporary directory.
pub const SOCKET_ENV: &str = "CUPERTINO_DIAG_JOURNAL_SOCKET";

/// The value of `CUPERTINO_DIAG_SCHEMA` on every event; `journalctl CUPERTINO_DIAG_SCHEMA=1` finds them.
pub const SCHEMA_FIELD: &str = "1";

/// The datagram of `record`, whose redacted line is `line` (without its newline), for the program `identifier`.
pub fn datagram(record: &Record, line: &str, identifier: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len() * 2 + 256);
    field(&mut out, "MESSAGE", record.message.as_bytes());
    field(
        &mut out,
        "PRIORITY",
        record.severity.priority().to_string().as_bytes(),
    );
    field(&mut out, "SYSLOG_IDENTIFIER", identifier.as_bytes());
    field(&mut out, "CUPERTINO_DIAG_SCHEMA", SCHEMA_FIELD.as_bytes());
    field(&mut out, "CUPERTINO_DIAG_ID", record.id.as_bytes());
    field(
        &mut out,
        "CUPERTINO_DIAG_SEVERITY",
        record.severity.name().as_bytes(),
    );
    field(&mut out, "CUPERTINO_DIAG_JSON", line.as_bytes());
    out
}

fn field(out: &mut Vec<u8>, key: &str, value: &[u8]) {
    out.extend_from_slice(key.as_bytes());
    if value.contains(&b'\n') {
        out.push(b'\n');
        out.extend_from_slice(&(value.len() as u64).to_le_bytes());
    } else {
        out.push(b'=');
    }
    out.extend_from_slice(value);
    out.push(b'\n');
}

/// Reads the fields of one datagram back; for tests and for checking the encoding.
pub fn parse(mut data: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut fields = Vec::new();
    while !data.is_empty() {
        let end = data.iter().position(|b| *b == b'\n' || *b == b'=')?;
        let key = String::from_utf8(data[..end].to_vec()).ok()?;
        if data[end] == b'=' {
            let rest = &data[end + 1..];
            let nl = rest.iter().position(|b| *b == b'\n')?;
            fields.push((key, rest[..nl].to_vec()));
            data = &rest[nl + 1..];
        } else {
            let rest = &data[end + 1..];
            let len = u64::from_le_bytes(rest.get(..8)?.try_into().ok()?) as usize;
            let value = rest.get(8..8 + len)?;
            if rest.get(8 + len) != Some(&b'\n') {
                return None;
            }
            fields.push((key, value.to_vec()));
            data = &rest[9 + len..];
        }
    }
    Some(fields)
}

/// The socket events are sent to.
pub struct Journal {
    path: PathBuf,
    socket: Option<UnixDatagram>,
}

impl Journal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Journal {
            path: path.into(),
            socket: None,
        }
    }

    /// [`SOCKET`], or the socket [`SOCKET_ENV`] names.
    pub fn system() -> Self {
        Journal::new(
            std::env::var_os(SOCKET_ENV)
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(SOCKET)),
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Sends one datagram; the caller falls back to standard error on an error.
    pub fn send(&mut self, datagram: &[u8]) -> io::Result<()> {
        if self.socket.is_none() {
            let socket = UnixDatagram::unbound()?;
            // a full journal must not hold the service up
            socket.set_nonblocking(true)?;
            self.socket = Some(socket);
        }
        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| io::Error::other("no socket"))?;
        let sent = socket.send_to(datagram, &self.path)?;
        if sent != datagram.len() {
            return Err(io::Error::other("the datagram was cut"));
        }
        Ok(())
    }
}
