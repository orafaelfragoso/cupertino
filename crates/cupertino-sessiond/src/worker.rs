//! The worker: one PAM transaction and, when it succeeds, the session it opens. The manager starts it as
//! `cupertino-sessiond worker` with an empty environment and one socketpair end as standard input. The order below is the
//! privileged path of the design (2.5) and is not to be rearranged:
//!
//! 1. no core dump and no ptrace attach while the process holds root;
//! 2. `pam_start`, the terminal item, then `pam_authenticate` and `pam_acct_mgmt`, the conversation relayed to the manager;
//! 3. the session environment into the PAM environment, `pam_setcred` and `pam_open_session` (`pam_systemd` registers the
//!    logind session here);
//! 4. `fork`; the child becomes the user, checks that it cannot become root again and executes the session command with an
//!    explicit environment;
//! 5. the worker waits for that one child, then closes the PAM session.
//!
//! The worker runs no threads (so `fork` is safe) and makes no D-Bus calls.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::atomic::{AtomicI32, Ordering};

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use pam_client2::{Context, ConversationHandler, ErrorCode, Flag};

use crate::channel::Channel;
use crate::reader;
use crate::users;
use crate::worker_protocol::{FromWorker, Mode, SessionClass, ToWorker, MAX_FRAME};
use cupertino_login_ipc::Outcome;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The seat of every session: the design has one console.
const SEAT: &str = "seat0";

/// How far a worker got, for the event of its failure.
#[derive(Default)]
struct Progress {
    /// The PAM service of `Begin`, once it came.
    service: Option<String>,
    /// The session's leader runs: a failure from here on is not one of the start.
    started: bool,
}

/// Entry point of `cupertino-sessiond worker`. Failures are reported to the manager and the journal, never with a secret.
pub fn run() -> ExitCode {
    // Safety: standard input is the socketpair end the manager gave this process, and nothing else owns it.
    let stream = UnixStream::from(unsafe { OwnedFd::from_raw_fd(libc::STDIN_FILENO) });
    let mut channel = Channel::new(stream, MAX_FRAME);
    let mut progress = Progress::default();
    match session(&mut channel, &mut progress) {
        Ok(status) => ExitCode::from(status),
        Err(e) => {
            eprintln!("cupertino-sessiond: worker: {e}");
            if !progress.started {
                // the error's text names steps and PAM's codes, never the account or what was typed
                diag::event(
                    Severity::Error,
                    "login.session.start_failed",
                    "a session could not be started",
                )
                .field("stage", Value::Public("worker"))
                .field(
                    "service",
                    Value::Public(progress.service.as_deref().unwrap_or("")),
                )
                .field("error", Value::Public(&e.to_string()))
                .emit();
            }
            // the manager may have gone: the journal line above is the record
            if let Ok(frame) = cupertino_login_ipc::encode(
                &FromWorker::Failed {
                    reason: e.to_string(),
                },
                MAX_FRAME,
            ) {
                let _ = channel.send_frame(&frame);
            }
            ExitCode::FAILURE
        }
    }
}

/// PAM's conversation, relayed to the manager: a prompt becomes a `Prompt` and waits for the `Answer`; the modules' messages
/// become `Notice`s. A prompt of any other style, a `Cancel` and a broken channel fail the conversation.
pub(crate) struct Relay {
    channel: Channel,
    /// PAM asked the user something.
    prompted: bool,
    /// A module printed a message before PAM asked anything.
    spoke_first: bool,
}

impl Relay {
    pub(crate) fn new(channel: Channel) -> Self {
        Relay {
            channel,
            prompted: false,
            spoke_first: false,
        }
    }

    /// What the conversation has done so far, for [`outcome_of`].
    fn talk(&self) -> Talk {
        Talk {
            said_before_asking: self.spoke_first,
        }
    }

