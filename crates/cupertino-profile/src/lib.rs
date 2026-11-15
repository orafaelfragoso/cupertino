//! Frame timelines: the spans a process's frames pass through (`tracing::trace_span!` in the toolkit, its renderers and
//! the compositor), recorded only when `CUPERTINO_PROFILE` asks.
//!
//! - `CUPERTINO_PROFILE=chrome:FILE` writes the process's spans to `FILE` in the Trace Event format (JSON), which
//!   Perfetto (`ui.perfetto.dev`) and `chrome://tracing` open. A path ending in `/` is a directory: each process writes
//!   `<program>-<pid>.json` in it, so one value serves a whole session.
//! - `CUPERTINO_PROFILE=tracy` streams them to the Tracy profiler, in a build with its client (the `tracy` feature and
//!   `--cfg cupertino_tracy`, see the README).
//! - Both: `chrome:DIR/,tracy`.
//!
//! Unset, no layer is installed: a span costs its callsite's check that nothing listens, a load and a compare.
//!
//! [`layer`] is the layer a process with a subscriber of its own adds to it; [`init`] installs one for a process
//! without (the toolkit's processes). The trace's times are the monotonic clock's, the clock of the frame and
//! presentation times, so the files of the processes of one session line up on one timeline: their events concatenated
//! into one array open as one trace, each process its own track.
//!
//! A file is written as the process goes, at most [`FLUSH_DELAY`] after a span ends, a whole event at a time, and
//! closed (its array's `]`) when the process exits. A process killed before that leaves a file without the closing
//! bracket, which the format allows and the viewers read.

use std::cell::RefCell;
use std::fmt::{self, Write as _};
use std::fs::File;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

/// The variable that turns the timelines on: `chrome:FILE`, `chrome:DIR/`, `tracy`, or several, comma separated.
pub const ENV: &str = "CUPERTINO_PROFILE";

/// The target of the events [`interval`] sends: a stretch of time already measured, drawn on a track of its own.
pub const INTERVAL_TARGET: &str = "cupertino_profile::interval";

/// How long after a span ends its event reaches the file, at most: a process killed loses no more than this.
pub const FLUSH_DELAY: Duration = Duration::from_millis(100);

/// Where the spans go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sink {
    /// A trace file in the Trace Event format.
    Chrome(PathBuf),
    /// The Tracy profiler.
    Tracy,
}

/// The sinks a value of [`ENV`] names for the process `program` of id `pid`; a `chrome:` path ending in `/` or naming a
/// directory gets the file `<program>-<pid>.json` in it.
pub fn parse(value: &str, program: &str, pid: u32) -> Result<Vec<Sink>, String> {
    let mut sinks = Vec::new();
    for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if part == "tracy" {
            sinks.push(Sink::Tracy);
            continue;
        }
        let Some(path) = part.strip_prefix("chrome:") else {
            return Err(format!("`{part}` is not chrome:FILE, chrome:DIR/ or tracy"));
        };
        if path.is_empty() {
            return Err("chrome: needs a file, or a directory ending in /".to_owned());
        }
        let path = if path.ends_with('/') || Path::new(path).is_dir() {
            Path::new(path).join(format!("{program}-{pid}.json"))
        } else {
            PathBuf::from(path)
        };
        if sinks.iter().any(|s| matches!(s, Sink::Chrome(_))) {
            return Err("one chrome: sink at most".to_owned());
        }
        sinks.push(Sink::Chrome(path));
    }
    Ok(sinks)
}

/// The process's program name, as files are named: its first argument's file name, with only letters, digits, `.`,
/// `_` and `-`.
pub fn program() -> String {
    let name = std::env::args_os()
        .next()
        .and_then(|a| {
            Path::new(&a)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        "process".to_owned()
    } else {
        name
    }
}

/// Whether a span or event is one of the timeline's: the workspace's spans, at any level, and [`interval`]s.
fn profiled(meta: &Metadata<'_>) -> bool {
    meta.target().starts_with("cupertino") && (meta.is_span() || meta.target() == INTERVAL_TARGET)
}

/// The layer of the sinks [`ENV`] names, filtered to the timeline's spans, for a process's subscriber; `None` when the
/// variable is unset or empty, or names nothing this build can record (said on standard error).
pub fn layer<S>() -> Option<Box<dyn Layer<S> + Send + Sync + 'static>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let value = std::env::var(ENV).ok().filter(|v| !v.is_empty())?;
    let program = program();
    let sinks = match parse(&value, &program, std::process::id()) {
        Ok(sinks) => sinks,
        Err(e) => {
            eprintln!("{program}: {ENV}: {e}");
            return None;
        }
    };
    layer_of(&sinks, &program)
}

