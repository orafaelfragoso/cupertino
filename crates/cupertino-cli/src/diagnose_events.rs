//! `cupertino diagnose events …` and `cupertino diagnose export`: the diagnostic events the desktop's processes recorded
//! (the user store, `session.log` and the system journal, read by `cupertino_diagnostics::events`), and one file with
//! them and the diagnostics report for a bug report.

use crate::{
    Arg, Args, Command, Context, Failure, FailureKind, Group, Reply, Run, Sink, Stability,
    ValueHint,
};
use cupertino_diagnostic_events::store::{Limits, Store};
use cupertino_diagnostic_events::{time, Record};
use cupertino_diagnostics::events::{self, Counts, Filter, FollowFrom, SourceReport, SourceStatus};
use cupertino_diagnostics::Sources;
use serde::Serialize;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Stdio};
use std::time::{Duration, Instant, SystemTime};

pub(crate) const GROUP: Group = Group {
    name: "diagnose",
    summary: "What happened on this machine: the diagnostic events of the desktop's processes",
    description: "Commands that read what the desktop's processes recorded when something failed. Every process \
                  records diagnostic events (schema cupertino.diag.v1): the user's own processes in a bounded store \
                  in ~/.local/state/cupertino/events, the system services in the system journal. The starts and \
                  ends of sessions come from session.log. Nothing is uploaded; the events stay on this machine \
                  until `cupertino diagnose events clear` deletes them or they expire after 14 days.",
    commands: &[COMMANDS],
};

/// The values `--severity` takes, from least to worst.
const SEVERITIES: &[&str] = &["info", "warn", "error", "fatal"];

const SINCE_HELP: &str =
    "events at or after this: a duration back from now (30s, 10m, 24h, 2d) or an RFC 3339 time";

pub(crate) const COMMANDS: &[Command] = &[
    Command {
        path: &["diagnose", "events"],
        summary: "Prints the diagnostic events of the user store, session.log and the system journal, oldest first",
        description: "Reads the diagnostic events the desktop's processes recorded and prints them merged in time \
                      order: the user store (~/.local/state/cupertino/events), the session starts and ends of \
                      session.log, and the system services' events in the system journal. Each event is one line \
                      in human form (its time, severity, component, id and message, then its fields as key=value), \
                      and an `event` record whose data is the cupertino.diag.v1 record in JSON forms. The final \
                      data lists the sources, each `ok`, `empty`, `denied`, `unavailable` or `corrupt` with the \
                      number of lines that are not events, and how many events of each severity were printed.\n\n\
                      Only members of the systemd-journal, wheel or adm groups can read the system services' \
                      events; for anyone else the journal is `denied` and a warning says so.\n\n\
                      --follow keeps printing new events until it is stopped (Ctrl-C), polling the store and \
                      session.log every 250 ms and following the journal.",
        examples: &[
            (
                "cupertino diagnose events --since 24h --severity error",
                "Print the errors and fatal events of the last day.",
            ),
            (
                "cupertino diagnose events --component cupertino-server --id server.gpu",
                "Print the server's GPU events.",
            ),
            (
                "cupertino --format jsonl diagnose events --follow",
                "Print new events as JSON lines until stopped.",
            ),
        ],
        stability: Stability::Stable,
        args: &[
            Arg::option("since", SINCE_HELP),
            Arg::option(
                "component",
                "only the events of this component, such as cupertino-server; repeat it for more",
            )
            .repeat(),
            Arg::option(
                "id",
                "only the events whose id starts with this, such as server.gpu; repeat it for more",
            )
            .repeat(),
            Arg::option("severity", "only the events of this severity or worse").values(SEVERITIES),
            Arg::flag("follow", "keep printing new events until stopped"),
        ],
        run: Run::Stream(events_run),
    },
    Command {
        path: &["diagnose", "events", "clear"],
        summary: "Deletes the diagnostic events of the user store",
        description: "Deletes every segment of the user store (~/.local/state/cupertino/events) under the store's \
                      lock, so no process writes meanwhile. The salt of the store's hashes stays, so later events \
                      still correlate with each other. The system journal and session.log are not changed.\n\n\
                      It asks for --yes; without it, it deletes nothing and fails with \
                      invalid_usage/confirmation_required.",
        examples: &[(
            "cupertino diagnose events clear --yes",
            "Delete the stored events.",
        )],
        stability: Stability::Stable,
        args: &[Arg::flag("yes", "delete without asking")],
        run: Run::Once(clear),
    },
    Command {
        path: &["diagnose", "export"],
        summary: "Writes the diagnostics report, the diagnostic events and the versions to one new JSON file",
        description: "Writes one JSON document (mode 0600) for a bug report: `schema`, `created`, `versions` (the \
                      command's version and commit, the OS image and the kernel), `report` (every diagnostics \
                      check, as cupertino-diagnose --json gives it, its texts through the free-text scrubber), \
                      `events` (the diagnostic events, already redacted when they were recorded) and `sources`.\n\n\
                      --include-logs adds `logs` with server.log, server.log.1 and session.log, each line passed \
                      through the free-text scrubber: home paths, user and host names, addresses and tokens are \
                      removed, with the store's salt so their hashes match the events'. Read the file before \
                      sharing it.\n\n\
                      It never overwrites: a file that exists fails with validation/file_exists.",
        examples: &[
            (
                "cupertino diagnose export --output report.json",
                "Write the report and the events to report.json.",
            ),
            (
                "cupertino diagnose export --output report.json --include-logs --since 2d",
                "Add the scrubbed logs, with the events of the last two days.",
            ),
        ],
        stability: Stability::Stable,
        args: &[
            Arg::option("output", "the file to write; it must not exist").hint(ValueHint::File),
            Arg::flag(
                "include-logs",
                "add server.log, server.log.1 and session.log, scrubbed line by line",
            ),
            Arg::option("since", SINCE_HELP),
        ],
        run: Run::Once(export),
    },
];

