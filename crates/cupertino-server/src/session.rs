//! `cupertino-server --session`: the session of a login. The login manager, a display manager's **Cupertino** entry and
//! `cupertino desktop start --backend tty` run it. It starts the compositor (`--udev`) as a child in a session of
//! processes of its own, waits for it, cleans up the user manager and locks the keyring after it, and ends with the
//! compositor's status, by a crash too, so the login screen comes back. What happened is a line in
//! `~/.local/state/cupertino/session.log`; the compositor's own log is `server.log` there.
//!
//! A first run can be bounded: a whole number of seconds in `~/.config/cupertino/session-limit` (or
//! `$CUPERTINO_SESSION_LIMIT`) stops the compositor after that long (`--run-for`), and the session ends it 30 s later if
//! it hangs instead: TERM, then KILL 10 s after that, to every process of the compositor's session of processes (also
//! when `dbus-run-session` ends first and the compositor outlives it). No file, no limit.
//!
//! TERM, INT and HUP stay blocked for the whole life of the process: one that comes during the clean-up after the
//! compositor is noted, not acted on, so the clean-up always runs to its end, and every call on the user bus in it is
//! bounded.

use std::ffi::OsString;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::activation_env::{DEFAULTS_RECORD, SESSION_TARGET};
use crate::gpu_reset::{append, local_now, state_dir};
use crate::logging::SESSION_END_ENV;

/// The argument that selects this mode.
pub const FLAG: &str = "--session";
/// Names the compositor program to run in place of this one (development).
pub const SERVER_ENV: &str = "CUPERTINO_SESSION_SERVER";
/// Bounds the session in whole seconds; overrides the `session-limit` file.
pub const LIMIT_ENV: &str = "CUPERTINO_SESSION_LIMIT";
/// The desktop name programs of the session see.
const DESKTOP: &str = "Cupertino";
/// How long a hung compositor gets after its limit before the session sends TERM.
const GRACE: Duration = Duration::from_secs(30);
/// How long after a TERM the session sends what is left of the compositor's session of processes gets KILL.
const KILL_AFTER: Duration = Duration::from_secs(10);
/// How often the session looks again for processes of the child's session that outlived the child.
const POLL: Duration = Duration::from_millis(100);
/// The most digits a limit may have.
const LIMIT_DIGITS: usize = 7;
/// How long connecting to the user bus, and each call on it, may take at the end of the session.
const BUS_TIMEOUT: Duration = Duration::from_secs(2);
/// The statuses a watchdog end is reported with: TERM sent, then KILL.
const STATUS_TIMED_OUT: i32 = 124;
const STATUS_KILLED: i32 = 137;
/// The variables the compositor puts in a shared user manager, removed with the session.
const SESSION_VARIABLES: [&str; 5] = [
    "WAYLAND_DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "DISPLAY",
    "XAUTHORITY",
];
const SECRETS: &str = "org.freedesktop.secrets";
const SECRETS_PATH: &str = "/org/freedesktop/secrets";
const SECRETS_SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_PREFIX: &str = "/org/freedesktop/secrets/collection/";
const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

/// Whether the command line asks for this mode: [`FLAG`] anywhere among the arguments.
pub fn requested(args: &[OsString]) -> bool {
    args.iter().skip(1).any(|a| a == FLAG)
}