/// The layer of `sinks` for the process `program`.
pub fn layer_of<S>(
    sinks: &[Sink],
    program: &str,
) -> Option<Box<dyn Layer<S> + Send + Sync + 'static>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let mut layers: Vec<Box<dyn Layer<S> + Send + Sync + 'static>> = Vec::new();
    for sink in sinks {
        match sink {
            Sink::Chrome(path) => match ChromeLayer::create(path, program) {
                Ok(chrome) => layers.push(chrome.with_filter(filter_fn(profiled)).boxed()),
                Err(e) => eprintln!("{program}: {ENV}: cannot write {}: {e}", path.display()),
            },
            Sink::Tracy => match tracy::layer() {
                Some(tracy) => layers.push(tracy),
                None => eprintln!(
                    "{program}: {ENV}: this build has no Tracy client (build with `--features cupertino-profile/tracy` \
                     and RUSTFLAGS=\"--cfg cupertino_tracy\")"
                ),
            },
        }
    }
    (!layers.is_empty()).then(|| layers.boxed())
}

/// Installs a subscriber of the sinks [`ENV`] names, for a process with none of its own; nothing when the variable is
/// unset, or when the process has a subscriber already.
pub fn init() {
    if std::env::var_os(ENV).is_none() || tracing::dispatcher::has_been_set() {
        return;
    }
    if let Some(layer) = layer::<tracing_subscriber::Registry>() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry().with(layer));
    }
}

/// The monotonic clock in nanoseconds: the clock the timelines are written in.
pub fn now_ns() -> u64 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    (t.tv_sec as u64).saturating_mul(1_000_000_000) + t.tv_nsec as u64
}

/// A stretch of time measured already, from `start_ns` to `end_ns` on the monotonic clock ([`now_ns`]), drawn as
/// `name` on a track of the calling thread's own beside it: a wait that the thread spent in its event loop, which no
/// span can cover. Costs nothing measurable when no timeline is recorded.
#[inline]
pub fn interval(name: &'static str, start_ns: u64, end_ns: u64) {
    tracing::trace!(target: "cupertino_profile::interval", name, start_ns, end_ns);
}

/// The Tracy layer, in a build with the client.
#[cfg(all(feature = "tracy", cupertino_tracy))]
mod tracy {
    use super::*;

    pub fn layer<S>() -> Option<Box<dyn Layer<S> + Send + Sync + 'static>>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let spans = |meta: &Metadata<'_>| meta.is_span() && profiled(meta);
        Some(
            tracing_tracy::TracyLayer::default()
                .with_filter(filter_fn(spans))
                .boxed(),
        )
    }
}

/// No Tracy client in this build.
#[cfg(not(all(feature = "tracy", cupertino_tracy)))]
mod tracy {
    use super::*;

    pub fn layer<S>() -> Option<Box<dyn Layer<S> + Send + Sync + 'static>>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        None
    }
}

/// The trace file and the events not written to it yet. The file is locked before the events, by the writer and by
/// the close alike, so what is written stays in order.
struct Shared {
    file: Mutex<Option<File>>,
    pending: Mutex<Pending>,
    ready: Condvar,
}

#[derive(Default)]
struct Pending {
    bytes: Vec<u8>,
    /// Whether an event was written: every later one is preceded by a comma.
    started: bool,
    /// The array is closed: nothing more is written.
    closed: bool,
}

impl Shared {
    /// Adds one event, its JSON object in `event`.
    fn push(&self, event: &str) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        if pending.closed {
            return;
        }
        let wake = pending.bytes.is_empty();
        if pending.started {
            pending.bytes.extend_from_slice(b",\n");
        }
        pending.started = true;
        pending.bytes.extend_from_slice(event.as_bytes());
        if wake {
            self.ready.notify_one();
        }
    }

    /// Writes what is pending, after the array's closing bracket when `close`.
    fn write(&self, close: bool) {
        let Ok(mut file) = self.file.lock() else {
            return;
        };
        let bytes = match self.pending.lock() {
            Ok(mut pending) => {
                if pending.closed {
                    return;
                }
                pending.closed = close;
                std::mem::take(&mut pending.bytes)
            }
            Err(_) => return,
        };
        let Some(f) = file.as_mut() else {
            return;
        };
        let written =
            f.write_all(&bytes)
                .and_then(|()| if close { f.write_all(b"\n]\n") } else { Ok(()) });
        if written.is_err() {
            // a full disk: the trace ends here
            *file = None;
        }
    }

    /// The writer's thread: waits for events, gives them [`FLUSH_DELAY`] to gather, writes them. It wakes only when
    /// there are events.
    fn writer(self: Arc<Shared>) {
        loop {
            {
                let Ok(mut pending) = self.pending.lock() else {
                    return;
                };
                while pending.bytes.is_empty() && !pending.closed {
                    pending = match self.ready.wait(pending) {
                        Ok(p) => p,
                        Err(_) => return,
                    };
                }
                if pending.closed {
                    return;
                }
            }
            std::thread::sleep(FLUSH_DELAY);
            self.write(false);
        }
    }
}