    fn ask(&mut self, echo: bool, prompt: &CStr) -> std::result::Result<CString, ErrorCode> {
        self.prompted = true;
        let frame = cupertino_login_ipc::encode(
            &FromWorker::Prompt {
                echo,
                text: prompt.to_string_lossy().into_owned(),
            },
            MAX_FRAME,
        )
        .map_err(|_| ErrorCode::CONV_ERR)?;
        self.channel
            .send_frame(&frame)
            .map_err(|_| ErrorCode::CONV_ERR)?;
        match self.channel.receive::<ToWorker>() {
            // `CString::new` copies the text; PAM takes the copy and clears it, and the allocator zeroes what is freed (`zeroing`)
            Ok(ToWorker::Answer { secret }) => {
                CString::new(secret.expose()).map_err(|_| ErrorCode::CONV_ERR)
            }
            Ok(_) | Err(_) => Err(ErrorCode::CONV_ERR),
        }
    }

    fn notice(&mut self, error: bool, message: &CStr) {
        self.spoke_first |= !self.prompted;
        let text = message.to_string_lossy().into_owned();
        if let Ok(frame) =
            cupertino_login_ipc::encode(&FromWorker::Notice { error, text }, MAX_FRAME)
        {
            // PAM's conversation returns at once; a manager that is gone fails the next prompt instead
            let _ = self.channel.send_frame(&frame);
        }
    }
}

impl ConversationHandler for Relay {
    fn prompt_echo_on(&mut self, prompt: &CStr) -> std::result::Result<CString, ErrorCode> {
        self.ask(true, prompt)
    }

    fn prompt_echo_off(&mut self, prompt: &CStr) -> std::result::Result<CString, ErrorCode> {
        self.ask(false, prompt)
    }

    fn text_info(&mut self, message: &CStr) {
        self.notice(false, message);
    }

    fn error_msg(&mut self, message: &CStr) {
        self.notice(true, message);
    }

    // the yes/no and binary styles are not shown on the login screen
    fn radio_prompt(&mut self, _prompt: &CStr) -> std::result::Result<bool, ErrorCode> {
        Err(ErrorCode::CONV_ERR)
    }
}

/// Which PAM call failed, for [`outcome_of`].
#[derive(Clone, Copy, Debug)]
enum Stage {
    Authenticate,
    Account,
}

/// What the PAM conversation did before a call failed.
#[derive(Clone, Copy, Debug)]
struct Talk {
    /// A module printed a message before the first prompt.
    said_before_asking: bool,
}

/// What a failed PAM call means for the user (the design's 2.4): a wrong password and an unknown account are the same
/// `Denied`; a lock-out is `Locked`; an expired account or password is `Expired`; `nologin` is `Refused`.
///
/// `pam_faillock` answers a locked account `AUTH_ERR`, not `MAXTRIES` (measured with Arch's stack), but its `preauth` step says
/// so before the password is asked for ("The account is locked due to 3 failed logins."; the stack goes on to ask and fails).
/// A wrong password is asked for first and nothing has been said, so `AUTH_ERR` after a message that came before the prompt
/// is the lock-out. The text is not matched (it is translated). Any refusal of that shape (`pam_nologin` has it too) is
/// reported `Locked`, and the screen shows PAM's message, which is the explanation the person needs; a silent `AUTH_ERR`
/// stays `Denied`.
fn outcome_of(stage: Stage, code: ErrorCode, talk: Talk) -> Outcome {
    match (stage, code) {
        (_, ErrorCode::MAXTRIES) => Outcome::Locked,
        (Stage::Authenticate, ErrorCode::AUTH_ERR) if talk.said_before_asking => Outcome::Locked,
        (
            Stage::Authenticate,
            ErrorCode::AUTH_ERR
            | ErrorCode::USER_UNKNOWN
            | ErrorCode::CRED_INSUFFICIENT
            | ErrorCode::CONV_ERR,
        ) => Outcome::Denied,
        (Stage::Account, ErrorCode::USER_UNKNOWN) => Outcome::Denied,
        (Stage::Account, ErrorCode::ACCT_EXPIRED | ErrorCode::NEW_AUTHTOK_REQD) => Outcome::Expired,
        (Stage::Account, ErrorCode::PERM_DENIED) => Outcome::Refused,
        _ => Outcome::Internal,
    }
}

