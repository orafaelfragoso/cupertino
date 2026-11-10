//! The session process: the lock screen and the session agent, a CupertinoKit application. It locks the session on a
//! request, shows one lock screen window per output ([`view::screen`]), sends the attempts to PAM and unlocks only when
//! the state machine has reached [`Phase::Unlocked`], which only an accepted attempt (or the session manager's proof
//! of a fast user switch) does. The agent shows the confirmation panel for log out, restart and shut down, has the
//! server quit the applications one at a time ([`logout`], with the interrupt panel when one does not quit) and then
//! carries out what was confirmed through logind.
//!
//! The lock itself lives in [`LoginRoles`]: they take the session lock before the first lock surface and unlock once
//! this application stopped wanting the lock and the lock windows are gone. Bus threads, PAM and the roles talk to the
//! application through channels that [`listen`] turns into messages.

use crate::options::SessionOptions;
use crate::roles::{self, Heard, LockWish, LoginRoles};
use crate::screen::{self, Result, Timer, FRAME};
use cupertino_kit::core::{Application, Command, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::text::TextSystem;
use cupertino_login::auth::{self, Authenticator};
use cupertino_login::autostart;
use cupertino_login::capabilities::{Answer, Capabilities, Inputs};
use cupertino_login::control::{self, Request};
use cupertino_login::hint::FailedAttempts;
use cupertino_login::interrupt::Choice;
use cupertino_login::lock_notices::{self, Notice};
use cupertino_login::login_manager::{LoginManager, Power};
use cupertino_login::logind::{Inhibitor, Logind, Signal};
use cupertino_login::logout::{self, LogOut};
use cupertino_login::low_battery::{self, LowBattery};
use cupertino_login::model::{
    AuthOutcome, LockScreen, Phase, HIDE_AFTER, HIDE_AFTER_EXPLICIT_LOCK,
};
use cupertino_login::panel::{Action, Decision, Frames, Kind, Panel};
use cupertino_login::power_button::{Effect, PowerButton};
use cupertino_login::probe;
use cupertino_login::restore::{self, Applications, SessionMark, Taken};
use cupertino_login::user::User;
use cupertino_login::view::{
    self, Clock, Field, Input, Metrics, Screen, LOCKED_TITLE, LOCK_APP_ID,
};
use cupertino_login::{session, upower};
use cupertino_modifiers::Profile;
use cupertino_shell_client::{
    IdleNotifier, IdleWatch, LayerShell, Session as ShellSession, SessionLockManager, Shell,
};
use cupertino_ui_wayland::{SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::Event;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::Instant;

/// What the bus and authentication threads tell the application.
#[derive(Clone, Debug)]
pub enum Bus {
    /// `Lock` or `LockNow` on the control interface, logind's `Lock` signal or the preparation for sleep; `explicit` when the
    /// person asked (`LockNow`).
    Lock { explicit: bool },
    /// `RequestLogOut`, `RequestRestart` or `RequestShutDown` on the control interface.
    Panel(Kind),
    /// `PowerKey` on the control interface: the power key went down (`true`) or up.
    PowerKey(bool),
    /// `SwitchToLoginWindow` on the control interface: lock, then hand the console to the login screen.
    SwitchToLoginWindow,
    /// `Sleep` on the control interface: sleep without a panel.
    Sleep,
    /// The session manager's `Unlock`: the person at the login screen proved this session's password.
    Unlock,
    /// The session manager's bus name has an owner again: it restarted and does not know this session's agent.
    ManagerRestarted,
    /// A message or the outcome of an attempt.
    Auth(auth::Event),
    /// A reader thread broke; the process exits so that the server restarts it.
    Failed(String),
    /// The power daemon's display device changed, or its watch stopped (the warnings stop; the lock screen goes on).
    Battery(std::result::Result<upower::Report, String>),
    /// The server's `ApplicationQuit`: the last window of the application being quit went away.
    ApplicationQuit(String),
    /// What the notification daemon lets the lock screen show now ([`lock_notices`]).
    LockNotices(Vec<Notice>),
}

#[derive(Clone, Debug)]
pub enum Message {
    Bus(Bus),
    Heard(Heard),
    Input(Input),
    Timer(u64),
    /// The server closed a lock surface.
    LockClosed,
    /// The named channel has no sender left.
    Unheard(&'static str),
}

/// The session while it is locked or being locked.
struct Lock {
    screen: LockScreen,
    /// The server's answer to the lock: confirmed (`true`) or refused or ended.
    answer: Option<bool>,
    /// The server confirmed the lock.
    confirmed: bool,
}

/// The controller consent and status panel.
struct DesktopControlPanel {
    epoch: u64,
    output: String,
    capabilities: u32,
    phase: u32,
}

/// The confirmation panel on screen.
struct PanelState {
    panel: Panel,
    /// Where its parts sit; its size is the window's.
    frames: Frames,
}

/// A confirmed log out, restart or shut down while the applications are quit.
struct Quitting {
    sequence: LogOut,
    /// What follows once no application is left.
    then: Action,
    /// The application the interrupt panel asks about.
    interrupt: Option<String>,
    /// The "Reopen windows" choice the panel was confirmed with.
    reopen: bool,
    /// The identities the server named as it asked each application to quit, in that order.
    quit: Vec<String>,
}

pub struct SessionApp {
    /// Asks the probe thread to read logind and the session manager again.
    refresh: mpsc::Sender<()>,
    bus: async_channel::Receiver<Bus>,
    heard: async_channel::Receiver<Heard>,
    /// The lock as the roles and this application share it.
    wish: Rc<RefCell<LockWish>>,
    /// Why the application ended, for [`run`].
    failure: Rc<RefCell<Option<String>>>,
    /// Keeps the lock and log-out requests coming.
    _session: ShellSession,
    _idle: Option<IdleWatch>,
    /// The session asks for reduced motion: the lock screen follows it.
    reduce_motion: bool,
    /// The seat went idle and input has not resumed since.
    idled: bool,
    outputs: Vec<(u32, Size)>,
    caps_lock: bool,
    metrics: Metrics,
    text: Arc<TextSystem>,
    clock: Clock,
    timer: Timer,
    user: User,
    authenticator: Authenticator,
    logind: Logind,
    /// The session manager: power actions and inhibitor locks.
    manager: LoginManager,
    control: control::Service,
    /// Held while the session is unlocked, so the system cannot sleep before the lock is up.
    inhibitor: Option<Inhibitor>,
    /// Held for the whole run: logind does not power off on the power key, [`SessionApp::power_key`] decides what it
    /// does.
    _power_key: Inhibitor,
    /// The hold timing of the power key.
    power_button: PowerButton,
    start: Instant,
    lock: Option<Lock>,
    /// `SwitchToLoginWindow` is waiting for the server to confirm the lock.
    switch_pending: bool,
    panel: Option<PanelState>,
    desktop_control: Option<DesktopControlPanel>,
    control_wish: Rc<RefCell<roles::ControlWish>>,
    /// A confirmed log-out is running: the power button panel does not show over it.
    logging_out: bool,
    /// The applications are being quit before a confirmed action.
    quitting: Option<Quitting>,
    /// The file of the "Reopen windows" choice and list ([`restore`]); `None` without a state or home directory.
    restore_path: Option<PathBuf>,
    /// Wrong passwords typed at the lock screens of this run.
    failed: FailedAttempts,
    /// `RetriesUntilHint`, see [`FailedAttempts::help_shown`].
    retries_until_hint: u32,
    /// The low-battery notification on screen.
    low_battery: LowBattery,
    /// The session bus, for the notifications and the server's `org.cupertino.Session1`.
    session_bus: zbus::blocking::Connection,
    /// The notifications the lock screen shows, as the daemon last listed them; shown only while locked.
    lock_notices: Vec<Notice>,
}

/// Why [`run`] returned without a failure.
pub enum Ended {
    /// The process belongs to no login session (a container, CI): there is no session to lock.
    NoLoginSession,
}

/// The next message on `channel`, or [`Message::Unheard`] once nothing can send on it.
pub fn listen<T: Send + 'static, M: Send + 'static>(
    channel: &async_channel::Receiver<T>,
    message: fn(T) -> M,
    unheard: M,
) -> Command<M> {
    let channel = channel.clone();
    Command::Perform(Box::pin(async move {
        channel.recv().await.map_or(unheard, message)
    }))
}

/// Connect, serve the control interface and run until a connection or thread fails.
pub fn run(options: &SessionOptions) -> Result<Ended> {
    let user = User::current()?;
    let mut system = WaylandWindowSystem::connect(Profile::cupertino())
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let conn = system.connection();
    let layer_shell = LayerShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let shell = Shell::bind(conn).ok_or("the server lacks cupertino_shell_v1")?;
    let (session, session_state, early) = shell.observe(conn)?;
    let lock_manager = SessionLockManager::bind(conn);
    let authority = cupertino_shell_client::ControlAuthority::bind(conn);
    let idle_notifier = IdleNotifier::bind(conn);
    let (bus, bus_messages) = async_channel::unbounded::<Bus>();
    let send = |bus: &async_channel::Sender<Bus>| {
        let bus = bus.clone();
        // the receiver lives as long as the application
        move |message: Bus| drop(bus.send_blocking(message))
    };

    let request = send(&bus);
    // `RefreshCapabilities` goes to the probe thread, not through the application: the menu's question must not wait
    let (refresh, refresh_requests) = mpsc::channel::<()>();
    let refresh_from_bus = refresh.clone();
    let on_request = Arc::new(move |request_made| {
        if request_made == Request::RefreshCapabilities {
            // the probe thread is gone only while the process ends
            let _ = refresh_from_bus.send(());
            return;
        }
        request(match request_made {
            Request::Lock { explicit } => Bus::Lock { explicit },
            Request::Panel(kind) => Bus::Panel(kind),
            Request::PowerKey { pressed } => Bus::PowerKey(pressed),
            Request::SwitchToLoginWindow => Bus::SwitchToLoginWindow,
            Request::Sleep => Bus::Sleep,
            Request::RefreshCapabilities => unreachable!("handled above"),
        })
    });
    let served = on_request.clone();
    let control = match control::serve(
        zbus::connection::Builder::session()?,
        user.display.clone(),
        Capabilities::default(),
        move |request| served(request),
    ) {
        Ok(control) => control,
        // Another process took the name first. Quitting would leave the session with no lock screen at all; instead this
        // process runs on the server's private connection, which carries the lock request, and on logind's signals.
        Err(zbus::Error::NameTaken) => {
            eprintln!(
                "cupertino-login: error: another process owns org.cupertino.Login1; the menus, the power key and the panels \
                 that come over the session bus do not reach this process, the lock still does"
            );
            control::serve_unnamed(
                zbus::connection::Builder::session()?,
                user.display.clone(),
                Capabilities::default(),
                move |request| on_request(request),
            )?
        }
        Err(e) => return Err(Box::new(e)),
    };

    let Some(logind) = Logind::connect()? else {
        return Ok(Ended::NoLoginSession);
    };
    let signal = send(&bus);
    logind.listen(move |s| {
        signal(match s {
            Ok(Signal::Lock | Signal::PrepareForSleep) => Bus::Lock { explicit: false },
            Err(e) => Bus::Failed(e.to_string()),
        })
    })?;
    let battery = send(&bus);
    upower::watch(logind.connection(), move |report| {
        battery(Bus::Battery(report.map_err(|e| e.to_string())))
    })?;
    let manager = LoginManager::connect()?;
    let unlock = send(&bus);
    manager.watch_unlock(move |signal| {
        unlock(match signal {
            Ok(()) => Bus::Unlock,
            Err(e) => Bus::Failed(e.to_string()),
        })
    })?;
    let restarted = send(&bus);
    manager.watch_owner(move |signal| {
        restarted(match signal {
            Ok(()) => Bus::ManagerRestarted,
            Err(e) => Bus::Failed(e.to_string()),
        })
    })?;
    let inhibitor = Some(logind.inhibit_sleep()?);
    let power_key = logind.block_power_key()?;

    let attempt = send(&bus);
    // the attempts' outcomes are the only thing this thread says; PAM's delay after a failure holds the outcome back
    let authenticator =
        Authenticator::spawn(user.login.clone(), move |event| attempt(Bus::Auth(event)))?;

    let conn = system.connection();
    let idle = match options.idle_timeout {
        Some(timeout) => {
            let notifier = idle_notifier.ok_or("the server lacks ext_idle_notifier_v1")?;
            let millis = u32::try_from(timeout.as_millis())
                .map_err(|_| "the idle timeout is longer than the protocol holds")?;
            let seat = conn.seat().ok_or("the server lacks wl_seat")?;
            Some(notifier.watch(conn, &seat, millis))
        }
        None => None,
    };
    let outputs = roles::outputs(conn);
    let session_bus = control.connection();
    let quit = send(&bus);
    session::watch_quit(&session_bus, move |app| {
        quit(match app {
            Ok(app) => Bus::ApplicationQuit(app),
            Err(e) => Bus::Failed(format!("reading ApplicationQuit: {e}")),
        })
    })?;
    let notices = send(&bus);
    // the lock screen works without the notifications: a watch that cannot start is reported, not fatal
    if let Err(e) = lock_notices::watch(&session_bus, move |list| notices(Bus::LockNotices(list))) {
        eprintln!("cupertino-login: the lock screen shows no notifications: {e}");
    }
    drop(bus);
    let login_items = start_login_items(&logind);
    let restore_path = restore::state_path_from_env();
    reopen_applications(restore_path.as_deref(), &logind, &login_items);
    // a session the manager did not start (one on a getty, for the tests) is not known to it; that is reported, not fatal
    match manager.session_ready() {
        Ok(()) => eprintln!("cupertino-login: session announced to the session manager"),
        Err(e) => eprintln!("cupertino-login: {e}"),
    }
    spawn_capability_probe(refresh_requests, &logind, &manager, &control)?;
    // the probe reads once at the start
    let _ = refresh.send(());

    let (heard, heard_messages) = async_channel::unbounded();
    // the session is locked as this process starts: its lock screen went away without unlocking (it crashed and the server
    // restarted this process), and the outputs stay black with nothing to type a password into until this one takes the lock
    if session_state.locked {
        eprintln!("cupertino-login: the session is locked without a lock screen: taking the lock");
        // the receiver is held just above
        let _ = heard.send_blocking(Heard::LockRequested { explicit: false });
    }
    let wish = Rc::new(RefCell::new(LockWish::default()));
    let control_wish = Rc::new(RefCell::new(roles::ControlWish::default()));
    let mut roles = LoginRoles::new(
        layer_shell,
        lock_manager,
        Some(session_state),
        Rc::clone(&wish),
        heard,
    )
    .with_control(authority, Rc::clone(&control_wish));
    // what the server said before the roles listened: a lock request that came with the connection
    for event in early {
        if let Event::Extension(event) = event {
            roles.event(&event);
        }
    }
    let system = system.with_roles(roles);
    let failure = Rc::new(RefCell::new(None));
    let ended = Rc::clone(&failure);
    let power_button = PowerButton::new(options.power_button_sleeps_system);
    let retries_until_hint = options.retries_until_hint;
    cupertino_kit::app::run_lock_screen_on_with::<SessionApp>(system, move |runtime| {
        let app = SessionApp {
            refresh,
            bus: bus_messages,
            heard: heard_messages,
            wish,
            failure: ended,
            _session: session,
            _idle: idle,
            reduce_motion: session_state.display.reduce_motion,
            idled: false,
            outputs,
            caps_lock: false,
            metrics: Metrics::of(&runtime.text),
            text: Arc::clone(&runtime.text),
            clock: Clock::default(),
            timer: Timer::default(),
            user,
            authenticator,
            logind,
            manager,
            control,
            inhibitor,
            _power_key: power_key,
            power_button,
            start: Instant::now(),
            lock: None,
            switch_pending: false,
            panel: None,
            desktop_control: None,
            control_wish,
            logging_out: false,
            quitting: None,
            restore_path,
            failed: FailedAttempts::default(),
            retries_until_hint,
            low_battery: LowBattery::new(),
            session_bus,
            lock_notices: Vec::new(),
        };
        let commands = vec![
            listen(&app.bus, Message::Bus, Message::Unheard("the bus threads")),
            listen(
                &app.heard,
                Message::Heard,
                Message::Unheard("the surface roles"),
            ),
        ];
        (app, commands)
    })?;
    let reason = failure.borrow_mut().take();
    Err(reason
        .unwrap_or_else(|| "the lock screen ended".to_owned())
        .into())
}

/// Start the session's login items (XDG autostart, [`autostart`]) one after the other, once per session: a restarted agent
/// finds the run claimed and starts nothing. Problems are reported and leave the lock screen running. Returns the IDs
/// (file names) of the items started.
fn start_login_items(logind: &Logind) -> HashSet<String> {
    let mut started = HashSet::new();
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else {
        eprintln!("cupertino-login: login items not started: XDG_RUNTIME_DIR is not set, so a second run cannot be told apart");
        return started;
    };
    match autostart::claim_run(Path::new(&runtime), logind.session_id()) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("cupertino-login: login items ran already in this session");
            return started;
        }
        Err(e) => {
            eprintln!("cupertino-login: login items not started: {e}");
            return started;
        }
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")));
    let dirs = autostart::directories(config_home, std::env::var("XDG_CONFIG_DIRS").ok());
    let found = match autostart::list(&dirs) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("cupertino-login: login items not started: {e}");
            return started;
        }
    };
    let env = autostart::Environment::from_process();
    for entry in found {
        match autostart::parse(&entry.id, &entry.path, &entry.text, &env) {
            Ok(item) => {
                if start_login_item(&item) {
                    started.insert(item.id);
                }
            }
            Err(skip) => eprintln!(
                "cupertino-login: login item skipped: {}: {skip:?}",
                entry.id
            ),
        }
    }
    started
}

