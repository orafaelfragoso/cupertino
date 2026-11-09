//! The event model: severities, typed values with their privacy class, the builder, and the `cupertino.diag.v1` record.

use std::fmt;
use std::panic::Location;
use std::path::Path;
use std::time::Duration;

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::redact::Policy;

/// The schema of a record; it changes only when a field is removed or changes meaning.
pub const SCHEMA: &str = "cupertino.diag.v1";

/// How bad an event is, from least to worst.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth knowing, nothing failed.
    Info,
    /// Something failed and the program went on.
    Warn,
    /// Something the user notices failed.
    Error,
    /// The process ended.
    Fatal,
}

impl Severity {
    pub const ALL: [Severity; 4] = [
        Severity::Info,
        Severity::Warn,
        Severity::Error,
        Severity::Fatal,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Error => "error",
            Severity::Fatal => "fatal",
        }
    }

    pub fn from_name(name: &str) -> Option<Severity> {
        Severity::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The syslog priority the journal stores: `crit`, `err`, `warning` and `info`.
    pub fn priority(self) -> u8 {
        match self {
            Severity::Info => 6,
            Severity::Warn => 4,
            Severity::Error => 3,
            Severity::Fatal => 2,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What kind of address or device name an [`Value::Address`] is; an event keeps the kind and a salted hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressKind {
    /// An IPv4 or IPv6 address.
    Ip,
    /// A MAC address.
    Mac,
    /// A Bluetooth device address.
    Bluetooth,
    /// A Wi-Fi network name.
    Ssid,
    /// A device's own name (a Bluetooth device's alias, a phone's name).
    DeviceName,
}

impl AddressKind {
    pub fn name(self) -> &'static str {
        match self {
            AddressKind::Ip => "ip",
            AddressKind::Mac => "mac",
            AddressKind::Bluetooth => "bluetooth",
            AddressKind::Ssid => "ssid",
            AddressKind::DeviceName => "device",
        }
    }
}

/// Whose name an [`Value::Identifier`] is; an event keeps only `[user]` or `[host]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    User,
    Host,
}

/// A field's value, typed by what it may hold: the type is its privacy class.
#[derive(Clone, Copy, Debug)]
pub enum Value<'a> {
    /// Text that names nothing private: an app id, a unit, component, connector or output name, an error kind, an
    /// interface or method, a GPU vendor, driver or PCI id, an error's text. It still goes through the free-text
    /// scrubber, so an error's text loses home paths, names, addresses and tokens.
    Public(&'a str),
    Count(u64),
    /// A signed number: an exit status, an errno.
    Int(i64),
    /// Written as whole milliseconds.
    Duration(Duration),
    Bool(bool),
    /// A file's path: kept under system directories and the desktop's own state and configuration, else a hash.
    Path(&'a Path),
    /// Something a person typed, chose or received: typed text, the clipboard, a window title, a notification's body,
    /// a search, a file name, a URL. Never kept.
    UserText(&'a str),
    /// A password, token, key, cookie, `Authorization` value or the text of an authentication conversation. Never
    /// kept.
    Secret(&'a str),
    /// A network or Bluetooth address, an SSID or a device name: its kind and a salted hash.
    Address(AddressKind, &'a str),
    /// A user or host name: `[user]` or `[host]`.
    Identifier(Identity, &'a str),
}

/// An event being built; [`Event::emit`] records it. Its `id` and `message` are fixed by the code: the message is never
/// made from inputs, which go into fields.
#[must_use = "an event is recorded only by `emit`"]
pub struct Event<'a> {
    pub(crate) severity: Severity,
    pub(crate) id: &'static str,
    pub(crate) message: &'static str,
    pub(crate) fields: Vec<(&'static str, Value<'a>)>,
    pub(crate) location: &'static Location<'static>,
}

/// Starts an event `id` (`area.subject.what`, stable and dotted) with a fixed `message`.
#[track_caller]
pub fn event<'a>(severity: Severity, id: &'static str, message: &'static str) -> Event<'a> {
    Event {
        severity,
        id,
        message,
        fields: Vec::new(),
        location: Location::caller(),
    }
}

impl<'a> Event<'a> {
    pub fn field(mut self, key: &'static str, value: Value<'a>) -> Self {
        self.fields.push((key, value));
        self
    }

    /// Records the event in the process's [`Emitter`](crate::Emitter); nothing (but a count) before one is installed.
    /// It never panics, waits at most for one bounded write, and returns nothing: the caller keeps its own error path.
    pub fn emit(self) {
        crate::emitter::emit(self);
    }

    pub fn id(&self) -> &'static str {
        self.id
    }

    /// The record of this event from `origin`, its values reduced by `policy`; `count` is how many events of this id
    /// the process has recorded, this one included.
    pub fn to_record(&self, policy: &Policy, origin: &Origin, count: u64) -> Record {
        let mut fields = Vec::with_capacity(self.fields.len());
        let mut redacted = Vec::new();
        for (key, value) in &self.fields {
            let reduced = policy.value(value);
            if reduced.rewritten {
                redacted.push((*key).to_owned());
            }
            fields.push(((*key).to_owned(), reduced.value));
        }
        Record {
            schema: SCHEMA.to_owned(),
            time: origin.time.clone(),
            boot_id: origin.boot_id.clone(),
            session: origin.session.clone(),
            component: origin.component.clone(),
            version: origin.version.clone(),
            commit: origin.commit.clone(),
            pid: origin.pid,
            id: self.id.to_owned(),
            severity: self.severity,
            message: self.message.to_owned(),
            fields: Fields(fields),
            source: source(self.location),
            redacted,
            count,
            suppressed: 0,
            truncated: false,
        }
    }
}

/// Where and when an event happens: the parts of a record that do not come from the event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub time: String,
    pub boot_id: Option<String>,
    pub session: Option<String>,
    pub component: String,
    pub version: String,
    pub commit: Option<String>,
    pub pid: u32,
}

/// Where an event comes from: the crate and the file's basename with the line, `cupertino-server:gpu_reset.rs:120`.
pub(crate) fn source(location: &Location<'_>) -> String {
    let file = location.file();
    let mut parts = file.rsplit(['/', '\\']);
    let base = parts.next().unwrap_or(file);
    // `crates/<crate>/src/<…>/<file>`: the directory above `src`
    let segments: Vec<&str> = file.split(['/', '\\']).collect();
    let krate = segments
        .iter()
        .rposition(|s| *s == "src" || *s == "tests" || *s == "examples")
        .and_then(|i| i.checked_sub(1))
        .map(|i| segments[i]);
    match krate {
        Some(krate) if !krate.is_empty() && krate != "." => {
            format!("{krate}:{base}:{}", location.line())
        }
        _ => format!("{base}:{}", location.line()),
    }
}

/// An event's fields, in the order they were given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fields(pub Vec<(String, serde_json::Value)>);

impl Fields {
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

impl Serialize for Fields {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Ordered;
        impl<'de> Visitor<'de> for Ordered {
            type Value = Fields;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of fields")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
                let mut fields = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    fields.push(entry);
                }
                Ok(Fields(fields))
            }
        }
        d.deserialize_map(Ordered)
    }
}

/// One `cupertino.diag.v1` event, as a line of the store or the journal holds it. Fields are serialised in this order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub schema: String,
    /// RFC 3339 in UTC, with milliseconds.
    pub time: String,
    /// The kernel's boot id, or `null` when it cannot be read.
    pub boot_id: Option<String>,
    /// The logind session (`XDG_SESSION_ID`), or `null`.
    pub session: Option<String>,
    /// The binary's name.
    pub component: String,
    pub version: String,
    /// The source commit of the build, or `null` when unknown.
    pub commit: Option<String>,
    pub pid: u32,
    pub id: String,
    pub severity: Severity,
    pub message: String,
    pub fields: Fields,
    /// The crate and the file's basename with the line; no directories.
    pub source: String,
    /// The keys of the fields the policy removed or rewrote.
    pub redacted: Vec<String>,
    /// How many events of this id the process has recorded so far, this one and suppressed ones included.
    pub count: u64,
    /// On the summary the rate limit writes when a window closes, how many events of this id were not written in it;
    /// 0 on every other record.
    pub suppressed: u64,
    /// The event was longer than the limit and lost field text or fields.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Why a line is not a record.
#[derive(Debug, PartialEq, Eq)]
pub enum Corrupt {
    /// Not JSON, or not the record's shape.
    Malformed,
    /// A record of another schema.
    Schema(String),
}

impl Record {
    /// One line of JSON, without the newline.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads one line; the schema must be [`SCHEMA`].
    pub fn parse(line: &str) -> Result<Record, Corrupt> {
        #[derive(Deserialize)]
        struct Probe {
            schema: String,
        }
        let probe: Probe = serde_json::from_str(line).map_err(|_| Corrupt::Malformed)?;
        if probe.schema != SCHEMA {
            return Err(Corrupt::Schema(probe.schema));
        }
        serde_json::from_str(line).map_err(|_| Corrupt::Malformed)
    }

    /// The record as one line of at most `limit` bytes (newline included): longer string fields are cut to
    /// [`FIELD_TEXT_LIMIT`] characters first, then fields are dropped from the last, and `truncated` is set.
    pub fn fit(&mut self, limit: usize) -> String {
        let mut line = self.to_json();
        if line.len() < limit {
            line.push('\n');
            return line;
        }
        self.truncated = true;
        for (_, value) in &mut self.fields.0 {
            if let serde_json::Value::String(s) = value {
                if s.chars().count() > FIELD_TEXT_LIMIT {
                    *s = s.chars().take(FIELD_TEXT_LIMIT).chain(['…']).collect();
                }
            }
        }
        line = self.to_json();
        while line.len() >= limit && self.fields.0.pop().is_some() {
            line = self.to_json();
        }
        if line.len() >= limit {
            // only the fixed text is left: the message and the redacted keys go too
            self.message
                .truncate(self.message.floor_char_boundary(FIELD_TEXT_LIMIT));
            self.redacted.clear();
            line = self.to_json();
        }
        line.push('\n');
        line
    }
}

/// How many characters of a string field a truncated event keeps.
pub const FIELD_TEXT_LIMIT: usize = 256;