/// How long a follow waits between two looks at the store, and at most for the journal.
const FOLLOW_STEP: Duration = Duration::from_millis(250);

/// The longest journal line a follow waits for; a longer one is dropped.
const JOURNAL_LINE_LIMIT: usize = 1 << 20;

/// The schema of `diagnose export`'s document.
pub const EXPORT_SCHEMA: &str = "cupertino.diag.export.v1";

/// The test's `Sources` when it gave some, else this machine's.
fn with_sources<T>(cx: &Context, run: impl FnOnce(&Sources) -> T) -> T {
    match cx.provided::<Sources>() {
        Some(sources) => run(sources),
        None => run(&Sources::system()),
    }
}

fn since(args: &Args, now: SystemTime) -> Result<Option<SystemTime>, Failure> {
    args.value("since")
        .map(|text| {
            events::parse_since(text, now).ok_or_else(|| {
                Failure::usage(
                    "invalid_value",
                    format!(
                        "--since takes a duration such as 30s, 10m, 24h or 2d, or an RFC 3339 time, not `{text}`"
                    ),
                )
            })
        })
        .transpose()
}

fn filter(args: &Args) -> Result<Filter, Failure> {
    Ok(Filter {
        since: since(args, SystemTime::now())?,
        components: args.values("component").to_vec(),
        ids: args.values("id").to_vec(),
        severity: args
            .value("severity")
            .and_then(cupertino_diagnostic_events::Severity::from_name),
    })
}

/// The final data of `diagnose events`.
#[derive(Serialize)]
struct Summary<'a> {
    sources: &'a [SourceReport],
    counts: Counts,
}

fn events_run(args: &Args, cx: &Context, sink: &mut Sink) -> Result<Reply, Failure> {
    let filter = filter(args)?;
    let follow = args.flag("follow");
    with_sources(cx, |sources| {
        if follow {
            sink.follow();
        }
        let (report, mut tail) = events::start(sources, &filter);
        for source in &report.sources {
            warn_about(source, sink)?;
        }
        let mut counts = Counts::default();
        for event in &report.events {
            print(&event.record, &mut counts, sink)?;
        }
        if follow {
            follow_new(
                sources,
                report.follow_from.as_ref(),
                &mut tail,
                &mut counts,
                sink,
            )?;
        }
        let human = summary_text(&report.sources, &counts);
        Ok(Reply::new(
            &Summary {
                sources: &report.sources,
                counts,
            },
            human,
        ))
    })
}

