//! How output is written while a command runs, and the stream rules.
//!
//! # JSON lines (`--format jsonl`)
//!
//! Every line is one JSON object with, in this order, `schema` ([`STREAM_SCHEMA`]), `command`, `request_id`, `seq`
//! (0, 1, 2… without a gap) and `type`, then the fields of its type:
//!
//! | `type` | Fields | When |
//! |---|---|---|
//! | `event` | `event` (a snake_case name, stable per command), `data` | [`Sink::event`] |
//! | `error` | `error`: `{kind, code, message}` | [`Sink::warn`]: a problem the command continues past |
//! | `result` | `data`: what the JSON envelope's `data` holds for a one-shot command | the command succeeded |
//! | `end` | `ok`, `exit_code`; `error` when `ok` is false; `reason: "stopped"` when a stop ended a follow stream | always, exactly once, last |
//!
//! A [`Run::Once`](crate::Run::Once) command prints `result` and `end`. A usage error before a command starts prints
//! `end` alone. A failed command prints `end` with its `error` and no `result`.
//!
//! # The other forms
//!
//! - `human`: an event prints its human line on stdout as it happens, a warning goes to stderr as
//!   `cupertino: warning: <message>`, the final reply's text follows on stdout, and a failure goes to stderr as
//!   `cupertino: <message>`. Nothing else is printed.
//! - `json`: one envelope at the end. A stream command's `data` is its final data object with two fields added:
//!   `events` (`[{event, data}]`, in order) and `warnings` (`[{kind, code, message}]`). A failed stream gives the error
//!   envelope, without the events. A follow stream in this form holds its events until it stops.
//!
//! In `json` and `jsonl` nothing goes to stderr.
//!
//! # A closed output
//!
//! Once stdout cannot be written, [`Sink::event`] and [`Sink::warn`] return [`Closed`] and the command should return
//! at once (`?` turns [`Closed`] into a [`Failure`]). Whatever it returns then, the runner writes nothing more:
//! - a closed pipe (`EPIPE`, `cupertino … | head -1`) is how a reader says it has seen enough: exit 0, stderr empty;
//! - any other write error (`ENOSPC`, `EIO`): exit 6, with one stderr line `cupertino: cannot write the output: …` in
//!   human form only. Its `kind`/`code` is `temporary`/`output_failed`, though no record can carry it.
//!
//! # Stopping
//!
//! The [`StopFlag`] of the runner asks a running stream to stop; the real command's flag is set by SIGINT and SIGTERM,
//! whose handlers are installed only when a [`Run::Stream`](crate::Run::Stream) command starts (a one-shot command
//! keeps the default actions) and both reset after the first signal, so a second one of either ends the process at
//! once, by the signal's default action. They are
//! installed without `SA_RESTART`, so a blocking system call returns `EINTR`; a command should still wait in steps (a
//! `poll` with a timeout, for example) and check [`Sink::stop_requested`] between them. A signal the parent set to be
//! ignored stays ignored.
//!
//! A stream that runs until stopped calls [`Sink::follow`] first. When it returns `Ok` after a stop, the stream ends
//! with `end {ok: true, exit_code: 0, reason: "stopped"}` (and exit 0). A finite stream that sees a stop returns
//! [`Failure::interrupted`] (`temporary`/`interrupted`, exit 6); one that finished its work anyway reports its result
//! as usual.

use crate::{Failure, FailureKind, Format, Reply, SCHEMA, STREAM_SCHEMA};
use serde::Serialize;
use serde_json::value::RawValue;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// The output was closed or cannot be written; the command should return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

impl From<Closed> for Failure {
    /// What a command returns after [`Closed`]; the runner does not print it.
    fn from(_: Closed) -> Self {
        Failure::new(
            FailureKind::Temporary,
            "output_failed",
            "the output was closed",
        )
    }
}

/// Asks a running stream to stop.
pub struct StopFlag {
    set: AtomicBool,
    signals: bool,
}

/// Set by the SIGINT and SIGTERM handler, read by the flags made with [`StopFlag::on_signals`].
static SIGNALLED: AtomicBool = AtomicBool::new(false);

