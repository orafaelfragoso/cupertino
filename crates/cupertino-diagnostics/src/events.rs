//! Reading the diagnostic events back (`cupertino.diag.v1`) from the three places they are kept, merged in time order:
//!
//! - the user store, `$XDG_STATE_HOME/cupertino/events/` (else `~/.local/state/cupertino/events/`), which the user's
//!   own processes write;
//! - `session.log`, whose `starting` and `the compositor …` lines `cupertino-session` writes. They are turned into
//!   records here, when they are read (`session.start`, `session.end`, `session.end_abnormal`), and never stored a
//!   second time;
//! - the system journal, whose entries with `CUPERTINO_DIAG_SCHEMA=1` the system services write, read with `journalctl`.
//!
//! [`read`] gives every event a [`Filter`] lets through and the state of each source; [`start`] does the same and
//! returns a [`Follow`] that gives the store's and `session.log`'s new lines afterwards.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cupertino_diagnostic_events::redact::{Policy, Salt};
use cupertino_diagnostic_events::store::{self, Contents};
use cupertino_diagnostic_events::{time, Fields, Record, Severity, SCHEMA};
use serde::Serialize;

use crate::Sources;

/// The name of the user store among the sources.
pub const STORE: &str = "store";
/// The name of `session.log` among the sources, and the `source` of the records made from it.
pub const SESSION_LOG: &str = "session.log";
/// The name of the system journal among the sources.
pub const JOURNAL: &str = "journal";
/// The `component` of the records made from `session.log`.
pub const SESSION_COMPONENT: &str = "cupertino-session";

/// The groups whose members may read the whole system journal.
pub const JOURNAL_GROUPS: [&str; 3] = ["systemd-journal", "wheel", "adm"];

/// What journalctl says on standard error when it shows only the caller's own journal, or nothing, for want of
/// permission.
const JOURNAL_DENIED_NOTICES: [&str; 3] = [
    "insufficient permissions",
    "not seeing messages from other users",
    "permission denied",
];

/// Which events to keep. An empty list lets everything through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// Events at or after this time.
    pub since: Option<SystemTime>,
    /// Events of these components (exact names).
    pub components: Vec<String>,
    /// Events whose id starts with one of these.
    pub ids: Vec<String>,
    /// Events of this severity or worse.
    pub severity: Option<Severity>,
}

impl Filter {
    pub fn matches(&self, record: &Record) -> bool {
        if let Some(since) = self.since {
            match time::parse(&record.time) {
                Some(t) if t >= since => {}
                _ => return false,
            }
        }
        (self.components.is_empty() || self.components.contains(&record.component))
            && (self.ids.is_empty() || self.ids.iter().any(|p| record.id.starts_with(p.as_str())))
            && self.severity.is_none_or(|s| record.severity >= s)
    }
}

/// `--since`: a duration back from `now` (a whole number with `s`, `m`, `h` or `d`: `30s`, `10m`, `24h`, `2d`) or an
/// RFC 3339 time (`2026-11-09T14:00:00Z`, `2026-11-09T15:00:00+01:00`). `None` for anything else.
pub fn parse_since(text: &str, now: SystemTime) -> Option<SystemTime> {
    let text = text.trim();
    if let Some(t) = time::parse(text) {
        return Some(t);
    }
    let unit = text.chars().last()?;
    let seconds = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return None,
    };
    let digits = &text[..text.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    now.checked_sub(Duration::from_secs(n.checked_mul(seconds)?))
}

/// What reading the system journal gave: the journal function of [`Sources`] returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalResult {
    /// journalctl could not be run: it is not installed, or failed to start. The text says why.
    Unavailable(String),
    /// What journalctl printed.
    Output(JournalOutput),
}

/// journalctl's output: `--output=json`, one JSON object per entry and line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JournalOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// It exited with 0.
    pub success: bool,
    /// Whether the caller is root or a member of one of [`JOURNAL_GROUPS`]; `None` when it cannot be told.
    pub reader: Option<bool>,
}

