//! The process's emitter: one per process, installed once, where every [`Event::emit`](crate::Event::emit) goes.

use std::collections::{HashMap, VecDeque};
use std::ffi::{CString, OsString};
use std::fmt::Display;
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::panic::Location;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, Once, OnceLock, TryLockError};
use std::time::{Duration, Instant, SystemTime};

use cupertino_build_info::Component;

use crate::event::{self, AddressKind, Event, Identity, Origin, Record, Severity, Value};
use crate::journal::{self, Journal};
use crate::rate::RateLimiter;
use crate::redact::{Policy, Salt};
use crate::store::{self, Limits, Store, Unavailable};
use crate::time;

/// How many events the memory ring of a process keeps.
pub const RING_EVENTS: usize = 256;
/// How many bytes of events the memory ring of a process keeps.
pub const RING_BYTES: usize = 256 << 10;

/// The id of the event written once the store works again after it could not be written.
pub const STORE_UNAVAILABLE: &str = "diag.store.unavailable";

/// How long the end of the process and a panic wait for an emitter another thread holds.
const EXIT_WAIT: Duration = Duration::from_millis(200);

/// Where a process's events go besides its memory ring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sink {
    /// The user store, `$XDG_STATE_HOME/cupertino/events` (else `~/.local/state/cupertino/events`): user processes.
    UserStore,
    /// A store in this directory.
    Store(PathBuf),
    /// The system journal (or the socket `CUPERTINO_DIAG_JOURNAL_SOCKET` names, in tests): system services.
    Journal,
    /// The memory ring alone: tests.
    Memory,
}

static EMITTER: OnceLock<Emitter> = OnceLock::new();

/// Events emitted while no emitter was installed.
static BEFORE_INSTALL: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Set while this thread records an event, so a panic inside it does not record again.
    static RECORDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The kernel's boot id, read once.
pub fn boot_id() -> Option<String> {
    static BOOT: OnceLock<Option<String>> = OnceLock::new();
    BOOT.get_or_init(|| {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    })
    .clone()
}

/// How many events were emitted before an emitter was installed; they are dropped.
pub fn dropped_before_install() -> u64 {
    BEFORE_INSTALL.load(Ordering::Relaxed)
}

enum Out {
    /// The store in `dir` (none when no directory could be named), opened on the first event and again after it failed.
    Store {
        dir: Option<PathBuf>,
        store: Option<Store>,
    },
    Journal(Journal),
    Memory,
}

struct State {
    out: Out,
    ring: VecDeque<String>,
    ring_bytes: usize,
    rate: RateLimiter,
    counts: HashMap<String, u64>,
    /// Fields every later event carries, reduced when an event is built.
    context: Vec<(&'static str, Owned)>,
    /// Events written to the ring alone since the store last worked, and the last reason.
    memory_only: u64,
    down: Option<Unavailable>,
    /// The reasons standard error was told about.
    warned: Vec<Unavailable>,
}

/// The process's emitter.
pub struct Emitter {
    component: &'static str,
    version: &'static str,
    panic_id: &'static str,
    exit_error_id: &'static str,
    boot_id: Option<String>,
    session: Option<String>,
    pid: u32,
    /// Made with the store's salt by the first event.
    policy: OnceLock<Policy>,
    limits: Limits,
    state: Mutex<State>,
    flusher: OnceLock<Option<std::thread::Thread>>,
}

impl std::fmt::Debug for Emitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emitter")
            .field("component", &self.component)
            .finish_non_exhaustive()
    }
}

impl Emitter {
    /// Installs the process's emitter for `component`, whose event ids start with `area` (`server`, `dock`): its
    /// panic is `<area>.panic`, its exit with an error `<area>.exit_error`. Once per process: a second call returns
    /// the first emitter as the error.
    pub fn install(
        component: Component,
        area: &'static str,
        sink: Sink,
    ) -> Result<&'static Emitter, &'static Emitter> {
        Self::install_with(component, area, sink, Limits::default())
    }