/// Start the applications the last session saved to reopen ([`restore`]), each once and only once: the list is emptied
/// on disk before anything starts, and an application a login item started already is not started again.
fn reopen_applications(path: Option<&Path>, logind: &Logind, login_items: &HashSet<String>) {
    let Some(path) = path else {
        return eprintln!(
            "cupertino-login: applications not reopened: neither XDG_STATE_HOME nor HOME is set"
        );
    };
    let current = SessionMark::current(logind.session_id());
    let ids = match restore::take_at_login(path, current.as_ref()) {
        Ok(Taken::Start(ids)) => ids,
        Ok(Taken::Nothing) => return,
        Ok(Taken::Disabled) => {
            return eprintln!("cupertino-login: applications not reopened: the choice is off")
        }
        Ok(Taken::SameSession) => {
            return eprintln!(
                "cupertino-login: applications not reopened: the list was saved in this session"
            )
        }
        Err(e) => {
            return eprintln!(
            "cupertino-login: applications not reopened: the saved list could not be emptied: {e}"
        )
        }
    };
    let report = restore::launch(
        &ids,
        &Applications::from_env(),
        login_items,
        |_, command| {
            let mut command = command.to_command();
            command.stdin(Stdio::null());
            cupertino_apps::launch::spawn("cupertino-login", command)
        },
    );
    for id in &report.started {
        eprintln!("cupertino-login: application reopened: {id}");
    }
    for (id, skip) in &report.skipped {
        eprintln!("cupertino-login: application not reopened: {id}: {skip:?}");
    }
}

