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

use cupertino_defaults::login::{default_admin_groups, LoginConfig, Problem, DEFAULT_ADMIN_GROUPS};
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
use crate::limits::{RateLimit, ATTEMPTS_PER_WINDOW, ATTEMPT_TIMEOUT, ATTEMPT_WINDOW};
use crate::logind::{self, Logind, SeatSession, Signal};
use crate::service::{self, Authorizer, Call, ReadyRefusal};
use crate::system_accessibility;
use crate::worker_protocol::{FromWorker, Mode, SessionClass, ToWorker, MAX_FRAME};
use crate::{picture, roster, users, vt};

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

/// Entry point of `cupertino-sessiond`: runs until a failure; systemd restarts the unit.
pub fn run() -> ExitCode {
    match Manager::start().and_then(|mut manager| manager.run()) {
        Ok(never) => match never {},
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
    /// The unique bus name of the agent that announced the session, for what the manager sends it.
    agent: Option<String>,
    /// The terminal the session runs on, held until the session is gone.
    _terminal: Option<File>,
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
}

impl Manager {
    /// Read the configuration, bind the socket and start the login screen.
    fn start() -> Result<Manager> {
        harden()?;
        let loaded = LoginConfig::load().map_err(|problems| {
            let lines: Vec<String> = problems.iter().map(describe).collect();
            format!("{}: {}", LoginConfig::path().display(), lines.join("; "))
        })?;
        let path = LoginConfig::path();
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

    fn run(&mut self) -> Result<std::convert::Infallible> {
        loop {
            for ready in self.wait()? {
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
        ]
        .into_iter()
        .chain(self.sessions.iter().map(|session| session.ready_deadline))
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

    /// A connection on the screen's socket: only the greeter account, and only one at a time.
    fn accept(&mut self) -> Result<()> {
        let (stream, _) = match self.listener.accept() {
            Ok(pair) => pair,
            // the peer went away between `poll` and `accept`
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(())
            }
            Err(e) => return Err(format!("accept: {e}").into()),
        };
        let peer = rustix::net::sockopt::socket_peercred(&stream)
            .map_err(|e| format!("SO_PEERCRED: {e}"))?;
        if peer.uid.as_raw() != self.greeter.uid {
            eprintln!(
                "cupertino-sessiond: refused a connection from uid {}",
                peer.uid.as_raw()
            );
            return Ok(());
        }
        if self.screen.is_some() {
            eprintln!("cupertino-sessiond: refused a second connection of the login screen");
            return Ok(());
        }
        if let Some(leader) = &self.greeter_leader {
            if !started_by(&stream, leader, Path::new("/proc")) {
                eprintln!("cupertino-sessiond: refused a connection of the greeter's user from outside the login screen");
                return Ok(());
            }
        }
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        self.screen = Some(Screen {
            channel: Channel::new(stream, MAX_REQUEST_FRAME),
            state: State::New,
        });
        eprintln!("cupertino-sessiond: the login screen connected");
        Ok(())
    }

    /// The screen sent bytes, or hung up.
    fn screen_readable(&mut self) {
        let Some(screen) = &mut self.screen else {
            return;
        };
        let request = match screen.channel.read_message::<Request>() {
            Ok(None) => return,
            Ok(Some(request)) => request,
            Err(ChannelError::Closed) => {
                eprintln!("cupertino-sessiond: the login screen disconnected");
                self.drop_screen();
                return;
            }
            Err(e) => {
                // `ChannelError` names the class of the problem, never the message
                eprintln!("cupertino-sessiond: closed the login screen: {e}");
                self.drop_screen();
                return;
            }
        };
        // an answer is for one open prompt of the open attempt; any other is dropped and its secret zeroed with it
        if let Request::Answer { conversation, .. } = &request {
            if !self
                .attempt
                .as_mut()
                .is_some_and(|attempt| attempt.answered(*conversation))
            {
                eprintln!("cupertino-sessiond: dropped an answer to a prompt that is not open");
                return;
            }
        }
        match screen.state.next(&request) {
            Ok(state) => screen.state = state,
            Err(refusal) => {
                eprintln!("cupertino-sessiond: closed the login screen: refused ({refusal:?})");
                self.refuse(refusal);
                return;
            }
        }
        match request {
            Request::Hello { .. } => self.greet(),
            Request::Authenticate { account } => self.authenticate(account),
            Request::AuthenticatePower { name, action } => self.authenticate_power(&name, action),
            Request::Answer { secret, .. } => self.answer(secret),
            Request::Cancel {} => self.cancel(),
            Request::Power { action } => self.screen_power(action),
        }
    }

    /// Tell the screen the version and everything it draws.
    fn greet(&mut self) {
        self.send(&Event::Hello { version: VERSION });
        match self.welcome() {
            Ok(event) => self.send(&event),
            Err(e) => {
                eprintln!("cupertino-sessiond: closed the login screen: {e}");
                self.drop_screen();
            }
        }
    }

    /// The roster of the picker. Its accounts become the ones an attempt may name.
    fn build_roster(&mut self) -> Result<roster::Roster> {
        let accounts = users::login_accounts()?;
        let admins = if self.config.hide_admin_users {
            users::admin_logins(&self.admin_groups)?
        } else {
            Vec::new()
        };
        let reported = &mut self.reported_pictures;
        let logged_in: Vec<u32> = self.sessions.iter().map(|session| session.uid).collect();
        let roster =
            roster::build(
                &self.config,
                accounts,
                &admins,
                &logged_in,
                |account| match picture::read(account.uid, Path::new(&account.home)) {
                    Ok(bytes) => bytes,
                    Err(problem) => {
                        if reported.insert(account.uid) {
                            eprintln!(
                                "cupertino-sessiond: no picture for uid {}: {problem}",
                                account.uid
                            );
                        }
                        None
                    }
                },
            );
        self.listed = roster
            .users
            .iter()
            .map(|row| (row.uid, row.login.clone()))
            .collect();
        Ok(roster)
    }

    /// The `Welcome` event: the roster, the power items and the hint setting.
    fn welcome(&mut self) -> Result<Event> {
        let roster = self.build_roster()?;
        let will_prompt = self.other_users_logged_in()?;
        Ok(Event::Welcome {
            users: roster.users,
            show_other: roster.show_other,
            power: roster::power_menu(&self.config),
            will_prompt,
            retries_until_hint: self.config.retries_until_hint,
        })
    }

    /// Send `event` to the screen and follow its state; a screen that cannot take it is dropped.
    fn send(&mut self, event: &Event) {
        let Some(screen) = &mut self.screen else {
            return;
        };
        let sent = encode(event, MAX_EVENT_FRAME)
            .map_err(|e| e.to_string())
            .and_then(|frame| screen.channel.send_frame(&frame).map_err(|e| e.to_string()));
        match sent {
            Ok(()) => screen.state = screen.state.after(event),
            Err(e) => {
                eprintln!("cupertino-sessiond: closed the login screen: {e}");
                self.drop_screen();
            }
        }
    }

    /// Tell the screen why it is refused, and close the connection.
    fn refuse(&mut self, reason: ProtocolRefusal) {
        self.send(&Event::Refused { reason });
        self.drop_screen();
    }

    /// The screen is gone: its open attempt ends with it.
    fn drop_screen(&mut self) {
        self.screen = None;
        self.cancel();
    }

    /// Whether an attempt may start now; when not, the screen has been answered `Busy`.
    fn may_start_attempt(&mut self) -> bool {
        if self.switch.is_some() {
            eprintln!(
                "cupertino-sessiond: a switch of the console is under way; refusing this attempt"
            );
            self.send(&Event::Result {
                outcome: Outcome::Busy,
            });
            return false;
        }
        if !self.attempts.allow(Instant::now()) {
            eprintln!("cupertino-sessiond: more than {ATTEMPTS_PER_WINDOW} attempts within {} s; refusing this one", ATTEMPT_WINDOW.as_secs());
            self.send(&Event::Result {
                outcome: Outcome::Busy,
            });
            return false;
        }
        true
    }

    /// `Authenticate`: check the account, then run the attempt in a worker.
    fn authenticate(&mut self, account: Account) {
        if !self.may_start_attempt() {
            return;
        }
        // a listed account is named by uid in the journal, a typed one not at all: people type passwords there
        let login = match account {
            Account::Listed { uid } => {
                eprintln!("cupertino-sessiond: attempt for uid {uid}");
                self.listed
                    .iter()
                    .find(|(listed, _)| *listed == uid)
                    .map(|(_, login)| login.clone())
            }
            Account::Typed { name } => {
                eprintln!("cupertino-sessiond: attempt for a typed name");
                is_login_name(&name).then_some(name)
            }
        };
        let Some(login) = login else {
            // not an account the screen could name: answered like a wrong password, after the same time
            self.deny_unknown_account();
            return;
        };
        if let Err(outcome) = self.start_attempt(login, None) {
            self.send(&Event::Result { outcome });
        }
    }

    /// `AuthenticatePower`: an administrator proves her password for a restart or shut down that other users' sessions would
    /// otherwise forbid. Only the password is checked; the action runs when the attempt is accepted. A name that is
    /// no administrator is answered like a wrong password after the same time, so the screen learns nothing about the accounts.
    fn authenticate_power(&mut self, name: &str, action: PowerAction) {
        if !self.may_start_attempt() {
            return;
        }
        // a typed name is not logged: people type passwords there
        eprintln!("cupertino-sessiond: attempt to authorise {action:?}");
        if action == PowerAction::Sleep || !self.power_shown(action) {
            eprintln!(
                "cupertino-sessiond: {action:?} needs no authorisation or is disabled; refusing"
            );
            self.send(&Event::Result {
                outcome: Outcome::Refused,
            });
            return;
        }
        let administrator = match users::admin_logins(&self.admin_groups) {
            Ok(administrators) => {
                is_login_name(name) && administrators.iter().any(|login| login == name)
            }
            Err(e) => {
                eprintln!("cupertino-sessiond: {e}");
                self.send(&Event::Result {
                    outcome: Outcome::Internal,
                });
                return;
            }
        };
        if !administrator {
            self.deny_unknown_account();
            return;
        }
        if let Err(outcome) = self.start_attempt(name.to_owned(), Some(action)) {
            self.send(&Event::Result { outcome });
        }
    }

    /// Answer the open request like a wrong password, after the time one takes.
    fn deny_unknown_account(&mut self) {
        self.attempt = Some(Attempt {
            worker: None,
            uid: None,
            prompt: None,
            deadline: None,
            decision: None,
            existing: None,
            power: None,
        });
        self.denial_at = Some(Instant::now() + INVALID_ACCOUNT_DELAY);
    }

    /// Start the worker; the outcome to report when that cannot be done. An account with a session already is only checked
    /// (the worker opens nothing and the attempt ends in a switch to that session); any other gets a terminal of its own, held
    /// until its session is gone.
    fn start_attempt(
        &mut self,
        login: String,
        power: Option<PowerAction>,
    ) -> std::result::Result<(), Outcome> {
        let uid = users::by_name(&login)
            .map_err(|e| {
                eprintln!("cupertino-sessiond: {e}");
                session_start_failed("account", &e.to_string());
                Outcome::Internal
            })?
            .map(|account| account.uid);
        let existing = uid
            .filter(|_| power.is_none())
            .and_then(|uid| self.sessions.iter().find(|session| session.uid == uid))
            .map(|session| session.id.clone());
        let (mode, vt, terminal) = if existing.is_some() || power.is_some() {
            (Mode::Verify, self.config.vt, None)
        } else {
            match vt::allocate(self.config.vt) {
                Ok(Some((vt, terminal))) => (Mode::Open, vt, Some(terminal)),
                Ok(None) => {
                    eprintln!("cupertino-sessiond: no virtual terminal is free for a session");
                    session_start_failed("terminal", "no virtual terminal is free");
                    return Err(Outcome::Busy);
                }
                Err(e) => {
                    eprintln!("cupertino-sessiond: allocating a virtual terminal: {e}");
                    session_start_failed("terminal", &e.to_string());
                    return Err(Outcome::Internal);
                }
            }
        };
        let begin = ToWorker::Begin {
            service: SESSION_SERVICE.to_owned(),
            login: login.clone(),
            tty: vt::device(vt),
            vt,
            class: SessionClass::User,
            mode,
            command: self.config.session_command.clone(),
            environment: Vec::new(),
            path: self.session_path.clone(),
            screen_reader: false,
            failures: self.refusals,
        };
        let worker = self
            .spawn_worker(Role::Session, &begin, terminal)
            .map_err(|e| {
                eprintln!("cupertino-sessiond: {e}");
                session_start_failed("worker", &e.to_string());
                Outcome::Internal
            })?;
        self.attempt = Some(Attempt {
            worker: Some(worker),
            uid,
            prompt: None,
            deadline: Some(Instant::now() + ATTEMPT_TIMEOUT),
            decision: None,
            existing,
            power,
        });
        Ok(())
    }

    /// `Answer`: pass the screen's answer to the worker that asked.
    fn answer(&mut self, secret: Secret) {
        let Some(attempt) = &mut self.attempt else {
            return;
        };
        attempt.deadline = Some(Instant::now() + ATTEMPT_TIMEOUT);
        let Some(id) = attempt.worker else { return };
        let sent = encode_secret(&ToWorker::Answer { secret }, MAX_FRAME);
        self.send_to_worker(id, sent);
    }

    /// `Cancel` (or the screen's end): stop an attempt that has not been accepted. Once the session starts it runs on.
    fn cancel(&mut self) {
        if cancel_attempt(&mut self.attempt, &mut self.workers) {
            self.denial_at = None;
        }
    }

    /// The attempt waited too long for the screen or its worker: end the worker, whatever the attempt had reached.
    fn abort_attempt(&mut self) {
        let Some(attempt) = &self.attempt else { return };
        let (waited_for, worker) = (
            if attempt.prompt.is_some() {
                "the screen's answer to a prompt"
            } else {
                "its worker"
            },
            attempt.worker,
        );
        eprintln!(
            "cupertino-sessiond: an attempt waited {} s for {waited_for}; cancelling it",
            ATTEMPT_TIMEOUT.as_secs()
        );
        if let Some(worker) =
            worker.and_then(|id| self.workers.iter_mut().find(|worker| worker.id == id))
        {
            if let Err(e) = worker.child.kill() {
                eprintln!("cupertino-sessiond: stopping worker {}: {e}", worker.id);
            }
        }
        self.attempt_failed();
    }

    /// Write a frame to worker `id`; a worker that cannot be written to is on its way out and the exit handles it.
    fn send_to_worker(
        &mut self,
        id: u64,
        frame: std::result::Result<zeroize::Zeroizing<Vec<u8>>, cupertino_login_ipc::FrameError>,
    ) {
        let Some(channel) = self
            .workers
            .iter_mut()
            .find(|worker| worker.id == id)
            .and_then(|worker| worker.channel.as_mut())
        else {
            return;
        };
        if let Err(e) = frame
            .map_err(|e| e.to_string())
            .and_then(|frame| channel.send_frame(&frame).map_err(|e| e.to_string()))
        {
            eprintln!("cupertino-sessiond: worker {id}: {e}");
        }
    }

    /// Take over what an earlier run of the manager left running (`KillMode=process` keeps the workers and their sessions): the
    /// login screen's session, so no second one is started, and the user sessions that the PAM services of the manager opened,
    /// so the roster marks their users and the end of each is followed. A session of another origin (a getty's) is not ours.
    fn adopt_sessions(&mut self) -> Result<()> {
        for found in self.logind.seat_sessions()? {
            let role = match (found.class.as_str(), found.service.as_str()) {
                ("greeter", GREETER_SERVICE) => Role::Greeter,
                ("user", SESSION_SERVICE | AUTO_LOGIN_SERVICE) => Role::Session,
                _ => continue,
            };
            if found.state == "closing" {
                continue;
            }
            if role == Role::Greeter
                && (found.uid != self.greeter.uid || self.greeter_session.is_some())
            {
                eprintln!("cupertino-sessiond: ignoring the login screen session {} (not the greeter account's, or a second one)", found.id);
                continue;
            }
            self.adopt(role, found)?;
        }
        Ok(())
    }

    /// Adopt one session: watch its leader and, for a user, note it in the table and hold its terminal.
    fn adopt(&mut self, role: Role, found: SeatSession) -> Result<()> {
        let leader = i32::try_from(found.leader)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| format!("session {} has no leader process", found.id))?;
        let watch = match pidfd_open(leader, PidfdFlags::empty()) {
            Ok(watch) => watch,
            Err(rustix::io::Errno::SRCH) => {
                // the leader is gone: the session is ending and nothing is left to supervise
                eprintln!(
                    "cupertino-sessiond: session {} has no leader any more; terminating it",
                    found.id
                );
                if let Err(e) = self.logind.terminate_session(&found.id) {
                    eprintln!("cupertino-sessiond: {e}");
                }
                return Ok(());
            }
            Err(e) => return Err(format!("pidfd_open for session {}: {e}", found.id).into()),
        };
        let key = self.next_worker;
        self.next_worker += 1;
        eprintln!(
            "cupertino-sessiond: adopted the {role:?} session {} (process {}, VT {})",
            found.id, found.leader, found.vt
        );
        match role {
            Role::Greeter => {
                self.greeter_session = Some(found.id.clone());
                self.greeter_adopted = true;
            }
            Role::Session => {
                let terminal = match vt::hold(found.vt) {
                    Ok(terminal) => Some(terminal),
                    Err(e) => {
                        eprintln!(
                            "cupertino-sessiond: holding {} for session {}: {e}",
                            vt::device(found.vt),
                            found.id
                        );
                        None
                    }
                };
                // an adopted session was running before this run: its agent announces itself again, so it counts as ready
                self.sessions.push(Session {
                    id: found.id.clone(),
                    uid: found.uid,
                    worker: None,
                    ready: true,
                    ready_deadline: None,
                    agent: None,
                    _terminal: terminal,
                });
            }
        }
        self.adopted.push(Adopted {
            key,
            id: found.id,
            leader: watch,
        });
        Ok(())
    }

    /// The leader of an adopted session exited: its worker closes the PAM session, and what the leader left in the session is
    /// terminated so that logind can remove it (as for a session this run started, see `worker_exited`).
    fn adopted_exited(&mut self, key: u64) {
        let Some(index) = self.adopted.iter().position(|adopted| adopted.key == key) else {
            return;
        };
        let adopted = self.adopted.remove(index);
        eprintln!(
            "cupertino-sessiond: the leader of the adopted session {} exited",
            adopted.id
        );
        self.leader_exited(&adopted.id);
    }

    /// The leader of session `id` exited: logind ends what it left behind, and the login screen takes the console now
    /// when the session had it. logind removes the session only once its last process is gone, which an app that ignores
    /// the end can put off for the stop timeout, and the console must not wait on a session whose compositor has ended.
    fn leader_exited(&mut self, id: &str) {
        if let Err(e) = self.logind.terminate_session(id) {
            eprintln!("cupertino-sessiond: {e}");
        }
        match self.logind.active_session() {
            Ok(Some(foreground)) if foreground == id => {
                eprintln!("cupertino-sessiond: session {id} ended; returning to the login screen");
                self.return_to_login_screen();
            }
            Ok(_) => {}
            Err(e) => eprintln!("cupertino-sessiond: {e}"),
        }
    }

    /// Log the configured account in once per boot, without a password, as the PAM service `cupertino-autologin`. It runs when
    /// the login screen's session is up. The claim is taken first, so a crash of the manager or a restart cannot repeat it, and
    /// it is skipped when the account has a session already (a person got there first).
    fn try_auto_login(&mut self) {
        let Some(login) = self.auto_login.take() else {
            return;
        };
        if let Err(reason) = self.auto_login_start(&login) {
            eprintln!("cupertino-sessiond: auto login: {reason}");
        }
    }

    fn auto_login_start(&mut self, login: &str) -> std::result::Result<(), String> {
        let account = users::login_accounts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|account| account.login == login)
            .ok_or_else(|| format!("{login:?} is not an ordinary account with a login shell"))?;
        if !auto_login::claim(Path::new(auto_login::DONE_FLAG))
            .map_err(|e| format!("{}: {e}", auto_login::DONE_FLAG))?
        {
            return Err("already done since boot".to_owned());
        }
        if self
            .logind
            .user_sessions()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|session| session.uid == account.uid)
        {
            return Err(format!("{login:?} has a session already"));
        }
        let (vt, terminal) = vt::allocate(self.config.vt)
            .map_err(|e| format!("allocating a virtual terminal: {e}"))?
            .ok_or("no virtual terminal is free for a session")?;
        let begin = ToWorker::Begin {
            service: AUTO_LOGIN_SERVICE.to_owned(),
            login: account.login,
            tty: vt::device(vt),
            vt,
            class: SessionClass::User,
            mode: Mode::Open,
            command: self.config.session_command.clone(),
            environment: Vec::new(),
            path: self.session_path.clone(),
            screen_reader: false,
            failures: 0,
        };
        let worker = self
            .spawn_worker(Role::Session, &begin, Some(terminal))
            .map_err(|e| e.to_string())?;
        self.auto_login_worker = Some(worker);
        eprintln!("cupertino-sessiond: auto login for uid {}", account.uid);
        Ok(())
    }

    /// Start the login screen's compositor as a session of class `greeter` on the login terminal.
    fn start_greeter(&mut self) -> Result<()> {
        let begin = ToWorker::Begin {
            service: GREETER_SERVICE.to_owned(),
            login: self.greeter.login.clone(),
            tty: vt::device(self.config.vt),
            vt: self.config.vt,
            class: SessionClass::Greeter,
            mode: Mode::Open,
            command: self.config.greeter_command.clone(),
            environment: vec![format!("{SOCKET_VARIABLE}={SOCKET_PATH}")],
            path: self.session_path.clone(),
            screen_reader: screen_reader_at_login(Path::new(system_accessibility::MACHINE_DIR)),
            failures: 0,
        };
        self.spawn_worker(Role::Greeter, &begin, None).map(drop)
    }

    /// Tell the login screen's worker whether the login screen is on the console: its screen reader runs only then.
    fn tell_greeter_shown(&mut self, shown: bool) {
        let Some(channel) = self
            .workers
            .iter_mut()
            .find(|worker| worker.role == Role::Greeter)
            .and_then(|worker| worker.channel.as_mut())
        else {
            return;
        };
        let sent = encode(&ToWorker::Shown { shown }, MAX_FRAME)
            .map_err(|e| e.to_string())
            .and_then(|frame| channel.send_frame(&frame).map_err(|e| e.to_string()));
        if let Err(e) = sent {
            eprintln!("cupertino-sessiond: cannot tell the login screen's worker whether it is shown: {e}");
        }
    }

    /// The seat's active session changed, by the manager's own switch or by any other (a key that changes the virtual
    /// terminal): the login screen's reader runs only while its session has the console.
    fn active_session_changed(&mut self) {
        let active = self.logind.active_session();
        if let Err(e) = &active {
            eprintln!("cupertino-sessiond: {e}; the login screen counts as hidden");
        }
        let shown = greeter_shown(
            active.ok().flatten().as_deref(),
            self.greeter_session.as_deref(),
        );
        self.tell_greeter_shown(shown);
    }

    /// Start `cupertino-sessiond worker` with one end of a socketpair as its standard input and send it `begin`.
    fn spawn_worker(
        &mut self,
        role: Role,
        begin: &ToWorker,
        terminal: Option<File>,
    ) -> Result<u64> {
        let (ours, theirs) = UnixStream::pair()?;
        let child = worker_command(std::env::current_exe()?, |k| std::env::var_os(k))
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .spawn()
            .map_err(|e| format!("starting a worker: {e}"))?;
        let exited = pidfd_open(
            rustix::process::Pid::from_raw(child.id() as i32)
                .ok_or("a worker has no process id")?,
            PidfdFlags::empty(),
        )
        .map_err(|e| format!("pidfd_open: {e}"))?;
        let mut channel = Channel::new(ours, MAX_FRAME);
        channel.send_frame(&encode(begin, MAX_FRAME)?)?;
        let id = self.next_worker;
        self.next_worker += 1;
        eprintln!("cupertino-sessiond: worker {id} started for the {role:?}");
        self.workers.push(Worker {
            id,
            role,
            child,
            exited,
            channel: Some(channel),
            terminal,
        });
        Ok(id)
    }

    /// A worker wrote.
    fn worker_message(&mut self, id: u64) {
        let Some(worker) = self.workers.iter_mut().find(|worker| worker.id == id) else {
            return;
        };
        let role = worker.role;
        let Some(channel) = &mut worker.channel else {
            return;
        };
        let message = match channel.read_message::<FromWorker>() {
            Ok(None) => return,
            Ok(Some(message)) => message,
            Err(e) => {
                // closed, or broken: either way nothing more comes, and the exit is handled through the process descriptor
                if !matches!(e, ChannelError::Closed) {
                    eprintln!("cupertino-sessiond: worker {id}: {e}");
                }
                worker.channel = None;
                return;
            }
        };
        let conversation = self.next_conversation;
        let current = self
            .attempt
            .as_ref()
            .is_some_and(|attempt| attempt.worker == Some(id));
        match message {
            FromWorker::Prompt { echo, text } if role == Role::Session && current => {
                if let Some(attempt) = &mut self.attempt {
                    attempt.prompt = Some(conversation);
                    self.next_conversation += 1;
                    attempt.deadline = Some(Instant::now() + ATTEMPT_TIMEOUT);
                }
                self.send(&Event::Prompt {
                    conversation,
                    echo,
                    text,
                });
            }
            // the login screen's own session asks nothing, and an attempt that was dropped has no one to ask
            FromWorker::Prompt { .. } => {
                self.send_to_worker(id, encode(&ToWorker::Cancel {}, MAX_FRAME))
            }
            FromWorker::Notice { error, text } if current => {
                self.send(&Event::Notice { error, text })
            }
            FromWorker::Notice { .. } => {}
            FromWorker::Decided { outcome } if current => self.decided(outcome),
            FromWorker::Decided { outcome } => {
                if self.auto_login_worker == Some(id) && outcome != Outcome::Accepted {
                    eprintln!("cupertino-sessiond: auto login was not accepted ({outcome:?})");
                }
            }
            FromWorker::Started {
                session_id,
                leader,
                uid,
            } => self.started(id, role, &session_id, leader, uid),
            FromWorker::Failed { reason } => {
                eprintln!("cupertino-sessiond: worker {id} failed: {reason}");
                if current {
                    self.attempt_failed();
                }
            }
        }
    }

    /// The worker decided the open attempt.
    fn decided(&mut self, outcome: Outcome) {
        let Some(attempt) = &mut self.attempt else {
            return;
        };
        attempt.decision = Some(outcome);
        if outcome != Outcome::Accepted {
            self.refusals = self.refusals.saturating_add(1);
            self.attempt = None;
            self.send(&Event::Result { outcome });
            return;
        }
        self.refusals = 0;
        if let Some(action) = attempt.power {
            // the administrator proved her password: the worker ends, and the action is the manager's to carry out
            self.attempt = None;
            self.send(&Event::Result { outcome });
            eprintln!("cupertino-sessiond: an administrator authorised {action:?}");
            if let Err(reason) = self.power(action, &Requester::Administrator) {
                eprintln!("cupertino-sessiond: power request {action:?} refused: {reason:?}");
                self.send(&Event::PowerRefused { reason });
                self.send(&Event::Reset);
            }
            return;
        }
        // the worker opens the session next, or, for a user who has one, ends
        attempt.deadline = Some(Instant::now() + ATTEMPT_TIMEOUT);
        let (uid, existing) = (attempt.uid, attempt.existing.clone());
        self.send(&Event::Result { outcome });
        let Some(uid) = uid else {
            eprintln!("cupertino-sessiond: the accepted account has no password database entry");
            return;
        };
        match existing {
            Some(session) => {
                eprintln!("cupertino-sessiond: returning to session {session} of uid {uid}");
                self.send(&Event::SessionStarting { uid });
                self.attempt = None;
                self.switch_to_session(&session);
            }
            None => {
                eprintln!("cupertino-sessiond: session starting for uid {uid}");
                self.send(&Event::SessionStarting { uid });
            }
        }
    }

    /// Bring the existing session `id` to the foreground: its agent unlocks it without a second password (the attempt just
    /// proved it; unlock result 4 of the design), then the console switches and the login screen is
    /// shown again for the next time.
    fn switch_to_session(&mut self, id: &str) {
        let agent = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .and_then(|session| session.agent.clone());
        match agent {
            Some(agent) => {
                if let Err(e) = service::unlock(&self._bus, &agent) {
                    eprintln!(
                        "cupertino-sessiond: telling the agent of session {id} to unlock: {e}"
                    );
                }
            }
            None => eprintln!("cupertino-sessiond: session {id} has no agent to unlock it"),
        }
        if let Err(e) = self.activate(id) {
            eprintln!("cupertino-sessiond: {e}");
        }
        self.show_login_screen();
    }

    /// Activate the logind session `id` and note the switch as under way.
    fn activate(&mut self, id: &str) -> std::result::Result<(), logind::Error> {
        // the screen reader of the login screen stops before another session takes the console
        let greeter = self.greeter_session.as_deref() == Some(id);
        if !greeter {
            self.tell_greeter_shown(false);
        }
        self.logind.activate_session(id)?;
        if greeter {
            self.tell_greeter_shown(true);
        }
        let now = Instant::now();
        self.switch = Some(Switch {
            target: id.to_owned(),
            next_poll: now + SWITCH_POLL,
            give_up_at: now + SWITCH_GIVE_UP,
        });
        Ok(())
    }

    /// `SwitchToLoginWindow` of a session's agent (which locked its screen first): the login screen takes the console.
    fn switch_to_login_window(
        &mut self,
        caller: &CallerSession,
    ) -> std::result::Result<(), service::SwitchRefusal> {
        if self.switch.is_some() {
            eprintln!("cupertino-sessiond: session {} asked for the login window while a switch is under way; ignored", caller.id);
            return Ok(());
        }
        let greeter = self
            .greeter_session
            .clone()
            .ok_or(service::SwitchRefusal::NoLoginScreen)?;
        eprintln!(
            "cupertino-sessiond: session {} switches to the login window",
            caller.id
        );
        self.activate(&greeter).map_err(|e| {
            eprintln!("cupertino-sessiond: {e}");
            service::SwitchRefusal::Failed
        })?;
        self.show_login_screen();
        Ok(())
    }

    /// A worker opened its session and runs its leader.
    fn started(&mut self, id: u64, role: Role, session_id: &str, leader: u32, uid: u32) {
        eprintln!("cupertino-sessiond: worker {id}: the {role:?} session {session_id} runs as process {leader}");
        if let Some(worker) = self.workers.iter_mut().find(|worker| worker.id == id) {
            // a session's worker supervises it alone from here; the login screen's keeps its channel
            if role == Role::Session {
                worker.channel = None;
            }
        }
        if role == Role::Greeter {
            self.greeter_session = Some(session_id.to_owned());
            self.greeter_leader = cupertino_peer::ProcessHandle::open(leader).ok();
            self.try_auto_login();
            return;
        }
        let terminal = self
            .workers
            .iter_mut()
            .find(|worker| worker.id == id)
            .and_then(|worker| worker.terminal.take());
        self.sessions.push(Session {
            id: session_id.to_owned(),
            uid,
            worker: Some(id),
            ready: false,
            ready_deadline: None,
            agent: None,
            _terminal: terminal,
        });
        if self
            .attempt
            .as_ref()
            .is_some_and(|attempt| attempt.worker == Some(id))
        {
            self.attempt = None;
        }
        match self.bring_to_console(session_id) {
            // the login screen keeps its spinner until the session's agent says it is up, at most [`SESSION_READY_TIMEOUT`]
            Ok(()) => {
                if let Some(session) = self
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                {
                    session.ready_deadline = Some(Instant::now() + SESSION_READY_TIMEOUT);
                }
            }
            Err(e) => {
                eprintln!("cupertino-sessiond: {e}");
                session_start_failed("console", &e.to_string());
                self.send(&Event::Reset);
            }
        }
    }

    /// The session is brought to the console as soon as its leader runs: the compositor of a session on a terminal that is not
    /// in the foreground does not start its shell processes (it waits for the terminal), so its agent cannot announce the
    /// session before the switch. A session on such a terminal stays inactive until then; if logind has activated it by itself,
    /// the login screen's session takes the console back first.
    fn bring_to_console(&mut self, session_id: &str) -> Result<()> {
        if self.logind.is_active(session_id)? {
            eprintln!("cupertino-sessiond: logind activated session {session_id} by itself; activating the login screen's session again");
            let greeter = self
                .greeter_session
                .as_deref()
                .ok_or("the login screen has no session to activate again")?;
            self.logind.activate_session(greeter)?;
        } else {
            eprintln!("cupertino-sessiond: session {session_id} is not active; activating it");
        }
        self.activate(session_id)?;
        Ok(())
    }

    /// The open attempt cannot go on (its worker failed or died): `Internal` before the decision, a `Reset` after it.
    fn attempt_failed(&mut self) {
        let Some(attempt) = self.attempt.take() else {
            return;
        };
        match attempt.decision {
            None => self.send(&Event::Result {
                outcome: Outcome::Internal,
            }),
            Some(_) => self.send(&Event::Reset),
        }
    }

    /// A deadline passed.
    fn timer(&mut self) -> Result<()> {
        let now = Instant::now();
        if self.denial_at.is_some_and(|at| at <= now) {
            self.denial_at = None;
            if self
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.worker.is_none())
            {
                self.attempt = None;
                self.refusals = self.refusals.saturating_add(1);
                self.send(&Event::Result {
                    outcome: Outcome::Denied,
                });
            }
        }
        if self
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.deadline)
            .is_some_and(|at| at <= now)
        {
            self.abort_attempt();
        }
        if self.greeter_restart_at.is_some_and(|at| at <= now) {
            self.greeter_restart_at = None;
            self.start_greeter()?;
        }
        if self
            .switch
            .as_ref()
            .is_some_and(|switch| switch.next_poll <= now)
        {
            self.follow_switch(now);
        }
        let mut gave_up = false;
        for session in self
            .sessions
            .iter_mut()
            .filter(|session| session.ready_deadline.is_some_and(|at| at <= now))
        {
            session.ready_deadline = None;
            gave_up = true;
            eprintln!("cupertino-sessiond: session {} sent no ready signal within {} s; showing the login screen again", session.id, SESSION_READY_TIMEOUT.as_secs());
        }
        if gave_up {
            self.show_login_screen();
        }
        Ok(())
    }

    /// Ask logind whether the session switched to is in the foreground; the switch is over when it is, or after
    /// [`SWITCH_GIVE_UP`].
    fn follow_switch(&mut self, now: Instant) {
        let Some(switch) = &mut self.switch else {
            return;
        };
        match self.logind.is_active(&switch.target) {
            Ok(true) => self.switch = None,
            Ok(false) if now >= switch.give_up_at => {
                eprintln!("cupertino-sessiond: session {} is not in the foreground {} s after the switch; the switch is over", switch.target, SWITCH_GIVE_UP.as_secs());
                self.switch = None;
            }
            Ok(false) => switch.next_poll = now + SWITCH_POLL,
            Err(e) => {
                // the target is gone (its session ended) or logind failed: either way nothing is under way
                eprintln!("cupertino-sessiond: {e}");
                self.switch = None;
            }
        }
    }

    /// A worker exited: reap it.
    fn worker_exited(&mut self, id: u64) -> Result<()> {
        let Some(index) = self.workers.iter().position(|worker| worker.id == id) else {
            return Ok(());
        };
        let mut worker = self.workers.remove(index);
        let status: ExitStatus = worker.child.wait()?;
        eprintln!(
            "cupertino-sessiond: worker {id} ({:?}) exited: {status}",
            worker.role
        );
        if self.auto_login_worker == Some(id) {
            self.auto_login_worker = None;
        }
        match worker.role {
            Role::Greeter => self.greeter_ended(Some(status)),
            Role::Session => {
                if self
                    .attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.worker == Some(id))
                {
                    self.attempt_failed();
                }
                // the leader is gone and its PAM session closed (the design's final hooks end the session's processes the
                // same way)
                let Some(ended) = self
                    .sessions
                    .iter()
                    .find(|session| session.worker == Some(id))
                    .map(|session| session.id.clone())
                else {
                    return Ok(());
                };
                self.leader_exited(&ended);
                Ok(())
            }
        }
    }

    /// The login screen's session ended, with its worker's `status` (none for a login screen of an earlier run, whose end
    /// logind alone reports): start it again after a second, unless it keeps failing.
    fn greeter_ended(&mut self, status: Option<ExitStatus>) -> Result<()> {
        self.greeter_session = None;
        self.greeter_leader = None;
        let now = Instant::now();
        let giving_up = greeter_gives_up(&mut self.greeter_failures, now);
        greeter_exited(status, self.greeter_failures.len(), giving_up);
        if giving_up {
            eprintln!("cupertino-sessiond: the login screen's session ended {GREETER_FAILURES} times within {} s; not starting it again", GREETER_FAILURE_WINDOW.as_secs());
            self.try_auto_login();
        } else {
            self.greeter_restart_at = Some(now + GREETER_RESTART_DELAY);
        }
        Ok(())
    }

    /// A thread that reads the system bus wrote.
    fn incoming_ready(&mut self) -> Result<()> {
        let mut counter = [0u8; 8];
        // the counter only wakes `poll`; the messages are in the channel
        match rustix::io::read(&*self.wake, &mut counter) {
            Ok(_) | Err(rustix::io::Errno::AGAIN) => {}
            Err(e) => return Err(format!("reading the wake-up: {e}").into()),
        }
        while let Ok(item) = self.incoming.try_recv() {
            match item {
                Incoming::Logind(Signal::SessionRemoved(id)) => self.session_removed(&id)?,
                Incoming::Logind(Signal::ActiveSessionChanged) => self.active_session_changed(),
                Incoming::Logind(Signal::Ended(reason)) => {
                    return Err(format!("logind's signals ended ({reason})").into())
                }
                Incoming::Call(call) => self.call(call),
            }
        }
        Ok(())
    }

    /// A method of `org.cupertino.LoginManager1` that needs the manager's state; the caller is authorised already.
    fn call(&mut self, call: Call) {
        match call {
            Call::Power {
                session,
                action,
                reply,
            } => {
                eprintln!(
                    "cupertino-sessiond: power request {action:?} from session {}",
                    session.id
                );
                let result = self.power(action, &Requester::Agent(&session));
                if reply.send(result).is_err() {
                    eprintln!("cupertino-sessiond: the caller of RequestPower stopped waiting");
                }
            }
            Call::SwitchToLoginWindow { session, reply } => {
                let result = self.switch_to_login_window(&session);
                if reply.send(result).is_err() {
                    eprintln!(
                        "cupertino-sessiond: the caller of SwitchToLoginWindow stopped waiting"
                    );
                }
            }
            Call::Ready {
                session,
                agent,
                reply,
            } => {
                let result = self.session_ready(&session, agent);
                if reply.send(result).is_err() {
                    eprintln!("cupertino-sessiond: the caller of SessionReady stopped waiting");
                }
            }
        }
    }

    /// The agent of a session this manager started is up: it is registered, and the login screen, which kept its spinner
    /// while the session started, is shown again.
    fn session_ready(
        &mut self,
        caller: &CallerSession,
        agent: String,
    ) -> std::result::Result<(), ReadyRefusal> {
        let id = caller.id.as_str();
        // the caller is the user the session was started for, as well as in the session
        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.id == id && session.uid == caller.uid)
            .ok_or(ReadyRefusal::UnknownSession)?;
        session.agent = Some(agent);
        // an agent that restarts, or that finds the manager restarted, announces the session again
        if session.ready {
            eprintln!("cupertino-sessiond: the agent of session {id} announced itself again");
            return Ok(());
        }
        session.ready = true;
        session.ready_deadline = None;
        eprintln!("cupertino-sessiond: session {id} is ready");
        self.show_login_screen();
        Ok(())
    }

    /// `Power` from the login screen: refused with `PowerRefused`, else carried out.
    fn screen_power(&mut self, action: PowerAction) {
        eprintln!("cupertino-sessiond: power request {action:?} from the login screen");
        if let Err(reason) = self.power(action, &Requester::Screen) {
            self.send(&Event::PowerRefused { reason });
        }
    }

    /// Restart, shut down or sleep. A restart or shut down ends other users' sessions: the login screen may ask only when none
    /// is open, and an agent only when none is open that belongs to another user or when it belongs to an administrator (the
    /// reference asks for an administrator's authentication, the dialog is a later unit's). A block inhibitor refuses
    /// the call although the manager is root.
    fn power(
        &mut self,
        action: PowerAction,
        requester: &Requester<'_>,
    ) -> std::result::Result<(), PowerRefusal> {
        if matches!(requester, Requester::Screen | Requester::Administrator)
            && !self.power_shown(action)
        {
            return Err(PowerRefusal::Disabled);
        }
        if action != PowerAction::Sleep && !self.may_end_other_sessions(requester)? {
            return Err(PowerRefusal::OtherUsersLoggedIn);
        }
        self.logind.power(action).map_err(|e| {
            if e.blocked_by_inhibitor() {
                return PowerRefusal::Inhibited;
            }
            eprintln!("cupertino-sessiond: {e}");
            power_failed(action, &e.to_string());
            PowerRefusal::Failed
        })
    }

    /// Whether the settings show the power menu item for `action`.
    fn power_shown(&self, action: PowerAction) -> bool {
        let menu = roster::power_menu(&self.config);
        match action {
            PowerAction::Restart => menu.restart,
            PowerAction::ShutDown => menu.shut_down,
            PowerAction::Sleep => menu.sleep,
        }
    }

    /// Whether a user session is open: Restart and Shut Down then ask the login screen for an administrator
    /// (the screen's rule of [`Manager::may_end_other_sessions`]).
    fn other_users_logged_in(&self) -> std::result::Result<bool, logind::Error> {
        Ok(!self.logind.user_sessions()?.is_empty())
    }

    /// Whether `requester` may end the sessions of users it is not.
    fn may_end_other_sessions(
        &self,
        requester: &Requester<'_>,
    ) -> std::result::Result<bool, PowerRefusal> {
        let failed = |e: &dyn std::fmt::Display| {
            eprintln!("cupertino-sessiond: {e}");
            PowerRefusal::Failed
        };
        let uids: Vec<u32> = self
            .logind
            .user_sessions()
            .map_err(|e| failed(&e))?
            .iter()
            .map(|session| session.uid)
            .collect();
        let asker = match requester {
            Requester::Screen => Asker::Screen,
            Requester::Administrator => Asker::Administrator,
            Requester::Agent(caller) => Asker::Agent { uid: caller.uid },
        };
        may_end_other_sessions(asker, &uids, |uid| {
            let administrators = users::admin_logins(&self.admin_groups).map_err(|e| failed(&e))?;
            let account = users::by_uid(uid).map_err(|e| failed(&e))?;
            Ok(account.is_some_and(|account| administrators.contains(&account.login)))
        })
    }

    /// A session of logind is gone. When it is one this manager started, its terminal is free again. The console goes back to
    /// the login screen only when the seat is left with no session in the foreground, that is when it was the one in the
    /// foreground: a session that ends in the background (its user is on the login screen or in another session) changes
    /// nothing on screen, and the login screen only learns that the list changed.
    fn session_removed(&mut self, id: &str) -> Result<()> {
        self.adopted.retain(|adopted| adopted.id != id);
        // the end of an adopted login screen is told by logind alone (a worker of this run reports the end of its own)
        if self.greeter_adopted && self.greeter_session.as_deref() == Some(id) {
            self.greeter_adopted = false;
            return self.greeter_ended(None);
        }
        let Some(index) = self.sessions.iter().position(|session| session.id == id) else {
            return Ok(());
        };
        // dropping the record releases the terminal
        self.sessions.remove(index);
        let foreground = match self.logind.active_session() {
            Ok(foreground) => foreground,
            Err(e) => {
                // the console is better on the login screen than on a terminal nothing draws on
                eprintln!("cupertino-sessiond: {e}");
                None
            }
        };
        if foreground.is_some() {
            eprintln!("cupertino-sessiond: session {id} ended in the background");
            self.refresh_users();
            return Ok(());
        }
        eprintln!("cupertino-sessiond: session {id} ended; returning to the login screen");
        self.return_to_login_screen();
        Ok(())
    }

    /// Bring the login screen to the console, cleared: the session in the foreground ended.
    fn return_to_login_screen(&mut self) {
        match self.greeter_session.clone() {
            Some(greeter) => {
                if let Err(e) = self.activate(&greeter) {
                    eprintln!("cupertino-sessiond: {e}");
                }
            }
            None => eprintln!("cupertino-sessiond: the login screen has no session to return to"),
        }
        self.show_login_screen();
    }

    /// Send the screen the list again: a session started or ended. What is typed on the screen stays.
    fn refresh_users(&mut self) {
        if self.screen.is_none() {
            return;
        }
        match self
            .build_roster()
            .and_then(|roster| Ok((roster, self.other_users_logged_in()?)))
        {
            Ok((roster, will_prompt)) => self.send(&Event::Users {
                users: roster.users,
                will_prompt,
            }),
            Err(e) => {
                eprintln!("cupertino-sessiond: {e}");
                self.drop_screen();
            }
        }
    }

    /// Clear the login screen and refresh its list: a session started or ended. An attempt that is not accepted yet ends, since
    /// the screen starts again from the picker.
    fn show_login_screen(&mut self) {
        if self.screen.is_none() {
            return;
        }
        self.cancel();
        self.send(&Event::Reset);
        self.refresh_users();
    }
}