/// Where a journal follow starts: after the last entry [`read`] saw, or at a time when it saw none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FollowFrom {
    Cursor(String),
    Since(SystemTime),
}

/// The state of one source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// It was read and gave events.
    Ok,
    /// It was read and gave no event the filter lets through.
    Empty,
    /// The caller may not read it: the system journal of a user outside [`JOURNAL_GROUPS`].
    Denied,
    /// It cannot be read: no store yet, no `session.log`, no journalctl.
    Unavailable,
    /// It was read, and `corrupt` of its lines are not events of this schema.
    Corrupt,
}

impl SourceStatus {
    pub fn name(self) -> &'static str {
        match self {
            SourceStatus::Ok => "ok",
            SourceStatus::Empty => "empty",
            SourceStatus::Denied => "denied",
            SourceStatus::Unavailable => "unavailable",
            SourceStatus::Corrupt => "corrupt",
        }
    }

    /// Whether anything was read from it.
    pub fn readable(self) -> bool {
        matches!(
            self,
            SourceStatus::Ok | SourceStatus::Empty | SourceStatus::Corrupt
        )
    }
}

/// One source and what reading it gave.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceReport {
    /// [`STORE`], [`SESSION_LOG`] or [`JOURNAL`].
    pub name: &'static str,
    pub status: SourceStatus,
    /// How many of its events the filter let through.
    pub events: usize,
    /// Lines that are not records of this schema.
    pub corrupt: usize,
    /// A last line cut short (store only).
    pub partial: usize,
    /// The file or directory read, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Why it is denied or unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SourceReport {
    fn new(name: &'static str, path: Option<&Path>) -> Self {
        SourceReport {
            name,
            status: SourceStatus::Empty,
            events: 0,
            corrupt: 0,
            partial: 0,
            path: path.map(|p| p.display().to_string()),
            detail: None,
        }
    }

    fn unavailable(mut self, detail: impl Into<String>) -> Self {
        self.status = SourceStatus::Unavailable;
        self.detail = Some(detail.into());
        self
    }

    /// `ok`, `empty` or `corrupt` from the counts.
    fn settle(mut self) -> Self {
        self.status = if self.corrupt > 0 {
            SourceStatus::Corrupt
        } else if self.events > 0 {
            SourceStatus::Ok
        } else {
            SourceStatus::Empty
        };
        self
    }

    /// `ok`, `empty`, `denied`, `unavailable` or `corrupt: N`.
    pub fn status_text(&self) -> String {
        match self.status {
            SourceStatus::Corrupt => format!("corrupt: {}", self.corrupt),
            status => status.name().to_owned(),
        }
    }
}

/// One event read back, and which source it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub record: Record,
    /// [`STORE`], [`SESSION_LOG`] or [`JOURNAL`].
    pub source: &'static str,
}

/// How many events of each severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub events: usize,
    pub info: usize,
    pub warn: usize,
    pub error: usize,
    pub fatal: usize,
}

impl Counts {
    pub fn of<'a>(records: impl IntoIterator<Item = &'a Record>) -> Self {
        let mut counts = Counts::default();
        for record in records {
            counts.add(record.severity);
        }
        counts
    }

    pub fn add(&mut self, severity: Severity) {
        self.events += 1;
        match severity {
            Severity::Info => self.info += 1,
            Severity::Warn => self.warn += 1,
            Severity::Error => self.error += 1,
            Severity::Fatal => self.fatal += 1,
        }
    }
}

/// The events of every source in time order, and the state of each source: the store, `session.log`, the journal.
#[derive(Clone, Debug, PartialEq)]
pub struct EventsReport {
    pub events: Vec<Event>,
    pub sources: Vec<SourceReport>,
    /// Where a journal follow starts so it neither misses nor repeats an entry; `None` when the journal cannot be
    /// read.
    pub follow_from: Option<FollowFrom>,
}

impl EventsReport {
    pub fn counts(&self) -> Counts {
        Counts::of(self.events.iter().map(|e| &e.record))
    }

    pub fn source(&self, name: &str) -> Option<&SourceReport> {
        self.sources.iter().find(|s| s.name == name)
    }
}

