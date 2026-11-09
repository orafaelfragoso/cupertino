//! Logging: standard error for development runs, and on the TTY a file in the state directory that survives the session
//! (the console is overwritten by the compositor, and a crash leaves nothing else to read).

use std::backtrace::Backtrace;
use std::ffi::{CString, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::IntoRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::Duration;

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const DEFAULT_FILTER: &str =
    "info,smithay=warn,usvg=error,resvg=error,smithay::backend::egl::ffi=off";

/// The log file a run is told to write, in any mode.
pub const LOG_FILE_ENV: &str = "CUPERTINO_LOG_FILE";

/// The TTY log: `CUPERTINO_LOG_FILE`, else `$XDG_STATE_HOME/cupertino/server.log`, else
/// `~/.local/state/cupertino/server.log`.
pub fn log_path(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(path) = var(LOG_FILE_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let state = var("XDG_STATE_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            var("HOME")
                .filter(|p| !p.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state.join("cupertino/server.log"))
}

/// Creates the log file for this run, mode 0600. The previous run's log is kept as `<name>.1`.
pub fn open_log(path: &PathBuf) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if path.exists() {
        let mut previous = path.clone().into_os_string();
        previous.push(".1");
        fs::rename(path, previous)?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Installs the subscriber, a panic hook that writes the panic and its backtrace to the log before the default report,
/// and the handler of the [`FATAL_SIGNALS`]. With a file, everything goes to it and only warnings and errors also reach
/// standard error. The panic hook chains to the one there before: with the diagnostic emitter installed first, a panic
/// is then recorded as `server.panic`, and the handler of the fatal signals records `server.fatal_signal`.
pub fn init(file: Option<File>) {
    let filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    match file {
        Some(file) => {
            if let Ok(crash_log) = file.try_clone() {
                CRASH_LOG.store(crash_log.into_raw_fd(), Ordering::Relaxed);
            }
            let file = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .with_filter(filter());
            tracing_subscriber::registry()
                .with(file)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(io::stderr)
                        .with_filter(LevelFilter::WARN),
                )
                .init();
        }
        None => tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(io::stderr)
                    .with_filter(filter()),
            )
            .init(),
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // the exit timer first: a log that blocks must not keep the session from ending
        if SESSION_END
            .get()
            .is_some_and(|end| end.thread == std::thread::current().id())
        {
            session_ends(&format!("panic: {info}"), 101);
        }
        tracing::error!("panic: {info}\nbacktrace:\n{}", Backtrace::force_capture());
        previous(info);
    }));
    prepare_fatal_record();
    catch_fatal_signals();
}

/// The signals that end the server at once, with no unwinding: an abort (a failed check in a library, a panic while
/// panicking) and a fault.
pub const FATAL_SIGNALS: [libc::c_int; 2] = [libc::SIGABRT, libc::SIGSEGV];

/// The names the `signal` field of `server.fatal_signal` gives the [`FATAL_SIGNALS`], in the same order.
const FATAL_SIGNAL_NAMES: [&str; FATAL_SIGNALS.len()] = ["SIGABRT", "SIGSEGV"];

/// The `server.fatal_signal` lines made ready for the handler, one per signal; none without a diagnostic store.
static FATAL_RECORD: OnceLock<diag::FatalRecord> = OnceLock::new();

/// Makes the `server.fatal_signal` lines ready while allocating is still safe, with the context the process has so far.
fn prepare_fatal_record() {
    let Some(emitter) = diag::Emitter::get() else {
        return;
    };
    if let Some(record) = emitter.prepare_fatal(
        "server.fatal_signal",
        "the server was ended by a fatal signal",
        "signal",
        &FATAL_SIGNAL_NAMES,
    ) {
        let _ = FATAL_RECORD.set(record);
    }
}

/// Where a fatal signal writes: the log file, or standard error without one.
static CRASH_LOG: AtomicI32 = AtomicI32::new(libc::STDERR_FILENO);