    /// [`Emitter::install`] with other store limits.
    pub fn install_with(
        component: Component,
        area: &'static str,
        sink: Sink,
        limits: Limits,
    ) -> Result<&'static Emitter, &'static Emitter> {
        let mut fresh = false;
        let emitter = EMITTER.get_or_init(|| {
            fresh = true;
            Emitter::new(component, area, sink, limits)
        });
        if fresh {
            Ok(emitter)
        } else {
            Err(emitter)
        }
    }

    /// The installed emitter.
    pub fn get() -> Option<&'static Emitter> {
        EMITTER.get()
    }

    fn new(component: Component, area: &'static str, sink: Sink, limits: Limits) -> Emitter {
        let var = |k: &str| std::env::var_os(k);
        // the store is opened, and its directory made, by the first event: a process that records none leaves nothing
        let out = match sink {
            Sink::UserStore => Out::Store {
                dir: store::user_dir(var),
                store: None,
            },
            Sink::Store(dir) => Out::Store {
                dir: Some(dir),
                store: None,
            },
            Sink::Journal => Out::Journal(Journal::system()),
            Sink::Memory => Out::Memory,
        };
        Emitter {
            component: component.name,
            version: component.version,
            panic_id: leak(format!("{area}.panic")),
            exit_error_id: leak(format!("{area}.exit_error")),
            boot_id: boot_id(),
            session: std::env::var("XDG_SESSION_ID")
                .ok()
                .filter(|s| !s.is_empty()),
            pid: std::process::id(),
            policy: OnceLock::new(),
            limits,
            state: Mutex::new(State {
                out,
                ring: VecDeque::new(),
                ring_bytes: 0,
                rate: RateLimiter::default(),
                counts: HashMap::new(),
                context: Vec::new(),
                memory_only: 0,
                down: None,
                warned: Vec::new(),
            }),
            flusher: OnceLock::new(),
        }
    }

    pub fn component(&self) -> &'static str {
        self.component
    }

    /// The redaction policy, with the salt of the store (made, with the store, on first use).
    pub fn policy(&self) -> &Policy {
        if let Some(policy) = self.policy.get() {
            return policy;
        }
        let mut state = self.lock();
        self.policy_in(&mut state)
    }

    /// The policy, made on first use while the state is held: the store's salt, opening the store; a service's salt; or
    /// one for the process alone.
    fn policy_in(&self, state: &mut State) -> &Policy {
        self.policy.get_or_init(|| {
            let salt = match &mut state.out {
                Out::Store {
                    dir: Some(dir),
                    store,
                } => {
                    if store.is_none() {
                        *store = Store::open(dir, self.limits).ok();
                    }
                    store
                        .as_ref()
                        .and_then(|s| s.salt().ok())
                        .unwrap_or_else(Salt::random)
                }
                Out::Store { dir: None, .. } | Out::Memory => Salt::random(),
                Out::Journal(_) => service_salt(|k| std::env::var_os(k)),
            };
            Policy::of_process(salt)
        })
    }

    /// The id of this process's panic event.
    pub fn panic_id(&self) -> &'static str {
        self.panic_id
    }

    /// The id of this process's exit-with-an-error event.
    pub fn exit_error_id(&self) -> &'static str {
        self.exit_error_id
    }

    /// The last events of this process (at most [`RING_EVENTS`] and [`RING_BYTES`]), oldest first, as their lines.
    pub fn recent(&self) -> Vec<String> {
        self.lock().ring.iter().cloned().collect()
    }

    /// Adds a field every later event of the process carries (the server's `backend` and `renderer`); a later call
    /// for the same key replaces it.
    pub fn set_context(&self, key: &'static str, value: Value<'_>) {
        let mut state = self.lock();
        state.context.retain(|(k, _)| *k != key);
        state.context.push((key, Owned::of(&value)));
    }

    /// Writes the summaries of every rate-limit window still open: at the end of the process, or before it may end.
    pub fn flush(&self) {
        let Some(mut state) = self.lock_bounded() else {
            return;
        };
        for record in state.rate.drain() {
            self.write(&mut state, record);
        }
    }

    /// Why the store could not be written the last time, if it could not.
    pub fn store_unavailable(&self) -> Option<Unavailable> {
        self.lock().down
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The state, unless another thread holds it longer than [`EXIT_WAIT`]: the process may be ending.
    fn lock_bounded(&self) -> Option<MutexGuard<'_, State>> {
        let until = Instant::now() + EXIT_WAIT;
        loop {
            match self.state.try_lock() {
                Ok(guard) => return Some(guard),
                Err(TryLockError::Poisoned(p)) => return Some(p.into_inner()),
                Err(TryLockError::WouldBlock) if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(TryLockError::WouldBlock) => return None,
            }
        }
    }

    fn record(&'static self, event: &Event<'_>, source: Option<String>, bounded: bool) {
        let state = if bounded {
            self.lock_bounded()
        } else {
            Some(self.lock())
        };
        let Some(mut state) = state else { return };
        let now = Instant::now();
        let count = {
            let c = state.counts.entry(event.id.to_owned()).or_insert(0);
            *c += 1;
            *c
        };
        let record = self.build(&mut state, event, count, source);
        let mut out = state.rate.due(now);
        out.extend(state.rate.offer(record, now));
        for record in out {
            self.write(&mut state, record);
        }
        let pending = state.rate.next_due().is_some();
        drop(state);
        if pending {
            self.wake_flusher();
        }
    }

    fn build(
        &self,
        state: &mut State,
        event: &Event<'_>,
        count: u64,
        source: Option<String>,
    ) -> Record {
        let origin = Origin {
            time: time::format(SystemTime::now()),
            boot_id: self.boot_id.clone(),
            session: self.session.clone(),
            component: self.component.to_owned(),
            version: self.version.to_owned(),
            commit: cupertino_build_info::COMMIT.map(str::to_owned),
            pid: self.pid,
        };
        let policy = self.policy_in(state);
        let mut record = event.to_record(policy, &origin, count);
        for (key, value) in &state.context {
            if event.fields.iter().any(|(k, _)| k == key) {
                continue;
            }
            let reduced = policy.value(&value.value());
            if reduced.rewritten {
                record.redacted.push((*key).to_owned());
            }
            record.fields.0.push(((*key).to_owned(), reduced.value));
        }
        if let Some(source) = source {
            record.source = source;
        }
        record
    }

    fn write(&self, state: &mut State, mut record: Record) {
        let line = record.fit(self.limits.event_bytes);
        push_ring(state, &line);
        match &mut state.out {
            Out::Memory => {}
            Out::Journal(journal) => {
                let datagram =
                    journal::datagram(&record, line.trim_end_matches('\n'), self.component);
                if journal.send(&datagram).is_err() {
                    let _ = write!(io::stderr(), "{}: diagnostic event: {line}", self.component);
                }
            }
            Out::Store { dir, store } => {
                let result = match (store.as_ref(), dir.as_deref()) {
                    (Some(s), _) => s.append(line.as_bytes()),
                    (None, Some(d)) => match Store::open(d, self.limits) {
                        Ok(s) => store.insert(s).append(line.as_bytes()),
                        Err(reason) => Err(reason),
                    },
                    (None, None) => Err(Unavailable::NoHome),
                };
                match result {
                    Ok(_) => {
                        if state.memory_only > 0 {
                            let kept = std::mem::take(&mut state.memory_only);
                            let reason = state.down.take().unwrap_or(Unavailable::Io);
                            self.store_back(state, reason, kept);
                        }
                        state.down = None;
                    }
                    Err(reason) => {
                        state.memory_only += 1;
                        state.down = Some(reason);
                        if matches!(
                            reason,
                            Unavailable::Symlink
                                | Unavailable::NotOwned
                                | Unavailable::Writable
                                | Unavailable::NotADirectory
                        ) {
                            // opened again, and checked again, on the next event
                            *store = None;
                        }
                        if !state.warned.contains(&reason) {
                            state.warned.push(reason);
                            let _ = writeln!(
                                io::stderr(),
                                "{}: warning: diagnostic events are kept in memory only: {}",
                                self.component,
                                reason.describe()
                            );
                        }
                    }
                }
            }
        }
    }

    /// The store works again after `kept` events went to the ring alone: says so in the store.
    fn store_back(&self, state: &mut State, reason: Unavailable, kept: u64) {
        let count = {
            let c = state
                .counts
                .entry(STORE_UNAVAILABLE.to_owned())
                .or_insert(0);
            *c += 1;
            *c
        };
        let event = event::event(
            Severity::Warn,
            STORE_UNAVAILABLE,
            "diagnostic events were kept in memory only while the store could not be written",
        )
        .field("reason", Value::Public(reason.code()))
        .field("kept_in_memory", Value::Count(kept));
        let mut record = self.build(state, &event, count, None);
        let line = record.fit(self.limits.event_bytes);
        push_ring(state, &line);
        if let Out::Store {
            store: Some(store), ..
        } = &state.out
        {
            let _ = store.append(line.as_bytes());
        }
    }

    /// Wakes the thread that writes the rate-limit summaries when their windows close; started on first use.
    fn wake_flusher(&'static self) {
        let thread = self.flusher.get_or_init(|| {
            std::thread::Builder::new()
                .name("diagnostic-events".into())
                .spawn(move || loop {
                    let next = {
                        let mut state = self.lock();
                        let now = Instant::now();
                        for record in state.rate.due(now) {
                            self.write(&mut state, record);
                        }
                        state.rate.next_due()
                    };
                    match next {
                        Some(at) => {
                            std::thread::park_timeout(at.saturating_duration_since(Instant::now()))
                        }
                        None => std::thread::park(),
                    }
                })
                .ok()
                .map(|handle| handle.thread().clone())
        });
        if let Some(thread) = thread {
            thread.unpark();
        }
    }

    /// Lines a fatal signal handler can write with no lock and no allocation: one per value of `key`, to a file of its
    /// own in the store (`fatal-<boot>-<pid>.jsonl`). `None` without a store directory.
    pub fn prepare_fatal(
        &self,
        id: &'static str,
        message: &'static str,
        key: &'static str,
        values: &[&'static str],
    ) -> Option<FatalRecord> {
        let dir = match &self.lock().out {
            Out::Store { dir: Some(dir), .. } => dir.clone(),
            _ => return None,
        };
        let boot: String = self
            .boot_id
            .as_deref()
            .unwrap_or("")
            .chars()
            .filter(char::is_ascii_hexdigit)
            .take(8)
            .collect();
        let boot = if boot.len() == 8 {
            boot
        } else {
            "unknown0".to_owned()
        };
        let path = dir.join(format!("fatal-{boot}-{}.jsonl", self.pid));
        let path = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut state = self.lock();
        let mut lines = Vec::new();
        for value in values {
            let event = event::event(Severity::Fatal, id, message).field(key, Value::Public(value));
            let mut record = self.build(&mut state, &event, 1, None);
            record.time = time::format(SystemTime::UNIX_EPOCH);
            let line = record.fit(self.limits.event_bytes.min(FATAL_LINE));
            let at = line.find("\"time\":\"")? + "\"time\":\"".len();
            lines.push((line.into_bytes(), at));
        }
        Some(FatalRecord { path, lines })
    }
}