/// Every event of the store, `session.log` and the journal that `filter` lets through, in time order.
pub fn read(sources: &Sources, filter: &Filter) -> EventsReport {
    start(sources, filter).0
}

/// [`read`], and a [`Follow`] that gives the store's and `session.log`'s lines written after it.
pub fn start(sources: &Sources, filter: &Filter) -> (EventsReport, Follow) {
    let started = SystemTime::now();
    let mut events = Vec::new();
    let mut reports = Vec::new();

    let mut store_tail = sources.store_dir().map(StoreTail::new);
    reports.push(match &mut store_tail {
        None => {
            SourceReport::new(STORE, None).unavailable("neither XDG_STATE_HOME nor HOME is set")
        }
        Some(tail) => {
            let report = SourceReport::new(STORE, Some(&tail.dir));
            match tail.poll(true) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    report.unavailable("no event has been stored yet")
                }
                Err(e) => report.unavailable(format!("the store cannot be read: {e}")),
                Ok(contents) => {
                    let mut report = report;
                    report.corrupt = contents.corrupt;
                    report.partial = contents.partial;
                    for stored in contents.records {
                        if filter.matches(&stored.record) {
                            report.events += 1;
                            events.push(Event {
                                record: stored.record,
                                source: STORE,
                            });
                        }
                    }
                    report.settle()
                }
            }
        }
    });

    let mut session_tail = sources
        .state_dir()
        .map(|state| SessionTail::new(state.join(SESSION_LOG), policy(sources)));
    reports.push(match &mut session_tail {
        None => SourceReport::new(SESSION_LOG, None)
            .unavailable("neither XDG_STATE_HOME nor HOME is set"),
        Some(tail) => {
            let report = SourceReport::new(SESSION_LOG, Some(&tail.path));
            match tail.poll() {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    report.unavailable("no session has run yet")
                }
                Err(e) => report.unavailable(format!("session.log cannot be read: {e}")),
                Ok(records) => {
                    let mut report = report;
                    for record in records.into_iter().filter(|r| filter.matches(r)) {
                        report.events += 1;
                        events.push(Event {
                            record,
                            source: SESSION_LOG,
                        });
                    }
                    report.settle()
                }
            }
        }
    });

    let (journal, mut journal_events, follow_from) =
        journal_source((sources.journal)(filter), filter, started);
    events.append(&mut journal_events);
    reports.push(journal);

    sort(&mut events);
    let follow = Follow {
        store: store_tail,
        session: session_tail,
        filter: filter.clone(),
    };
    (
        EventsReport {
            events,
            sources: reports,
            follow_from,
        },
        follow,
    )
}

/// Sorts by time, keeping the order of events of the same time; an event whose time cannot be read goes first.
pub fn sort(events: &mut [Event]) {
    events.sort_by_cached_key(|e| time::parse(&e.record.time).unwrap_or(UNIX_EPOCH));
}

/// The new lines of the store and `session.log` after [`start`].
pub struct Follow {
    store: Option<StoreTail>,
    session: Option<SessionTail>,
    filter: Filter,
}

impl Follow {
    /// The events written since the last call that the filter lets through, in time order. Segments made, rotated or
    /// deleted meanwhile are fine; a line still being written waits for the next call.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if let Some(Ok(contents)) = self.store.as_mut().map(|t| t.poll(false)) {
            events.extend(
                contents
                    .records
                    .into_iter()
                    .filter(|s| self.filter.matches(&s.record))
                    .map(|s| Event {
                        record: s.record,
                        source: STORE,
                    }),
            );
        }
        if let Some(Ok(records)) = self.session.as_mut().map(SessionTail::poll) {
            events.extend(
                records
                    .into_iter()
                    .filter(|r| self.filter.matches(r))
                    .map(|record| Event {
                        record,
                        source: SESSION_LOG,
                    }),
            );
        }
        sort(&mut events);
        events
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }
}