/// The traces this process writes, closed when it exits.
static OPEN: OnceLock<Mutex<Vec<Arc<Shared>>>> = OnceLock::new();

/// Writes what every trace of this process holds and closes them: what a process that ends without exiting normally
/// (`_exit`) calls first. Later spans are not written.
pub fn finish() {
    close_traces();
}

extern "C" fn close_traces() {
    if let Some(open) = OPEN.get() {
        if let Ok(open) = open.lock() {
            for shared in open.iter() {
                shared.write(true);
            }
        }
    }
}

/// A layer writing each span as a complete event (`"ph":"X"`) when it ends, with its fields as the event's arguments;
/// a field named `label` goes into the event's name after the span's. [`interval`]s are written on a track of their
/// thread's own.
pub struct ChromeLayer {
    shared: Arc<Shared>,
    pid: u32,
}

/// What a span records, kept with it until it ends.
struct Fields {
    label: Option<String>,
    /// The arguments' JSON members, without braces.
    args: String,
}

/// The calling thread's spans entered and not exited, newest last, with the time each was entered.
struct Thread {
    tid: i32,
    named: bool,
    open: Vec<(u64, u64)>,
    line: String,
}

thread_local! {
    static THREAD: RefCell<Thread> = RefCell::new(Thread {
        tid: rustix::thread::gettid().as_raw_nonzero().get(),
        named: false,
        open: Vec::new(),
        line: String::new(),
    });
}

/// The track an [`interval`] of the thread `tid` is drawn on.
fn interval_track(tid: i32) -> i64 {
    i64::from(tid) + 1_000_000_000
}

impl ChromeLayer {
    /// A layer writing to a new file at `path` (its directory made as needed), for the process `program`.
    pub fn create(path: &Path, program: &str) -> io::Result<ChromeLayer> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = File::create(path)?;
        file.write_all(b"[\n")?;
        let shared = Arc::new(Shared {
            file: Mutex::new(Some(file)),
            pending: Mutex::new(Pending::default()),
            ready: Condvar::new(),
        });
        let writer = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("profile-writer".into())
            .spawn(move || writer.writer())?;
        let open = OPEN.get_or_init(|| {
            // Safety: `close_traces` is an `extern "C"` function that takes no arguments and lives as long as the
            // process.
            unsafe { libc::atexit(close_traces) };
            Mutex::new(Vec::new())
        });
        if let Ok(mut open) = open.lock() {
            open.push(Arc::clone(&shared));
        }
        let pid = std::process::id();
        let mut name = String::new();
        json_str(&mut name, program);
        shared.push(&format!(
            "{{\"name\":\"process_name\",\"ph\":\"M\",\"pid\":{pid},\"tid\":0,\"args\":{{\"name\":{name}}}}}"
        ));
        Ok(ChromeLayer { shared, pid })
    }

    /// Names the calling thread's tracks once, by the thread's name.
    fn name_thread(&self, t: &mut Thread) {
        if t.named {
            return;
        }
        t.named = true;
        let thread = std::thread::current();
        let mut name = String::new();
        json_str(&mut name, thread.name().unwrap_or("thread"));
        let mut waits = String::new();
        json_str(
            &mut waits,
            &format!("{} waits", thread.name().unwrap_or("thread")),
        );
        let pid = self.pid;
        self.shared.push(&format!(
            "{{\"name\":\"thread_name\",\"ph\":\"M\",\"pid\":{pid},\"tid\":{},\"args\":{{\"name\":{name}}}}}",
            t.tid
        ));
        self.shared.push(&format!(
            "{{\"name\":\"thread_name\",\"ph\":\"M\",\"pid\":{pid},\"tid\":{},\"args\":{{\"name\":{waits}}}}}",
            interval_track(t.tid)
        ));
    }

    /// Writes a complete event named `name` (and `label`) on track `tid`, from `start` for `end - start` nanoseconds.
    #[allow(clippy::too_many_arguments)]
    fn complete(
        &self,
        line: &mut String,
        name: &str,
        label: Option<&str>,
        target: &str,
        tid: i64,
        (start, end): (u64, u64),
        args: &str,
    ) {
        line.clear();
        line.push_str("{\"name\":");
        match label {
            Some(label) => json_str(line, &format!("{name} {label}")),
            None => json_str(line, name),
        }
        line.push_str(",\"cat\":");
        json_str(line, target);
        let _ = write!(
            line,
            ",\"ph\":\"X\",\"ts\":{},\"dur\":{},\"pid\":{},\"tid\":{tid}",
            Micros(start),
            Micros(end.saturating_sub(start)),
            self.pid
        );
        if !args.is_empty() {
            line.push_str(",\"args\":{");
            line.push_str(args);
            line.push('}');
        }
        line.push('}');
        self.shared.push(line);
    }
}