/// Steps 1 to 5; the exit status of the leader. `progress` says how far it got.
fn session(channel: &mut Channel, progress: &mut Progress) -> Result<u8> {
    harden()?;
    let ToWorker::Begin {
        service,
        login,
        tty,
        vt,
        class,
        mode,
        command,
        environment,
        path,
        screen_reader,
        failures,
    } = channel.receive()?
    else {
        return Err("the first message is not Begin".into());
    };
    progress.service = Some(service.clone());

    // the conversation reads the answers from the same socket; frames are read whole, so the two readers never split one
    let mut context = Context::new(
        &service,
        Some(&login),
        Relay::new(Channel::new(channel.stream().try_clone()?, MAX_FRAME)),
    )?;
    context.set_tty(Some(&tty))?;
    let decision = context
        .authenticate(Flag::NONE)
        .map_err(|e| (Stage::Authenticate, e))
        .and_then(|()| {
            context
                .acct_mgmt(Flag::NONE)
                .map_err(|e| (Stage::Account, e))
        });
    let outcome = match &decision {
        Ok(()) => Outcome::Accepted,
        Err((stage, e)) => {
            let outcome = outcome_of(*stage, e.code(), context.conversation().talk());
            // the PAM text for the journal (module messages, no secret); `Internal` is the only outcome that hides a cause
            eprintln!(
                "cupertino-sessiond: worker: {stage:?} refused ({}): {e}",
                outcome_name(outcome)
            );
            // never the account: the login is not a field, and PAM's conversation is not recorded at all
            diag::event(
                Severity::Warn,
                "login.auth.failed",
                "an authentication was refused",
            )
            .field("service", Value::Public(&service))
            .field("stage", Value::Public(stage_name(*stage)))
            .field("error", Value::Public(&format!("{:?}", e.code())))
            .field("outcome", Value::Public(outcome_name(outcome)))
            .field("failures", Value::Count(failures.saturating_add(1)))
            .emit();
            outcome
        }
    };
    channel.send_frame(&cupertino_login_ipc::encode(
        &FromWorker::Decided { outcome },
        MAX_FRAME,
    )?)?;
    // a refused account opens nothing, and neither does a user who has a session already: the manager switches to it, and the
    // PAM transaction ends with this process (no credentials were established and no session opened)
    if outcome != Outcome::Accepted || mode == Mode::Verify {
        return Ok(0);
    }
    let authenticated = context.user()?;

    for entry in [
        "XDG_SESSION_TYPE=wayland".to_owned(),
        format!("XDG_SESSION_CLASS={}", class.name()),
        format!("XDG_SEAT={SEAT}"),
        format!("XDG_VTNR={vt}"),
    ] {
        context.putenv(entry)?;
    }
    let session = context
        .open_session(Flag::NONE)
        .map_err(|e| format!("opening the session: {e}"))?;

    let account = users::by_name(&authenticated)?
        .ok_or("the authenticated account has no password database entry")?;
    let session_id = session
        .getenv("XDG_SESSION_ID")
        .ok_or("the PAM stack did not register a login session (pam_systemd missing?)")?
        .to_owned();
    let mut variables: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in session.envlist().iter_tuples() {
        variables.insert(
            name.to_string_lossy().into_owned(),
            value.to_string_lossy().into_owned(),
        );
    }
    let runtime = variables
        .get("XDG_RUNTIME_DIR")
        .cloned()
        .unwrap_or_default();
    let bus = variables.get("DBUS_SESSION_BUS_ADDRESS").cloned();
    let mut environment = environment;
    // the login screen's compositor alone turns the screen reader on and off, over a socket only it is given
    let toggle = if class == SessionClass::Greeter {
        let (mine, theirs) = rustix::net::socketpair(
            rustix::net::AddressFamily::UNIX,
            rustix::net::SocketType::SEQPACKET,
            rustix::net::SocketFlags::CLOEXEC,
            None,
        )?;
        environment.push(format!("{}={}", reader::READER_FD_ENV, reader::READER_FD));
        Some((mine, theirs))
    } else {
        None
    };
    let mut launch = Launch::new(&account, &command, variables, &environment, &path)?;
    launch.toggle = toggle.as_ref().map(|(_, theirs)| theirs.as_raw_fd());
    let leader = spawn_leader(&launch)?;

    // the manager learns the session now; it needs nothing from this process afterwards
    let started = cupertino_login_ipc::encode(
        &FromWorker::Started {
            session_id: session_id.clone(),
            leader: leader as u32,
            uid: account.uid,
        },
        MAX_FRAME,
    )?;
    let _ = channel.send_frame(&started);
    progress.started = true;
    let status = match toggle {
        Some((mine, theirs)) => {
            drop(theirs);
            let reader = reader::Reader::new(
                reader::Account {
                    login: launch.login.clone(),
                    uid: account.uid,
                    gid: account.gid,
                },
                reader::resolve(reader::READERS).as_deref(),
                runtime.into(),
                bus.as_deref(),
                leader as u32,
                reader::session_procs(&session_id),
                screen_reader,
            );
            supervise(channel, leader, &mine, reader)
        }
        None => wait_for(leader),
    };
    session
        .close(Flag::NONE)
        .map_err(|e| format!("closing the session: {e}"))?;
    Ok(status)
}

