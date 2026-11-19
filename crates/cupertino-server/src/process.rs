//! The process behind a client connection, named so that a reused process id is another process.
//!
//! Reading `/proc` or the file system for a process the server did not choose can stall for good (a program or root on a
//! FUSE mount, a process whose memory map is held), so none of it runs on the compositor's loop. The process of a
//! connection is read once on a thread of its own ([`ProcessLookup`], started as the connection is accepted or bound to
//! the process the server started for it) and remembered; the loop waits for it once, until [`IDENTITY_WAIT`]. A check
//! that must be fresh runs on a thread the loop waits for until a deadline ([`Lookups::run`]). The threads are counted
//! ([`LOOKUP_LIMIT`], [`CONNECTION_LOOKUP_LIMIT`]): one that stalls stays counted until it ends, and none starts at a
//! limit. A lookup that did not answer in time names no process and allows nothing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cupertino_peer::ProcessHandle;
use smithay::reexports::wayland_server::{Client, DisplayHandle};

use crate::state::ClientState;

/// A process id and the clock tick the process started at (`/proc/<pid>/stat`, field 22).
pub type ProcessIdentity = (u32, u64);

/// How many lookup threads may be alive at once, counting those still reading after the loop stopped waiting for them.
pub const LOOKUP_LIMIT: usize = 32;

/// How many lookup threads of one connection may be alive at once: the reading of its process and one check.
pub const CONNECTION_LOOKUP_LIMIT: usize = 2;

/// How long the loop waits, once per connection, for its process to be read when that reading is still under way.
pub const IDENTITY_WAIT: Duration = Duration::from_millis(20);

/// How long the loop waits for a check that reads `/proc` or the file system for a process or program it did not
/// choose, or for a program it starts.
pub const CHECK_WAIT: Duration = Duration::from_millis(100);

/// A count of alive lookup threads.
pub type Alive = Arc<AtomicUsize>;

/// The lookup threads of a server, at most `limit` alive at once.
#[derive(Clone, Debug)]
pub struct Lookups {
    alive: Alive,
    limit: usize,
}

/// One alive lookup thread, counted in every count it holds while it lives.
struct Counted(Vec<Alive>);

impl Drop for Counted {
    fn drop(&mut self) {
        for alive in &self.0 {
            alive.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Count one more thread in `alive` unless `limit` are alive.
fn enter(alive: &Alive, limit: usize) -> bool {
    if alive.fetch_add(1, Ordering::SeqCst) >= limit {
        alive.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    true
}

impl Lookups {
    /// Lookups of which at most `limit` are alive at once.
    pub fn new(limit: usize) -> Self {
        Self {
            alive: Alive::default(),
            limit,
        }
    }

    /// The server's lookups, [`LOOKUP_LIMIT`] at most. They count threads of the process, which runs one server.
    pub fn shared() -> &'static Lookups {
        static SHARED: OnceLock<Lookups> = OnceLock::new();
        SHARED.get_or_init(|| Lookups::new(LOOKUP_LIMIT))
    }

    /// How many lookup threads are alive.
    pub fn alive(&self) -> usize {
        self.alive.load(Ordering::SeqCst)
    }

    /// Run `f` on a thread of its own, counted here and, given an `owner` count and its limit, there. `false`, and `f`
    /// dropped without running, at either limit or when no thread starts.
    pub fn spawn(&self, owner: Option<(&Alive, usize)>, f: impl FnOnce() + Send + 'static) -> bool {
        let mut counted = Counted(Vec::with_capacity(2));
        if let Some((alive, limit)) = owner {
            if !enter(alive, limit) {
                return false;
            }
            counted.0.push(alive.clone());
        }
        if !enter(&self.alive, self.limit) {
            return false;
        }
        counted.0.push(self.alive.clone());
        std::thread::Builder::new()
            .name("cupertino-lookup".into())
            .spawn(move || {
                let _counted = counted;
                f();
            })
            .is_ok()
    }

    /// Run `f` on a thread of its own ([`Lookups::spawn`]) and wait for its answer until `wait` has passed. `None` when
    /// it did not answer by then or did not start; a thread that reads on is left to end and its answer is dropped.
    pub fn run<T: Send + 'static>(
        &self,
        owner: Option<(&Alive, usize)>,
        wait: Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (answer, answered) = std::sync::mpsc::sync_channel(1);
        if !self.spawn(owner, move || {
            let _ = answer.send(f());
        }) {
            return None;
        }
        answered.recv_timeout(wait).ok()
    }
}

/// What the server knows of the process behind a connection, read once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessFacts {
    /// The process id and its start.
    pub process: ProcessIdentity,
    /// The path the process started its program from (`/proc/<pid>/exe`, without the mark of a program removed since).
    pub program: Option<PathBuf>,
    /// That program, when only the system can have put it there ([`read_facts`]).
    pub trusted_executable: Option<PathBuf>,
    /// The `XDG_ACTIVATION_TOKEN` the process was started with.
    pub launch_token: Option<String>,
}

/// Where the reading of a connection's process is.
#[derive(Debug, Default)]
enum Phase {
    #[default]
    Unstarted,
    /// Under way; `waited` once the loop waited for it in vain, so it is not waited for again.
    Reading { waited: bool },
    /// Read: `None` when the process could not be named.
    Read(Option<Arc<ProcessFacts>>),
}

/// The process of one connection, read once off the loop and remembered.
#[derive(Debug, Default)]
pub struct ProcessLookup {
    phase: Mutex<Phase>,
    answered: Condvar,
}

impl ProcessLookup {
    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether no reading was started yet.
    pub fn is_unstarted(&self) -> bool {
        matches!(*self.phase(), Phase::Unstarted)
    }