/// The actions the [`FATAL_SIGNALS`] had before [`catch_fatal_signals`], in the same order.
static PREVIOUS_ACTIONS: OnceLock<[libc::sigaction; FATAL_SIGNALS.len()]> = OnceLock::new();

/// The alternate stack of the thread that starts the server, which runs its event loop: big enough for the backtrace
/// of a stack overflow, with a guard page below it.
const SIGNAL_STACK: usize = 1 << 20;

fn catch_fatal_signals() {
    let fault_stack = signal_stack();
    let mut previous = [unsafe { std::mem::zeroed::<libc::sigaction>() }; FATAL_SIGNALS.len()];
    for (signal, previous) in FATAL_SIGNALS.iter().zip(&mut previous) {
        // Safety: the action is fully initialised, the handler has the signature handlers without SA_SIGINFO take,
        // and `previous` is valid for writing.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = fatal_signal as extern "C" fn(libc::c_int) as usize;
            // a fault while the handler runs ends the process at once instead of running it again
            libc::sigemptyset(&mut action.sa_mask);
            for blocked in FATAL_SIGNALS {
                libc::sigaddset(&mut action.sa_mask, blocked);
            }
            // a fault may be a stack overflow, with no stack left to handle it on
            if *signal == libc::SIGSEGV && fault_stack {
                action.sa_flags = libc::SA_ONSTACK;
            }
            libc::sigaction(*signal, &action, previous);
        }
    }
    let _ = PREVIOUS_ACTIONS.set(previous);
}

/// Gives the calling thread an alternate signal stack of [`SIGNAL_STACK`] bytes, in place of the standard library's
/// smaller one. False when it cannot be mapped.
fn signal_stack() -> bool {
    let page = 4096;
    // Safety: a private anonymous mapping, never unmapped; its first page is made inaccessible as the guard, and the
    // rest is handed to `sigaltstack`, which keeps the pointer.
    unsafe {
        let base = libc::mmap(
            std::ptr::null_mut(),
            SIGNAL_STACK + page,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        if base == libc::MAP_FAILED {
            return false;
        }
        libc::mprotect(base, page, libc::PROT_NONE);
        let stack = libc::stack_t {
            ss_sp: base.cast::<u8>().add(page).cast(),
            ss_flags: 0,
            ss_size: SIGNAL_STACK,
        };
        libc::sigaltstack(&stack, std::ptr::null_mut()) == 0
    }
}

/// Writes the signal to the session-end file, the log and the diagnostic store (`server.fatal_signal`), then the
/// backtrace, and ends the process as the signal's previous action would. The reason and the event are written with no
/// allocation and no lock first. The backtrace allocates and takes locks, which a crash inside the allocator may hold, so
/// an alarm ends the process if it hangs.
extern "C" fn fatal_signal(signal: libc::c_int) {
    let name: &[u8] = match signal {
        libc::SIGABRT => b"SIGABRT",
        libc::SIGSEGV => b"SIGSEGV",
        _ => b"a fatal signal",
    };
    let index = FATAL_SIGNALS.iter().position(|s| *s == signal);
    if let Some(end) = SESSION_END.get() {
        // a reason written already (a panic, a lost rendering context) is the real one, and a crash in the teardown it
        // started keeps it; `cupertino-session` removes the file before the start
        // Safety: `open`, `write` and `close` are async-signal-safe; the path is a C string made before any crash.
        unsafe {
            let fd = libc::open(
                end.c_path.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                0o600,
            );
            if fd >= 0 {
                write_raw(fd, &[b"the server was ended by ", name, b"\n"]);
                libc::close(fd);
            }
        }
    }
    let log = CRASH_LOG.load(Ordering::Relaxed);
    write_raw(log, &[b"fatal signal ", name, b"; backtrace:\n"]);
    if let (Some(record), Some(index)) = (FATAL_RECORD.get(), index) {
        let mut line = [0u8; diag::FATAL_LINE];
        let len = record.render(index, &mut line);
        // Safety: `open`, `write` and `close` are async-signal-safe; the path is a C string made before any crash, and
        // the line was rendered into this stack buffer without allocating.
        unsafe {
            let fd = libc::open(
                record.path().as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND | libc::O_CLOEXEC,
                0o600,
            );
            if fd >= 0 {
                write_raw(fd, &[&line[..len]]);
                libc::close(fd);
            }
        }
    }
    // Safety: `sigaction`, `sigprocmask` and `alarm` are async-signal-safe and take initialised values.
    unsafe {
        let mut default: libc::sigaction = std::mem::zeroed();
        default.sa_sigaction = libc::SIG_DFL;
        libc::sigaction(libc::SIGALRM, &default, std::ptr::null_mut());
        let mut alarm: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut alarm);
        libc::sigaddset(&mut alarm, libc::SIGALRM);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &alarm, std::ptr::null_mut());
        libc::alarm(TEARDOWN_LIMIT.as_secs() as libc::c_uint);
    }
    let trace = Backtrace::force_capture().to_string();
    write_raw(log, &[trace.as_bytes(), b"\n"]);
    // Safety: the previous action was read by `sigaction` itself; the signal stays blocked until this handler returns,
    // so the raised one is taken by that action then (or the fault happens again, for a fault).
    unsafe {
        if let (Some(previous), Some(index)) = (PREVIOUS_ACTIONS.get(), index) {
            libc::sigaction(signal, &previous[index], std::ptr::null_mut());
        }
        libc::raise(signal);
    }
}