/// Run one login item detached from the application; its exit is reaped on a thread. False when it did not start.
fn start_login_item(item: &autostart::Item) -> bool {
    let mut command = Process::new(&item.argv[0]);
    command.args(&item.argv[1..]).stdin(Stdio::null());
    if let Some(dir) = &item.dir {
        command.current_dir(dir);
    }
    match command.spawn() {
        Ok(mut child) => {
            eprintln!("cupertino-login: login item started: {}", item.id);
            let id = item.id.clone();
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("cupertino-login: login item {id}: {e}");
                }
            });
            true
        }
        Err(e) => {
            eprintln!("cupertino-login: login item {} did not start: {e}", item.id);
            false
        }
    }
}

/// Start the thread that keeps `Capabilities` current ([`probe`]).
fn spawn_capability_probe(
    requests: mpsc::Receiver<()>,
    logind: &Logind,
    manager: &LoginManager,
    control: &control::Service,
) -> Result<()> {
    let (logind, manager, control) = (logind.clone(), manager.clone(), control.clone());
    std::thread::Builder::new()
        .name("login-capabilities".into())
        .spawn(move || {
            let answer = |action: Power| match logind.can(action) {
                Ok(answer) => answer,
                Err(e) => {
                    eprintln!("cupertino-login: {e}");
                    Answer::NotAvailable
                }
            };
            probe::run(
                requests,
                || Inputs {
                    can_suspend: answer(Power::Sleep),
                    can_reboot: answer(Power::Restart),
                    can_power_off: answer(Power::ShutDown),
                    session_manager: manager.is_present(),
                },
                |capabilities| {
                    if let Err(e) = control.set_capabilities(capabilities) {
                        eprintln!("cupertino-login: publishing the capabilities: {e}");
                    }
                },
            )
        })?;
    Ok(())
}