fn warn_about(source: &SourceReport, sink: &mut Sink) -> Result<(), crate::Closed> {
    match source.status {
        SourceStatus::Denied => sink.warn(Failure::new(
            FailureKind::Denied,
            "journal_denied",
            format!(
                "the system journal cannot be read, so the system services' events are missing: {}",
                source.detail.as_deref().unwrap_or("permission denied")
            ),
        )),
        SourceStatus::Corrupt => sink.warn(Failure::new(
            FailureKind::Validation,
            "corrupt_lines",
            format!(
                "{} lines of the {} are not diagnostic events",
                source.corrupt, source.name
            ),
        )),
        _ => Ok(()),
    }
}

fn print(record: &Record, counts: &mut Counts, sink: &mut Sink) -> Result<(), crate::Closed> {
    counts.add(record.severity);
    sink.event("event", record, &line(record))
}

/// `<time>  <severity>  <component>  <id>  <message>  key=value…`
fn line(record: &Record) -> String {
    let mut line = format!(
        "{}  {:<5}  {}  {}  {}",
        record.time,
        record.severity.name(),
        record.component,
        record.id,
        record.message
    );
    for (key, value) in &record.fields.0 {
        let text = match value {
            serde_json::Value::String(s)
                if !s.is_empty() && !s.contains(|c: char| c.is_whitespace() || c == '"') =>
            {
                s.clone()
            }
            other => other.to_string(),
        };
        let _ = write!(line, " {key}={text}");
    }
    if record.suppressed > 0 {
        let _ = write!(line, " suppressed={}", record.suppressed);
    }
    line
}

fn summary_text(sources: &[SourceReport], counts: &Counts) -> String {
    let mut text = format!(
        "{} events: {} fatal, {} error, {} warn, {} info\n",
        counts.events, counts.fatal, counts.error, counts.warn, counts.info
    );
    for source in sources {
        let _ = write!(text, "{}: {}", source.name, source.status_text());
        if let Some(detail) = &source.detail {
            let _ = write!(text, " ({detail})");
        }
        text.push('\n');
    }
    text
}

/// Prints the events written after the first read until a stop or a closed output: the store and `session.log` every
/// [`FOLLOW_STEP`], the journal as `journalctl --follow` prints it. It never blocks longer than one step: the journal's
/// output is read without blocking, after a `poll` with a timeout, and a stop is checked between steps.
fn follow_new(
    sources: &Sources,
    from: Option<&FollowFrom>,
    tail: &mut events::Follow,
    counts: &mut Counts,
    sink: &mut Sink,
) -> Result<(), Failure> {
    let mut journal = match from.map(|from| JournalFollow::spawn(sources, from)) {
        Some(Err(e)) => {
            sink.warn(Failure::new(
                FailureKind::Unavailable,
                "journal_follow_failed",
                format!("the system journal cannot be followed: {e}"),
            ))?;
            None
        }
        Some(Ok(journal)) => Some(journal),
        None => None,
    };
    let mut store_read = Instant::now();
    loop {
        if sink.stop_requested() {
            return Ok(());
        }
        let wait = FOLLOW_STEP.saturating_sub(store_read.elapsed());
        wait_readable(journal.as_ref().and_then(JournalFollow::fd), wait);
        if sink.stop_requested() {
            return Ok(());
        }
        let mut batch = Vec::new();
        if let Some(j) = journal.as_mut() {
            j.read_available(tail.filter(), &mut batch);
        }
        if store_read.elapsed() >= FOLLOW_STEP {
            batch.extend(tail.poll());
            store_read = Instant::now();
        }
        events::sort(&mut batch);
        for event in batch {
            print(&event.record, counts, sink)?;
        }
    }
}

