//! The manager's main loop: one thread, one `poll` over the login screen's socket, its connection and the workers.
//!
//! The manager is root. It never decodes an image, links no rendering code and takes nothing from the login screen except the
//! typed messages of `cupertino-login-ipc`. Logging follows one rule: a message never holds what the screen sent. Errors of
//! decoding are logged by their class only (serde's text embeds the offending value), listed accounts are logged by uid.

use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{chown, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_defaults::compositor::{COMPOSITOR_CONF_ENV, GREETER_CONF_PATH};
use cupertino_defaults::login::{
    default_admin_groups, LoginConfig, Problem, Replaced, DEFAULT_ADMIN_GROUPS,
};
use cupertino_diagnostic_events::{self as diag, Severity, Value};
use cupertino_login_ipc::message::{MAX_EVENT_FRAME, MAX_REQUEST_FRAME, SOCKET_PATH};
use cupertino_login_ipc::{
    encode, encode_secret, is_login_name, Account, Event, Outcome, PowerAction, PowerRefusal,
    ProtocolRefusal, Request, Secret, State, VERSION,
};
use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags, Timespec};
use rustix::process::{pidfd_open, PidfdFlags};

use crate::auto_login;
use crate::caller::{may_end_other_sessions, Asker, CallerSession};
use crate::channel::{Channel, ChannelError};
use crate::core_limit;
use crate::limits::{RateLimit, ATTEMPTS_PER_WINDOW, ATTEMPT_TIMEOUT, ATTEMPT_WINDOW};
use crate::logind::{self, Logind, SeatSession, Signal};
use crate::service::{self, Authorizer, Call, ReadyRefusal};
use crate::system_accessibility;
use crate::upgrade::{self, Decision};
use crate::worker_protocol::{FromWorker, Mode, SessionClass, ToWorker, MAX_FRAME};
use crate::{picture, roster, users, vt, wallpaper};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The environment variable that tells the login screen where the socket is.
const SOCKET_VARIABLE: &str = "CUPERTINO_LOGIN_SOCK";
/// PAM service of the login screen's own session: no authentication, a logind session of class `greeter`.
const GREETER_SERVICE: &str = "cupertino-greeter";
/// PAM service of a user's session (the package ships `/etc/pam.d/cupertino-session`).
const AUTO_LOGIN_SERVICE: &str = "cupertino-autologin";
const SESSION_SERVICE: &str = "cupertino-session";
/// How long an attempt with an account that cannot exist waits before it is answered `Denied`: as long as `pam_unix` takes
/// to refuse a wrong password (`pam_faildelay`, 2 s in the image), so the answer does not tell the two apart (approximate).
const INVALID_ACCOUNT_DELAY: Duration = Duration::from_secs(2);
/// A login screen's session that ends is started again after this long (throttle, for shell agents,
/// here).
const GREETER_RESTART_DELAY: Duration = Duration::from_secs(1);
/// The login screen failing this many times within [`GREETER_FAILURE_WINDOW`] is not started again: a loop at 1 Hz on a broken
/// GPU is worse than a black terminal (approximate, ours; other terminals still have a getty).
const GREETER_FAILURES: usize = 5;
const GREETER_FAILURE_WINDOW: Duration = Duration::from_secs(30);
/// How long a write to the login screen may take before the screen is dropped (approximate, ours). The loop has one thread,
/// so a screen that stops reading must not stall it.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the manager asks logind whether the session it switched to is in the foreground yet, and how long it waits for that
/// before it stops treating the switch as under way (approximate, ours; a virtual terminal switch takes a few frames).
const SWITCH_POLL: Duration = Duration::from_millis(100);
const SWITCH_GIVE_UP: Duration = Duration::from_secs(5);
/// How long a started session may take to announce that its agent is up before the login screen is shown again without it
/// (the design's login waits for the session at most 15.0 s).
const SESSION_READY_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a started session may take to have its first frame ready on its terminal in the background (`SessionPrepared`)
/// before it is brought to the console all the same, so a compositor that never says so does not keep the person on the login
/// screen (approximate, ours: a session is ready in under a second).
const PREPARE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a session that logs out stays after the login screen took the console, so the login screen's compositor has put
/// its first frame up before the end of the session's compositor takes that compositor's last frame off the display
/// (approximate, ours: a frame takes a few milliseconds).
const LOG_OUT_GRACE: Duration = Duration::from_millis(500);
/// How long the login screen's session has to end when the manager hands over to a new version before its worker is
/// killed and logind ends what is left (approximate, ours: the compositor ends in well under a second).
const GREETER_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Entry point of `cupertino-sessiond`: runs until a failure, or until it hands the console over to a new version of
/// itself ([`upgrade`]); systemd restarts the unit either way.
pub fn run() -> ExitCode {
    match Manager::start().and_then(|mut manager| manager.run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cupertino-sessiond: {e}");
            diag::exit_error(&e);
            ExitCode::FAILURE
        }
    }
}