/// The longest line a [`FatalRecord`] holds.
pub const FATAL_LINE: usize = 2048;

/// Event lines made ready for a fatal signal handler; see [`Emitter::prepare_fatal`].
pub struct FatalRecord {
    path: CString,
    lines: Vec<(Vec<u8>, usize)>,
}

impl FatalRecord {
    /// The file the handler opens (`O_WRONLY | O_CREAT | O_APPEND`, mode 0600) and writes [`FatalRecord::render`] to.
    pub fn path(&self) -> &std::ffi::CStr {
        &self.path
    }

    /// Writes line `index` with the current time into `out` and returns its length (0 for an unknown index).
    /// Async-signal-safe: it copies, reads the clock and does arithmetic.
    pub fn render(&self, index: usize, out: &mut [u8; FATAL_LINE]) -> usize {
        let Some((line, at)) = self.lines.get(index) else {
            return 0;
        };
        let len = line.len().min(FATAL_LINE);
        out[..len].copy_from_slice(&line[..len]);
        let mut stamp = [0u8; time::LEN];
        time::format_into(SystemTime::now(), &mut stamp);
        if at + time::LEN <= len {
            out[*at..at + time::LEN].copy_from_slice(&stamp);
        }
        len
    }
}

/// A context field's value, kept until an event is built.
#[derive(Clone, Debug)]
enum Owned {
    Public(String),
    Count(u64),
    Int(i64),
    Duration(Duration),
    Bool(bool),
    Path(PathBuf),
    UserText(String),
    Secret(String),
    Address(AddressKind, String),
    Identifier(Identity, String),
}