/// Runs the session with the server arguments of `args` (the command line, [`FLAG`] left out) and returns the status
/// the process ends with.
pub fn run(args: &[OsString]) -> i32 {
    let state = state_dir();
    if let Some(dir) = &state {
        let _ = std::fs::create_dir_all(dir);
    }
    let log = Log(state.as_ref().map(|dir| dir.join("session.log")));
    // the compositor writes why it ended there (a panic, a lost rendering context, an error)
    let end_file = state.as_ref().map(|dir| dir.join("session-end"));
    if let Some(path) = &end_file {
        let _ = std::fs::remove_file(path);
    }

    let limit = limit(&log);
    let mut server_args: Vec<OsString> = vec!["--udev".into()];
    if let Some(seconds) = limit {
        server_args.push("--run-for".into());
        server_args.push(seconds.to_string().into());
        log.note(&format!("starting with a limit of {seconds} s"));
    } else {
        log.note("starting");
    }
    server_args.extend(args.iter().skip(1).filter(|a| *a != FLAG).cloned());

    let server = std::env::var_os(SERVER_ENV)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("/usr/bin/cupertino-server"));
    // without a session bus of its own (the display manager provides one), the server gets a fresh one
    let mut command = if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|a| !a.is_empty())
    {
        let mut command = Command::new(&server);
        command.args(&server_args);
        command
    } else {
        let mut command = Command::new("dbus-run-session");
        command.arg("--").arg(&server).args(&server_args);
        command
    };
    command.env("XDG_CURRENT_DESKTOP", DESKTOP);
    if let Some(path) = &end_file {
        command.env(SESSION_END_ENV, path);
    }
    // SAFETY: `setsid`, `sigemptyset` and `pthread_sigmask` are async-signal-safe. The child leads a session of
    // processes of its own, so a signal for the session reaches everything in it (dbus-run-session ends on TERM without
    // passing it on), and starts with none of the signals blocked that this process waits for.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            libc::pthread_sigmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
            Ok(())
        });
    }

    // blocked until the process ends, so that a stop during the clean-up cannot end it half way; the threads the
    // clean-up starts inherit the mask
    let signals = Signals::block();
    let started = Instant::now();
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            log.note(&format!("could not start the compositor: {e}"));
            return 127;
        }
    };
    let mut outcome = supervise(child.id() as libc::pid_t, &signals, limit);
    let seconds = started.elapsed().as_secs();

    end_shared_session(&log);
    // a stop that came after the compositor ended, before or during the clean-up
    outcome.stopped |= signals.stop_pending();

    let reason = end_file.as_deref().and_then(read_reason);
    log.note(&end_line(&outcome, reason.as_deref(), seconds));
    outcome.status
}

/// How the compositor ended.
struct Outcome {
    /// The status the session ends with: the exit code, 128 and the signal, or a watchdog status.
    status: i32,
    /// A signal for the session arrived (a logout, the login manager's stop).
    stopped: bool,
}

/// The line the end of the compositor writes to `session.log`, after its time.
fn end_line(outcome: &Outcome, reason: Option<&str>, seconds: u64) -> String {
    let status = outcome.status;
    if outcome.stopped && status != 0 {
        return format!(
            "the compositor was stopped by the session (status {status}) after {seconds} s"
        );
    }
    if let Some(reason) = reason {
        return format!("the compositor ended with status {status} after {seconds} s: {reason}");
    }
    match status {
        0 => format!("the compositor ended normally after {seconds} s"),
        STATUS_TIMED_OUT | STATUS_KILLED => format!(
            "the compositor did not stop in time and was ended by the watchdog (status {status}) after {seconds} s"
        ),
        129..=159 => format!(
            "the compositor was ended by signal {} (status {status}) after {seconds} s; see server.log",
            status - 128
        ),
        _ => format!("the compositor ended with status {status} after {seconds} s; see server.log"),
    }
}

/// The first line of the reason the compositor wrote, control characters removed; the file is removed.
fn read_reason(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let _ = std::fs::remove_file(path);
    let bytes = &bytes[..bytes.len().min(512)];
    let text = String::from_utf8_lossy(bytes);
    let line: String = text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    (!line.is_empty()).then_some(line)
}

/// `session.log`, best effort: a state directory that cannot be written must not keep the compositor from starting.
struct Log(Option<PathBuf>);

impl Log {
    fn note(&self, text: &str) {
        if let Some(path) = &self.0 {
            let _ = append(path, &format!("{} {text}\n", local_now()));
        }
    }
}

/// The limit in seconds: [`LIMIT_ENV`], else the first line of `session-limit` in the configuration directory. Not a
/// whole number, more than [`LIMIT_DIGITS`] digits or zero is no limit.
fn limit(log: &Log) -> Option<u64> {
    let text = std::env::var(LIMIT_ENV)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            let file = config_dir()?.join("session-limit");
            let text = std::fs::read_to_string(file).ok()?;
            Some(text.lines().next().unwrap_or_default().trim().to_owned())
        })?;
    parse_limit(&text, log)
}