impl StopFlag {
    /// A flag only [`StopFlag::set`] sets.
    pub const fn new() -> Self {
        Self {
            set: AtomicBool::new(false),
            signals: false,
        }
    }

    /// A flag SIGINT and SIGTERM also set once a stream command starts; the process's own flag, kept in a `static`.
    pub const fn on_signals() -> Self {
        Self {
            set: AtomicBool::new(false),
            signals: true,
        }
    }

    /// Asks the stream to stop; any thread may call it.
    pub fn set(&self) {
        self.set.store(true, Ordering::SeqCst);
    }

    pub fn is_set(&self) -> bool {
        self.set.load(Ordering::SeqCst) || (self.signals && SIGNALLED.load(Ordering::SeqCst))
    }

    /// Installs the signal handlers for a flag made with [`StopFlag::on_signals`]; the first call does.
    pub(crate) fn arm(&self) {
        static INSTALLED: std::sync::Once = std::sync::Once::new();
        if self.signals {
            INSTALLED.call_once(install_handlers);
        }
    }
}

impl Default for StopFlag {
    fn default() -> Self {
        Self::new()
    }
}

/// The signals whose handler [`install_handlers`] installed; a signal the parent ignored is not one of them.
static HANDLED: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

/// The signals that ask a stream to stop, in the order of [`HANDLED`].
const STOP_SIGNALS: [libc::c_int; 2] = [libc::SIGINT, libc::SIGTERM];

extern "C" fn on_signal(_: libc::c_int) {
    // Only atomics and sigaction(2), both async-signal-safe. `SA_RESETHAND` already gave the delivered signal its
    // default action back; the other one gets it here, so a second signal of either kind ends the process.
    SIGNALLED.store(true, Ordering::SeqCst);
    for (signal, handled) in STOP_SIGNALS.iter().zip(&HANDLED) {
        if handled.swap(false, Ordering::SeqCst) {
            // SAFETY: a zeroed sigaction is `SIG_DFL` with an empty mask and no flags.
            unsafe {
                let default: libc::sigaction = std::mem::zeroed();
                libc::sigaction(*signal, &default, std::ptr::null_mut());
            }
        }
    }
}

fn install_handlers() {
    for (signal, handled) in STOP_SIGNALS.into_iter().zip(&HANDLED) {
        // SAFETY: the action is fully initialised (zeroed, an empty mask, a handler that only stores an atomic), and
        // the old action is read into a zeroed struct.
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(signal, std::ptr::null(), &mut old) != 0
                || old.sa_sigaction == libc::SIG_IGN
            {
                continue;
            }
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            action.sa_flags = libc::SA_RESETHAND;
            libc::sigemptyset(&mut action.sa_mask);
            // marked first, so a signal right after the install resets it too
            handled.store(true, Ordering::SeqCst);
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                handled.store(false, Ordering::SeqCst);
            }
        }
    }
}

enum State {
    Open,
    /// The reader closed the pipe.
    Closed,
    Failed(io::Error),
}

/// The output of one command line in one format: the records, their sequence numbers and the write state.
pub(crate) struct Lines<'w> {
    format: Format,
    command: &'w str,
    request_id: &'w str,
    out: &'w mut dyn Write,
    err: &'w mut dyn Write,
    seq: u64,
    state: State,
    /// A stream in JSON form: the envelope collects the events and warnings.
    collect: bool,
    events: Vec<Collected>,
    warnings: Vec<Failure>,
}

#[derive(Serialize)]
struct Collected {
    event: String,
    data: Box<RawValue>,
}

#[derive(Serialize)]
struct Head<'a> {
    schema: &'static str,
    command: &'a str,
    request_id: &'a str,
    seq: u64,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct Record<'a, B: Serialize> {
    #[serde(flatten)]
    head: Head<'a>,
    #[serde(flatten)]
    body: B,
}

#[derive(Serialize)]
struct EventBody<'a> {
    event: &'a str,
    data: &'a RawValue,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a Failure,
}

#[derive(Serialize)]
struct ResultBody<'a> {
    data: &'a RawValue,
}