impl Owned {
    fn of(value: &Value<'_>) -> Owned {
        match *value {
            Value::Public(s) => Owned::Public(s.to_owned()),
            Value::Count(n) => Owned::Count(n),
            Value::Int(n) => Owned::Int(n),
            Value::Duration(d) => Owned::Duration(d),
            Value::Bool(b) => Owned::Bool(b),
            Value::Path(p) => Owned::Path(p.to_owned()),
            Value::UserText(s) => Owned::UserText(s.to_owned()),
            Value::Secret(s) => Owned::Secret(s.to_owned()),
            Value::Address(kind, s) => Owned::Address(kind, s.to_owned()),
            Value::Identifier(who, s) => Owned::Identifier(who, s.to_owned()),
        }
    }

    fn value(&self) -> Value<'_> {
        match self {
            Owned::Public(s) => Value::Public(s),
            Owned::Count(n) => Value::Count(*n),
            Owned::Int(n) => Value::Int(*n),
            Owned::Duration(d) => Value::Duration(*d),
            Owned::Bool(b) => Value::Bool(*b),
            Owned::Path(p) => Value::Path(p),
            Owned::UserText(s) => Value::UserText(s),
            Owned::Secret(s) => Value::Secret(s),
            Owned::Address(kind, s) => Value::Address(*kind, s),
            Owned::Identifier(who, s) => Value::Identifier(*who, s),
        }
    }
}