/// No core file and no debugger can read this process's memory while it holds a password or root.
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

/// Everything the leader needs, built before `fork` so the child allocates nothing it can fail on.
struct Launch {
    login: CString,
    uid: libc::uid_t,
    gid: libc::gid_t,
    home: CString,
    program: CString,
    arguments: Vec<CString>,
    environment: Vec<CString>,
    /// The login screen's end of the screen reader's toggle socket, given to the leader as [`reader::READER_FD`].
    toggle: Option<libc::c_int>,
}

impl Launch {
    /// The environment is exactly the PAM environment (`XDG_*` and what `pam_systemd` and `pam_env` add), the login
    /// variables and the configured extras; nothing of the manager's own and nothing of the login screen's.
    fn new(
        account: &users::Account,
        command: &[String],
        mut variables: BTreeMap<String, String>,
        extras: &[String],
        path: &str,
    ) -> Result<Self> {
        for (name, value) in [
            ("HOME", account.home.as_str()),
            ("USER", &account.login),
            ("LOGNAME", &account.login),
            ("SHELL", &account.shell),
            ("PATH", path),
        ] {
            variables.insert(name.to_owned(), value.to_owned());
        }
        for extra in extras {
            let (name, value) = extra
                .split_once('=')
                .ok_or("an extra environment entry has no `=`")?;
            variables.insert(name.to_owned(), value.to_owned());
        }
        let cstring =
            |text: &str| CString::new(text).map_err(|_| "a launch string contains a nul byte");
        let arguments = command
            .iter()
            .map(|word| cstring(word))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let program = arguments
            .first()
            .ok_or("the session command is empty")?
            .clone();
        Ok(Launch {
            login: cstring(&account.login)?,
            uid: account.uid,
            gid: account.gid,
            home: cstring(&account.home)?,
            program,
            arguments,
            environment: variables
                .iter()
                .map(|(name, value)| cstring(&format!("{name}={value}")))
                .collect::<std::result::Result<Vec<_>, _>>()?,
            toggle: None,
        })
    }
}