/// The store, read from where the last read ended in each segment (by its inode, so a segment deleted and made again
/// under the same name is read from its start).
struct StoreTail {
    dir: PathBuf,
    offsets: HashMap<PathBuf, (u64, u64)>,
}

impl StoreTail {
    fn new(dir: PathBuf) -> Self {
        StoreTail {
            dir,
            offsets: HashMap::new(),
        }
    }

    /// The whole lines written since the last poll. A last line without its newline is left for the next poll; the
    /// first poll counts it as `partial`.
    fn poll(&mut self, first: bool) -> io::Result<Contents> {
        let segments = store::segments(&self.dir)?;
        let mut contents = Contents::default();
        let mut present = HashMap::new();
        for segment in segments {
            let mut bytes = Vec::new();
            let mut at = 0;
            let read = File::open(&segment.path).and_then(|mut f| {
                let meta = f.metadata()?;
                at = match self.offsets.get(&segment.path) {
                    Some(&(ino, offset)) if ino == meta.ino() && offset <= meta.len() => offset,
                    // a new file under the same name
                    _ => 0,
                };
                f.seek(SeekFrom::Start(at))?;
                f.read_to_end(&mut bytes).map(|_| meta.ino())
            });
            let ino = match read {
                Ok(ino) => ino,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    contents.vanished += 1;
                    continue;
                }
                Err(e) => return Err(e),
            };
            let whole = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            store::read_lines(&bytes[..whole], &mut contents);
            if first && whole < bytes.len() {
                contents.partial += 1;
            }
            present.insert(segment.path, (ino, at + whole as u64));
        }
        self.offsets = present;
        Ok(contents)
    }
}

/// `session.log`, read from where the last read ended, its lines made into records.
struct SessionTail {
    path: PathBuf,
    offset: u64,
    policy: Policy,
    counts: HashMap<&'static str, u64>,
}

impl SessionTail {
    fn new(path: PathBuf, policy: Policy) -> Self {
        SessionTail {
            path,
            offset: 0,
            policy,
            counts: HashMap::new(),
        }
    }

    fn poll(&mut self) -> io::Result<Vec<Record>> {
        let mut file = File::open(&self.path)?;
        let len = file.metadata()?.len();
        if len < self.offset {
            // a new log
            self.offset = 0;
            self.counts.clear();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let whole = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        self.offset += whole as u64;
        let text = String::from_utf8_lossy(&bytes[..whole]);
        let mut records = Vec::new();
        for line in text.lines() {
            if let Some(mut record) = session_record(line, &self.policy) {
                let id: &'static str = match record.id.as_str() {
                    "session.start" => "session.start",
                    "session.end" => "session.end",
                    _ => "session.end_abnormal",
                };
                let count = self.counts.entry(id).or_insert(0);
                *count += 1;
                record.count = *count;
                records.push(record);
            }
        }
        Ok(records)
    }
}

/// The ids of the records made from `session.log`.
pub const SESSION_IDS: [&str; 3] = ["session.start", "session.end", "session.end_abnormal"];

