//! The session process: the lock screen and the session agent. Glue between the state machine and the toolkit: requests
//! the session lock, keeps one lock surface per output, routes keys to the state machine and its attempts to PAM, and
//! unlocks only when the state machine has reached [`Phase::Unlocked`], which only an accepted attempt does. The agent
//! shows the confirmation panel for log out, restart and shut down, has the server quit the applications one at a time
//! ([`logout`], with the interrupt panel when one does not quit) and then carries out what was confirmed through logind.

use crate::options::SessionOptions;
use crate::screen::{self, FrameKey, Notifier, Result, Scene};
use cupertino_assets::Assets;
use cupertino_client::{
    Anchor, Button, Client, Event, KeyboardInteractivity, Layer, LayerSurfaceConfig, LockState,
    Margin, OutputId, PointerEvent, SurfaceId,
};
use cupertino_login::auth::{self, Authenticator};
use cupertino_login::autostart;
use cupertino_login::capabilities::{Answer, Capabilities, Inputs};
use cupertino_login::control::{self, Request};
use cupertino_login::hint::FailedAttempts;
use cupertino_login::interrupt::{self, Choice};
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
use cupertino_login::render::{PanelView, Renderer};
use cupertino_login::user::User;
use cupertino_login::{session, upower};
use cupertino_theme::Theme;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// What the bus and authentication threads tell the loop.
enum Message {
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
}

/// The session while it is locked or being locked.
struct Lock {
    screen: LockScreen,
    /// The lock surface of each output.
    surfaces: Vec<(SurfaceId, OutputId)>,
    /// The server confirmed the lock.
    confirmed: bool,
    /// The last frame drawn.
    drawn: Option<FrameKey>,
}

/// The confirmation panel on screen.
struct PanelState {
    panel: Panel,
    /// Where its parts sit; its size is the surface's.
    frames: Frames,
    surface: SurfaceId,
    /// The countdown number the last frame showed (`Some(None)`: a panel without a countdown); `None`: not drawn yet.
    drawn: Option<Option<u32>>,
}

/// A confirmed log out, restart or shut down while the applications are quit.
struct Quitting {
    sequence: LogOut,
    /// What follows once no application is left.
    then: Action,
    interrupt: Option<InterruptState>,
}

/// The interrupt panel on screen for the application `app`.
struct InterruptState {
    app: String,
    frames: interrupt::Frames,
    surface: SurfaceId,
}

/// Namespace of the panel's layer surface for the server.
const PANEL_NAMESPACE: &str = "login-panel";

struct App {
    /// Asks the probe thread to read logind and the session manager again.
    refresh: mpsc::Sender<()>,
    client: Client,
    user: User,
    renderer: Renderer,
    authenticator: Authenticator,
    logind: Logind,
    /// The session manager: power actions and inhibitor locks.
    manager: LoginManager,
    control: control::Service,
    /// Held while the session is unlocked, so the system cannot sleep before the lock is up.
    inhibitor: Option<Inhibitor>,
    /// Held for the whole run: logind does not power off on the power key, [`App::power_key`] decides what it does.
    _power_key: Inhibitor,
    /// The hold timing of the power key.
    power_button: PowerButton,
    messages: Receiver<Message>,
    start: Instant,
    lock: Option<Lock>,
    /// `SwitchToLoginWindow` is waiting for the server to confirm the lock.
    switch_pending: bool,
    panel: Option<PanelState>,
    /// A confirmed log-out is running: the power button panel does not show over it.
    logging_out: bool,
    /// The applications are being quit before a confirmed action.
    quitting: Option<Quitting>,
    /// Wrong passwords typed at the lock screens of this run.
    failed: FailedAttempts,
    /// `RetriesUntilHint`, see [`FailedAttempts::help_shown`].
    retries_until_hint: u32,
    /// The low-battery notification on screen.
    low_battery: LowBattery,
    /// The session bus, for the notifications and the server's `org.cupertino.Session1`.
    session_bus: zbus::blocking::Connection,
}

/// Why [`run`] returned without a failure.
pub enum Ended {
    /// The process belongs to no login session (a container, CI): there is no session to lock.
    NoLoginSession,
}