/// `cupertino-sessiond --check-config`: print each line of the settings file that was not understood, and each key the
/// manager ignores because the file is not root's alone, and fail when there is one, so packaging and administrators see
/// what the manager only logs. A refused file lists its refusals alone.
pub fn check_config() -> ExitCode {
    let path = LoginConfig::path();
    if let Err(e) = File::open(&path) {
        if e.kind() == io::ErrorKind::PermissionDenied {
            eprintln!(
                "{}: cannot be read by this user; run the check as root",
                path.display()
            );
            return ExitCode::FAILURE;
        }
    }
    let problems: Vec<String> = match LoginConfig::load() {
        Ok(loaded) => {
            let mut problems: Vec<String> = loaded
                .problems
                .iter()
                .map(|p| format!("{}; the setting takes its safe value", describe(p)))
                .collect();
            if let Some(warning) = replaced_warning(&path, &loaded.replaced) {
                problems.push(warning);
            }
            if let Err(reason) = auto_login::settings_are_trusted(&path) {
                let config = &loaded.config;
                for (key, set) in [
                    ("auto-login", config.auto_login.is_some()),
                    ("admin-groups", config.admin_groups.is_some()),
                ] {
                    if set {
                        problems.push(format!("{key} is ignored: {reason}"));
                    }
                }
            }
            problems
        }
        Err(problems) => problems
            .iter()
            .map(|p| format!("{}; the manager refuses the file", describe(p)))
            .collect(),
    };
    for problem in &problems {
        eprintln!("{}: {problem}", path.display());
    }
    if problems.is_empty() {
        println!("{}: ok", path.display());
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// The one warning for the commands of the settings file at `path` that name a removed program and run its
/// replacement: which ones, what runs, and the package's new file to merge. `None` when there is none.
fn replaced_warning(path: &Path, replaced: &[Replaced]) -> Option<String> {
    if replaced.is_empty() {
        return None;
    }
    let commands: Vec<String> = replaced
        .iter()
        .map(|r| {
            format!(
                "{} names {}, which this version no longer has, and runs `{}` instead",
                r.key,
                r.program,
                r.command.join(" ")
            )
        })
        .collect();
    Some(format!(
        "{}; merge {}.pacnew into the file",
        commands.join("; "),
        path.display()
    ))
}

/// A problem of the settings file as its line number and message; one of the whole file (line 0) as its message.
fn describe((line, message): &Problem) -> String {
    if *line == 0 {
        message.clone()
    } else {
        format!("line {line}: {message}")
    }
}

/// What a worker is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Greeter,
    Session,
}

struct Worker {
    id: u64,
    role: Role,
    child: Child,
    /// Readable once the worker has exited.
    exited: OwnedFd,
    /// `None` once a session's worker has reported `Started`: it supervises the session alone from then on.
    channel: Option<Channel>,
    /// The terminal of a session until it has started, then the session's.
    terminal: Option<File>,
}

/// Whether the peer of `stream` is `leader` or one of its descendants. The peer is held by the pidfd the kernel gives
/// for the socket and must still be running after its parents were read, and so must the leader, so a reused pid passes
/// for neither; a peer whose parents cannot be read is not.
fn started_by(
    stream: &UnixStream,
    leader: &cupertino_peer::ProcessHandle,
    proc_root: &Path,
) -> bool {
    let Ok(peer) = cupertino_peer::ProcessHandle::of_peer(stream) else {
        return false;
    };
    crate::reader::descends_from(proc_root, peer.pid, leader.pid)
        && !peer.has_exited()
        && !leader.has_exited()
}

/// The machine's `screen_reader_at_login` in `dir` (`/etc/cupertino`), read with the rules of every system file; off
/// without a file or with one the rules refuse.
fn screen_reader_at_login(dir: &Path) -> bool {
    let path = dir.join(cupertino_defaults::accessibility::ACCESSIBILITY_FILE);
    match cupertino_defaults::system::read_root_file(&path) {
        Ok(text) => {
            cupertino_defaults::accessibility::SystemAccessibility::from_text(
                text.as_deref().unwrap_or(""),
            )
            .0
            .screen_reader_at_login
        }
        Err(e) => {
            eprintln!("cupertino-sessiond: ignoring {}: {e}", path.display());
            false
        }
    }
}

/// A session the manager started for a user.
struct Session {
    /// The logind session id.
    id: String,
    /// The user the session belongs to.
    uid: u32,
    /// The worker that supervises the session; `None` for one adopted after a restart of the manager (its worker is not a child
    /// of this process, so the end of the session is told by [`Adopted`] and logind instead).
    worker: Option<u64>,
    /// The session's agent has called `SessionReady`.
    ready: bool,
    /// When the login screen stops waiting for `SessionReady`; `None` once the session is ready or the wait is over.
    ready_deadline: Option<Instant>,
    /// When the session is brought to the console without its `SessionPrepared`: `Some` while its compositor gets its first
    /// frame ready on its terminal in the background, `None` once the session was brought to the console.
    prepare_deadline: Option<Instant>,
    /// The agent announced the session before it was on the console: the login screen is cleared once the switch to the
    /// session is over, not while it is still shown.
    clear_when_shown: bool,
    /// The session logs out (`LogOut`): it is not returned to, and it ends once the login screen has the console.
    ending: Option<Ending>,
    /// The unique bus name of the agent that announced the session, for what the manager sends it.
    agent: Option<String>,
    /// The terminal the session runs on, held until the session is gone.
    _terminal: Option<File>,
}

/// Where a session that logs out is in its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// The login screen is taking the console.
    AfterSwitch,
    /// The login screen has the console: logind ends the session then.
    At(Instant),
    /// logind was asked to end it.
    Asked,
}