fn parse_limit(text: &str, log: &Log) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        log.note(&format!(
            "ignoring the session limit '{text}': it is not a whole number of seconds"
        ));
        return None;
    }
    let digits = text.trim_start_matches('0');
    if digits.len() > LIMIT_DIGITS {
        log.note("ignoring the session limit: more than 7 digits");
        return None;
    }
    digits.parse().ok().filter(|seconds| *seconds > 0)
}

/// `$XDG_CONFIG_HOME/cupertino`, else `~/.config/cupertino`.
fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cupertino"))
}

/// The signals the session waits for, blocked so that they queue until it asks for them: the session's stop (TERM,
/// INT, HUP) and the end of its child (CHLD). They are never unblocked: one still pending when the process ends goes
/// with it.
struct Signals {
    set: libc::sigset_t,
}

/// The session's stop.
const STOP_SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

impl Signals {
    fn block() -> Signals {
        // SAFETY: the set is a plain value the calls initialise; the mask of the calling thread only changes, and the
        // threads it starts later inherit it. The child empties it again before `exec`.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signal in STOP_SIGNALS.into_iter().chain([libc::SIGCHLD]) {
                libc::sigaddset(&mut set, signal);
            }
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            Signals { set }
        }
    }

    /// The next of the signals, or None once `timeout` passed.
    fn wait(&self, timeout: Option<Duration>) -> Option<libc::c_int> {
        let timeout = timeout.map(|t| libc::timespec {
            tv_sec: t.as_secs() as libc::time_t,
            tv_nsec: t.subsec_nanos() as libc::c_long,
        });
        loop {
            // SAFETY: `set` was initialised by `block`; the info and the timeout outlive the call.
            let signal = unsafe {
                let mut info: libc::siginfo_t = std::mem::zeroed();
                match &timeout {
                    Some(t) => libc::sigtimedwait(&self.set, &mut info, t),
                    None => libc::sigwaitinfo(&self.set, &mut info),
                }
            };
            if signal >= 0 {
                return Some(signal);
            }
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EINTR) => continue,
                _ => return None,
            }
        }
    }

    /// Whether a stop signal is pending, blocked since it came.
    fn stop_pending(&self) -> bool {
        // SAFETY: the set is a plain value the calls fill in.
        unsafe {
            let mut pending: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut pending);
            libc::sigpending(&mut pending) == 0
                && STOP_SIGNALS
                    .iter()
                    .any(|signal| libc::sigismember(&pending, *signal) == 1)
        }
    }
}

/// Waits for the child `pid`: a signal for the session goes on to every process of the child's session, and with a
/// limit the watchdog ends a compositor that outlives it by [`GRACE`]. After the first TERM the session sends (a stop or
/// the watchdog), it waits until nothing of the child's session is left, also when the child ended first (as
/// `dbus-run-session` does on TERM while the compositor under it goes on), and sends KILL to what is left
/// [`KILL_AFTER`] after that TERM.
fn supervise(pid: libc::pid_t, signals: &Signals, limit: Option<u64>) -> Outcome {
    let mut stopped = false;
    let mut timed_out = false;
    let mut killed = false;
    let mut ended: Option<i32> = None;
    let limit_at = limit.map(|seconds| Instant::now() + Duration::from_secs(seconds) + GRACE);
    let mut kill_at: Option<Instant> = None;
    loop {
        if ended.is_none() {
            ended = reap(pid).map(|reaped| match reaped {
                Reaped::Exit(code) => code,
                Reaped::Signal(signal) => 128 + signal,
            });
        }
        if let Some(code) = ended {
            if kill_at.is_none() || killed || !session_alive(pid) {
                let status = match (timed_out, killed) {
                    (false, _) => code,
                    (true, false) => STATUS_TIMED_OUT,
                    (true, true) => STATUS_KILLED,
                };
                return Outcome { status, stopped };
            }
        }
        let next = [limit_at.filter(|_| !timed_out), kill_at.filter(|_| !killed)]
            .into_iter()
            .flatten()
            .min();
        let mut timeout = next.map(|at| at.saturating_duration_since(Instant::now()));
        // processes that are not this one's children end without a signal to it, so they are looked for again
        if ended.is_some() {
            timeout = Some(timeout.map_or(POLL, |t| t.min(POLL)));
        }
        match signals.wait(timeout) {
            Some(libc::SIGCHLD) => {}
            Some(_) => {
                stopped = true;
                signal_session(pid, libc::SIGTERM);
                kill_at.get_or_insert(Instant::now() + KILL_AFTER);
            }
            None => {
                let now = Instant::now();
                if !timed_out && limit_at.is_some_and(|at| now >= at) {
                    timed_out = true;
                    signal_session(pid, libc::SIGTERM);
                    kill_at.get_or_insert(now + KILL_AFTER);
                } else if !killed && kill_at.is_some_and(|at| now >= at) {
                    killed = true;
                    signal_session(pid, libc::SIGKILL);
                }
            }
        }
    }
}