/// The record of one `session.log` line `<date> <time> <text>` (local time), or `None` for a line that is not a start
/// or an end: `starting` is `session.start`; `the compositor ended normally …` and `the compositor was stopped by the
/// session …` are `session.end`; every other `the compositor …` line is `session.end_abnormal`. Free text in the line
/// (the reason the compositor gave) goes through `policy`'s scrubber. Its `count` is 1; [`read`] numbers them.
pub fn session_record(line: &str, policy: &Policy) -> Option<Record> {
    let stamp = line.get(..19)?;
    let text = line.get(19..)?.strip_prefix(' ')?;
    let when = local_to_utc(stamp)?;
    let mut fields: Vec<(String, serde_json::Value)> = Vec::new();
    let mut redacted = Vec::new();
    let mut put = |key: &str, value: serde_json::Value| fields.push((key.to_owned(), value));
    let (id, severity, message) = if text == "starting" {
        ("session.start", Severity::Info, "the session started")
    } else if let Some(rest) = text.strip_prefix("starting with a limit of ") {
        put("limit_seconds", number_before(rest, " s")?.into());
        ("session.start", Severity::Info, "the session started")
    } else {
        let rest = text.strip_prefix("the compositor ")?;
        let status = number_after(rest, "status ");
        let seconds = number_after(rest, "after ");
        let normal = rest.starts_with("ended normally");
        let stopped = rest.starts_with("was stopped by the session");
        let cause = if normal {
            "normal"
        } else if stopped {
            "stopped"
        } else if rest.starts_with("did not stop in time") {
            "watchdog"
        } else if rest.starts_with("was ended by signal ") {
            put("signal", number_after(rest, "signal ")?.into());
            "signal"
        } else if let Some((_, reason)) = rest
            .starts_with("ended with status ")
            .then(|| rest.split_once(" s: "))
            .flatten()
        {
            let scrubbed = policy.text(reason);
            if scrubbed != reason {
                redacted.push("reason".to_owned());
            }
            put("reason", scrubbed.into());
            "reason"
        } else if rest.starts_with("ended with status ") {
            "status"
        } else {
            let scrubbed = policy.text(text);
            if scrubbed != text {
                redacted.push("text".to_owned());
            }
            put("text", scrubbed.into());
            "unknown"
        };
        put("cause", cause.into());
        put("status", status.unwrap_or(0).into());
        if let Some(seconds) = seconds {
            put("seconds", seconds.into());
        }
        if normal || stopped {
            ("session.end", Severity::Info, "the session ended")
        } else {
            (
                "session.end_abnormal",
                Severity::Error,
                "the compositor ended abnormally",
            )
        }
    };
    Some(Record {
        schema: SCHEMA.to_owned(),
        time: time::format(when),
        boot_id: None,
        session: None,
        component: SESSION_COMPONENT.to_owned(),
        version: String::new(),
        commit: None,
        pid: 0,
        id: id.to_owned(),
        severity,
        message: message.to_owned(),
        fields: Fields(fields),
        source: SESSION_LOG.to_owned(),
        redacted,
        count: 1,
        suppressed: 0,
        truncated: false,
    })
}