/// A session of an earlier run of the manager that this run found in logind (`Service` names the PAM service that opened it, so
/// a getty's session is not one). Its worker is not a child of this process: nothing here learns that the leader exited, so the
/// leader is watched through a descriptor and what it leaves in the session is terminated like a worker's exit would.
struct Adopted {
    key: u64,
    id: String,
    /// Readable once the session's leader has exited.
    leader: OwnedFd,
}

/// The attempt that is open: at most one.
struct Attempt {
    /// The worker that runs it; `None` for an account that cannot exist, answered when `denial_at` falls due.
    worker: Option<u64>,
    /// The user id of the account; `None` for a typed name that no account has.
    uid: Option<u32>,
    /// The conversation id of the prompt the screen has not answered yet.
    prompt: Option<u64>,
    /// When the attempt is cancelled if nothing has happened: the screen has not answered a prompt, or the worker has not
    /// answered the screen's answer or opened the session ([`ATTEMPT_TIMEOUT`]).
    deadline: Option<Instant>,
    /// How the worker decided: `Accepted` until the session runs, then the attempt ends.
    decision: Option<Outcome>,
    /// The id of the user's session when the account has one already: the attempt checks the password and switches to it.
    existing: Option<String>,
    /// The power action an administrator authenticates for: the attempt only checks the password and, once it is
    /// accepted, the action is carried out.
    power: Option<PowerAction>,
}

impl Attempt {
    /// Whether `conversation` is the prompt that is open; it is answered once, so a replay of it is not.
    fn answered(&mut self, conversation: u64) -> bool {
        self.prompt.take_if(|open| *open == conversation).is_some()
    }
}