enum Reaped {
    Exit(i32),
    Signal(i32),
}

/// The child's end, once it has ended.
fn reap(pid: libc::pid_t) -> Option<Reaped> {
    let mut status = 0;
    // SAFETY: `status` outlives the call; WNOHANG returns at once.
    let found = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    if found != pid {
        return None;
    }
    if libc::WIFEXITED(status) {
        Some(Reaped::Exit(libc::WEXITSTATUS(status)))
    } else if libc::WIFSIGNALED(status) {
        Some(Reaped::Signal(libc::WTERMSIG(status)))
    } else {
        None
    }
}

/// Sends `signal` to every process of the session `sid` leads, and to its process group when none is found.
fn signal_session(sid: libc::pid_t, signal: libc::c_int) {
    let members = session_members(sid);
    for pid in &members {
        // SAFETY: `kill` takes no pointers.
        unsafe { libc::kill(*pid, signal) };
    }
    if members.is_empty() {
        // SAFETY: as above; a negative pid names the process group.
        unsafe { libc::kill(-sid, signal) };
    }
}

/// Whether a process of the session `sid` leads still runs.
fn session_alive(sid: libc::pid_t) -> bool {
    !session_members(sid).is_empty()
}

/// The processes of the session `sid` leads that have not ended; zombies are left out.
fn session_members(sid: libc::pid_t) -> Vec<libc::pid_t> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<libc::pid_t>().ok()?;
            let (state, session) = stat_of(&entry.path())?;
            (session == sid && !matches!(state, 'Z' | 'X')).then_some(pid)
        })
        .collect()
}

/// The state and the session id in `/proc/<pid>/stat`: the first and the fourth field after the command's closing
/// parenthesis.
fn stat_of(proc_dir: &Path) -> Option<(char, libc::pid_t)> {
    let stat = std::fs::read_to_string(proc_dir.join("stat")).ok()?;
    let mut fields = stat[stat.rfind(')')? + 1..].split_whitespace();
    let state = fields.next()?.chars().next()?;
    let session = fields.nth(2)?.parse().ok()?;
    Some((state, session))
}

/// On a shared user bus, the session target and the variables the compositor put in the user manager end with it, and
/// every collection of the Secret Service is locked. The stop is queued, not waited for: the login screen comes back
/// when the session ends, whatever the target's services take.
fn end_shared_session(log: &Log) {
    let Some(conn) = connect_user_bus() else {
        return;
    };
    if !has_owner(&conn, SYSTEMD) || !unit_active(&conn, SESSION_TARGET) {
        return;
    }
    let _ = conn.call_method(
        Some(SYSTEMD),
        MANAGER_PATH,
        Some(MANAGER),
        "StopUnit",
        &(SESSION_TARGET, "replace"),
    );
    let mut names: Vec<String> = SESSION_VARIABLES.iter().map(|n| (*n).to_owned()).collect();
    names.extend(recorded_defaults());
    let _ = conn.call_method(
        Some(SYSTEMD),
        MANAGER_PATH,
        Some(MANAGER),
        "UnsetEnvironment",
        &(names,),
    );
    log.note("stopped cupertino-session.target and removed the session's variables from the user manager");
    lock_secrets(&conn, log);
}

