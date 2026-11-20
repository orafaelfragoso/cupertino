//! The session process: the lock screen and the session agent, a CupertinoKit application. It locks the session on a
//! request, shows one lock screen window per output ([`view::screen`]), sends the attempts to PAM and unlocks only when
//! the state machine has reached [`Phase::Unlocked`], which only an accepted attempt (or the session manager's proof
//! of a fast user switch) does. The agent shows the confirmation panel for log out, restart and shut down and asks the
//! server's `org.cupertino.Session1` to carry out what was confirmed; the server quits the applications first, and the
//! agent shows the interrupt panel when one does not quit.
//!
//! The lock itself lives in [`LoginRoles`]: they take the session lock before the first lock surface and unlock once
//! this application stopped wanting the lock and the lock windows are gone. Bus threads, PAM and the roles talk to the
//! application through channels that [`listen`] turns into messages.

use crate::options::SessionOptions;
use crate::roles::{self, Heard, LockWish, LoginRoles};
use crate::screen::{self, Result, Timer, FRAME};
use cupertino_defaults::login::ScreenSettings;
use cupertino_keyboard::modifiers::Profile;
use cupertino_kit::core::{Application, Command, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_login::auth::{self, Authenticator};
use cupertino_login::control::{self, Request};
use cupertino_login::hint::FailedAttempts;
use cupertino_login::lock_notices::{self, Notice};
use cupertino_login::login_manager::LoginManager;
use cupertino_login::logind::{Inhibitor, Logind, Signal};
use cupertino_login::model::{
    AuthOutcome, LockScreen, Phase, HIDE_AFTER, HIDE_AFTER_EXPLICIT_LOCK,
};
use cupertino_login::status::{self, StatusItems};
use cupertino_login::user::User;
use cupertino_login::view::{
    self, Access, Clock, ControlChoice, ControlRequest, Field, Input, Metrics, RequestingApp,
    Screen, Status, LOCKED_TITLE, LOCK_APP_ID,
};
use cupertino_login::wallpaper::{self, Source};
use cupertino_shell_client::{
    IdleNotifier, IdleWatch, LayerShell, Session as ShellSession, SessionLockManager, Shell,
};
use cupertino_ui_wayland::{SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::Event;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// What the bus and authentication threads tell the application.
#[derive(Clone, Debug)]
pub enum Bus {
    /// `Lock` on the control interface, logind's `Lock` signal or the preparation for sleep.
    Lock { explicit: bool },
    /// The session manager's `Unlock`: the person at the login screen proved this session's password.
    Unlock,
    /// The session manager's bus name has an owner again: it restarted and does not know this session's agent.
    ManagerRestarted,
    /// A message or the outcome of an attempt.
    Auth(auth::Event),
    /// A reader thread broke; the process exits so that the server restarts it.
    Failed(String),
    /// What the notification daemon lets the lock screen show now ([`lock_notices`]).
    LockNotices(Vec<Notice>),
    /// The wallpaper of the lock of this number is ready.
    Wallpaper { lock: u64, source: Source },
    /// A reading of the status items.
    Status(StatusItems),
}

/// What the lock screen shows until its wallpaper is ready, and the longest it waits for it before it shows its
/// interface over that (approximate): a picture still being read comes in under the interface when it is ready.
static WAITING: Source = Source::Solid([0, 0, 0]);
const WALLPAPER_WAIT: f64 = 0.5;

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
    /// This lock's number: the wallpaper read for an earlier one is not this one's.
    number: u64,
    /// When the lock was asked for, and when its screen showed (its wallpaper ready, or [`WALLPAPER_WAIT`] over).
    asked: f64,
    shown: Option<f64>,
    /// The picture the screens stand on, once read.
    wallpaper: Option<Source>,
    /// The password help is open.
    help_open: bool,
    /// The server's answer to the lock: confirmed (`true`) or refused or ended.
    answer: Option<bool>,
    /// The server confirmed the lock.
    confirmed: bool,
}

/// The controller consent panel.
struct DesktopControlPanel {
    epoch: u64,
    request: ControlRequest,
    phase: u32,
}

/// Device pixels of the app icon renditions the consent panel loads: its 64 pt icon at scales 1 and 2.
const CONTROL_ICON_PX: [u32; 2] = [64, 128];

pub struct SessionApp {
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
    /// The display options the field and its buttons follow.
    access: Access,
    /// What `login.conf` says the screens show.
    settings: ScreenSettings,
    /// The last reading of the status items.
    status: StatusItems,
    /// The clock's hour cycle: the session's.
    hours: cupertino_defaults::menubar::HourCycle,
    /// The session's appearance is dark: the lock screen stands on its dark wallpaper.
    dark: bool,
    /// The number of the last lock taken.
    locks: u64,
    /// For the threads that read a wallpaper.
    bus_sender: async_channel::Sender<Bus>,
    /// The seat went idle and input has not resumed since.
    idled: bool,
    outputs: Vec<(u32, Size)>,
    caps_lock: bool,
    metrics: Metrics,
    clock: Clock,
    timer: Timer,
    user: User,
    authenticator: Authenticator,
    logind: Logind,
    /// The session manager: power actions and inhibitor locks.
    manager: LoginManager,
    /// Keeps `org.cupertino.Login1` on the bus.
    _control: control::Service,
    /// Held while the session is unlocked, so the system cannot sleep before the lock is up.
    inhibitor: Option<Inhibitor>,
    start: Instant,
    lock: Option<Lock>,
    desktop_control: Option<DesktopControlPanel>,
    /// The app the next consent request of this epoch names (authority version 2).
    control_app: Option<(u64, cupertino_shell_client::ConsentApp)>,
    control_wish: Rc<RefCell<roles::ControlWish>>,
    /// Wrong passwords typed at the lock screens of this run.
    failed: FailedAttempts,
    /// `RetriesUntilHint`, see [`FailedAttempts::help_shown`].
    retries_until_hint: u32,
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
    // this thread is the session's UI thread from here: the applications reopened before its loop starts are its to reap
    let _instance = cupertino_kit::Instance::new("login_ui").enter();
    let user = User::current()?;
    let mut system = WaylandWindowSystem::connect_as(Profile::cupertino(), SessionApp::RENDERER)
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
    let on_request = Arc::new(move |Request::Lock| request(Bus::Lock { explicit: false }));
    let served = on_request.clone();
    let control = match control::serve(zbus::connection::Builder::session()?, move |request| {
        served(request)
    }) {
        Ok(control) => control,
        // Another process took the name first. Quitting would leave the session with no lock screen at all; instead this
        // process runs on the server's private connection, which carries the lock request, and on logind's signals.
        Err(zbus::Error::NameTaken) => {
            eprintln!(
                "cupertino-login: error: another process owns org.cupertino.Login1; a lock asked for over the session bus \
                 does not reach this process, the lock request of the server's connection still does"
            );
            control::serve_unnamed(zbus::connection::Builder::session()?, move |request| {
                on_request(request)
            })?
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
    // the status items: the Wi-Fi radio, the battery and, when `login.conf` asks for it, the input source
    let status_items = bus.clone();
    status::poll(None, ScreenSettings::load().show_input_menu, move |items| {
        status_items.send_blocking(Bus::Status(items)).is_ok()
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
    let notices = send(&bus);
    // the lock screen works without the notifications: a watch that cannot start is reported, not fatal
    if let Err(e) = lock_notices::watch(&session_bus, move |list| notices(Bus::LockNotices(list))) {
        eprintln!("cupertino-login: the lock screen shows no notifications: {e}");
    }
    let bus_sender = bus.clone();
    drop(bus);
    // the login screen shows this session's wallpaper after it
    cupertino_login::login_wallpaper::keep_current();
    // a session the manager did not start (one on a getty, for the tests) is not known to it; that is reported, not fatal
    match manager.session_ready() {
        Ok(()) => eprintln!("cupertino-login: session announced to the session manager"),
        Err(e) => eprintln!("cupertino-login: {e}"),
    }

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
    let retries_until_hint = options.retries_until_hint;
    cupertino_kit::app::run_lock_screen_on_with::<SessionApp>(system, move |runtime| {
        let app = SessionApp {
            bus: bus_messages,
            heard: heard_messages,
            wish,
            failure: ended,
            _session: session,
            _idle: idle,
            reduce_motion: session_state.display.reduce_motion,
            access: Access {
                increase_contrast: session_state.display.increase_contrast,
                reduce_transparency: session_state.display.reduce_transparency,
            },
            settings: ScreenSettings::load(),
            status: StatusItems::default(),
            hours: screen::session_hour_cycle(),
            dark: session_state.dark,
            locks: 0,
            bus_sender,
            idled: false,
            outputs,
            caps_lock: false,
            metrics: Metrics::of(&runtime.text),
            clock: Clock::default(),
            timer: Timer::default(),
            user,
            authenticator,
            logind,
            manager,
            _control: control,
            inhibitor,
            start: Instant::now(),
            lock: None,
            desktop_control: None,
            control_app: None,
            control_wish,
            failed: FailedAttempts::default(),
            retries_until_hint,
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

impl Application for SessionApp {
    type Model = SessionApp;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The lock screen must always draw, whatever becomes of the GPU, and it is mostly at rest.
    const RENDERER: cupertino_kit::core::RendererClass = cupertino_kit::core::RendererClass::Cpu;

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
            let dim = lock.dim(now);
            // nothing of the interface shows before its wallpaper does
            let shown = lock.shown.is_some();
            let wallpaper = lock.wallpaper.as_ref().unwrap_or(&WAITING);
            for &(output, size) in &app.outputs {
                let content = view::screen(
                    &Screen {
                        size,
                        metrics: app.metrics,
                        clock: &app.clock,
                        user: Some(&app.user),
                        field: Field::Password {
                            generation: screen.field(),
                            // nothing is typed while an attempt is checked
                            enabled: screen.phase() != Phase::Unlocking && !screen.checking(),
                            busy: screen.checking(),
                        },
                        name: None,
                        typed: screen.typed(),
                        caps_lock: app.caps_lock,
                        spinner: screen.spinner(now),
                        denied: screen.failure_outline(),
                        denied_said: screen.denied_said(),
                        notice: screen.notice(),
                        help,
                        help_open: help && lock.help_open,
                        opacity: if shown { screen.opacity(now) } else { 0.0 },
                        block: 1.0,
                        shake: screen.shake(now),
                        hidden: screen.hidden(),
                        lock_message: app.settings.lock_message.as_deref(),
                        back: false,
                        locked: false,
                        hide_user: app.settings.hide_user_avatar_and_name,
                        access: app.access,
                        large_clock: app.settings.large_clock,
                        avatar_from: None,
                        power: None,
                        status: Status {
                            items: &app.status,
                            access: None,
                        },
                        notices: &app.lock_notices,
                        wallpaper,
                        dim,
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
        if app.lock.is_none() {
            if let Some(control) = app.desktop_control.as_ref().filter(|c| c.phase == 1) {
                windows.push(view::desktop_control(
                    WindowKey(Key::from("desktop-control")),
                    &control.request,
                    Message::Input,
                ));
            }
        }
        windows
    }
}

impl Lock {
    /// How much of the dimming shows at `now`: none before the screen shows, all of it [`wallpaper::DIM_FADE`] after.
    fn dim(&self, now: f64) -> f64 {
        self.shown.map_or(0.0, |at| wallpaper::dimming_at(now - at))
    }
}

impl SessionApp {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Seconds until the application next has something to do by itself: a frame of a fade, the shake or the spinner,
    /// the screen's next change or the clock's minute.
    fn next_wakeup(&self, now: f64) -> Option<f64> {
        let screen = self.lock.as_ref().map(|lock| {
            let s = &lock.screen;
            let opacity = s.opacity(now);
            let dim = lock.dim(now);
            let moving = (opacity > 0.0 && opacity < 1.0)
                || (lock.shown.is_some() && dim < 1.0)
                || matches!(s.phase(), Phase::Failed { .. } | Phase::Unlocking)
                || s.spinner(now).is_some();
            let minute = screen::to_next_minute().unwrap_or(60.0);
            let mut by_screen = s.next_wakeup(now).map_or(minute, |w| w.min(minute));
            if lock.shown.is_none() {
                by_screen = by_screen.min((lock.asked + WALLPAPER_WAIT - now).max(0.0));
            }
            if moving {
                by_screen.min(FRAME)
            } else {
                by_screen
            }
        });
        screen
    }

    fn bus_message(&mut self, now: f64, bus: Bus) -> Result<()> {
        match bus {
            Bus::Lock { explicit } => self.request_lock(now, explicit)?,
            Bus::Unlock => match &mut self.lock {
                Some(lock) => {
                    eprintln!("cupertino-login: unlocked by a fast user switch");
                    lock.screen.unlock_by_switch(now);
                }
                None => eprintln!("cupertino-login: unlock ignored: the session is not locked"),
            },
            Bus::ManagerRestarted => match self.manager.session_ready() {
                Ok(()) => eprintln!(
                    "cupertino-login: session announced again to the restarted session manager"
                ),
                Err(e) => eprintln!("cupertino-login: {e}"),
            },
            Bus::Auth(event) => self.handle_auth(now, event)?,
            Bus::Failed(reason) => return Err(reason.into()),
            Bus::Status(items) => self.status = items,
            // a list that comes while the session is not locked is a late one: it never shows
            Bus::LockNotices(list) => {
                self.lock_notices = if self.lock.is_some() {
                    list
                } else {
                    Vec::new()
                }
            }
            Bus::Wallpaper { lock, source } => {
                if let Some(held) = self.lock.as_mut().filter(|l| l.number == lock) {
                    held.wallpaper = Some(source);
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
            Heard::Idle(idle) => {
                self.idled = idle;
                // the display sleeps or wakes: what was typed does not wait for whoever comes
                if let Some(lock) = &mut self.lock {
                    lock.screen.clear_field();
                }
            }
            // this process is the one that holds the lock
            Heard::LockRequested { explicit } => self.request_lock(now, explicit)?,
            Heard::Display {
                reduce_motion,
                increase_contrast,
                reduce_transparency,
            } => {
                self.reduce_motion = reduce_motion;
                self.access = Access {
                    increase_contrast,
                    reduce_transparency,
                };
                if let Some(lock) = &mut self.lock {
                    lock.screen.set_reduce_motion(reduce_motion);
                }
            }
            Heard::Appearance(dark) => {
                let changed = self.dark != dark;
                self.dark = dark;
                // the lock screen takes the wallpaper of the new appearance; the old one shows until it is read
                if let (true, Some(lock)) = (changed, &self.lock) {
                    self.read_wallpaper(lock.number);
                }
            }
            Heard::DesktopControl(event) => {
                use cupertino_shell_client::ControlEvent;
                match event {
                    ControlEvent::ConsentApp { epoch, app } => {
                        self.control_app = Some((epoch, app))
                    }
                    ControlEvent::ConsentRequested {
                        epoch,
                        capabilities,
                        output,
                        ..
                    } => {
                        let app = self
                            .control_app
                            .take()
                            .filter(|(e, _)| *e == epoch)
                            .map(|(_, app)| requesting_app(app));
                        if self.lock.is_some() {
                            // nobody can answer now: this request is refused, nothing is remembered
                            self.control_wish.borrow_mut().action =
                                Some((epoch, ControlChoice::Dismiss));
                        } else {
                            self.control_wish.borrow_mut().phase = 1;
                            self.control_wish.borrow_mut().output = output.clone();
                            self.desktop_control = Some(DesktopControlPanel {
                                epoch,
                                request: ControlRequest {
                                    output,
                                    capabilities,
                                    app,
                                },
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

    /// What the person did on the lock screen or the consent panel.
    fn input(&mut self, now: f64, input: Input) -> Result<()> {
        if let Input::DesktopControl(choice) = input {
            if let Some(control) = &self.desktop_control {
                self.control_wish.borrow_mut().action = Some((control.epoch, choice));
            }
            return Ok(());
        }
        let Some(lock) = &mut self.lock else {
            return Ok(());
        };
        match input {
            Input::Activity => lock.screen.activity(now),
            Input::Edited(typed) => lock.screen.edit_to(now, typed),
            Input::Help => lock.help_open = !lock.help_open,
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
        if std::mem::take(&mut self.idled) {
            self.request_lock(now, false)?;
        }
        let Some(lock) = &mut self.lock else {
            return Ok(());
        };
        self.clock = screen::clock(self.hours)?;
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
        if lock.shown.is_none() && (lock.wallpaper.is_some() || now - lock.asked >= WALLPAPER_WAIT)
        {
            lock.shown = Some(now);
            lock.screen.show_from(now);
        }
        lock.screen.tick(now);
        if lock.screen.phase() == Phase::Unlocked {
            return self.unlock();
        }
        Ok(())
    }

    /// Read the wallpaper of the lock `number` for the session's appearance on a thread of its own: it comes back as
    /// [`Bus::Wallpaper`].
    fn read_wallpaper(&self, number: u64) {
        let (sender, dark) = (self.bus_sender.clone(), self.dark);
        let spawned = std::thread::Builder::new()
            .name("lock wallpaper".into())
            .spawn(move || {
                let settings = cupertino_defaults::compositor::CompositorSettings::path()
                    .and_then(|path| {
                        cupertino_defaults::compositor::CompositorSettings::load(&path, None)
                            .map_err(|e| eprintln!("cupertino-login: {}: {e}", path.display()))
                            .ok()
                    })
                    .map(|(settings, _)| settings)
                    .unwrap_or_default();
                let source = wallpaper::for_session(&settings, dark, |dark| {
                    cupertino_assets::Assets::discover().wallpaper_path(dark)
                });
                // the receiver lives as long as the application
                let _ = sender.send_blocking(Bus::Wallpaper {
                    lock: number,
                    source,
                });
            });
        if let Err(e) = spawned {
            eprintln!("cupertino-login: reading the wallpaper: {e}");
        }
    }

    /// Lock the session: the roles take the lock and every output gets its lock screen window. `explicit`: the person
    /// asked, so the lock screen hides after 5 s without input instead of 30 s. Does nothing when the session is locked
    /// already.
    fn request_lock(&mut self, now: f64, explicit: bool) -> Result<()> {
        if let Some(lock) = &mut self.lock {
            // the system is about to sleep, or the session is locked again: the field starts empty
            lock.screen.clear_field();
            return Ok(());
        }
        let mut screen = LockScreen::new(
            now,
            if explicit {
                HIDE_AFTER_EXPLICIT_LOCK
            } else {
                HIDE_AFTER
            },
        );
        screen.set_reduce_motion(self.reduce_motion);
        let settled = cupertino_shell_client::test_clock::motion_settled();
        if settled {
            screen.settle_interface();
        }
        self.locks += 1;
        self.lock = Some(Lock {
            screen,
            number: self.locks,
            asked: now,
            // a fixed test clock shows the screen at once, its dimming fully in
            shown: settled.then_some(f64::NEG_INFINITY),
            wallpaper: None,
            help_open: false,
            answer: None,
            confirmed: false,
        });
        self.read_wallpaper(self.locks);
        self.wish.borrow_mut().wanted = true;
        self.clock = screen::clock(self.hours)?;
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
        // nothing of this lock's list stays for the next one, nor its pictures
        self.lock_notices.clear();
        view::forget_pictures();
        if lock.answer != Some(true) {
            return Err("the session unlocked before the server confirmed the lock".into());
        }
        self.wish.borrow_mut().wanted = false;
        eprintln!("cupertino-login: unlocked");
        self.inhibitor = Some(self.logind.inhibit_sleep()?);
        Ok(())
    }
}

/// The consent panel's view of the app `app`, with its icon from its desktop entry when it has one.
fn requesting_app(app: cupertino_shell_client::ConsentApp) -> RequestingApp {
    let icon = if app.desktop {
        let mut icons = cupertino_icons::ThemeIcons::new(&cupertino_assets::Assets::discover());
        CONTROL_ICON_PX
            .iter()
            .filter_map(|&px| icons.load_for_entry(&app.identity, px))
            .map(|pixmap| cupertino_kit::controls::ImageSource {
                id: cupertino_kit::core::next_resource_id(),
                image: cupertino_kit::render::ImageResource {
                    revision: 1,
                    width: pixmap.width(),
                    height: pixmap.height(),
                    stride: pixmap.width() * 4,
                    format: cupertino_kit::render::PixelFormat::Rgba8Premultiplied,
                    pixels: pixmap.take().into(),
                },
            })
            .collect()
    } else {
        Vec::new()
    };
    RequestingApp {
        name: app.name,
        identity: app.identity,
        desktop: app.desktop,
        services: app.services,
        remembered: app.remembered,
        icon,
    }
}