#[derive(Serialize)]
struct EndBody<'a> {
    ok: bool,
    exit_code: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a Failure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

#[derive(Serialize)]
struct Envelope<'a> {
    schema: &'static str,
    command: &'a str,
    request_id: &'a str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a RawValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a Failure>,
}

/// Prints the result of a command line that ran no stream (help, the version, a usage error) and gives its exit code.
pub(crate) fn finish(
    format: Format,
    command: &str,
    request_id: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
    result: Result<Reply, Failure>,
    reason: Option<&str>,
) -> u8 {
    Lines::new(format, command, request_id, out, err).finish(result, reason)
}

impl<'w> Lines<'w> {
    pub(crate) fn new(
        format: Format,
        command: &'w str,
        request_id: &'w str,
        out: &'w mut dyn Write,
        err: &'w mut dyn Write,
    ) -> Self {
        Self {
            format,
            command,
            request_id,
            out,
            err,
            seq: 0,
            state: State::Open,
            collect: false,
            events: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Writes `text` to stdout and flushes it, unless stdout already failed.
    fn write(&mut self, text: &[u8]) -> Result<(), Closed> {
        if !matches!(self.state, State::Open) {
            return Err(Closed);
        }
        match self.out.write_all(text).and_then(|()| self.out.flush()) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.state = if e.kind() == io::ErrorKind::BrokenPipe {
                    State::Closed
                } else {
                    State::Failed(e)
                };
                Err(Closed)
            }
        }
    }

    fn record(&mut self, kind: &'static str, body: impl Serialize) -> Result<(), Closed> {
        if !matches!(self.state, State::Open) {
            return Err(Closed);
        }
        let record = Record {
            head: Head {
                schema: STREAM_SCHEMA,
                command: self.command,
                request_id: self.request_id,
                seq: self.seq,
                kind,
            },
            body,
        };
        let mut line = serde_json::to_vec(&record).expect("a record serializes");
        line.push(b'\n');
        self.seq += 1;
        self.write(&line)
    }

    fn event(&mut self, name: &str, data: Box<RawValue>, human: &str) -> Result<(), Closed> {
        match self.format {
            Format::Human if human.is_empty() => self.alive(),
            Format::Human if human.ends_with('\n') => self.write(human.as_bytes()),
            Format::Human => self.write(format!("{human}\n").as_bytes()),
            Format::Json => {
                self.events.push(Collected {
                    event: name.into(),
                    data,
                });
                self.alive()
            }
            Format::Jsonl => self.record(
                "event",
                EventBody {
                    event: name,
                    data: &data,
                },
            ),
        }
    }

    pub(crate) fn warn(&mut self, failure: &Failure) -> Result<(), Closed> {
        match self.format {
            Format::Human => {
                // stderr is the last place to report anything; a failure to write it is not reported either
                let _ = writeln!(self.err, "cupertino: warning: {}", failure.message);
                self.alive()
            }
            Format::Json => {
                self.warnings.push(failure.clone());
                self.alive()
            }
            Format::Jsonl => self.record("error", ErrorBody { error: failure }),
        }
    }

    fn alive(&self) -> Result<(), Closed> {
        match self.state {
            State::Open => Ok(()),
            _ => Err(Closed),
        }
    }

    /// Prints the command's result and returns the exit code.
    pub(crate) fn finish(mut self, result: Result<Reply, Failure>, reason: Option<&str>) -> u8 {
        let code = result.as_ref().map_or_else(|f| f.kind.exit_code(), |_| 0);
        if matches!(self.state, State::Open) {
            let _ = match (&result, self.format) {
                (Ok(reply), Format::Human) => self.write(reply.human.as_bytes()),
                (Err(failure), Format::Human) => {
                    let _ = writeln!(self.err, "cupertino: {}", failure.message);
                    Ok(())
                }
                (_, Format::Json) => self.envelope(&result),
                (Ok(reply), Format::Jsonl) => self
                    .record("result", ResultBody { data: &reply.data })
                    .and_then(|()| self.end(code, None, reason)),
                (Err(failure), Format::Jsonl) => self.end(code, Some(failure), None),
            };
        }
        match &self.state {
            State::Open => code,
            State::Closed => 0,
            State::Failed(e) => {
                if self.format == Format::Human {
                    let _ = writeln!(self.err, "cupertino: cannot write the output: {e}");
                }
                FailureKind::Temporary.exit_code()
            }
        }
    }