impl Application for SessionApp {
    type Model = SessionApp;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (SessionApp, Vec<Command<Message>>) {
        unreachable!(
            "the session process starts only through `run`, which hands the model to the runtime"
        )
    }

    fn update(app: &mut SessionApp, message: Message) -> Vec<Command<Message>> {
        let now = app.now();
        let mut commands = Vec::new();
        let result = match message {
            Message::Bus(bus) => {
                commands.push(listen(
                    &app.bus,
                    Message::Bus,
                    Message::Unheard("the bus threads"),
                ));
                app.bus_message(now, bus)
            }
            Message::Heard(heard) => {
                commands.push(listen(
                    &app.heard,
                    Message::Heard,
                    Message::Unheard("the surface roles"),
                ));
                app.heard(now, heard)
            }
            Message::Input(input) => app.input(now, input),
            Message::Timer(id) => {
                app.timer.fired(id);
                Ok(())
            }
            Message::LockClosed => Err("the server closed a surface of the lock screen".into()),
            Message::Unheard(what) => Err(format!("{what} stopped").into()),
        };
        match result.and_then(|()| app.housekeeping(now)) {
            Ok(()) => {
                let wait = app.next_wakeup(now);
                commands.extend(app.timer.schedule(now, wait, Message::Timer));
                commands
            }
            Err(e) => {
                *app.failure.borrow_mut() = Some(e.to_string());
                vec![Command::Exit]
            }
        }
    }