/// Waits until `fd` has something to read, at most `timeout`; without a descriptor it only waits. A signal ends the
/// wait early (the stop handlers are installed without `SA_RESTART`).
fn wait_readable(fd: Option<i32>, timeout: Duration) {
    let mut pollfd = libc::pollfd {
        // a negative descriptor is ignored, which makes the poll a plain wait
        fd: fd.unwrap_or(-1),
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one valid pollfd for the call.
    unsafe { libc::poll(&mut pollfd, 1, ms) };
}

/// The `journalctl --follow` child: its output read without blocking, killed and reaped when dropped.
struct JournalFollow {
    child: Child,
    stdout: ChildStdout,
    buffer: Vec<u8>,
    /// Its output ended.
    done: bool,
}

impl JournalFollow {
    fn spawn(sources: &Sources, from: &FollowFrom) -> io::Result<Self> {
        let mut command = (sources.journal_follow)(from);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("no output pipe"));
        };
        let fd = stdout.as_raw_fd();
        // SAFETY: fcntl on a descriptor this process owns, with plain integer flags.
        let nonblocking = unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
        };
        let error = (!nonblocking).then(io::Error::last_os_error);
        let journal = JournalFollow {
            child,
            stdout,
            buffer: Vec::new(),
            done: false,
        };
        match error {
            // dropped: killed and reaped
            Some(e) => Err(e),
            None => Ok(journal),
        }
    }

    fn fd(&self) -> Option<i32> {
        (!self.done).then(|| self.stdout.as_raw_fd())
    }

    /// Reads what the child has written so far and adds the events of its whole lines that `filter` lets through.
    fn read_available(&mut self, filter: &Filter, out: &mut Vec<events::Event>) {
        if self.done {
            return;
        }
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match self.stdout.read(&mut chunk) {
                Ok(0) => {
                    self.done = true;
                    break;
                }
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // a stop signal: the caller looks at the flag
                Err(e) if e.kind() == io::ErrorKind::Interrupted => break,
                Err(_) => {
                    self.done = true;
                    break;
                }
            }
        }
        let whole = self
            .buffer
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        let lines: Vec<u8> = self.buffer.drain(..whole).collect();
        for line in lines.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            if let events::JournalEntry::Event(record, _) = events::journal_entry(line) {
                if filter.matches(&record) {
                    out.push(events::Event {
                        record: *record,
                        source: events::JOURNAL,
                    });
                }
            }
        }
        if self.buffer.len() > JOURNAL_LINE_LIMIT {
            self.buffer.clear();
        }
        if self.done {
            // it ended on its own: reaped now, not when the follow ends
            let _ = self.child.try_wait();
        }
    }
}