/// The pid `forward_signal` passes termination requests to.
static LEADER: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward_signal(signal: libc::c_int) {
    let leader = LEADER.load(Ordering::Relaxed);
    if leader > 0 {
        // Safety: `kill` is async-signal-safe and takes integers.
        unsafe { libc::kill(leader, signal) };
    }
}

/// `fork`, and in the child become the user and `execve`. A failure in the child comes back as the error: the child writes
/// it to a pipe that closes on a successful `execve`.
fn spawn_leader(launch: &Launch) -> Result<libc::pid_t> {
    let mut pipe = [0 as libc::c_int; 2];
    // Safety: `pipe` has room for two descriptors.
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(format!("pipe2: {}", io::Error::last_os_error()).into());
    }
    // Safety: both descriptors are fresh and owned here.
    let (report_read, report_write) =
        unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
    // Safety: the worker has no other thread, so the child of `fork` may allocate.
    match unsafe { libc::fork() } {
        -1 => Err(format!("fork: {}", io::Error::last_os_error()).into()),
        0 => {
            drop(report_read);
            // Safety: in the child, which never returns.
            unsafe { become_user_and_exec(launch, report_write.as_raw_fd()) }
        }
        leader => {
            drop(report_write);
            LEADER.store(leader, Ordering::Relaxed);
            for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
                // Safety: the handler only calls `kill`; the zeroed `sigaction` has an empty mask and no flags.
                unsafe {
                    let mut action: libc::sigaction = std::mem::zeroed();
                    action.sa_sigaction = forward_signal as *const () as usize;
                    libc::sigaction(signal, &action, std::ptr::null_mut());
                }
            }
            let mut message = [0u8; 512];
            let count = loop {
                // Safety: the buffer is valid for its length.
                let count = unsafe {
                    libc::read(
                        report_read.as_raw_fd(),
                        message.as_mut_ptr().cast(),
                        message.len(),
                    )
                };
                if count >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    break count;
                }
            };
            if count > 0 {
                wait_for(leader);
                return Err(format!(
                    "starting the session: {}",
                    String::from_utf8_lossy(&message[..count as usize])
                )
                .into());
            }
            Ok(leader)
        }
    }
}

/// Tell the parent why the child failed, with the current `errno` after `step`, and end the child.
///
/// # Safety
/// Call in the child of `fork` only.
pub(crate) unsafe fn fail(report: libc::c_int, step: &str) -> ! {
    fail_with(report, &format!("{step}: {}", io::Error::last_os_error()))
}

/// # Safety
/// Call in the child of `fork` only.
pub(crate) unsafe fn fail_with(report: libc::c_int, text: &str) -> ! {
    libc::write(report, text.as_ptr().cast(), text.len());
    libc::_exit(127)
}