    fn view(app: &SessionApp) -> Vec<Window<Message>> {
        let now = app.now();
        let mut windows = Vec::new();
        if let Some(lock) = app
            .lock
            .as_ref()
            .filter(|l| l.screen.phase() != Phase::Unlocked)
        {
            let screen = &lock.screen;
            let help = app
                .failed
                .help_shown(&app.user.login, app.retries_until_hint);
            for &(output, size) in &app.outputs {
                let content = view::screen(
                    &Screen {
                        size,
                        metrics: app.metrics,
                        clock: &app.clock,
                        user: Some(&app.user),
                        field: Field::Password {
                            generation: screen.field(),
                            enabled: screen.phase() != Phase::Unlocking,
                            busy: screen.checking(),
                        },
                        caps_lock: app.caps_lock,
                        spinner: screen.spinner(now),
                        denied: screen.failure_outline(),
                        notice: screen.notice(),
                        help,
                        opacity: screen.opacity(now),
                        shake: screen.shake(now),
                        hidden: screen.hidden(),
                        power: None,
                        notices: &app.lock_notices,
                    },
                    LOCKED_TITLE,
                    Message::Input,
                );
                windows.push(view::full_screen(
                    WindowKey(Key::from(format!("lock-{output}"))),
                    format!("{LOCK_APP_ID}.{output}"),
                    LOCKED_TITLE,
                    size,
                    content,
                    Some(Message::LockClosed),
                ));
            }
        }
        if app.lock.is_none() && app.panel.is_none() && !app.logging_out {
            if let Some(control) = app.desktop_control.as_ref().filter(|c| c.phase == 1) {
                windows.push(view::desktop_control(
                    WindowKey(Key::from("desktop-control")),
                    &control.output,
                    control.capabilities,
                    control.phase,
                    Message::Input,
                ));
            }
        }
        if let Some(panel) = &app.panel {
            windows.push(view::panel(
                WindowKey(Key::from("panel")),
                panel.panel.kind(),
                panel.panel.seconds_left(now),
                panel.panel.reopen(),
                &panel.frames,
                Message::Input,
            ));
        }
        if let Some(name) = app.quitting.as_ref().and_then(|q| q.interrupt.as_deref()) {
            windows.push(view::interrupt(
                WindowKey(Key::from("interrupt")),
                name,
                Message::Input,
            ));
        }
        windows
    }
}