impl Drop for JournalFollow {
    fn drop(&mut self) {
        // SIGKILL cannot be caught, so the wait after it returns at once
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn clear(args: &Args, cx: &Context) -> Result<Reply, Failure> {
    if !args.flag("yes") {
        return Err(Failure::usage(
            "confirmation_required",
            "`diagnose events clear` deletes every stored diagnostic event; run it with --yes to do so",
        ));
    }
    with_sources(cx, |sources| {
        let Some(dir) = sources.store_dir() else {
            return Err(Failure::new(
                FailureKind::Validation,
                "no_home",
                "neither XDG_STATE_HOME nor HOME is set, so there is no event store",
            ));
        };
        let deleted = if std::fs::symlink_metadata(&dir).is_err() {
            0
        } else {
            Store::open(&dir, Limits::default())
                .and_then(|store| store.clear())
                .map_err(|e| {
                    Failure::new(
                        FailureKind::Validation,
                        "store_unavailable",
                        format!(
                            "the event store {} cannot be cleared: {}",
                            dir.display(),
                            e.describe()
                        ),
                    )
                })?
        };
        Ok(Reply::new(
            &serde_json::json!({ "store": dir, "deleted": deleted }),
            format!("deleted {deleted} segments of {}\n", dir.display()),
        ))
    })
}

/// What the build knows of itself and the system, with the field names of a version report.
fn versions(sources: &Sources) -> serde_json::Value {
    let os = cupertino_build_info::read_os_image(&sources.root).unwrap_or_default();
    serde_json::json!({
        "cli": env!("CARGO_PKG_VERSION"),
        "commit": cupertino_build_info::COMMIT,
        "os_image": {
            "name": os.name,
            "id": os.id,
            "version_id": os.version_id,
            "build_id": os.build_id,
            "image_id": os.image_id,
            "image_version": os.image_version,
        },
        "kernel": cupertino_build_info::kernel_in(&sources.root),
    })
}

/// The logs `--include-logs` adds, from the state directory.
const LOGS: [&str; 3] = ["server.log", "server.log.1", "session.log"];

/// Passes the free text under the keys of `keys` through the scrubber, wherever they are in `value`.
fn scrub(
    value: &mut serde_json::Value,
    keys: &[&str],
    policy: &cupertino_diagnostic_events::redact::Policy,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                match v {
                    serde_json::Value::String(s) if keys.contains(&key.as_str()) => {
                        *s = policy.text(s)
                    }
                    other => scrub(other, keys, policy),
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|v| scrub(v, keys, policy)),
        _ => {}
    }
}

fn export(args: &Args, cx: &Context) -> Result<Reply, Failure> {
    let Some(output) = args.value("output") else {
        return Err(Failure::usage(
            "missing_output",
            "`diagnose export` needs --output FILE, a file that does not exist yet",
        ));
    };
    let path = match Path::new(output) {
        p if p.is_absolute() => p.to_path_buf(),
        p => cx
            .current_dir()
            .map_err(|e| {
                Failure::new(
                    FailureKind::Temporary,
                    "write_failed",
                    format!("no working directory: {e}"),
                )
            })?
            .join(p),
    };
    let filter = Filter {
        since: since(args, SystemTime::now())?,
        ..Filter::default()
    };
    let include_logs = args.flag("include-logs");
    with_sources(cx, |sources| {
        let report = events::read(sources, &filter);
        let checks = cupertino_diagnostics::run(sources, &[]);
        // the store's salt when there is a store, so hashes in the logs match the events'
        let policy = events::policy(sources);
        let mut checks_json = serde_json::to_value(&checks).expect("a report serializes");
        scrub(&mut checks_json, &["summary", "value", "fix"], &policy);
        let mut sources_json = serde_json::to_value(&report.sources).expect("sources serialize");
        scrub(&mut sources_json, &["path", "detail"], &policy);
        let records: Vec<&Record> = report.events.iter().map(|e| &e.record).collect();
        let mut document = serde_json::json!({
            "schema": EXPORT_SCHEMA,
            "created": time::format(SystemTime::now()),
            "versions": versions(sources),
            "report": checks_json,
            "events": records,
            "sources": sources_json,
        });
        let mut logs = Vec::new();
        if include_logs {
            let mut map = serde_json::Map::new();
            for name in LOGS {
                let Some(bytes) = sources
                    .state_dir()
                    .and_then(|state| std::fs::read(state.join(name)).ok())
                else {
                    continue;
                };
                let text = String::from_utf8_lossy(&bytes);
                let lines: Vec<String> = text.lines().map(|l| policy.text(l)).collect();
                map.insert(name.to_owned(), lines.into());
                logs.push(name);
            }
            document["logs"] = map.into();
        }
        let mut bytes = serde_json::to_vec_pretty(&document).expect("the document serializes");
        bytes.push(b'\n');
        write_new(&path, &bytes)?;
        Ok(Reply::new(
            &serde_json::json!({
                "output": path,
                "checks": checks.checks.len(),
                "events": records.len(),
                "logs": logs,
            }),
            format!(
                "wrote {}: {} checks, {} events{}\n",
                path.display(),
                checks.checks.len(),
                records.len(),
                if logs.is_empty() {
                    String::new()
                } else {
                    format!(", {}", logs.join(", "))
                }
            ),
        ))
    })
}

/// Writes `bytes` to a new file `path`, mode 0600; an existing file (a symbolic link too) is never replaced.
fn write_new(path: &PathBuf, bytes: &[u8]) -> Result<(), Failure> {
    let failed = |e: io::Error| {
        Failure::new(
            FailureKind::Temporary,
            "write_failed",
            format!("cannot write {}: {e}", path.display()),
        )
    };
    let mut file = OpenOptions::new()
        .write(true)
        // O_EXCL
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| match e.kind() {
            io::ErrorKind::AlreadyExists => Failure::new(
                FailureKind::Validation,
                "file_exists",
                format!("{} exists; choose a file that does not", path.display()),
            ),
            _ => failed(e),
        })?;
    if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(failed(e));
    }
    Ok(())
}