fn write_raw(fd: libc::c_int, parts: &[&[u8]]) {
    for part in parts {
        let mut rest = *part;
        while !rest.is_empty() {
            // Safety: `write` is async-signal-safe and reads `rest.len()` bytes of `rest`.
            let written = unsafe { libc::write(fd, rest.as_ptr().cast(), rest.len()) };
            if written <= 0 {
                return;
            }
            rest = &rest[written as usize..];
        }
    }
}

/// The file `cupertino-session` reads, when the compositor has ended, for the reason it writes to the session log.
pub const SESSION_END_ENV: &str = "CUPERTINO_SESSION_END";

/// How long the teardown after a panic or a lost rendering context may take before the server exits without finishing
/// it, so the display manager's login screen comes back within seconds even when a driver call hangs.
pub const TEARDOWN_LIMIT: Duration = Duration::from_secs(2);

const REASON_LIMIT: usize = 400;

struct SessionEnd {
    path: PathBuf,
    c_path: CString,
    thread: ThreadId,
}

static SESSION_END: OnceLock<SessionEnd> = OnceLock::new();

/// Makes the calling thread the session's when `CUPERTINO_SESSION_END` names a file: a panic on it, and
/// [`session_ends`], write their reason there and bound the teardown.
pub fn record_session_end(var: impl Fn(&str) -> Option<OsString>) {
    let Some(path) = var(SESSION_END_ENV).filter(|p| !p.is_empty()) else {
        return;
    };
    let Ok(c_path) = CString::new(path.as_bytes()) else {
        return;
    };
    let _ = SESSION_END.set(SessionEnd {
        path: PathBuf::from(path),
        c_path,
        thread: std::thread::current().id(),
    });
}

/// The session ends for `reason`: records `server.session_end` ([`server_ended`]), writes the reason for
/// `cupertino-session` and exits with `status` once [`TEARDOWN_LIMIT`] has passed, if the server is still tearing down.
/// Only the event without [`record_session_end`].
pub fn session_ends(reason: &str, status: i32) {
    server_ended(status, Some(reason));
    let Some(end) = SESSION_END.get() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("teardown-limit".into())
        .spawn(move || {
            std::thread::sleep(TEARDOWN_LIMIT);
            // Safety: `_exit` takes no pointers. It skips logging and exit handlers on purpose: the thread that hangs
            // may hold the log, or be inside a driver's handler.
            unsafe { libc::_exit(status) }
        });
    if let Err(e) = write_reason(&end.path, reason) {
        tracing::warn!("cannot write the session end {}: {e}", end.path.display());
    }
}