/// The child of `fork`: the privilege drop of the design, then `execve`.
///
/// # Safety
/// Call in the child of `fork` only. It never returns.
unsafe fn become_user_and_exec(launch: &Launch, report: libc::c_int) -> ! {
    if libc::setsid() < 0 {
        fail(report, "setsid");
    }
    // the groups first, then the group, then the user: the order in which each step is still permitted
    if libc::initgroups(launch.login.as_ptr(), launch.gid) != 0 {
        fail(report, "initgroups");
    }
    if libc::setgid(launch.gid) != 0 {
        fail(report, "setgid");
    }
    if libc::setuid(launch.uid) != 0 {
        fail(report, "setuid");
    }
    // the drop must be permanent: root must not be regainable
    if libc::setuid(0) == 0 || libc::getuid() != launch.uid || libc::geteuid() != launch.uid {
        fail_with(report, "the privilege drop is not permanent");
    }
    if libc::chdir(launch.home.as_ptr()) != 0 {
        fail(report, "chdir to the home directory");
    }
    libc::umask(0o022);
    // default dispositions and an empty mask: the worker's handlers and blocked signals do not reach the session
    for signal in 1..=libc::SIGRTMAX() {
        if signal != libc::SIGKILL && signal != libc::SIGSTOP {
            libc::signal(signal, libc::SIG_DFL);
        }
    }
    let mut empty: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut empty);
    libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
    // standard input from /dev/null; standard output and error stay on the journal
    let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
    if null < 0 || libc::dup2(null, libc::STDIN_FILENO) < 0 {
        fail(report, "opening /dev/null");
    }
    // the login screen's toggle socket becomes descriptor 3: the report pipe moves above it first
    let mut report = report;
    let mut first_closed = 3;
    if let Some(toggle) = launch.toggle {
        report = libc::fcntl(report, libc::F_DUPFD_CLOEXEC, reader::READER_FD + 1);
        if report < 0 {
            libc::_exit(127);
        }
        let placed = if toggle == reader::READER_FD {
            libc::fcntl(toggle, libc::F_SETFD, 0)
        } else {
            libc::dup2(toggle, reader::READER_FD)
        };
        if placed < 0 {
            fail(report, "placing the screen reader's socket");
        }
        first_closed = reader::READER_FD as libc::c_uint + 1;
    }
    // every other descriptor above 2 goes, except the report pipe (it closes by itself on `execve`)
    let close_range = |first: libc::c_uint, last: libc::c_uint| {
        libc::syscall(libc::SYS_close_range, first, last, 0)
    };
    let kept = report as libc::c_uint;
    if (kept > first_closed && close_range(first_closed, kept - 1) != 0)
        || close_range(kept + 1, libc::c_uint::MAX) != 0
    {
        fail(report, "close_range");
    }
    let mut arguments: Vec<*const libc::c_char> =
        launch.arguments.iter().map(|word| word.as_ptr()).collect();
    arguments.push(std::ptr::null());
    let mut environment: Vec<*const libc::c_char> = launch
        .environment
        .iter()
        .map(|entry| entry.as_ptr())
        .collect();
    environment.push(std::ptr::null());
    libc::execve(
        launch.program.as_ptr(),
        arguments.as_ptr(),
        environment.as_ptr(),
    );
    fail(report, "execve")
}

/// The login screen's session: wait for `leader` while running the screen reader as the compositor (`toggle`) and the
/// manager (`channel`) ask. The reader stops before this returns, and for good once the manager is gone.
fn supervise(
    channel: &mut Channel,
    leader: libc::pid_t,
    toggle: &OwnedFd,
    mut reader: reader::Reader,
) -> u8 {
    use rustix::event::{poll, PollFd, PollFlags};
    let Some(leader_fd) = rustix::process::Pid::from_raw(leader).and_then(|pid| {
        rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).ok()
    }) else {
        return wait_for(leader);
    };
    let mut manager = true;
    let mut compositor = true;
    loop {
        let readable = |fd: &PollFd<'_>| {
            fd.revents()
                .intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR)
        };
        let mut fds = vec![PollFd::new(&leader_fd, PollFlags::IN)];
        let at_manager = manager.then(|| {
            fds.push(PollFd::new(channel.stream(), PollFlags::IN));
            fds.len() - 1
        });
        let at_compositor = compositor.then(|| {
            fds.push(PollFd::new(toggle, PollFlags::IN));
            fds.len() - 1
        });
        let reader_fd = reader.pidfd();
        let at_reader = reader_fd.as_ref().map(|fd| {
            fds.push(PollFd::new(fd, PollFlags::IN));
            fds.len() - 1
        });
        match poll(&mut fds, None) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => {
                eprintln!("cupertino-sessiond: worker: poll: {e}");
                reader.stop();
                return wait_for(leader);
            }
        }
        let ready: Vec<bool> = fds.iter().map(readable).collect();
        drop(fds);
        if ready[0] {
            reader.stop();
            return wait_for(leader);
        }
        if at_reader.is_some_and(|i| ready[i]) {
            eprintln!("cupertino-sessiond: worker: the screen reader ended");
            reader.exited();
        }
        if at_compositor.is_some_and(|i| ready[i]) {
            let mut frame = [0u8; reader::MAX_REQUEST + 1];
            match rustix::net::recv(toggle, &mut frame, rustix::net::RecvFlags::DONTWAIT) {
                Ok((0, _)) | Err(_) => compositor = false,
                Ok((count, _)) => {
                    if let Some(reply) = reader.request(&frame[..count.min(frame.len())]) {
                        let _ = rustix::net::send(
                            toggle,
                            reply.text(),
                            rustix::net::SendFlags::DONTWAIT,
                        );
                    }
                }
            }
        }
        if at_manager.is_some_and(|i| ready[i]) {
            match channel.read_message::<ToWorker>() {
                Ok(Some(ToWorker::Shown { shown })) => reader.shown(shown),
                Ok(Some(_)) => {
                    eprintln!("cupertino-sessiond: worker: ignored a message for the login screen")
                }
                Ok(None) => {}
                Err(_) => {
                    // without the manager no one says the login screen is shown again: the reader ends for good
                    manager = false;
                    reader.shown(false);
                }
            }
        }
    }
}