/// End the open attempt and its worker, unless the attempt was accepted (the session runs on). Returns whether it was not.
fn cancel_attempt(attempt: &mut Option<Attempt>, workers: &mut [Worker]) -> bool {
    if attempt
        .as_ref()
        .is_some_and(|attempt| attempt.decision == Some(Outcome::Accepted))
    {
        return false;
    }
    let Some(attempt) = attempt.take() else {
        return true;
    };
    let Some(worker) = attempt
        .worker
        .and_then(|id| workers.iter_mut().find(|worker| worker.id == id))
    else {
        return true;
    };
    // before the session is accepted the worker holds nothing that needs a clean end
    if let Err(e) = worker.child.kill() {
        eprintln!("cupertino-sessiond: stopping worker {}: {e}", worker.id);
    }
    true
}

/// A switch of the console between sessions that logind has not finished: no second one starts meanwhile
/// (the design ignores a switch request while one is under way).
struct Switch {
    /// The session being brought to the foreground.
    target: String,
    next_poll: Instant,
    give_up_at: Instant,
}

/// The one connection of the login screen.
struct Screen {
    channel: Channel,
    state: State,
}

/// What `poll` reported ready.
#[derive(Clone, Copy, Debug)]
enum Ready {
    Listener,
    Screen,
    WorkerExited(u64),
    WorkerMessage(u64),
    /// The leader of an adopted session (its [`Adopted::key`]) exited.
    AdoptedExited(u64),
    /// logind sent a signal, or the bus service a command.
    Incoming,
    /// A deadline passed.
    Timer,
}

/// What the threads that read the system bus tell the main loop.
enum Incoming {
    Logind(Signal),
    Call(Call),
}

/// The way into the main loop for the threads that read the system bus: the message, then the eventfd that wakes `poll`.
#[derive(Clone)]
struct Mailbox {
    sender: Sender<Incoming>,
    wake: Arc<OwnedFd>,
}

impl Mailbox {
    /// Queue `item` and wake the loop; `false` when the loop is gone.
    fn post(&self, item: Incoming) -> bool {
        if self.sender.send(item).is_err() {
            return false;
        }
        if let Err(e) = rustix::io::write(&*self.wake, &1u64.to_ne_bytes()) {
            eprintln!("cupertino-sessiond: waking the main loop: {e}");
        }
        true
    }
}

/// Who asks for a power action.
enum Requester<'a> {
    /// The login screen, through its socket.
    Screen,
    /// A session's agent, through the bus, authorised as `caller`.
    Agent(&'a CallerSession),
    /// The login screen, after an administrator proved her password for this action.
    Administrator,
}

/// [`Authorizer`] on logind, for the bus service's threads.
struct LogindAuthorizer {
    logind: Logind,
    admin_groups: Vec<String>,
}

impl Authorizer for LogindAuthorizer {
    fn session_of(&self, pid: u32) -> std::result::Result<Option<CallerSession>, String> {
        self.logind.session_of_pid(pid).map_err(|e| e.to_string())
    }

    fn is_administrator(&self, uid: u32) -> std::result::Result<bool, String> {
        users::is_administrator(uid, &self.admin_groups).map_err(|e| e.to_string())
    }
}

/// Where the installed programs are: `/usr`, and the prefix the build names (`CUPERTINO_INSTALL_PREFIX`), as the server
/// has it.
fn installed_roots() -> impl Iterator<Item = &'static Path> {
    ["/usr"]
        .into_iter()
        .chain(option_env!("CUPERTINO_INSTALL_PREFIX"))
        .map(Path::new)
}