/// Whether `server.session_end` was recorded: the first end is the real one (a lost rendering context, then the error
/// `main` returns for it).
static ENDED: AtomicBool = AtomicBool::new(false);

/// Records `server.session_end`, once per process: the `status` the server ends with and, on an error end, the error's
/// text. `info` for status 0, `error` otherwise.
pub fn server_ended(status: i32, error: Option<&str>) {
    if ENDED.swap(true, Ordering::Relaxed) {
        return;
    }
    let event = if status == 0 {
        diag::event(Severity::Info, "server.session_end", "the server ended")
    } else {
        diag::event(Severity::Error, "server.session_end", "the server ended")
    };
    let event = event.field("status", Value::Int(status.into()));
    match error {
        Some(error) => event.field("error", Value::Public(error)).emit(),
        None => event.emit(),
    }
}

/// One line of at most [`REASON_LIMIT`] bytes, control characters as spaces, mode 0600.
fn write_reason(path: &Path, reason: &str) -> io::Result<()> {
    let mut line: String = reason
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if line.len() > REASON_LIMIT {
        let mut end = REASON_LIMIT;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
    }
    line.push('\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    io::Write::write_all(&mut file, line.as_bytes())
}

/// The events this test process recorded, for the unit tests of the modules that record them. Each test that reads them
/// installs a memory-only emitter first; nextest runs every test in a process of its own.
#[cfg(test)]
pub(crate) mod test_events {
    use cupertino_diagnostic_events::{self as diag, Record};

    /// Installs this process's emitter, which keeps its events in memory only.
    pub fn memory() {
        diag::init(diag::component!(), "server", diag::Sink::Memory);
    }

    /// The events of `id` this process recorded, oldest first.
    pub fn recorded(id: &str) -> Vec<Record> {
        diag::recent()
            .iter()
            .filter_map(|line| Record::parse(line.trim_end()).ok())
            .filter(|record| record.id == id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn the_first_end_of_the_server_is_the_one_recorded() {
        test_events::memory();
        // a lost rendering context ends the session, then `main` gets the error it caused
        session_ends("the rendering context was lost (the GPU hung)", 1);
        server_ended(
            1,
            Some("the rendering context was lost; sessions in a row: 2"),
        );
        let ends = test_events::recorded("server.session_end");
        assert_eq!(ends.len(), 1, "{ends:?}");
        let end = &ends[0];
        assert_eq!(end.severity, Severity::Error);
        assert_eq!(end.message, "the server ended");
        assert_eq!(end.fields.get("status").unwrap(), 1);
        assert_eq!(
            end.fields.get("error").unwrap(),
            "the rendering context was lost (the GPU hung)"
        );
        assert!(
            end.source.starts_with("cupertino-server:logging.rs:"),
            "{}",
            end.source
        );
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn the_log_file_follows_the_override_then_the_state_directory_then_home() {
        let both = env(&[
            ("CUPERTINO_LOG_FILE", "/tmp/x.log"),
            ("XDG_STATE_HOME", "/s"),
        ]);
        assert_eq!(log_path(both), Some(PathBuf::from("/tmp/x.log")));
        assert_eq!(
            log_path(env(&[("XDG_STATE_HOME", "/s"), ("HOME", "/h")])),
            Some(PathBuf::from("/s/cupertino/server.log"))
        );
        assert_eq!(
            log_path(env(&[("HOME", "/h")])),
            Some(PathBuf::from("/h/.local/state/cupertino/server.log"))
        );
        assert_eq!(log_path(env(&[])), None);
    }

    #[test]
    fn a_new_run_keeps_the_previous_log_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/server.log");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first run").unwrap();
        let mut file = open_log(&path).unwrap();
        io::Write::write_all(&mut file, b"second run").unwrap();
        let read = |p: PathBuf| {
            let mut text = String::new();
            File::open(p).unwrap().read_to_string(&mut text).unwrap();
            text
        };
        assert_eq!(read(path.clone()), "second run");
        assert_eq!(read(dir.path().join("state/server.log.1")), "first run");
        let mode = fs::metadata(&path).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
    }
}