fn push_ring(state: &mut State, line: &str) {
    state.ring.push_back(line.to_owned());
    state.ring_bytes += line.len();
    while state.ring.len() > RING_EVENTS || state.ring_bytes > RING_BYTES {
        match state.ring.pop_front() {
            Some(old) => state.ring_bytes -= old.len(),
            None => break,
        }
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The salt of a system service: `diagnostic-salt` in the state directory systemd gives it (`STATE_DIRECTORY`), else
/// one for the process alone.
fn service_salt(var: impl Fn(&str) -> Option<OsString>) -> Salt {
    var("STATE_DIRECTORY")
        .and_then(|dirs| {
            let first = dirs.as_bytes().split(|b| *b == b':').next()?.to_vec();
            (!first.is_empty()).then(|| {
                PathBuf::from(std::ffi::OsString::from(std::ffi::OsStr::from_bytes(
                    &first,
                )))
            })
        })
        .and_then(|dir| Salt::load_or_create(&dir.join("diagnostic-salt")).ok())
        .unwrap_or_else(Salt::random)
}

pub(crate) fn emit(event: Event<'_>) {
    let Some(emitter) = EMITTER.get() else {
        BEFORE_INSTALL.fetch_add(1, Ordering::Relaxed);
        return;
    };
    record_guarded(emitter, &event, None, false);
}

fn record_guarded(
    emitter: &'static Emitter,
    event: &Event<'_>,
    source: Option<String>,
    bounded: bool,
) {
    if RECORDING.with(|r| r.replace(true)) {
        return;
    }
    emitter.record(event, source, bounded);
    RECORDING.with(|r| r.set(false));
}

/// The process's emitter: installs it (see [`Emitter::install`]; a second call keeps the first), chains the panic hook
/// ([`install_panic_hook`]) and writes the open rate-limit summaries when the process exits. The one line each
/// program's `main` starts with. With [`TEST_PANIC_ENV`] set to `area`, it panics once all is in place.
pub fn init(component: Component, area: &'static str, sink: Sink) -> &'static Emitter {
    let emitter = Emitter::install(component, area, sink).unwrap_or_else(|first| first);
    install_panic_hook();
    static AT_EXIT: Once = Once::new();
    AT_EXIT.call_once(|| {
        extern "C" {
            fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
        }
        extern "C" fn at_exit() {
            if let Some(emitter) = EMITTER.get() {
                emitter.flush();
            }
        }
        // SAFETY: `atexit` keeps a plain function pointer; `at_exit` takes the lock only for a bounded time.
        unsafe {
            atexit(at_exit);
        }
    });
    if std::env::var_os(TEST_PANIC_ENV).is_some_and(|named| named == area) {
        panic!("{TEST_PANIC_ENV} asked this process to panic");
    }
    emitter
}

/// Names the `area` of a process that should panic right after [`init`]: tests only, so a test drives the panic path of
/// a real binary.
pub const TEST_PANIC_ENV: &str = "CUPERTINO_DIAG_TEST_PANIC";

/// Chains a panic hook that records `<area>.panic` (`fatal`, the message through the free-text scrubber, the location's
/// file basename and line) and writes the open summaries, then calls the hook that was there before. Once per process.
pub fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record_panic(info);
            previous(info);
        }));
    });
}