/// Connect, serve the control interface and run until a connection or thread fails.
pub fn run(options: &SessionOptions) -> Result<Ended> {
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let renderer = Renderer::new(theme.tokens.typography(), &assets)?;
    let user = User::current()?;

    let mut client = Client::connect_shell()?;
    let (sender, messages) = mpsc::channel();
    let notifier = Notifier::new(sender, client.waker());

    let request_notifier = notifier.clone();
    // `RefreshCapabilities` goes to the probe thread, not through the loop: the menu's question must not wait for it
    let (refresh, refresh_requests) = mpsc::channel::<()>();
    let refresh_from_bus = refresh.clone();
    let on_request = std::sync::Arc::new(move |request| {
        if request == Request::RefreshCapabilities {
            // the probe thread is gone only while the process ends
            let _ = refresh_from_bus.send(());
            return;
        }
        request_notifier.send(match request {
            Request::Lock { explicit } => Message::Lock { explicit },
            Request::Panel(kind) => Message::Panel(kind),
            Request::PowerKey { pressed } => Message::PowerKey(pressed),
            Request::SwitchToLoginWindow => Message::SwitchToLoginWindow,
            Request::Sleep => Message::Sleep,
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
    let signal_notifier = notifier.clone();
    logind.listen(move |signal| {
        signal_notifier.send(match signal {
            Ok(Signal::Lock | Signal::PrepareForSleep) => Message::Lock { explicit: false },
            Err(e) => Message::Failed(e.to_string()),
        })
    })?;
    let battery_notifier = notifier.clone();
    upower::watch(logind.connection(), move |report| {
        battery_notifier.send(Message::Battery(report.map_err(|e| e.to_string())))
    })?;
    let manager = LoginManager::connect()?;
    let unlock_notifier = notifier.clone();
    manager.watch_unlock(move |signal| {
        unlock_notifier.send(match signal {
            Ok(()) => Message::Unlock,
            Err(e) => Message::Failed(e.to_string()),
        })
    })?;
    let restart_notifier = notifier.clone();
    manager.watch_owner(move |signal| {
        restart_notifier.send(match signal {
            Ok(()) => Message::ManagerRestarted,
            Err(e) => Message::Failed(e.to_string()),
        })
    })?;
    let inhibitor = Some(logind.inhibit_sleep()?);
    let power_key = logind.block_power_key()?;

    let auth_notifier = notifier.clone();
    let authenticator = Authenticator::spawn(user.login.clone(), move |event| {
        auth_notifier.send(Message::Auth(event))
    })?;

    if let Some(timeout) = options.idle_timeout {
        client.watch_idle(timeout)?;
    }
    let session_bus = control.connection();
    let quit_notifier = notifier;
    session::watch_quit(&session_bus, move |quit| {
        quit_notifier.send(match quit {
            Ok(app) => Message::ApplicationQuit(app),
            Err(e) => Message::Failed(format!("reading ApplicationQuit: {e}")),
        })
    })?;
    start_login_items(&logind);
    // a session the manager did not start (one on a getty, for the tests) is not known to it; that is reported, not fatal
    match manager.session_ready() {
        Ok(()) => eprintln!("cupertino-login: session announced to the session manager"),
        Err(e) => eprintln!("cupertino-login: {e}"),
    }
    spawn_capability_probe(refresh_requests, &logind, &manager, &control)?;
    let mut app = App {
        refresh,
        client,
        user,
        renderer,
        authenticator,
        logind,
        manager,
        control,
        inhibitor,
        _power_key: power_key,
        power_button: PowerButton::new(options.power_button_sleeps_system),
        messages,
        start: Instant::now(),
        lock: None,
        switch_pending: false,
        panel: None,
        logging_out: false,
        quitting: None,
        failed: FailedAttempts::default(),
        retries_until_hint: options.retries_until_hint,
        low_battery: LowBattery::new(),
        session_bus,
    };
    app.refresh_capabilities();
    loop {
        let timeout = app.timeout()?;
        for event in app.client.wait(timeout)? {
            app.handle(event)?;
        }
        app.housekeeping()?;
    }
}

/// Start the session's login items (XDG autostart, [`autostart`]) one after the other, once per session: a restarted agent
/// finds the run claimed and starts nothing. Problems are reported and leave the lock screen running.
fn start_login_items(logind: &Logind) {
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else {
        eprintln!("cupertino-login: login items not started: XDG_RUNTIME_DIR is not set, so a second run cannot be told apart");
        return;
    };
    match autostart::claim_run(std::path::Path::new(&runtime), logind.session_id()) {
        Ok(true) => {}
        Ok(false) => return eprintln!("cupertino-login: login items ran already in this session"),
        Err(e) => return eprintln!("cupertino-login: login items not started: {e}"),
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")));
    let dirs = autostart::directories(config_home, std::env::var("XDG_CONFIG_DIRS").ok());
    let found = match autostart::list(&dirs) {
        Ok(found) => found,
        Err(e) => return eprintln!("cupertino-login: login items not started: {e}"),
    };
    let env = autostart::Environment::from_process();
    for entry in found {
        match autostart::parse(&entry.id, &entry.path, &entry.text, &env) {
            Ok(item) => start_login_item(&item),
            Err(skip) => eprintln!(
                "cupertino-login: login item skipped: {}: {skip:?}",
                entry.id
            ),
        }
    }
}

/// Run one login item detached from the loop; its exit is reaped on a thread.
fn start_login_item(item: &autostart::Item) {
    let mut command = Command::new(&item.argv[0]);
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
        }
        Err(e) => eprintln!("cupertino-login: login item {} did not start: {e}", item.id),
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

impl App {
    /// Ask for a fresh reading of the menu's capabilities; the probe thread answers on its own.
    fn refresh_capabilities(&self) {
        // the thread is gone only while the process ends
        let _ = self.refresh.send(());
    }

    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// How long the loop may sleep: until the screen next changes by itself, the clock's minute turns, the panel's
    /// countdown shows another number, a held power key reaches the hold time or the quit sequence's timer runs out.
    fn timeout(&self) -> Result<Option<Duration>> {
        let now = self.now();
        let screen = match &self.lock {
            Some(lock) => {
                let to_next_minute = 60 - screen::unix_seconds()?.rem_euclid(60) as u64;
                let by_screen = lock.screen.next_wakeup(now);
                Some(by_screen.map_or(Duration::from_secs(to_next_minute), |s| {
                    Duration::from_secs_f64(s).min(Duration::from_secs(to_next_minute))
                }))
            }
            None => None,
        };
        let panel = self
            .panel
            .as_ref()
            .and_then(|p| p.panel.next_wakeup(now))
            .map(Duration::from_secs_f64);
        let power_key = self
            .power_button
            .next_wakeup(now)
            .map(Duration::from_secs_f64);
        let quitting = self
            .quitting
            .as_ref()
            .and_then(|q| q.sequence.next_wakeup(now))
            .map(Duration::from_secs_f64);
        Ok(screen
            .into_iter()
            .chain(panel)
            .chain(power_key)
            .chain(quitting)
            .min())
    }

    fn handle(&mut self, event: Event) -> Result<()> {
        let now = self.now();
        match event {
            Event::Draw(surface) => self.draw(surface)?,
            Event::Key {
                surface,
                pressed: true,
                key,
                mods,
                character,
            } if self.panel.as_ref().is_some_and(|p| p.surface == surface) => {
                let decision = screen::typed_key(key, mods, character)
                    .and_then(|typed| self.panel.as_ref().and_then(|p| p.panel.press(typed)));
                if let Some(decision) = decision {
                    self.decide(decision)?;
                }
            }
            Event::Pointer {
                surface,
                event:
                    PointerEvent::Button {
                        button: Button::Left,
                        pressed: true,
                        position,
                    },
            } if self.panel.as_ref().is_some_and(|p| p.surface == surface) => {
                let decision = self
                    .panel
                    .as_ref()
                    .and_then(|p| p.frames.button_at(position));
                if let Some(decision) = decision {
                    self.decide(decision)?;
                }
            }
            Event::Key {
                surface,
                pressed: true,
                key,
                mods,
                character,
            } if self.interrupt_surface() == Some(surface) => {
                if let Some(choice) =
                    screen::typed_key(key, mods, character).and_then(interrupt::press)
                {
                    self.choose(now, choice)?;
                }
            }
            Event::Pointer {
                surface,
                event:
                    PointerEvent::Button {
                        button: Button::Left,
                        pressed: true,
                        position,
                    },
            } if self.interrupt_surface() == Some(surface) => {
                let choice = self
                    .quitting
                    .as_ref()
                    .and_then(|q| q.interrupt.as_ref())
                    .and_then(|i| i.frames.button_at(position));
                if let Some(choice) = choice {
                    self.choose(now, choice)?;
                }
            }
            Event::Key {
                pressed: true,
                key,
                mods,
                character,
                ..
            } => {
                let Some(lock) = &mut self.lock else {
                    return Ok(());
                };
                match screen::typed_key(key, mods, character) {
                    Some(typed) => {
                        if let Some(password) = lock.screen.press(now, typed) {
                            self.authenticator.attempt(password);
                        }
                    }
                    None => lock.screen.activity(now),
                }
            }
            Event::Pointer {
                event: PointerEvent::Motion(_) | PointerEvent::Button { .. },
                ..
            } => {
                if let Some(lock) = &mut self.lock {
                    lock.screen.activity(now);
                }
            }
            Event::OutputsChanged => self.arrange_surfaces()?,
            Event::Closed(surface) if self.panel.as_ref().is_some_and(|p| p.surface == surface) => {
                self.panel = None;
                eprintln!("cupertino-login: panel cancelled");
            }
            Event::Closed(surface) if self.interrupt_surface() == Some(surface) => {
                self.quitting
                    .as_mut()
                    .expect("the interrupt panel belongs to a quit sequence")
                    .interrupt = None;
                self.choose(now, Choice::Cancel)?;
            }
            Event::Closed(_) => return Err("the server closed a surface of the lock screen".into()),
            // the draw that follows a configure or scale change does the work
            Event::Configured { .. } | Event::ScaleChanged { .. } => {}
            Event::Key { .. } | Event::KeyboardLeave(_) | Event::Pointer { .. } => {}
            // the screens have one fixed look, and this process is the one that holds the lock
            Event::LockRequested { explicit } => self.request_lock(now, explicit)?,
            Event::Appearance { .. } | Event::DisplayOptions(_) | Event::SessionLock { .. } => {}
            Event::PopupDone(_)
            | Event::ActivationToken { .. }
            | Event::MenuBar { .. }
            | Event::Dock { .. } => {}
        }
        Ok(())
    }

    /// Carry out the queued requests, follow the lock and the screen's timers, and redraw what changed.
    fn housekeeping(&mut self) -> Result<()> {
        let now = self.now();
        while let Ok(message) = self.messages.try_recv() {
            match message {
                Message::Lock { explicit } => self.request_lock(now, explicit)?,
                Message::Panel(kind) => self.show_panel(now, kind)?,
                Message::PowerKey(pressed) => self.power_key(now, pressed)?,
                Message::SwitchToLoginWindow => {
                    self.switch_pending = true;
                    self.request_lock(now, false)?;
                }
                Message::Sleep => self.request_power(Power::Sleep)?,
                Message::Unlock => match &mut self.lock {
                    Some(lock) => {
                        eprintln!("cupertino-login: unlocked by a fast user switch");
                        lock.screen.unlock_by_switch(now);
                    }
                    None => eprintln!("cupertino-login: unlock ignored: the session is not locked"),
                },
                Message::ManagerRestarted => {
                    self.refresh_capabilities();
                    match self.manager.session_ready() {
                        Ok(()) => eprintln!(
                            "cupertino-login: session announced again to the restarted session manager"
                        ),
                        Err(e) => eprintln!("cupertino-login: {e}"),
                    }
                }
                Message::Auth(event) => self.handle_auth(now, event)?,
                Message::Failed(reason) => return Err(reason.into()),
                Message::Battery(report) => self.battery(report),
                Message::ApplicationQuit(app) => {
                    let Some(quitting) = &mut self.quitting else {
                        continue;
                    };
                    eprintln!("cupertino-login: application quit: {app}");
                    let step = quitting.sequence.quit(now, &app);
                    self.follow(now, step)?;
                }
            }
        }
        let step = self.quitting.as_mut().and_then(|q| q.sequence.tick(now));
        self.follow(now, step)?;
        if self.client.take_idle() {
            self.request_lock(now, false)?;
        }
        if let Some(effect) = self.power_button.tick(now) {
            self.power_effect(now, effect)?;
        }
        if let Some(decision) = self.panel.as_ref().and_then(|p| p.panel.tick(now)) {
            self.decide(decision)?;
        }
        if let Some(panel) = &mut self.panel {
            if panel.drawn != Some(panel.panel.seconds_left(now)) {
                self.client.request_redraw(panel.surface)?;
            }
        }
        let Some(lock) = &mut self.lock else {
            return Ok(());
        };
        match self.client.lock_state() {
            Some(LockState::Finished) => {
                return Err("the server refused or ended the session lock".into())
            }
            Some(LockState::Locked) if !lock.confirmed => {
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
        let scene = Scene {
            screen: &lock.screen,
            caps_lock: self.client.caps_lock(),
            help: self
                .failed
                .help_shown(&self.user.login, self.retries_until_hint),
            power: None,
        };
        let key = FrameKey::new(&scene, now)?;
        if lock.drawn.as_ref() != Some(&key) {
            for (surface, _) in &lock.surfaces {
                self.client.request_redraw(*surface)?;
            }
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

    /// Ask the server for the lock and give every output its surface; does nothing when the session is locked already.
    /// A panel that is open is cancelled: the lock screen takes the keyboard.
    /// Lock the session. `explicit`: the person asked, so the lock screen hides after 5 s without input instead of 30 s.
    fn request_lock(&mut self, now: f64, explicit: bool) -> Result<()> {
        if self.lock.is_some() {
            return Ok(());
        }
        if self.panel.is_some() {
            self.decide(Decision::Cancel)?;
        }
        if self.interrupt_surface().is_some() {
            self.choose(now, Choice::Cancel)?;
        }
        // the lock screen swallows a key that is down
        self.power_button.reset();
        self.client.lock_session()?;
        self.lock = Some(Lock {
            screen: LockScreen::new(
                now,
                if explicit {
                    HIDE_AFTER_EXPLICIT_LOCK
                } else {
                    HIDE_AFTER
                },
            ),
            surfaces: Vec::new(),
            confirmed: false,
            drawn: None,
        });
        self.control.set_locked(true)?;
        self.arrange_surfaces()
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

    /// Keep exactly one lock surface per output the server has described.
    fn arrange_surfaces(&mut self) -> Result<()> {
        let Some(lock) = &mut self.lock else {
            return Ok(());
        };
        screen::arrange_surfaces(&mut self.client, &mut lock.surfaces, |client, output| {
            Ok(client.lock_surface(output)?)
        })
    }

    fn draw(&mut self, surface: SurfaceId) -> Result<()> {
        let now = self.now();
        if let Some(panel) = self.panel.as_mut().filter(|p| p.surface == surface) {
            let view = PanelView {
                kind: panel.panel.kind(),
                seconds_left: panel.panel.seconds_left(now),
                frames: panel.frames.clone(),
            };
            let renderer = &mut self.renderer;
            self.client.draw(surface, |pixmap, scale| {
                renderer.render_panel(&view, pixmap, scale.factor())
            })?;
            panel.drawn = Some(view.seconds_left);
            return Ok(());
        }
        if let Some(shown) = self
            .quitting
            .as_ref()
            .and_then(|q| q.interrupt.as_ref())
            .filter(|i| i.surface == surface)
        {
            let renderer = &mut self.renderer;
            return Ok(self.client.draw(surface, |pixmap, scale| {
                renderer.render_interrupt(&shown.app, &shown.frames, pixmap, scale.factor())
            })?);
        }
        // of a surface destroyed since
        let Some(lock) = self
            .lock
            .as_mut()
            .filter(|lock| lock.surfaces.iter().any(|&(s, _)| s == surface))
        else {
            return Ok(());
        };
        let scene = Scene {
            screen: &lock.screen,
            caps_lock: self.client.caps_lock(),
            help: self
                .failed
                .help_shown(&self.user.login, self.retries_until_hint),
            power: None,
        };
        let key = FrameKey::new(&scene, now)?;
        screen::draw(
            &mut self.client,
            &mut self.renderer,
            &key,
            Some(&self.user),
            None,
            surface,
        )?;
        lock.drawn = Some(key);
        Ok(())
    }

    /// End the lock after an accepted attempt and hold sleep back again.
    fn unlock(&mut self) -> Result<()> {
        self.client.unlock_session()?;
        self.lock = None;
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
        let output = self
            .client
            .outputs()
            .into_iter()
            .find(|o| o.geometry.is_some())
            .ok_or("the server has no output")?
            .id;
        let frames = self.renderer.panel_frames(kind);
        let surface = self.client.layer_surface(&LayerSurfaceConfig {
            namespace: PANEL_NAMESPACE.to_string(),
            output: Some(output),
            layer: Layer::Overlay,
            // no anchor: the server centres the surface on the output
            anchor: Anchor::empty(),
            size: (frames.width as u32, frames.height as u32),
            margin: Margin::default(),
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::Exclusive,
        })?;
        self.panel = Some(PanelState {
            panel: Panel::new(kind, now),
            frames,
            surface,
            drawn: None,
        });
        eprintln!("cupertino-login: panel shown: {}", kind.log_name());
        Ok(())
    }

    /// Close the panel with `decision`; an action is carried out through logind.
    fn decide(&mut self, decision: Decision) -> Result<()> {
        let PanelState { surface, .. } = self
            .panel
            .take()
            .expect("a decision is only made on an open panel");
        self.client.destroy_surface(surface)?;
        match decision {
            Decision::Cancel => eprintln!("cupertino-login: panel cancelled"),
            Decision::Do(action) => {
                eprintln!("cupertino-login: panel confirmed: {}", action.log_name());
                self.carry_out(self.now(), action)?;
            }
        }
        Ok(())
    }

    /// Ask the session manager to restart, shut down or sleep. A refusal (another user's session is open, an inhibitor) is
    /// reported and the session goes on; any other failure ends the process, which the server restarts.
    fn request_power(&mut self, action: Power) -> Result<()> {
        match self.manager.request_power(action) {
            Ok(()) => {}
            Err(e) if e.refusal().is_some() => {
                eprintln!("cupertino-login: {action:?} refused: {e}");
                self.logging_out = false;
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
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// Sleep at once; log out, restart and shut down after the applications have quit.
    fn carry_out(&mut self, now: f64, action: Action) -> Result<()> {
        if action == Action::Sleep {
            return self.request_power(Power::Sleep);
        }
        self.logging_out = true;
        let (sequence, step) = LogOut::start();
        self.quitting = Some(Quitting {
            sequence,
            then: action,
            interrupt: None,
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
                logout::Do::AskNext => {
                    match session::quit_next_application(&self.session_bus) {
                        Ok((app, windows)) => {
                            if !app.is_empty() {
                                eprintln!("cupertino-login: quitting application: {app} ({windows} windows)");
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
                    }
                }
                logout::Do::Terminate => {
                    let Quitting {
                        then, interrupt, ..
                    } = self.quitting.take().expect("checked above");
                    if let Some(shown) = interrupt {
                        self.client.destroy_surface(shown.surface)?;
                    }
                    eprintln!("cupertino-login: no application left: {}", then.log_name());
                    match then {
                        Action::LogOut => self.logind.terminate_session()?,
                        Action::Restart => self.request_power(Power::Restart)?,
                        Action::ShutDown => self.request_power(Power::ShutDown)?,
                        Action::Sleep => unreachable!("sleep does not quit the applications"),
                    }
                }
                logout::Do::Interrupt(app) => self.show_interrupt(now, app)?,
                logout::Do::CloseInterrupt => {
                    if let Some(shown) = quitting.interrupt.take() {
                        self.client.destroy_surface(shown.surface)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn interrupt_surface(&self) -> Option<SurfaceId> {
        self.quitting
            .as_ref()
            .and_then(|q| q.interrupt.as_ref())
            .map(|i| i.surface)
    }

    /// Show the interrupt panel for `app` on the first output. On a locked session there is nobody to answer it: the
    /// sequence is cancelled.
    fn show_interrupt(&mut self, now: f64, app: String) -> Result<()> {
        eprintln!("cupertino-login: application did not quit: {app}");
        if self.lock.is_some() {
            return self.choose(now, Choice::Cancel);
        }
        let output = self
            .client
            .outputs()
            .into_iter()
            .find(|o| o.geometry.is_some())
            .ok_or("the server has no output")?
            .id;
        let surface = self.client.layer_surface(&LayerSurfaceConfig {
            namespace: PANEL_NAMESPACE.to_string(),
            output: Some(output),
            layer: Layer::Overlay,
            // no anchor: the server centres the surface on the output
            anchor: Anchor::empty(),
            size: (interrupt::WIDTH as u32, interrupt::HEIGHT as u32),
            margin: Margin::default(),
            exclusive_zone: -1,
            keyboard: KeyboardInteractivity::Exclusive,
        })?;
        let quitting = self
            .quitting
            .as_mut()
            .expect("the interrupt panel belongs to a quit sequence");
        quitting.interrupt = Some(InterruptState {
            app,
            frames: interrupt::Frames::new(),
            surface,
        });
        Ok(())
    }

    /// The answer on the interrupt panel: Cancel ends the sequence and the session goes on; Force Quit kills the
    /// application, which then has another full timeout to go away.
    fn choose(&mut self, now: f64, choice: Choice) -> Result<()> {
        let quitting = self
            .quitting
            .as_mut()
            .expect("a choice is only made while the applications are quit");
        if let Some(shown) = quitting.interrupt.take() {
            self.client.destroy_surface(shown.surface)?;
        }
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