    /// Start `read` on a thread of `lookups`, counted in `owner` too, unless a reading was started already. At a limit
    /// nothing starts and the next use tries again; `false` then.
    pub fn start(
        self: &Arc<Self>,
        lookups: &Lookups,
        owner: (&Alive, usize),
        read: impl FnOnce() -> Option<ProcessFacts> + Send + 'static,
    ) -> bool {
        {
            let mut phase = self.phase();
            if !matches!(*phase, Phase::Unstarted) {
                return true;
            }
            *phase = Phase::Reading { waited: false };
        }
        let lookup = self.clone();
        let started = lookups.spawn(Some(owner), move || {
            let facts = read().map(Arc::new);
            *lookup.phase() = Phase::Read(facts);
            lookup.answered.notify_all();
        });
        if !started {
            *self.phase() = Phase::Unstarted;
            tracing::debug!("no thread to read a connection's process: it is named on a later use");
        }
        started
    }

    /// The facts once read, without waiting.
    pub fn known(&self) -> Option<Arc<ProcessFacts>> {
        match &*self.phase() {
            Phase::Read(facts) => facts.clone(),
            Phase::Unstarted | Phase::Reading { .. } => None,
        }
    }

    /// The facts, waiting until `deadline` for a reading under way the first time only: once the loop waited in vain it
    /// answers at once until the reading ends. `None` while nothing was read and when the process could not be named.
    pub fn wait_until(&self, deadline: Instant) -> Option<Arc<ProcessFacts>> {
        let mut phase = self.phase();
        loop {
            match &mut *phase {
                Phase::Read(facts) => return facts.clone(),
                Phase::Unstarted | Phase::Reading { waited: true } => return None,
                Phase::Reading { waited } => {
                    let now = Instant::now();
                    if now >= deadline {
                        *waited = true;
                        tracing::debug!(
                            "a connection's process was not read in time: it names nothing for now"
                        );
                        return None;
                    }
                    phase = self
                        .answered
                        .wait_timeout(phase, deadline - now)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                }
            }
        }
    }
}

/// The facts of `process`: its start, its program and the activation token it was started with. `None` when it exited
/// while it was read (its process id may then name another process) or its start cannot be read.
pub fn read_facts(process: &ProcessHandle) -> Option<ProcessFacts> {
    let pid = process.pid;
    let start = parent_and_start(pid)?.1;
    let link = format!("/proc/{pid}/exe");
    let program = std::fs::read_link(&link).ok().map(started_from);
    let trusted_executable = program.as_ref().and_then(|exe| {
        // through the link: the file the process runs, wherever it lies
        let running = std::fs::metadata(&link).ok()?;
        trusted_program(&running, exe).then(|| exe.clone())
    });
    let launch_token = std::fs::read(format!("/proc/{pid}/environ"))
        .ok()
        .and_then(|environ| environ_value(&environ, "XDG_ACTIVATION_TOKEN"));
    // what was read describes the process only while it is the one the descriptor holds
    if process.has_exited() {
        return None;
    }
    Some(ProcessFacts {
        process: (pid, start),
        program,
        trusted_executable,
        launch_token,
    })
}

/// Names the process behind a client; [`peer_process`] in the server.
pub type ProcessOf = Box<dyn Fn(&DisplayHandle, &Client) -> Option<ProcessIdentity>>;

/// The process behind `client` ([`ClientState::process`]): the one that connected to the socket, else the one the server
/// started for the connection. `None` when it is not known or not read in time ([`ClientState::process_facts`]).
pub fn peer_process(_: &DisplayHandle, client: &Client) -> Option<ProcessIdentity> {
    Some(client.get_data::<ClientState>()?.process_facts()?.process)
}

/// Names the program of the process behind a client; [`trusted_executable`] in the server.
pub type ExecutableOf = Box<dyn Fn(&DisplayHandle, &Client) -> Option<PathBuf>>;

/// The program the process behind `client` runs (`/proc/<pid>/exe`), when only the system can have put it there: the
/// file and every folder above it belong to root and nobody else may write to them. A program the user could write or
/// copy into place names nothing, and a process id reused while it was read names nothing either. A program replaced
/// since the process started it, as a package upgrade does, is named by the path it was started from. The file the
/// process runs must be the one at that path here, not a file of the same path in another mount namespace (a folder or
/// file mounted over it there). Read once per connection ([`ProcessFacts::trusted_executable`]).
pub fn trusted_executable(_: &DisplayHandle, client: &Client) -> Option<PathBuf> {
    client
        .get_data::<ClientState>()?
        .process_facts()?
        .trusted_executable
        .clone()
}

/// Whether a process that runs the file `running` (its metadata through `/proc/<pid>/exe`) started from `exe` runs a
/// program only the system can have installed: `exe` is [`system_owned`] and `running` is the file at `exe` in this
/// server's mount namespace, the same device and inode, or, for a program removed or replaced since it started, a
/// removed file on that device that belonged to root and nobody else could write to.
fn trusted_program(running: &std::fs::Metadata, exe: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(installed) = std::fs::metadata(exe) else {
        return false;
    };
    let same_file = running.dev() == installed.dev() && running.ino() == installed.ino();
    let replaced = running.dev() == installed.dev()
        && running.nlink() == 0
        && running.uid() == 0
        && running.mode() & 0o022 == 0;
    system_owned(exe) && (same_file || replaced)
}

/// Names the activation token a client's process was started with; [`launch_token`] in the server.
pub type LaunchTokenOf = Box<dyn Fn(&DisplayHandle, &Client) -> Option<String>>;

/// The `XDG_ACTIVATION_TOKEN` the process behind `client` was started with, for a program that never presents it.
/// Holding a token is the right to use it, so reading it from the process is no more than the process could present
/// itself; a process id reused while it was read names nothing. Read once per connection
/// ([`ProcessFacts::launch_token`]).
pub fn launch_token(_: &DisplayHandle, client: &Client) -> Option<String> {
    client
        .get_data::<ClientState>()?
        .process_facts()?
        .launch_token
        .clone()
}

/// The value of `name` in a NUL-separated environment block.
fn environ_value(environ: &[u8], name: &str) -> Option<String> {
    environ.split(|b| *b == 0).find_map(|entry| {
        let value = entry.strip_prefix(name.as_bytes())?.strip_prefix(b"=")?;
        std::str::from_utf8(value)
            .ok()
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    })
}

/// What the kernel marks a program that was removed or replaced while it runs with, after its path in `/proc/<pid>/exe`.
const REMOVED_PROGRAM_MARK: &str = " (deleted)";

/// The path a process started its program from, out of the link `/proc/<pid>/exe` holds: without the mark of a program
/// removed since.
fn started_from(link: PathBuf) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let bytes = link.as_os_str().as_bytes();
    match bytes.strip_suffix(REMOVED_PROGRAM_MARK.as_bytes()) {
        Some(path) => PathBuf::from(std::ffi::OsStr::from_bytes(path)),
        None => link,
    }
}