struct Manager {
    config: LoginConfig,
    greeter: users::Account,
    session_path: String,
    listener: UnixListener,
    screen: Option<Screen>,
    workers: Vec<Worker>,
    next_worker: u64,
    /// The id the next prompt gets: unique over the manager's life, so no answer fits a prompt of another attempt.
    next_conversation: u64,
    logind: Logind,
    /// What the threads that read the system bus queued, and the descriptor that wakes `poll` for it.
    incoming: Receiver<Incoming>,
    wake: Arc<OwnedFd>,
    /// The connection that serves `org.cupertino.LoginManager1`; the interface is served while it lives.
    _bus: zbus::blocking::Connection,
    /// The sessions started for users.
    sessions: Vec<Session>,
    /// When the login screen's worker failed, within the window that decides whether to start it again.
    greeter_failures: VecDeque<Instant>,
    /// When the login screen's session is started again.
    greeter_restart_at: Option<Instant>,
    /// The logind session of the login screen, once its leader runs.
    greeter_session: Option<String>,
    /// The login screen's session was found in logind when the manager started: no worker of this run supervises it.
    greeter_adopted: bool,
    /// The leader of the login screen's session when a worker of this run started it: only its descendants may connect to
    /// the login screen's socket, so the screen reader (a child of the worker) never can.
    greeter_leader: Option<cupertino_peer::ProcessHandle>,
    /// Sessions of an earlier run that this run watches.
    adopted: Vec<Adopted>,
    /// The account to log in without a password once the login screen's session is up; `None` when the setting is off, refused
    /// or already used.
    auto_login: Option<String>,
    /// The groups whose members are administrators: `admin-groups` of a trusted `login.conf`, else the build's.
    admin_groups: Vec<String>,
    /// The worker that runs the auto login, until it starts the session or ends.
    auto_login_worker: Option<u64>,
    /// Attempts refused in a row since the last accepted one, for the journal.
    refusals: u64,
    /// The attempt that is open.
    attempt: Option<Attempt>,
    /// The switch of the console that is under way.
    switch: Option<Switch>,
    /// How many attempts the screen may start.
    attempts: RateLimit,
    /// The accounts of the last user list sent: (uid, login). `Listed` accounts must be among them.
    listed: Vec<(u32, String)>,
    /// When the open attempt, for an account that cannot exist, is answered `Denied`.
    denial_at: Option<Instant>,
    /// The terminal descriptors held open, see [`vt`].
    _held_terminals: Vec<File>,
    /// Users whose picture file was found unusable, so the journal says it once.
    reported_pictures: HashSet<u32>,
    /// The manager's copies of the users' wallpapers, which the login screen shows.
    wallpapers: wallpaper::Store,
    /// The file this program was started from, watched for a new version; `None` when it cannot be known.
    upgrade: Option<upgrade::Watch>,
    /// When the manager looks at its file next.
    upgrade_check_at: Instant,
    /// The manager has stopped the login screen for a new version and ends.
    handed_over: bool,
}

impl Manager {
    /// Read the configuration, bind the socket and start the login screen.
    fn start() -> Result<Manager> {
        core_limit::harden()?;
        let loaded = LoginConfig::load().map_err(|problems| {
            let lines: Vec<String> = problems.iter().map(describe).collect();
            format!("{}: {}", LoginConfig::path().display(), lines.join("; "))
        })?;
        let path = LoginConfig::path();
        if let Some(warning) = replaced_warning(&path, &loaded.replaced) {
            eprintln!("cupertino-sessiond: {}: {warning}", path.display());
        }
        for problem in &loaded.problems {
            eprintln!(
                "cupertino-sessiond: {}: {}; the setting takes its safe value",
                path.display(),
                describe(problem)
            );
        }
        if !loaded.problems.is_empty() {
            eprintln!(
                "cupertino-sessiond: {}: {} line(s) not understood, the login screen starts with their safe values; `cupertino-sessiond --check-config` lists them",
                path.display(),
                loaded.problems.len()
            );
        }
        let config = loaded.config;
        let greeter = users::by_name(&config.greeter_user)?.ok_or_else(|| {
            format!(
                "greeter-user {:?} has no password database entry",
                config.greeter_user
            )
        })?;
        if greeter.uid == 0 {
            return Err("greeter-user must not be root".into());
        }
        // auto login skips the password and the administrators' groups grant the machine's settings: only a settings file
        // that root alone can change may set them
        let trusted = auto_login::settings_are_trusted(&LoginConfig::path());
        let auto_login = config.auto_login.clone().filter(|_| match &trusted {
            Ok(()) => true,
            Err(reason) => {
                eprintln!("cupertino-sessiond: auto login refused: {reason}");
                false
            }
        });
        let admin_groups = config
            .admin_groups
            .clone()
            .filter(|_| match &trusted {
                Ok(()) => true,
                Err(reason) => {
                    eprintln!(
                        "cupertino-sessiond: admin-groups refused, the build's {DEFAULT_ADMIN_GROUPS} apply: {reason}"
                    );
                    false
                }
            })
            .unwrap_or_else(default_admin_groups);
        // the machine's files are watched by every compositor, the login screen's first: their directory must be there
        if let Err(e) =
            system_accessibility::ensure_dir(Path::new(system_accessibility::MACHINE_DIR))
        {
            eprintln!(
                "cupertino-sessiond: {}: {e}; the machine's accessibility settings apply after a restart only",
                system_accessibility::MACHINE_DIR
            );
        }
        let listener = bind(Path::new(SOCKET_PATH), greeter.uid, greeter.gid)?;
        let (logind, incoming, mailbox) = watch_logind()?;
        let bus = serve_bus(&mailbox, admin_groups.clone())?;
        let upgrade = upgrade::Watch::this_process()
            .map_err(|e| eprintln!("cupertino-sessiond: the program's file is unknown ({e}); a new version starts only with the service"))
            .ok();
        let wallpapers = wallpaper::Store::new(greeter.gid);
        let mut manager = Manager {
            session_path: users::session_path()?,
            _held_terminals: vec![vt::hold(config.vt)
                .map_err(|e| format!("holding {}: {e}", vt::device(config.vt)))?],
            config,
            greeter,
            listener,
            screen: None,
            workers: Vec::new(),
            next_worker: 0,
            next_conversation: 0,
            logind,
            incoming,
            wake: mailbox.wake,
            _bus: bus,
            sessions: Vec::new(),
            greeter_failures: VecDeque::new(),
            greeter_restart_at: None,
            greeter_session: None,
            greeter_adopted: false,
            greeter_leader: None,
            adopted: Vec::new(),
            auto_login,
            admin_groups,
            auto_login_worker: None,
            refusals: 0,
            attempt: None,
            switch: None,
            attempts: RateLimit::new(ATTEMPTS_PER_WINDOW, ATTEMPT_WINDOW),
            listed: Vec::new(),
            denial_at: None,
            reported_pictures: HashSet::new(),
            wallpapers,
            upgrade,
            upgrade_check_at: Instant::now() + upgrade::CHECK_INTERVAL,
            handed_over: false,
        };
        manager.adopt_sessions()?;
        if manager.greeter_session.is_none() {
            manager.start_greeter()?;
        } else {
            manager.try_auto_login();
        }
        notify_ready()?;
        Ok(manager)
    }