/// Wait for `leader`; its exit status, or 128 plus the signal that ended it.
fn wait_for(leader: libc::pid_t) -> u8 {
    let mut status = 0;
    loop {
        // Safety: `status` is a valid out pointer.
        if unsafe { libc::waitpid(leader, &mut status, 0) } >= 0 {
            break;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return u8::MAX;
        }
    }
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status) as u8
    } else {
        (128 + libc::WTERMSIG(status)) as u8
    }
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Authenticate => "authenticate",
        Stage::Account => "account",
    }
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Accepted => "accepted",
        Outcome::Denied => "denied",
        Outcome::Locked => "locked",
        Outcome::Expired => "expired",
        Outcome::Refused => "refused",
        Outcome::Busy => "busy",
        Outcome::Internal => "internal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A conversation that asked for a password, as a wrong one has.
    const ASKED: Talk = Talk {
        said_before_asking: false,
    };

    #[test]
    fn a_wrong_password_and_an_unknown_account_are_the_same_outcome() {
        assert_eq!(
            outcome_of(Stage::Authenticate, ErrorCode::AUTH_ERR, ASKED),
            Outcome::Denied
        );
        assert_eq!(
            outcome_of(Stage::Authenticate, ErrorCode::USER_UNKNOWN, ASKED),
            Outcome::Denied
        );
        assert_eq!(
            outcome_of(Stage::Account, ErrorCode::USER_UNKNOWN, ASKED),
            Outcome::Denied
        );
    }

    #[test]
    fn a_refusal_that_said_why_before_the_password_prompt_is_a_lock_out() {
        // pam_faillock's lock-out: two info messages, then the prompt, then AUTH_ERR
        let locked = Talk {
            said_before_asking: true,
        };
        assert_eq!(
            outcome_of(Stage::Authenticate, ErrorCode::AUTH_ERR, locked),
            Outcome::Locked
        );
        // a wrong password: the prompt came first and nothing was said before it
        assert_eq!(
            outcome_of(Stage::Authenticate, ErrorCode::AUTH_ERR, ASKED),
            Outcome::Denied
        );
        // the account stage keeps its own mapping
        assert_eq!(
            outcome_of(Stage::Account, ErrorCode::AUTH_ERR, locked),
            Outcome::Internal
        );
    }

    #[test]
    fn the_relay_tells_what_the_conversation_did() {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut relay = Relay::new(Channel::new(theirs, MAX_FRAME));
        assert!(!relay.talk().said_before_asking);
        relay.text_info(c"The account is locked due to 3 failed logins.");
        assert!(relay.talk().said_before_asking);
        // the manager is gone, so the prompt fails at once
        drop(ours);
        assert_eq!(
            relay.prompt_echo_off(c"Password: ").unwrap_err(),
            ErrorCode::CONV_ERR
        );

        // a message after the prompt is not a message before it
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut relay = Relay::new(Channel::new(theirs, MAX_FRAME));
        drop(ours);
        assert!(relay.prompt_echo_off(c"Password: ").is_err());
        relay.text_info(c"Your password will expire in 2 days.");
        assert!(!relay.talk().said_before_asking);
    }

    #[test]
    fn the_other_pam_codes_map_as_the_design_says() {
        assert_eq!(
            outcome_of(Stage::Authenticate, ErrorCode::MAXTRIES, ASKED),
            Outcome::Locked
        );
        assert_eq!(
            outcome_of(Stage::Account, ErrorCode::ACCT_EXPIRED, ASKED),
            Outcome::Expired
        );
        assert_eq!(
            outcome_of(Stage::Account, ErrorCode::NEW_AUTHTOK_REQD, ASKED),
            Outcome::Expired
        );
        assert_eq!(
            outcome_of(Stage::Account, ErrorCode::PERM_DENIED, ASKED),
            Outcome::Refused
        );
        // a cancelled conversation is no one's failure to report
        assert_eq!(
            outcome_of(Stage::Authenticate, ErrorCode::CONV_ERR, ASKED),
            Outcome::Denied
        );
        for code in [
            ErrorCode::ABORT,
            ErrorCode::SYSTEM_ERR,
            ErrorCode::AUTHINFO_UNAVAIL,
            ErrorCode::BUF_ERR,
        ] {
            assert_eq!(
                outcome_of(Stage::Authenticate, code, ASKED),
                Outcome::Internal,
                "{code:?}"
            );
        }
    }

    #[test]
    fn the_relay_refuses_a_prompt_when_the_answer_is_a_cancel_or_the_manager_is_gone() {
        use cupertino_login_ipc::encode;
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut manager = Channel::new(ours, MAX_FRAME);
        let mut relay = Relay::new(Channel::new(theirs, MAX_FRAME));
        manager
            .send_frame(&encode(&ToWorker::Cancel {}, MAX_FRAME).unwrap())
            .unwrap();
        assert_eq!(
            relay.prompt_echo_off(c"Password: ").unwrap_err(),
            ErrorCode::CONV_ERR
        );
        assert!(
            matches!(manager.receive::<FromWorker>().unwrap(), FromWorker::Prompt { echo: false, text } if text == "Password: ")
        );
        drop(manager);
        assert_eq!(
            relay.prompt_echo_on(c"Name: ").unwrap_err(),
            ErrorCode::CONV_ERR
        );
    }

    #[test]
    fn the_relay_returns_the_answer_to_pam() {
        use cupertino_login_ipc::{encode_secret, Secret};
        use zeroize::Zeroizing;
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut manager = Channel::new(ours, MAX_FRAME);
        let mut relay = Relay::new(Channel::new(theirs, MAX_FRAME));
        let secret = Secret::new(Zeroizing::new("hunter2".to_owned())).unwrap();
        manager
            .send_frame(&encode_secret(&ToWorker::Answer { secret }, MAX_FRAME).unwrap())
            .unwrap();
        assert_eq!(
            relay
                .prompt_echo_off(c"Password: ")
                .unwrap()
                .to_str()
                .unwrap(),
            "hunter2"
        );
    }

    #[test]
    fn the_modules_messages_become_notices_in_order() {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut manager = Channel::new(ours, MAX_FRAME);
        let mut relay = Relay::new(Channel::new(theirs, MAX_FRAME));
        relay.error_msg(c"The account is locked due to 3 failed logins.");
        relay.text_info(c"Your password will expire in 2 days.");
        assert!(
            matches!(manager.receive::<FromWorker>().unwrap(), FromWorker::Notice { error: true, text } if text.starts_with("The account is locked"))
        );
        assert!(
            matches!(manager.receive::<FromWorker>().unwrap(), FromWorker::Notice { error: false, text } if text.starts_with("Your password"))
        );
    }
}