impl SessionApp {
    /// Ask for a fresh reading of the menu's capabilities; the probe thread answers on its own.
    fn refresh_capabilities(&self) {
        // the thread is gone only while the process ends
        let _ = self.refresh.send(());
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Seconds until the application next has something to do by itself: a frame of a fade, the shake or the spinner,
    /// the screen's next change, the clock's minute, the panel's countdown, a held power key or the quit sequence.
    fn next_wakeup(&self, now: f64) -> Option<f64> {
        let screen = self.lock.as_ref().map(|lock| {
            let s = &lock.screen;
            let opacity = s.opacity(now);
            let moving = (opacity > 0.0 && opacity < 1.0)
                || matches!(s.phase(), Phase::Failed { .. } | Phase::Unlocking)
                || s.spinner(now).is_some();
            let minute = screen::to_next_minute().unwrap_or(60.0);
            let by_screen = s.next_wakeup(now).map_or(minute, |w| w.min(minute));
            if moving {
                by_screen.min(FRAME)
            } else {
                by_screen
            }
        });
        let panel = self.panel.as_ref().and_then(|p| p.panel.next_wakeup(now));
        let power_key = self.power_button.next_wakeup(now);
        let quitting = self
            .quitting
            .as_ref()
            .and_then(|q| q.sequence.next_wakeup(now));
        [screen, panel, power_key, quitting]
            .into_iter()
            .flatten()
            .reduce(f64::min)
    }

    fn bus_message(&mut self, now: f64, bus: Bus) -> Result<()> {
        match bus {
            Bus::Lock { explicit } => self.request_lock(now, explicit)?,
            Bus::Panel(kind) => self.show_panel(now, kind)?,
            Bus::PowerKey(pressed) => self.power_key(now, pressed)?,
            Bus::SwitchToLoginWindow => {
                self.switch_pending = true;
                self.request_lock(now, false)?;
            }
            Bus::Sleep => self.request_power(Power::Sleep)?,
            Bus::Unlock => match &mut self.lock {
                Some(lock) => {
                    eprintln!("cupertino-login: unlocked by a fast user switch");
                    lock.screen.unlock_by_switch(now);
                }
                None => eprintln!("cupertino-login: unlock ignored: the session is not locked"),
            },
            Bus::ManagerRestarted => {
                self.refresh_capabilities();
                match self.manager.session_ready() {
                    Ok(()) => eprintln!(
                        "cupertino-login: session announced again to the restarted session manager"
                    ),
                    Err(e) => eprintln!("cupertino-login: {e}"),
                }
            }
            Bus::Auth(event) => self.handle_auth(now, event)?,
            Bus::Failed(reason) => return Err(reason.into()),
            Bus::Battery(report) => self.battery(report),
            // a list that comes while the session is not locked is a late one: it never shows
            Bus::LockNotices(list) => {
                self.lock_notices = if self.lock.is_some() {
                    list
                } else {
                    Vec::new()
                }
            }
            Bus::ApplicationQuit(app) => {
                if let Some(quitting) = &mut self.quitting {
                    eprintln!("cupertino-login: application quit: {app}");
                    let step = quitting.sequence.quit(now, &app);
                    self.follow(now, step)?;
                }
            }
        }
        Ok(())
    }

    fn heard(&mut self, now: f64, heard: Heard) -> Result<()> {
        match heard {
            Heard::Outputs(outputs) => self.outputs = outputs,
            Heard::CapsLock(on) => self.caps_lock = on,
            // an answer to a lock replaced since belongs to no one
            Heard::Lock { generation, locked } => {
                if self.wish.borrow().generation == Some(generation) {
                    if let Some(lock) = &mut self.lock {
                        lock.answer = Some(locked);
                    }
                }
            }
            Heard::Idle(idle) => self.idled = idle,
            // this process is the one that holds the lock
            Heard::LockRequested { explicit } => self.request_lock(now, explicit)?,
            Heard::LogOutRequested => self.show_panel(now, Kind::LogOut)?,
            Heard::ReduceMotion(on) => {
                self.reduce_motion = on;
                if let Some(lock) = &mut self.lock {
                    lock.screen.set_reduce_motion(on);
                }
            }
            Heard::DesktopControl(event) => {
                use cupertino_shell_client::ControlEvent;
                match event {
                    ControlEvent::ConsentRequested {
                        epoch,
                        capabilities,
                        output,
                        ..
                    } => {
                        if self.lock.is_some() || self.panel.is_some() || self.logging_out {
                            self.control_wish.borrow_mut().action = Some((epoch, false, false));
                        } else {
                            self.control_wish.borrow_mut().phase = 1;
                            self.control_wish.borrow_mut().output = output.clone();
                            self.desktop_control = Some(DesktopControlPanel {
                                epoch,
                                capabilities,
                                output,
                                phase: 1,
                            });
                        }
                    }
                    ControlEvent::ConsentChanged { epoch, phase }
                        if self
                            .desktop_control
                            .as_ref()
                            .is_some_and(|c| c.epoch == epoch) =>
                    {
                        self.control_wish.borrow_mut().phase = phase;
                        if matches!(phase, 0 | 4) {
                            self.desktop_control = None;
                        } else if let Some(c) = &mut self.desktop_control {
                            c.phase = phase;
                        }
                    }
                    _ => {}
                }
            }
            Heard::NoLock => return Err("the server lacks ext_session_lock_manager_v1".into()),
        }
        Ok(())
    }

    /// What the person did on the lock screen or a panel.
    fn input(&mut self, now: f64, input: Input) -> Result<()> {
        match input {
            Input::DesktopControl { allow, stop } => {
                if let Some(control) = &self.desktop_control {
                    self.control_wish.borrow_mut().action = Some((control.epoch, allow, stop));
                }
                return Ok(());
            }
            Input::Decide(decision) if self.panel.is_some() => return self.decide(decision),
            Input::Reopen(on) => {
                // the panel's checkbox; the power-button panel has none and keeps the saved choice
                if let Some(panel) = &mut self.panel {
                    if panel.panel.reopen() != on {
                        panel.panel.toggle_reopen();
                    }
                }
                return Ok(());
            }
            Input::Interrupt(choice) if self.interrupt_shown() => return self.choose(now, choice),
            _ => {}
        }
        let Some(lock) = &mut self.lock else {
            return Ok(());
        };
        match input {
            Input::Activity => lock.screen.activity(now),
            Input::Edited => lock.screen.edit(now),
            Input::Escape => lock.screen.escape(now),
            Input::Submit(typed) => {
                if let Some(password) = lock.screen.submit(now, typed.secret()) {
                    self.authenticator.attempt(password);
                }
            }
            // the greeter's and the panels' inputs
            _ => {}
        }
        Ok(())
    }

    /// Carry out what the timers ask for and follow the lock.
    fn housekeeping(&mut self, now: f64) -> Result<()> {
        let step = self.quitting.as_mut().and_then(|q| q.sequence.tick(now));
        self.follow(now, step)?;
        if std::mem::take(&mut self.idled) {
            self.request_lock(now, false)?;
        }
        if let Some(effect) = self.power_button.tick(now) {
            self.power_effect(now, effect)?;
        }
        if let Some(decision) = self.panel.as_ref().and_then(|p| p.panel.tick(now)) {
            self.decide(decision)?;
        }
        let Some(lock) = &mut self.lock else {
            return Ok(());
        };
        self.clock = screen::clock()?;
        match lock.answer {
            Some(false) => return Err("the server refused or ended the session lock".into()),
            Some(true) if !lock.confirmed => {
                lock.confirmed = true;
                // the lock is up: sleep may go on
                self.inhibitor = None;
                eprintln!("cupertino-login: locked");
            }
            _ => {}
        }
        // the screen is locked before the console leaves it: what the next person sees of this session is its lock screen
        if self.switch_pending && lock.confirmed {
            self.switch_pending = false;
            match self.manager.switch_to_login_window() {
                Ok(()) => eprintln!("cupertino-login: switching to the login window"),
                Err(e) => eprintln!("cupertino-login: {e}"),
            }
        }
        lock.screen.tick(now);
        if lock.screen.phase() == Phase::Unlocked {
            return self.unlock();
        }
        Ok(())
    }

    /// Post, replace or withdraw the low-battery notification. The notification service may be missing or fail: that is
    /// reported and the warning is tried again at the next change.
    fn battery(&mut self, report: std::result::Result<upower::Report, String>) {
        let battery = match report {
            Ok(upower::Report::Battery(battery)) => battery,
            Ok(upower::Report::NoDaemon(reason)) => {
                eprintln!("cupertino-login: no battery warnings: {reason}");
                return;
            }
            Err(reason) => {
                eprintln!("cupertino-login: battery warnings stopped: {reason}");
                return;
            }
        };
        match self.low_battery.report(battery.level, battery.percentage) {
            Some(low_battery::Action::Post { replaces, notice }) => {
                match upower::post(&self.session_bus, replaces, &notice) {
                    Ok(id) => {
                        self.low_battery.posted(id);
                        eprintln!(
                            "cupertino-login: low battery: {:?} at {:.0} %, notification {id}",
                            battery.level, battery.percentage
                        );
                    }
                    Err(e) => eprintln!("cupertino-login: {e}"),
                }
            }
            Some(low_battery::Action::Close(id)) => match upower::close(&self.session_bus, id) {
                Ok(()) => {
                    eprintln!("cupertino-login: low battery cleared, notification {id} withdrawn")
                }
                Err(e) => eprintln!("cupertino-login: {e}"),
            },
            None => {}
        }
    }

    /// Lock the session: the roles take the lock and every output gets its lock screen window. `explicit`: the person
    /// asked, so the lock screen hides after 5 s without input instead of 30 s. Does nothing when the session is locked
    /// already; a panel that is open is cancelled, since the lock screen takes the keyboard.
    fn request_lock(&mut self, now: f64, explicit: bool) -> Result<()> {
        if self.lock.is_some() {
            return Ok(());
        }
        if self.panel.is_some() {
            self.decide(Decision::Cancel)?;
        }
        if self.interrupt_shown() {
            self.choose(now, Choice::Cancel)?;
        }
        // the lock screen swallows a key that is down
        self.power_button.reset();
        let mut screen = LockScreen::new(
            now,
            if explicit {
                HIDE_AFTER_EXPLICIT_LOCK
            } else {
                HIDE_AFTER
            },
        );
        screen.set_reduce_motion(self.reduce_motion);
        if cupertino_shell_client::test_clock::motion_settled() {
            screen.settle_interface();
        }
        self.lock = Some(Lock {
            screen,
            answer: None,
            confirmed: false,
        });
        self.wish.borrow_mut().wanted = true;
        self.clock = screen::clock()?;
        self.control.set_locked(true)?;
        Ok(())
    }

    fn handle_auth(&mut self, now: f64, event: auth::Event) -> Result<()> {
        let lock = self
            .lock
            .as_mut()
            .ok_or("an authentication result arrived while the session is not locked")?;
        match event {
            auth::Event::Notice(text) => lock.screen.notify(text),
            auth::Event::Outcome(outcome) => {
                match &outcome {
                    AuthOutcome::Denied => {
                        self.failed.record(&self.user.login);
                        eprintln!("cupertino-login: authentication failed");
                    }
                    AuthOutcome::Error(reason) => {
                        eprintln!("cupertino-login: authentication error: {reason}")
                    }
                    AuthOutcome::Success => {}
                }
                lock.screen.finish_auth(now, outcome)?;
            }
        }
        Ok(())
    }

    /// End the lock after an accepted attempt and hold sleep back again. The lock windows go with the next view; the
    /// roles then unlock.
    fn unlock(&mut self) -> Result<()> {
        let lock = self.lock.take().expect("only a locked session unlocks");
        // nothing of this lock's list stays for the next one
        self.lock_notices.clear();
        if lock.answer != Some(true) {
            return Err("the session unlocked before the server confirmed the lock".into());
        }
        self.wish.borrow_mut().wanted = false;
        self.control.set_locked(false)?;
        eprintln!("cupertino-login: unlocked");
        self.inhibitor = Some(self.logind.inhibit_sleep()?);
        Ok(())
    }

    /// Show the confirmation panel for `kind` on the first output, in place of one that is open. The control interface
    /// refuses the request on a locked session; one that arrives between the lock and the property change is ignored.
    fn show_panel(&mut self, now: f64, kind: Kind) -> Result<()> {
        if self.lock.is_some() {
            eprintln!("cupertino-login: panel request ignored: the session is locked");
            return Ok(());
        }
        if self.panel.is_some() {
            self.decide(Decision::Cancel)?;
        }
        if self.outputs.is_empty() {
            return Err("the server has no output".into());
        }
        let reopen = self
            .restore_path
            .as_deref()
            .map_or(restore::REOPEN_DEFAULT, restore::choice);
        self.panel = Some(PanelState {
            panel: Panel::new(kind, now, reopen),
            frames: view::panel_frames(kind, &self.text),
        });
        eprintln!("cupertino-login: panel shown: {}", kind.log_name());
        Ok(())
    }

    /// Close the panel with `decision`; an action is carried out through logind. Confirming a log out, restart or shut
    /// down remembers the panel's "Reopen windows" choice; cancelling forgets a change to it.
    fn decide(&mut self, decision: Decision) -> Result<()> {
        let PanelState { panel, .. } = self
            .panel
            .take()
            .expect("a decision is only made on an open panel");
        match decision {
            Decision::Cancel => eprintln!("cupertino-login: panel cancelled"),
            Decision::Do(action) => {
                eprintln!("cupertino-login: panel confirmed: {}", action.log_name());
                if action != Action::Sleep && panel.kind().has_checkbox() {
                    self.remember_choice(panel.reopen());
                }
                self.carry_out(self.now(), action, panel.reopen())?;
            }
        }
        Ok(())
    }

    /// Store the "Reopen windows" choice, with no list yet: the list follows once the applications have quit.
    fn remember_choice(&self, reopen: bool) {
        let Some(path) = &self.restore_path else {
            return;
        };
        match restore::save(path, reopen, Vec::new(), None) {
            Ok(()) => eprintln!("cupertino-login: reopen windows at the next login: {reopen}"),
            Err(e) => eprintln!("cupertino-login: the reopen choice is not saved: {e}"),
        }
    }

    /// Save the applications that were open, for the next login, just before the session ends ([`restore`]).
    fn save_open_applications(&self, reopen: bool, quit: &[String]) {
        let Some(path) = &self.restore_path else {
            return eprintln!(
                "cupertino-login: applications not saved: neither XDG_STATE_HOME nor HOME is set"
            );
        };
        let (apps, skipped) = if reopen {
            restore::saved_list(quit.iter().map(String::as_str), &Applications::from_env())
        } else {
            (Vec::new(), Vec::new())
        };
        for (identity, skip) in &skipped {
            eprintln!(
                "cupertino-login: application not saved for the next login: {identity}: {skip:?}"
            );
        }
        let mark = SessionMark::current(self.logind.session_id());
        if reopen && mark.is_none() {
            eprintln!(
                "cupertino-login: applications not saved: this boot and session cannot be named"
            );
        }
        match restore::save(path, reopen, apps.clone(), mark) {
            Ok(()) if reopen => eprintln!(
                "cupertino-login: saved for the next login: {}",
                if apps.is_empty() {
                    "no application".to_string()
                } else {
                    apps.join(" ")
                }
            ),
            Ok(()) => {}
            Err(e) => eprintln!("cupertino-login: applications not saved for the next login: {e}"),
        }
    }

    /// The session goes on after all (a refused restart or shut down): a list saved for the next login is dropped, so it
    /// is not started after some later end of the session.
    fn discard_saved_applications(&self) {
        if let Some(path) = &self.restore_path {
            if let Err(e) = restore::discard(path) {
                eprintln!("cupertino-login: the saved applications are not dropped: {e}");
            }
        }
    }

    /// Ask the session manager to restart, shut down or sleep. A refusal (another user's session is open, an inhibitor) is
    /// reported and the session goes on; any other failure ends the process, which the server restarts.
    fn request_power(&mut self, action: Power) -> Result<()> {
        match self.manager.request_power(action) {
            Ok(()) => {}
            Err(e) if e.refusal().is_some() => {
                eprintln!("cupertino-login: {action:?} refused: {e}");
                self.logging_out = false;
                self.discard_saved_applications();
            }
            // a session another display manager started: logind does it
            Err(e) if e.manager_absent() => self.power_through_logind(action)?,
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// Restart, shut down or sleep through logind, for a session that has no session manager. A refusal (another user's
    /// session is open, a block inhibitor, the person may not) is reported and the session goes on.
    fn power_through_logind(&mut self, action: Power) -> Result<()> {
        eprintln!("cupertino-login: no session manager: asking logind to {action:?}");
        match self.logind.power(action) {
            Ok(()) => {}
            Err(e) if e.is_refusal() => {
                eprintln!("cupertino-login: {action:?} refused: {e}");
                self.logging_out = false;
                self.discard_saved_applications();
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// Sleep at once; log out, restart and shut down after the applications have quit, with the "Reopen windows" choice
    /// `reopen`.
    fn carry_out(&mut self, now: f64, action: Action, reopen: bool) -> Result<()> {
        if action == Action::Sleep {
            return self.request_power(Power::Sleep);
        }
        self.logging_out = true;
        let (sequence, step) = LogOut::start();
        self.quitting = Some(Quitting {
            sequence,
            then: action,
            interrupt: None,
            reopen,
            quit: Vec::new(),
        });
        self.follow(now, Some(step))
    }

    /// Do what the quit sequence asks for, and what that leads to.
    fn follow(&mut self, now: f64, mut step: Option<logout::Do>) -> Result<()> {
        while let Some(next) = step.take() {
            let quitting = self
                .quitting
                .as_mut()
                .expect("the quit sequence only asks while it runs");
            match next {
                logout::Do::AskNext => match session::quit_next_application(&self.session_bus) {
                    Ok((app, windows)) => {
                        if !app.is_empty() {
                            eprintln!(
                                "cupertino-login: quitting application: {app} ({windows} windows)"
                            );
                            quitting.quit.push(app.clone());
                        }
                        step = quitting.sequence.answered(now, &app, windows);
                    }
                    Err(e) => {
                        eprintln!(
                            "cupertino-login: applications not quit before the {}: {e}",
                            quitting.then.log_name()
                        );
                        step = Some(logout::Do::Terminate);
                    }
                },
                logout::Do::Terminate => {
                    let Quitting {
                        then, reopen, quit, ..
                    } = self.quitting.take().expect("checked above");
                    eprintln!("cupertino-login: no application left: {}", then.log_name());
                    self.save_open_applications(reopen, &quit);
                    match then {
                        Action::LogOut => self.logind.terminate_session()?,
                        Action::Restart => self.request_power(Power::Restart)?,
                        Action::ShutDown => self.request_power(Power::ShutDown)?,
                        Action::Sleep => unreachable!("sleep does not quit the applications"),
                    }
                }
                logout::Do::Interrupt(app) => self.show_interrupt(now, app)?,
                logout::Do::CloseInterrupt => quitting.interrupt = None,
            }
        }
        Ok(())
    }

    fn interrupt_shown(&self) -> bool {
        self.quitting
            .as_ref()
            .is_some_and(|q| q.interrupt.is_some())
    }

    /// Show the interrupt panel for `app` on the first output. On a locked session there is nobody to answer it: the
    /// sequence is cancelled.
    fn show_interrupt(&mut self, now: f64, app: String) -> Result<()> {
        eprintln!("cupertino-login: application did not quit: {app}");
        if self.lock.is_some() {
            return self.choose(now, Choice::Cancel);
        }
        if self.outputs.is_empty() {
            return Err("the server has no output".into());
        }
        self.quitting
            .as_mut()
            .expect("the interrupt panel belongs to a quit sequence")
            .interrupt = Some(app);
        Ok(())
    }

    /// The answer on the interrupt panel: Cancel ends the sequence and the session goes on; Force Quit kills the
    /// application, which then has another full timeout to go away.
    fn choose(&mut self, now: f64, choice: Choice) -> Result<()> {
        let quitting = self
            .quitting
            .as_mut()
            .expect("a choice is only made while the applications are quit");
        quitting.interrupt = None;
        match choice {
            Choice::Cancel => {
                let quitting = self.quitting.take().expect("checked above");
                self.logging_out = false;
                eprintln!(
                    "cupertino-login: {} cancelled while quitting the applications",
                    quitting.then.log_name()
                );
            }
            Choice::ForceQuit => {
                let app = quitting
                    .sequence
                    .force_quit(now)
                    .expect("Force Quit is only offered on the interrupt panel");
                // a failure leaves the sequence waiting: the panel comes back when the timeout runs out again
                match session::force_quit_application(&self.session_bus, &app) {
                    Ok(processes) => {
                        eprintln!("cupertino-login: force quit: {app} ({processes} processes)")
                    }
                    Err(e) => eprintln!("cupertino-login: force quit of {app} failed: {e}"),
                }
            }
        }
        Ok(())
    }

    /// The power key went down or up (`PowerKey` on the control interface). The lock screen swallows both.
    fn power_key(&mut self, now: f64, pressed: bool) -> Result<()> {
        if self.lock.is_some() {
            self.power_button.reset();
            return Ok(());
        }
        eprintln!(
            "cupertino-login: power key {}",
            if pressed { "down" } else { "up" }
        );
        if pressed {
            self.power_button.press(now);
            return Ok(());
        }
        match self.power_button.release(now) {
            Some(effect) => self.power_effect(now, effect),
            None => Ok(()),
        }
    }

    /// Carry out what the hold timing asks for. The panel does not show over another panel or a running log-out.
    fn power_effect(&mut self, now: f64, effect: Effect) -> Result<()> {
        match effect {
            Effect::Sleep => {
                eprintln!("cupertino-login: power button: sleep");
                self.request_power(Power::Sleep)?;
            }
            Effect::ShowPanel if self.panel.is_some() || self.logging_out => eprintln!(
                "cupertino-login: power button hold ignored: a panel or a log-out is running"
            ),
            Effect::ShowPanel => self.show_panel(now, Kind::PowerButton)?,
        }
        Ok(())
    }
}