/// Whether the login screen is on the console: its session is the seat's active one. An unknown active session (logind did
/// not answer) or an unknown login screen session counts as hidden.
/// Counts an end of the login screen's session at `now` among `ends`, the ends within [`GREETER_FAILURE_WINDOW`];
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

/// `cupertino-sessiond worker` (`exe`) with an empty environment, but for the journal socket the manager's events go
/// to when it names one (`CUPERTINO_DIAG_JOURNAL_SOCKET`, set only by tests): the worker's events go to the same journal.
fn worker_command(exe: PathBuf, var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Command {
    let mut command = Command::new(exe);
    command.arg("worker").env_clear();
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

/// No core file and no ptrace attach on the process that holds passwords and root.
fn harden() -> Result<()> {
    // Safety: `prctl(PR_SET_DUMPABLE)` takes integers only.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        return Err(format!("PR_SET_DUMPABLE: {}", io::Error::last_os_error()).into());
    }
    rustix::process::setrlimit(
        rustix::process::Resource::Core,
        rustix::process::Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_is_private_to_its_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("greeter.sock");
        // Safety: `getuid` and `getgid` have no preconditions.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        let _listener = bind(&path, uid, gid).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // a stale socket of an earlier run does not stop the next bind
        drop(_listener);
        bind(&path, uid, gid).unwrap();
    }

    #[test]
    fn the_login_screen_is_shown_only_while_its_session_has_the_console() {
        assert!(greeter_shown(Some("c1"), Some("c1")));
        assert!(!greeter_shown(Some("2"), Some("c1")), "a person's session");
        assert!(
            !greeter_shown(None, Some("c1")),
            "no session in the foreground, or logind did not answer"
        );
        assert!(!greeter_shown(None, None));
        assert!(
            !greeter_shown(Some("c1"), None),
            "no login screen session known"
        );
    }

    #[test]
    fn only_the_login_screens_own_processes_may_connect() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("greeter.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let _client = UnixStream::connect(&path).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        let me = cupertino_peer::ProcessHandle::open(std::process::id()).unwrap();
        assert!(
            started_by(&accepted, &me, Path::new("/proc")),
            "the leader itself"
        );
        let mut sibling = Command::new("/usr/bin/sleep").arg("60").spawn().unwrap();
        let other = cupertino_peer::ProcessHandle::open(sibling.id()).unwrap();
        assert!(
            !started_by(&accepted, &other, Path::new("/proc")),
            "not a descendant of the leader"
        );
        sibling.kill().unwrap();
        sibling.wait().unwrap();
    }

    fn attempt_asking(conversation: u64) -> Attempt {
        Attempt {
            worker: Some(1),
            uid: Some(1000),
            prompt: Some(conversation),
            deadline: None,
            decision: None,
            existing: None,
            power: None,
        }
    }

    #[test]
    fn an_answer_is_taken_once_and_only_for_the_prompt_that_is_open() {
        let mut attempt = attempt_asking(5);
        assert!(!attempt.answered(4), "an older prompt");
        assert!(!attempt.answered(6), "a prompt that was never asked");
        assert_eq!(
            attempt.prompt,
            Some(5),
            "wrong answers leave the prompt open"
        );
        assert!(attempt.answered(5));
        assert!(!attempt.answered(5), "a replay of the answer");
        assert_eq!(attempt.prompt, None);
    }

    #[test]
    fn an_answer_of_a_finished_attempt_does_not_fit_the_next_attempt() {
        let mut finished = attempt_asking(5);
        assert!(finished.answered(5));
        // the greeter died mid-PAM and its successor replays the answer after the next attempt's first prompt
        let mut next = attempt_asking(6);
        assert!(!next.answered(5));
        assert!(next.answered(6));
    }

    fn running_worker(id: u64) -> Worker {
        Worker {
            id,
            role: Role::Session,
            child: std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .unwrap(),
            exited: OwnedFd::from(File::open("/dev/null").unwrap()),
            channel: None,
            terminal: None,
        }
    }

    /// SEC-06: the login screen dies while PAM waits for its answer: the attempt ends, its worker is killed and the
    /// screen that replaces it cannot answer the dead prompt.
    #[test]
    fn a_screen_that_dies_mid_attempt_ends_the_attempt_and_kills_its_worker() {
        let mut workers = vec![running_worker(1), running_worker(2)];
        let mut attempt = Some(attempt_asking(5));
        assert!(cancel_attempt(&mut attempt, &mut workers));
        assert!(attempt.is_none());
        let killed = workers[0].child.wait().unwrap();
        assert!(
            !killed.success(),
            "the worker of the attempt kept running: {killed:?}"
        );
        assert!(
            workers[1].child.try_wait().unwrap().is_none(),
            "another worker was stopped"
        );

        let mut next = attempt_asking(6);
        assert!(!next.answered(5), "the dead screen's prompt took an answer");
        workers[1].child.kill().unwrap();
        workers[1].child.wait().unwrap();
    }

    /// Once PAM accepted, the session runs on: the screen going away does not stop it.
    #[test]
    fn an_accepted_attempt_survives_its_screen() {
        let mut workers = vec![running_worker(1)];
        let mut attempt = Some(Attempt {
            decision: Some(Outcome::Accepted),
            ..attempt_asking(5)
        });
        assert!(!cancel_attempt(&mut attempt, &mut workers));
        assert!(attempt.is_some());
        assert!(workers[0].child.try_wait().unwrap().is_none());
        workers[0].child.kill().unwrap();
        workers[0].child.wait().unwrap();
    }

    /// A worker starts with nothing of the manager's environment but the journal socket of a test, so a test's worker
    /// never writes to the system journal.
    #[test]
    fn a_worker_gets_an_empty_environment_but_the_tests_journal_socket() {
        let envs = |command: &Command| -> Vec<(String, Option<String>)> {
            command
                .get_envs()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.map(|v| v.to_string_lossy().into_owned()),
                    )
                })
                .collect()
        };
        let exe = PathBuf::from("/usr/lib/cupertino/cupertino-sessiond");
        let plain = worker_command(exe.clone(), |_| None);
        assert_eq!(plain.get_program(), exe.as_os_str());
        assert_eq!(plain.get_args().collect::<Vec<_>>(), ["worker"]);
        assert!(envs(&plain).is_empty());
        let test = worker_command(exe, |k| {
            (k == diag::journal::SOCKET_ENV || k == "HOME").then(|| "/tmp/journal".into())
        });
        assert_eq!(
            envs(&test),
            [(
                diag::journal::SOCKET_ENV.to_owned(),
                Some("/tmp/journal".to_owned())
            )]
        );
    }

    /// A sleep logind fails is `power.suspend.failed`; a restart or shut down it fails is `login.power.failed`.
    #[test]
    fn a_power_action_logind_fails_is_recorded_with_its_action_and_error() {
        diag::init(diag::component!(), "login", diag::Sink::Memory);
        let before = diag::recent().len();
        let refused =
            "putting the computer to sleep: org.freedesktop.DBus.Error.AccessDenied: denied";
        power_failed(PowerAction::Sleep, refused);
        power_failed(PowerAction::Restart, "restarting the computer: timed out");
        power_failed(
            PowerAction::ShutDown,
            "shutting the computer down: timed out",
        );
        let records: Vec<diag::Record> = diag::recent()[before..]
            .iter()
            .map(|line| diag::Record::parse(line).unwrap())
            .collect();
        let seen: Vec<(&str, Option<&str>)> = records
            .iter()
            .map(|r| {
                (
                    r.id.as_str(),
                    r.fields.get("action").and_then(|a| a.as_str()),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("power.suspend.failed", Some("sleep")),
                ("login.power.failed", Some("restart")),
                ("login.power.failed", Some("shut_down")),
            ]
        );
        assert!(records.iter().all(|r| r.severity == Severity::Error));
        assert_eq!(records[0].fields.get("error").unwrap(), refused);
    }

    /// The login screen is started again until it ends [`GREETER_FAILURES`] times within [`GREETER_FAILURE_WINDOW`];
    /// each end is recorded with its status and whether it is started again.
    #[test]
    fn each_end_of_the_login_screen_is_recorded_with_its_status_and_whether_it_restarts() {
        use std::os::unix::process::ExitStatusExt;
        diag::init(diag::component!(), "login", diag::Sink::Memory);
        let before = diag::recent().len();
        let start = Instant::now();
        let mut ends = VecDeque::new();
        // four ends within the window: started again each time
        for i in 0..GREETER_FAILURES as u64 - 1 {
            let now = start + Duration::from_secs(i);
            let giving_up = greeter_gives_up(&mut ends, now);
            assert!(!giving_up);
            greeter_exited(Some(ExitStatus::from_raw(1 << 8)), ends.len(), giving_up);
        }
        // the fifth, killed by SIGSEGV: not started again
        let fifth = start + Duration::from_secs(GREETER_FAILURES as u64 - 1);
        let giving_up = greeter_gives_up(&mut ends, fifth);
        assert!(giving_up);
        greeter_exited(
            Some(ExitStatus::from_raw(libc::SIGSEGV)),
            ends.len(),
            giving_up,
        );
        // after the window the count starts again; a login screen of an earlier run has no status
        let later = fifth + GREETER_FAILURE_WINDOW + Duration::from_secs(1);
        let giving_up = greeter_gives_up(&mut ends, later);
        assert!(!giving_up);
        assert_eq!(ends.len(), 1);
        greeter_exited(None, ends.len(), giving_up);

        let records: Vec<diag::Record> = diag::recent()[before..]
            .iter()
            .map(|line| diag::Record::parse(line).unwrap())
            .collect();
        assert_eq!(records.len(), GREETER_FAILURES + 1);
        for record in &records {
            assert_eq!(record.id, "login.greeter.exited");
            assert_eq!(record.severity, Severity::Warn);
        }
        let first = &records[0].fields;
        assert_eq!(first.get("status").unwrap(), 1);
        assert!(first.get("signal").is_none());
        assert_eq!(first.get("ended_in_window").unwrap(), 1);
        assert_eq!(first.get("restarting").unwrap(), true);
        let fifth = &records[GREETER_FAILURES - 1].fields;
        assert!(fifth.get("status").is_none());
        assert_eq!(fifth.get("signal").unwrap(), i64::from(libc::SIGSEGV));
        assert_eq!(
            fifth.get("ended_in_window").unwrap(),
            GREETER_FAILURES as u64
        );
        assert_eq!(fifth.get("restarting").unwrap(), false);
        let adopted = &records[GREETER_FAILURES].fields;
        assert_eq!(adopted.get("adopted").unwrap(), true);
        assert_eq!(adopted.get("ended_in_window").unwrap(), 1);
        assert_eq!(adopted.get("restarting").unwrap(), true);
    }
}