/// Whether `path` and each folder above it belong to root and are writable by nobody else.
fn system_owned(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    path.is_absolute()
        && path.ancestors().all(|p| {
            std::fs::symlink_metadata(p)
                .is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0 && !m.file_type().is_symlink())
        })
}

/// Field 22 of a `/proc/<pid>/stat` line. The command name in field 2 may hold spaces and parentheses, so the fields are
/// counted from the last `)`.
fn start_time(stat: &str) -> Option<u64> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    after_name.split_whitespace().nth(19)?.parse().ok()
}

/// Bytes of `/proc/<pid>/stat` read at most. The line is a few hundred bytes (its command name is at most 64), so this
/// bounds the read of a process the caller did not choose.
const STAT_LIMIT: u64 = 4096;

/// The parent process id (field 4) and the start time (field 22) of process `pid`, from a bounded read of
/// `/proc/<pid>/stat`; `None` when the process is gone or the line is not one. It reads `/proc`: not for the loop.
pub fn parent_and_start(pid: u32) -> Option<(u32, u64)> {
    use std::io::Read;
    let mut stat = String::new();
    std::fs::File::open(format!("/proc/{pid}/stat"))
        .ok()?
        .take(STAT_LIMIT)
        .read_to_string(&mut stat)
        .ok()?;
    parent_and_start_of(&stat)
}

/// Fields 4 and 22 of a `/proc/<pid>/stat` line, counted from the last `)` like [`start_time`].
fn parent_and_start_of(stat: &str) -> Option<(u32, u64)> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    let parent = after_name.split_whitespace().nth(1)?.parse().ok()?;
    Some((parent, start_time(stat)?))
}