/// The user bus, every call on it bounded by [`BUS_TIMEOUT`]; `None` when it cannot be reached within that time either.
/// A bus that takes the connection and never answers keeps a thread, which ends with the process.
fn connect_user_bus() -> Option<Connection> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("session-bus".into())
        .spawn(move || {
            let conn = zbus::blocking::connection::Builder::session()
                .map(|b| b.method_timeout(BUS_TIMEOUT))
                .and_then(|b| b.build());
            let _ = sender.send(conn.ok());
        })
        .ok()?;
    receiver.recv_timeout(BUS_TIMEOUT).ok().flatten()
}

/// The defaults the compositor set because the manager had no value of the user's own, from [`DEFAULTS_RECORD`],
/// which is removed. Only names of capitals, digits and underscores are taken.
fn recorded_defaults() -> Vec<String> {
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else {
        return Vec::new();
    };
    let path = Path::new(&dir).join(DEFAULTS_RECORD);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let _ = std::fs::remove_file(&path);
    text.lines()
        .filter(|n| is_variable_name(n))
        .map(str::to_owned)
        .collect()
}

fn is_variable_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_uppercase() || b == b'_')
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn has_owner(conn: &Connection, name: &str) -> bool {
    conn.call_method(Some(BUS), BUS_PATH, Some(BUS), "NameHasOwner", &(name,))
        .ok()
        .and_then(|reply| reply.body().deserialize::<bool>().ok())
        .unwrap_or(false)
}

fn unit_active(conn: &Connection, unit: &str) -> bool {
    let Some(path) = conn
        .call_method(
            Some(SYSTEMD),
            MANAGER_PATH,
            Some(MANAGER),
            "GetUnit",
            &(unit,),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<OwnedObjectPath>().ok())
    else {
        return false;
    };
    let Some(state) = conn
        .call_method(
            Some(SYSTEMD),
            path.as_str(),
            Some(PROPERTIES),
            "Get",
            &("org.freedesktop.systemd1.Unit", "ActiveState"),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<OwnedValue>().ok())
    else {
        return false;
    };
    matches!(
        Value::from(state).downcast_ref::<&str>(),
        Ok("active" | "activating" | "reloading")
    )
}

/// Locks every collection of the Secret Service on the user bus (gnome-keyring's, unlocked by PAM at login). The
/// provider outlives the session while the user manager runs (a second session, lingering), and nothing it holds stays
/// readable once the person has logged out. A provider that is not running is not started. The object paths are checked
/// before they are passed on; secrets are never read here.
fn lock_secrets(conn: &Connection, log: &Log) {
    if !has_owner(conn, SECRETS) {
        return;
    }
    let Some(value) = conn
        .call_method(
            Some(SECRETS),
            SECRETS_PATH,
            Some(PROPERTIES),
            "Get",
            &(SECRETS_SERVICE, "Collections"),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<OwnedValue>().ok())
    else {
        return;
    };
    let Ok(collections) = Vec::<OwnedObjectPath>::try_from(value) else {
        log.note("could not lock the keyring: an unexpected list of collections");
        return;
    };
    if collections.iter().any(|path| !is_collection(path.as_str())) {
        log.note("could not lock the keyring: an unexpected collection path");
        return;
    }
    if collections.is_empty() {
        return;
    }
    let count = collections.len();
    match conn.call_method(
        Some(SECRETS),
        SECRETS_PATH,
        Some(SECRETS_SERVICE),
        "Lock",
        &(collections,),
    ) {
        Ok(_) => log.note(&format!("locked the keyring ({count} collections)")),
        Err(_) => log.note("could not lock the keyring"),
    }
}

/// A collection's object path: the collection prefix and a non-empty rest of letters, digits, `_` and `/`.
fn is_collection(path: &str) -> bool {
    path.strip_prefix(COLLECTION_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'/')
    })
}