/// The whole number after the first `mark` in `text`.
fn number_after(text: &str, mark: &str) -> Option<u64> {
    let (_, rest) = text.split_once(mark)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The whole number at the start of `text`, followed by `end`.
fn number_before(text: &str, end: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    text[digits.len()..].starts_with(end).then_some(())?;
    digits.parse().ok()
}

/// A local time without a zone, `YYYY-MM-DD HH:MM:SS` as `date` writes it, as the instant it names (`mktime`, which
/// reads `TZ` and the system's zone and picks daylight saving time itself).
pub fn local_to_utc(text: &str) -> Option<SystemTime> {
    let b = text.as_bytes();
    if b.len() != 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b' '
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i32> {
        let s = &text[from..to];
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    // SAFETY: an all-zero `tm` is valid; mktime reads and normalises only its fields.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = num(0, 4)? - 1900;
    tm.tm_mon = num(5, 7)? - 1;
    tm.tm_mday = num(8, 10)?;
    tm.tm_hour = num(11, 13)?;
    tm.tm_min = num(14, 16)?;
    tm.tm_sec = num(17, 19)?;
    tm.tm_isdst = -1;
    if !(0..12).contains(&tm.tm_mon) || !(1..=31).contains(&tm.tm_mday) || tm.tm_hour > 23 {
        return None;
    }
    // SAFETY: `tm` is a valid, initialised struct that mktime may modify.
    let secs = unsafe { libc::mktime(&mut tm) };
    let secs = u64::try_from(secs).ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

/// `time` as local wall-clock time, `YYYY-MM-DD HH:MM:SS` (`localtime_r`): the form `session.log` writes.
pub fn local_time(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: an all-zero `tm` is valid, and localtime_r writes only into it.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call.
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return time::format(time);
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// One line of `journalctl --output=json`.
#[derive(Clone, Debug, PartialEq)]
pub enum JournalEntry {
    /// An event, with the entry's cursor.
    Event(Box<Record>, Option<String>),
    /// Not JSON, no `CUPERTINO_DIAG_JSON`, or a value that is not a record of this schema; with the cursor when
    /// there is one.
    Corrupt(Option<String>),
}

/// Reads one line of `journalctl --output=json`. `CUPERTINO_DIAG_JSON` is a string, or an array of byte values for a
/// value that is not UTF-8 or is large; a field the entry holds twice is an array of those, of which the first counts.
pub fn journal_entry(line: &[u8]) -> JournalEntry {
    let Ok(serde_json::Value::Object(entry)) = serde_json::from_slice::<serde_json::Value>(line)
    else {
        return JournalEntry::Corrupt(None);
    };
    let cursor = entry
        .get("__CURSOR")
        .and_then(|c| c.as_str())
        .map(str::to_owned);
    let Some(text) = entry.get("CUPERTINO_DIAG_JSON").and_then(journal_value) else {
        return JournalEntry::Corrupt(cursor);
    };
    match Record::parse(&text) {
        Ok(record) => JournalEntry::Event(Box::new(record), cursor),
        Err(_) => JournalEntry::Corrupt(cursor),
    }
}

fn journal_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(items) if items.iter().all(|i| i.is_u64()) => {
            let bytes: Option<Vec<u8>> = items
                .iter()
                .map(|i| i.as_u64().and_then(|b| u8::try_from(b).ok()))
                .collect();
            String::from_utf8(bytes?).ok()
        }
        serde_json::Value::Array(items) => items.first().and_then(journal_value),
        _ => None,
    }
}

/// The journal's report, events and where a follow starts.
fn journal_source(
    result: JournalResult,
    filter: &Filter,
    started: SystemTime,
) -> (SourceReport, Vec<Event>, Option<FollowFrom>) {
    let report = SourceReport::new(JOURNAL, None);
    let output = match result {
        JournalResult::Unavailable(detail) => {
            return (report.unavailable(detail), Vec::new(), None)
        }
        JournalResult::Output(output) => output,
    };
    let mut report = report;
    let mut events = Vec::new();
    let mut cursor = None;
    let mut entries = 0;
    for line in output.stdout.split(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        entries += 1;
        match journal_entry(line) {
            JournalEntry::Event(record, c) => {
                cursor = c.or(cursor);
                if filter.matches(&record) {
                    report.events += 1;
                    events.push(Event {
                        record: *record,
                        source: JOURNAL,
                    });
                }
            }
            JournalEntry::Corrupt(c) => {
                cursor = c.or(cursor);
                report.corrupt += 1;
            }
        }
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    let notice = JOURNAL_DENIED_NOTICES.iter().any(|n| stderr.contains(n));
    // journalctl's own notice says so; without it, a caller outside the reader groups that saw nothing is denied too
    let denied = notice || (output.reader == Some(false) && entries == 0);
    let follow_from = cursor
        .map(FollowFrom::Cursor)
        .unwrap_or(FollowFrom::Since(started));
    if denied {
        report.status = SourceStatus::Denied;
        report.detail = Some(format!(
            "only members of the {} groups can read the system services' events",
            JOURNAL_GROUPS.join(", ")
        ));
        return (report, events, None);
    }
    if !output.success && entries == 0 {
        let first = String::from_utf8_lossy(&output.stderr)
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("journalctl failed")
            .trim()
            .to_owned();
        return (report.unavailable(first), events, None);
    }
    (report.settle(), events, Some(follow_from))
}

/// The salt of the user store, read and never made, so hashes of `session.log` reasons match the store's; a random
/// one when the store has none.
pub fn store_salt(sources: &Sources) -> Salt {
    sources
        .store_dir()
        .and_then(|dir| fs::read(dir.join("salt")).ok())
        .and_then(|bytes| <[u8; 16]>::try_from(bytes.as_slice()).ok())
        .map(Salt::from_bytes)
        .unwrap_or_else(Salt::random)
}

/// The redaction policy of the user [`Sources`] describes: its `HOME`, `USER` (or `LOGNAME`) and host name, with the
/// store's salt ([`store_salt`]). On the running system it is `Policy::of_process` with that salt.
pub fn policy(sources: &Sources) -> Policy {
    let host = fs::read_to_string(sources.installed("/proc/sys/kernel/hostname")).ok();
    Policy::new(
        sources.var("HOME").map(Path::new),
        sources.var("USER").or_else(|| sources.var("LOGNAME")),
        host.as_deref(),
        store_salt(sources),
    )
}

/// The system journal function of [`Sources::system`]: `journalctl --output=json --no-pager --all
/// CUPERTINO_DIAG_SCHEMA=1 [--since=@SECONDS]`, with no input and the C locale.
pub fn journalctl(filter: &Filter) -> JournalResult {
    let mut command = Command::new("journalctl");
    command.args([
        "--output=json",
        "--no-pager",
        "--all",
        "CUPERTINO_DIAG_SCHEMA=1",
    ]);
    if let Some(since) = filter.since {
        command.arg(format!("--since=@{}", epoch_seconds(since)));
    }
    let output = command
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    match output {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            JournalResult::Unavailable("journalctl is not installed".to_owned())
        }
        Err(e) => JournalResult::Unavailable(format!("journalctl cannot be run: {e}")),
        Ok(output) => JournalResult::Output(JournalOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            success: output.status.success(),
            reader: journal_reader(),
        }),
    }
}

/// The journal follow command of [`Sources::system`]: `journalctl --follow` from `from`, every line from there on.
pub fn journalctl_follow(from: &FollowFrom) -> Command {
    let mut command = Command::new("journalctl");
    command.args([
        "--follow",
        "--output=json",
        "--no-pager",
        "--all",
        "--lines=all",
        "CUPERTINO_DIAG_SCHEMA=1",
    ]);
    match from {
        FollowFrom::Cursor(cursor) => command.arg(format!("--after-cursor={cursor}")),
        FollowFrom::Since(since) => command.arg(format!("--since=@{}", epoch_seconds(*since))),
    };
    command.env("LC_ALL", "C");
    command
}

fn epoch_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Whether the calling process is root or a member of one of [`JOURNAL_GROUPS`]; `None` when its groups cannot be
/// read or none of those groups exists.
pub fn journal_reader() -> Option<bool> {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return Some(true);
    }
    // SAFETY: a size of 0 asks only for the number of groups.
    let n = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    let mut groups = vec![0 as libc::gid_t; usize::try_from(n).ok()?];
    // SAFETY: `groups` has room for `n` entries.
    let n = unsafe { libc::getgroups(n, groups.as_mut_ptr()) };
    groups.truncate(usize::try_from(n).ok()?);
    // SAFETY: getegid has no preconditions.
    groups.push(unsafe { libc::getegid() });
    let mut known = false;
    for name in JOURNAL_GROUPS {
        if let Some(gid) = group_id(name) {
            known = true;
            if groups.contains(&gid) {
                return Some(true);
            }
        }
    }
    known.then_some(false)
}

fn group_id(name: &str) -> Option<libc::gid_t> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: an all-zero `group` is valid; getgrnam_r fills it and points its strings into `buffer`.
    let mut group: libc::group = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::group = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call and `buffer`'s length is given.
    let rc = unsafe {
        libc::getgrnam_r(
            name.as_ptr(),
            &mut group,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    (rc == 0 && !found.is_null()).then_some(group.gr_gid)
}

/// How many events of each id and severity, worst first, then by id.
pub fn by_id<'a>(
    events: impl IntoIterator<Item = &'a Event>,
) -> Vec<(String, Severity, usize, String)> {
    let mut map: BTreeMap<(std::cmp::Reverse<Severity>, String), (usize, String)> = BTreeMap::new();
    for event in events {
        let entry = map
            .entry((
                std::cmp::Reverse(event.record.severity),
                event.record.id.clone(),
            ))
            .or_insert((0, String::new()));
        entry.0 += 1;
        entry.1.clone_from(&event.record.time);
    }
    map.into_iter()
        .map(|((severity, id), (n, last))| (id, severity.0, n, last))
        .collect()
}