    /// Serve until a failure, or (`Ok`) until the manager handed the console over to a new version.
    fn run(&mut self) -> Result<()> {
        loop {
            if self.handed_over {
                return Ok(());
            }
            for ready in self.wait()? {
                if self.handed_over {
                    break;
                }
                match ready {
                    Ready::Listener => self.accept()?,
                    Ready::Screen => self.screen_readable(),
                    Ready::WorkerMessage(id) => self.worker_message(id),
                    Ready::WorkerExited(id) => self.worker_exited(id)?,
                    Ready::AdoptedExited(key) => self.adopted_exited(key),
                    Ready::Incoming => self.incoming_ready()?,
                    Ready::Timer => self.timer()?,
                }
            }
        }
    }

    /// Block until something is ready or a deadline passes.
    fn wait(&self) -> Result<Vec<Ready>> {
        let mut sources = vec![(Ready::Listener, PollFd::new(&self.listener, PollFlags::IN))];
        if let Some(screen) = &self.screen {
            sources.push((
                Ready::Screen,
                PollFd::new(screen.channel.stream(), PollFlags::IN),
            ));
        }
        for worker in &self.workers {
            sources.push((
                Ready::WorkerExited(worker.id),
                PollFd::new(&worker.exited, PollFlags::IN),
            ));
            if let Some(channel) = &worker.channel {
                sources.push((
                    Ready::WorkerMessage(worker.id),
                    PollFd::new(channel.stream(), PollFlags::IN),
                ));
            }
        }
        for adopted in &self.adopted {
            sources.push((
                Ready::AdoptedExited(adopted.key),
                PollFd::new(&adopted.leader, PollFlags::IN),
            ));
        }
        sources.push((Ready::Incoming, PollFd::new(&*self.wake, PollFlags::IN)));
        let (kinds, mut fds): (Vec<Ready>, Vec<PollFd<'_>>) = sources.into_iter().unzip();
        let deadline = [
            self.denial_at,
            self.greeter_restart_at,
            self.attempt.as_ref().and_then(|attempt| attempt.deadline),
            self.switch.as_ref().map(|switch| switch.next_poll),
            self.upgrade.is_some().then_some(self.upgrade_check_at),
        ]
        .into_iter()
        .chain(self.sessions.iter().flat_map(|session| {
            [
                session.ready_deadline,
                session.prepare_deadline,
                session.ending.and_then(|ending| match ending {
                    Ending::At(at) => Some(at),
                    _ => None,
                }),
            ]
        }))
        .flatten()
        .min();
        let timeout = deadline.map(|at| {
            let left = at.saturating_duration_since(Instant::now());
            Timespec {
                tv_sec: left.as_secs() as i64,
                tv_nsec: i64::from(left.subsec_nanos()),
            }
        });
        loop {
            match poll(&mut fds, timeout.as_ref()) {
                Ok(_) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(format!("poll: {e}").into()),
            }
        }
        // a hang-up or an error is readable too: the read reports it
        let mut ready: Vec<Ready> = kinds
            .into_iter()
            .zip(&fds)
            .filter(|(_, fd)| !fd.revents().is_empty())
            .map(|(kind, _)| kind)
            .collect();
        if deadline.is_some_and(|at| at <= Instant::now()) {
            ready.push(Ready::Timer);
        }
        Ok(ready)
    }
}

mod screen;
mod sessions;

/// whether the session is not started again.
fn greeter_gives_up(ends: &mut VecDeque<Instant>, now: Instant) -> bool {
    ends.push_back(now);
    while ends
        .front()
        .is_some_and(|&at| now.duration_since(at) > GREETER_FAILURE_WINDOW)
    {
        ends.pop_front();
    }
    ends.len() >= GREETER_FAILURES
}

/// Records `login.greeter.exited`: the login screen's session ended, with its worker's `status` (its exit code, or the
/// signal that ended it; none for a login screen adopted from an earlier run), the `ended` time within
/// [`GREETER_FAILURE_WINDOW`], and whether it is started again.
fn greeter_exited(status: Option<ExitStatus>, ended: usize, giving_up: bool) {
    use std::os::unix::process::ExitStatusExt;
    let mut event = diag::event(
        Severity::Warn,
        "login.greeter.exited",
        "the login screen's session ended",
    );
    match status {
        Some(status) => {
            if let Some(code) = status.code() {
                event = event.field("status", Value::Int(i64::from(code)));
            }
            if let Some(signal) = status.signal() {
                event = event.field("signal", Value::Int(i64::from(signal)));
            }
        }
        None => event = event.field("adopted", Value::Bool(true)),
    }
    event
        .field("ended_in_window", Value::Count(ended as u64))
        .field("restarting", Value::Bool(!giving_up))
        .emit();
}

/// The running program image, whatever became of its file since.
const OWN_IMAGE: &str = "/proc/self/exe";

/// The program image of this process ([`OWN_IMAGE`], which the kernel resolves in the child to the image it was forked
/// from), so a worker runs the manager's own code even after a package upgrade replaced or removed the file it was started
/// from: that path would then name the new program, or nothing. The first argument is the one this process was started
/// with.
pub fn own_program() -> Command {
    let mut command = Command::new(OWN_IMAGE);
    if let Some(name) = std::env::args_os().next() {
        std::os::unix::process::CommandExt::arg0(&mut command, name);
    }
    command
}

/// `cupertino-sessiond worker` ([`own_program`]) with an empty environment, but for the journal socket the manager's events
/// go to when it names one (`CUPERTINO_DIAG_JOURNAL_SOCKET`, set only by tests): the worker's events go to the same journal.
fn worker_command(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Command {
    let mut command = own_program();
    command.arg("worker").env_clear();
    // the worker hardens itself again, and gives its session the limit the manager was started with
    core_limit::restore_in_child(&mut command);
    if let Some(socket) = var(diag::journal::SOCKET_ENV).filter(|s| !s.is_empty()) {
        command.env(diag::journal::SOCKET_ENV, socket);
    }
    command
}

/// Records that logind failed to carry out `action` for a reason other than an inhibitor: `power.suspend.failed` for
/// sleep, `login.power.failed` for a restart or a shut down. `error` is logind's answer.
fn power_failed(action: PowerAction, error: &str) {
    let event = match action {
        PowerAction::Sleep => diag::event(
            Severity::Error,
            "power.suspend.failed",
            "the computer could not be put to sleep",
        ),
        PowerAction::Restart | PowerAction::ShutDown => diag::event(
            Severity::Error,
            "login.power.failed",
            "the computer could not be restarted or shut down",
        ),
    };
    let name = match action {
        PowerAction::Sleep => "sleep",
        PowerAction::Restart => "restart",
        PowerAction::ShutDown => "shut_down",
    };
    event
        .field("action", Value::Public(name))
        .field("error", Value::Public(error))
        .emit();
}

/// Records `login.session.start_failed`: the manager could not start a session at `stage` (`account`, `terminal`,
/// `worker`, `console`). `error` names steps and errors, never the account.
fn session_start_failed(stage: &'static str, error: &str) {
    diag::event(
        Severity::Error,
        "login.session.start_failed",
        "a session could not be started",
    )
    .field("stage", Value::Public(stage))
    .field("service", Value::Public(SESSION_SERVICE))
    .field("error", Value::Public(error))
    .emit();
}

fn greeter_shown(active: Option<&str>, greeter: Option<&str>) -> bool {
    active.is_some() && active == greeter
}

/// Connect to logind and start its signal thread. The thread queues each signal and writes the eventfd that `poll` watches.
fn watch_logind() -> Result<(Logind, Receiver<Incoming>, Mailbox)> {
    let logind = Logind::connect()?;
    let (sender, incoming) = channel();
    let mailbox = Mailbox {
        sender,
        wake: Arc::new(eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?),
    };
    let signals = mailbox.clone();
    // the receiver lives as long as the manager
    logind.watch(move |signal| {
        signals.post(Incoming::Logind(signal));
    })?;
    Ok((logind, incoming, mailbox))
}

/// Serve `org.cupertino.LoginManager1` on the system bus. Its methods run on the connection's thread, with a connection of
/// their own to logind.
fn serve_bus(mailbox: &Mailbox, admin_groups: Vec<String>) -> Result<zbus::blocking::Connection> {
    let authorizer = Arc::new(LogindAuthorizer {
        logind: Logind::connect()?,
        admin_groups,
    });
    let commands = mailbox.clone();
    let sink: service::Sink = Arc::new(move |call| commands.post(Incoming::Call(call)));
    let machine = service::MachineFiles {
        dir: PathBuf::from(system_accessibility::MACHINE_DIR),
        settings_program: Arc::new(|exe: &Path, file: &std::fs::Metadata| {
            installed_roots().any(|root| exe == root.join("bin/cupertino-settings"))
                && service::only_root_changes(file)
        }),
    };
    Ok(
        service::serve(zbus::Address::system()?, authorizer, sink, machine)
            .map_err(|e| format!("serving {}: {e}", service::BUS_NAME))?,
    )
}

/// Tell systemd the manager is up (`Type=notify`): the socket listens, the login screen's session is starting and the bus name
/// is taken, so a unit ordered after this one finds the name. Outside systemd there is no `NOTIFY_SOCKET` and nothing to tell.
fn notify_ready() -> Result<()> {
    use std::os::linux::net::SocketAddrExt;
    let Some(path) = std::env::var_os("NOTIFY_SOCKET").filter(|path| !path.is_empty()) else {
        return Ok(());
    };
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str());
    // an abstract socket's name starts with `@`
    let address = match bytes.strip_prefix(b"@") {
        Some(name) => std::os::unix::net::SocketAddr::from_abstract_name(name)?,
        None => std::os::unix::net::SocketAddr::from_pathname(&path)?,
    };
    std::os::unix::net::UnixDatagram::unbound()?
        .send_to_addr(b"READY=1", &address)
        .map_err(|e| format!("telling systemd the manager is ready: {e}"))?;
    Ok(())
}

/// Bind the socket at `path`, mode 0600, owned by `uid`. A socket of an earlier run is replaced.
fn bind(path: &Path, uid: u32, gid: u32) -> Result<UnixListener> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("removing {}: {e}", path.display()).into()),
    }
    let listener =
        UnixListener::bind(path).map_err(|e| format!("binding {}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    chown(path, Some(uid), Some(gid))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}