    fn end(
        &mut self,
        code: u8,
        error: Option<&Failure>,
        reason: Option<&str>,
    ) -> Result<(), Closed> {
        self.record(
            "end",
            EndBody {
                ok: error.is_none(),
                exit_code: code,
                error,
                reason,
            },
        )
    }

    fn envelope(&mut self, result: &Result<Reply, Failure>) -> Result<(), Closed> {
        let collected;
        let data = match result {
            Ok(reply) if self.collect => {
                collected = with_events(&reply.data, &self.events, &self.warnings);
                Some(collected.as_ref())
            }
            Ok(reply) => Some(reply.data.as_ref()),
            Err(_) => None,
        };
        let envelope = Envelope {
            schema: SCHEMA,
            command: self.command,
            request_id: self.request_id,
            ok: result.is_ok(),
            data,
            error: result.as_ref().err(),
        };
        let mut line = serde_json::to_vec(&envelope).expect("the envelope serializes");
        line.push(b'\n');
        self.write(&line)
    }
}

/// A stream's final data object with `events` and `warnings` added after its own fields. Data that is not an object,
/// which the stream rules do not allow, is kept under `result`.
fn with_events(data: &RawValue, events: &[Collected], warnings: &[Failure]) -> Box<RawValue> {
    let events = serde_json::to_string(events).expect("events serialize");
    let warnings = serde_json::to_string(warnings).expect("warnings serialize");
    let text = data.get();
    debug_assert!(
        text.starts_with('{'),
        "a stream command's data is a JSON object: {text}"
    );
    let merged = match text.strip_suffix('}') {
        Some(fields) if text.starts_with('{') => {
            let fields = fields.trim_end();
            let comma = if fields == "{" { "" } else { "," };
            format!("{fields}{comma}\"events\":{events},\"warnings\":{warnings}}}")
        }
        _ => format!("{{\"result\":{text},\"events\":{events},\"warnings\":{warnings}}}"),
    };
    RawValue::from_string(merged).expect("the merged data is JSON")
}

/// What a [`Run::Stream`](crate::Run::Stream) command reports through while it runs; see the [module](self) rules.
pub struct Sink<'w> {
    lines: Lines<'w>,
    stop: &'w StopFlag,
    follow: bool,
}

impl<'w> Sink<'w> {
    pub(crate) fn new(mut lines: Lines<'w>, stop: &'w StopFlag) -> Self {
        lines.collect = true;
        Self {
            lines,
            stop,
            follow: false,
        }
    }

    /// The form the output is printed in, for a command whose work differs by form (a stream that only the JSON lines
    /// form can follow, for example).
    pub fn format(&self) -> Format {
        self.lines.format
    }

    /// Reports an event: `name` is a snake_case name stable for the command, `data` its JSON data, `human` the line
    /// people see (empty to print nothing in human form).
    pub fn event(&mut self, name: &str, data: &impl Serialize, human: &str) -> Result<(), Closed> {
        debug_assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "event names are snake_case: {name}"
        );
        let data = serde_json::value::to_raw_value(data).expect("event data serializes to JSON");
        self.lines.event(name, data, human)
    }

    /// Reports a problem the command continues past.
    pub fn warn(&mut self, failure: Failure) -> Result<(), Closed> {
        self.lines.warn(&failure)
    }

    /// Declares a stream that runs until it is stopped; a stop then ends it with exit 0 and `reason: "stopped"`.
    pub fn follow(&mut self) {
        self.follow = true;
    }

    /// Whether SIGINT, SIGTERM or the caller asked the stream to stop.
    pub fn stop_requested(&self) -> bool {
        self.stop.is_set()
    }

    pub(crate) fn finish(self, result: Result<Reply, Failure>) -> u8 {
        let stopped = self.follow && result.is_ok() && self.stop.is_set();
        self.lines.finish(result, stopped.then_some("stopped"))
    }
}