fn record_panic(info: &std::panic::PanicHookInfo<'_>) {
    let Some(emitter) = EMITTER.get() else {
        BEFORE_INSTALL.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("a panic without a message");
    let location = info
        .location()
        .map(|l| format!("{}:{}", basename(l.file()), l.line()));
    let thread = std::thread::current();
    let mut event = event::event(Severity::Fatal, emitter.panic_id, "the process panicked")
        .field("panic", Value::Public(payload));
    if let Some(location) = &location {
        event = event.field("location", Value::Public(location));
    }
    if let Some(name) = thread.name() {
        event = event.field("thread", Value::Public(name));
    }
    let source = info.location().map(|l| {
        let l: &Location<'_> = l;
        event::source(l)
    });
    record_guarded(emitter, &event, source, true);
    emitter.flush();
}

fn basename(file: &str) -> &str {
    file.rsplit(['/', '\\']).next().unwrap_or(file)
}

/// Records `<area>.exit_error` (`error`, the error's text through the free-text scrubber) for a process about to exit
/// because of `error`, and writes the open summaries. The caller still prints and returns its error as before.
#[track_caller]
pub fn exit_error(error: &dyn Display) {
    let Some(emitter) = EMITTER.get() else {
        BEFORE_INSTALL.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let text = error.to_string();
    let event = event::event(
        Severity::Error,
        emitter.exit_error_id,
        "the process ended with an error",
    )
    .field("error", Value::Public(&text));
    record_guarded(emitter, &event, None, false);
    emitter.flush();
}

/// The installed emitter's [`Emitter::recent`], or nothing.
pub fn recent() -> Vec<String> {
    EMITTER.get().map(Emitter::recent).unwrap_or_default()
}

/// The installed emitter's [`Emitter::flush`].
pub fn flush() {
    if let Some(emitter) = EMITTER.get() {
        emitter.flush();
    }
}

/// The installed emitter's [`Emitter::set_context`].
pub fn set_context(key: &'static str, value: Value<'_>) {
    if let Some(emitter) = EMITTER.get() {
        emitter.set_context(key, value);
    }
}