impl<S> Layer<S> for ChromeLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.fields().is_empty() {
            return;
        }
        let mut fields = Fields {
            label: None,
            args: String::new(),
        };
        attrs.record(&mut Args(&mut fields));
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(fields);
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        match extensions.get_mut::<Fields>() {
            Some(fields) => values.record(&mut Args(fields)),
            None => {
                let mut fields = Fields {
                    label: None,
                    args: String::new(),
                };
                values.record(&mut Args(&mut fields));
                extensions.insert(fields);
            }
        }
    }

    fn on_enter(&self, id: &Id, _ctx: Context<'_, S>) {
        let now = now_ns();
        let _ = THREAD.try_with(|t| t.borrow_mut().open.push((id.into_u64(), now)));
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let now = now_ns();
        let Some(span) = ctx.span(id) else {
            return;
        };
        let meta = span.metadata();
        let _ = THREAD.try_with(|t| {
            let mut t = t.borrow_mut();
            let key = id.into_u64();
            let Some(at) = t.open.iter().rposition(|(open, _)| *open == key) else {
                return;
            };
            let (_, start) = t.open.remove(at);
            self.name_thread(&mut t);
            let tid = i64::from(t.tid);
            let extensions = span.extensions();
            let fields = extensions.get::<Fields>();
            let mut line = std::mem::take(&mut t.line);
            self.complete(
                &mut line,
                meta.name(),
                fields.and_then(|f| f.label.as_deref()),
                meta.target(),
                tid,
                (start, now),
                fields.map_or("", |f| f.args.as_str()),
            );
            t.line = line;
        });
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != INTERVAL_TARGET {
            return;
        }
        let mut seen = Interval::default();
        event.record(&mut seen);
        let (Some(name), Some(start), Some(end)) = (seen.name, seen.start, seen.end) else {
            return;
        };
        let _ = THREAD.try_with(|t| {
            let mut t = t.borrow_mut();
            self.name_thread(&mut t);
            let tid = interval_track(t.tid);
            let mut line = std::mem::take(&mut t.line);
            self.complete(
                &mut line,
                &name,
                None,
                INTERVAL_TARGET,
                tid,
                (start, end),
                "",
            );
            t.line = line;
        });
    }
}

/// Nanoseconds written as microseconds, the format's unit, to the nanosecond.
struct Micros(u64);

impl fmt::Display for Micros {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:03}", self.0 / 1000, self.0 % 1000)
    }
}

/// `s` as a JSON string.
fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Records a span's fields: `label` apart, the others as the members of its arguments.
struct Args<'a>(&'a mut Fields);

impl Args<'_> {
    fn member(&mut self, field: &Field) -> &mut String {
        let args = &mut self.0.args;
        if !args.is_empty() {
            args.push(',');
        }
        json_str(args, field.name());
        args.push(':');
        args
    }
}

impl Visit for Args<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "label" {
            self.0.label = Some(value.to_owned());
            return;
        }
        json_str(self.member(field), value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let text = format!("{value:?}");
        self.record_str(field, &text);
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let _ = write!(self.member(field), "{value}");
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        let _ = write!(self.member(field), "{value}");
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        let _ = write!(self.member(field), "{value}");
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if value.is_finite() {
            let _ = write!(self.member(field), "{value}");
        } else {
            json_str(self.member(field), &value.to_string());
        }
    }
}

/// The fields of an [`interval`].
#[derive(Default)]
struct Interval {
    name: Option<String>,
    start: Option<u64>,
    end: Option<u64>,
}

impl Visit for Interval {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.name = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "name" {
            self.name = Some(format!("{value:?}"));
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "start_ns" => self.start = Some(value),
            "end_ns" => self.end = Some(value),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
