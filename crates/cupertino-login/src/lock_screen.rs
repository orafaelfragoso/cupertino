//! The lock screen of a user's session, a CupertinoKit program on `cupertino-kit-shell` (`run_lock_screen`). It locks
//! the session on a request (the server's, logind's, `org.cupertino.Login1`'s, the seat's idleness), shows one lock
//! surface per output ([`view::screen`] on `Role::Lock`), sends the attempts to PAM on Kit's I/O pool and unlocks
//! only when the state machine has reached [`Phase::Unlocked`], which only an accepted attempt (or the session
//! manager's proof of a fast user switch) does. Until W6.14 it also shows the desktop-control consent panel
//! ([`consent`]), the authority the server gives the `session_lock` role.
//!
//! Nothing waits on the UI thread: the buses are Kit's connections the loop ticks, their signals futures of the loop
//! that hand news over a channel ([`Heard`]), and every file, PAM and the wallpaper's decoding run on the I/O pool.

mod consent;
mod system;

use crate::auth;
use crate::hint::FailedAttempts;
use crate::lock_notices::Notice;
use crate::logind::Inhibitor;
use crate::model::{AuthOutcome, LockScreen, Phase, HIDE_AFTER, HIDE_AFTER_EXPLICIT_LOCK};
use crate::options::SessionOptions;
use crate::screen::{self, Result, Timer, FRAME};
use crate::status::StatusItems;
use crate::user::User;
use crate::view::{
    self, Access, Clock, Field, Input, Made, Metrics, Screen, ScreenPictures, Status, Wants,
    LOCKED_TITLE, LOCK_APP_ID,
};
use crate::wallpaper::{self, Source};
use cupertino_defaults::login::ScreenSettings;
use cupertino_defaults::menubar::HourCycle;
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{
    Application, Command, Environment, Key, RendererClass, Window, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::SystemAppearance;
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::session::{self, Change, IdleWatch};
use cupertino_kit_shell::{lock, Event, Program, Role, Surface};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub use system::Stream;

/// What the lock screen shows until its wallpaper is ready, and the longest it waits for it before it shows its
/// interface over that (approximate): a picture still being read comes in under the interface when it is ready.
static WAITING: Source = Source::Solid([0, 0, 0]);
const WALLPAPER_WAIT: f64 = 0.5;

/// How long reading and decoding the wallpaper should take on the I/O pool: the settings file and a picture of up to
/// 256 MiB (`wallpaper::MAX_FILE_BYTES`) on a local disk.
const WALLPAPER_DEADLINE: Duration = Duration::from_secs(10);

/// How long reading the user's picture should take on the I/O pool: one small file in the home directory.
const PICTURE_DEADLINE: Duration = Duration::from_secs(2);

/// The options `run` was given, for the program's `init`.
static OPTIONS: Mutex<Option<SessionOptions>> = Mutex::new(None);

/// What the bus futures, PAM and the settings watch tell the lock screen, in the order they happened.
#[derive(Clone, Debug)]
pub enum Heard {
    /// `Lock` on the control interface, logind's `Lock` signal or the preparation for sleep.
    Lock,
    /// The session manager's `Unlock`: the person at the login screen proved this session's password.
    Unlock,
    /// The session manager's bus name has an owner again: it restarted and does not know this session's agent.
    ManagerRestarted,
    /// A reading of the status items is out of date.
    StatusDue,
    /// The compositor's settings changed: the login screen's copy of the wallpaper is written again.
    WallpaperChanged,
    /// The notifications component's list for the lock screen changed.
    NoticesDue,
    /// Text a PAM module printed during the attempt.
    Notice(String),
    /// The attempt is decided.
    Outcome(AuthOutcome),
    /// A stream is subscribed to: what it carries from now on is heard.
    Following(Stream),
    /// A stream ended, and why.
    Ended(Stream, String),
}

/// What the server tells the lock screen through `cupertino-kit-shell`.
#[derive(Clone, Debug)]
pub enum Shell {
    /// The session is locked or unlocked; once at start.
    Locked(bool),
    /// The server asks the lock screen to lock.
    LockRequested {
        explicit: bool,
    },
    /// The seat went idle (`true`) or input resumed.
    Idle(bool),
    Appearance(SystemAppearance),
    Outputs,
    /// The server confirmed the session lock (`true`), or refused or ended it.
    Lock(bool),
    Consent(cupertino_kit_shell::consent::Event),
    CapsLock(bool),
    Closed(WindowKey),
}

#[derive(Clone, Debug)]
pub enum Message {
    Session(Option<zbus::Connection>),
    System(Option<zbus::Connection>),
    /// A bus connection closed.
    BusClosed(&'static str),
    Served(system::Served),
    LoginSession(std::result::Result<Option<zbus::zvariant::OwnedObjectPath>, String>),
    Inhibited(std::result::Result<Arc<Inhibitor>, String>),
    Announced(std::result::Result<(), String>),
    Heard(Heard),
    Shell(Shell),
    Input(Input),
    Timer(u64),
    Settings(ScreenSettings),
    Hours(HourCycle),
    Picture(Option<ImageSource>),
    /// The wallpaper of the lock of this number is ready.
    Wallpaper {
        lock: u64,
        source: Source,
    },
    Status(StatusItems),
    Notices(Vec<Notice>),
    ConsentIcons {
        run: cupertino_kit_shell::consent::Run,
        icons: Vec<ImageSource>,
    },
    /// A picture of the screens was made on the CPU pool.
    Made(Made),
    /// A task that has nothing to report ended.
    Done,
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

impl Lock {
    /// How much of the dimming shows at `now`: none before the screen shows, all of it [`wallpaper::DIM_FADE`] after.
    fn dim(&self, now: f64) -> f64 {
        self.shown.map_or(0.0, |at| wallpaper::dimming_at(now - at))
    }
}

/// One output and the lock surface it gets while the session is locked.
#[derive(Clone, Debug)]
struct LockOutput {
    id: OutputId,
    key: WindowKey,
    /// The number in its window's app id, kept while the output is there.
    number: u64,
    size: Size,
    /// Device pixels per point.
    scale: f64,
}

/// The lock screen program.
pub struct LockScreenProgram;

/// What the lock screen holds.
pub struct Model {
    start: Instant,
    heard: (async_channel::Sender<Heard>, async_channel::Receiver<Heard>),
    session: Option<zbus::Connection>,
    system: Option<zbus::Connection>,
    options: SessionOptions,
    _idle: Option<IdleWatch>,
    /// The session asks for reduced motion: the lock screen follows it.
    reduce_motion: bool,
    /// The display options the field and its buttons follow.
    access: Access,
    /// The session's appearance is dark: the lock screen stands on its dark wallpaper.
    dark: bool,
    /// What `login.conf` says the screens show; the defaults until it is read.
    settings: ScreenSettings,
    /// The clock's hour cycle: the session's.
    hours: HourCycle,
    status: system::Reading<StatusItems>,
    /// The notifications the lock screen shows, as the component last listed them; shown only while locked.
    notices: system::Reading<Vec<Notice>>,
    outputs: Vec<LockOutput>,
    next_output: u64,
    caps_lock: bool,
    env: Environment,
    metrics: Metrics,
    /// The pictures the lock surfaces stand on, made on the CPU pool.
    pictures: ScreenPictures,
    clock: Clock,
    timer: Timer,
    user: User,
    /// Held while the session is unlocked, so the system cannot sleep before the lock is up.
    inhibitor: Option<Arc<Inhibitor>>,
    /// logind knows this session: the inhibitor is taken again at each unlock.
    logind: bool,
    /// The streams subscribed to so far.
    followed: Vec<Stream>,
    /// The seat went idle and input has not resumed since.
    idled: bool,
    /// The number of the last lock taken.
    locks: u64,
    lock: Option<Lock>,
    consent: Option<consent::Panel>,
    /// Wrong passwords typed at the lock screens of this run.
    failed: FailedAttempts,
}

impl Drop for Model {
    /// The connections' executors are ticked by the program's loop, which has ended: a task still on one (a follower,
    /// the object server of `org.cupertino.Login1`) would keep its connection. So they are closed, at most
    /// [`screen::CLOSE_WAIT`].
    fn drop(&mut self) {
        screen::close([self.session.take(), self.system.take()]);
    }
}

/// Run the lock screen with `options` on the connection the server gave the process, until it ends: `Ok` for an end
/// the server should not restart (the process is in no login session), else why it failed.
pub fn run(options: SessionOptions) -> std::result::Result<(), String> {
    *OPTIONS.lock().unwrap_or_else(PoisonError::into_inner) = Some(options);
    let environment = cupertino_kit::app::environment().map_err(|e| e.to_string())?;
    cupertino_kit_shell::run_lock_screen::<LockScreenProgram>(environment)
        .map_err(|e| e.to_string())?;
    screen::ended("the lock screen")
}

/// Write the login screen's copy of the session's wallpaper on the I/O pool.
fn write_login_wallpaper() -> Command<Message> {
    cupertino_kit::task::io(
        WALLPAPER_DEADLINE,
        crate::login_wallpaper::write_current,
        |()| Message::Done,
    )
}

/// The next piece of news on `heard`.
fn listen(heard: &async_channel::Receiver<Heard>) -> Command<Message> {
    let heard = heard.clone();
    Command::Perform(Box::pin(async move {
        match heard.recv().await {
            Ok(news) => Message::Heard(news),
            // the model holds a sender, so the channel never closes while it lives
            Err(_) => std::future::pending().await,
        }
    }))
}

impl Application for LockScreenProgram {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The lock screen must always draw, whatever becomes of the GPU, and it is mostly at rest.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Command<Message>>) {
        let options = OPTIONS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or(SessionOptions {
                idle_timeout: None,
                retries_until_hint: 0,
            });
        let heard = async_channel::unbounded();
        let user = User::current().unwrap_or_else(|e| {
            eprintln!("cupertino-login: {e}");
            User::typed(String::new())
        });
        let idle = options.idle_timeout.map(session::watch_idle);
        cupertino_kit_shell::consent::bind();
        let env = cupertino_kit_shell::environment();
        let mut model = Model {
            start: Instant::now(),
            heard,
            session: None,
            system: None,
            options,
            _idle: idle,
            reduce_motion: false,
            access: Access::default(),
            dark: false,
            settings: ScreenSettings::default(),
            hours: crate::user::locale_hour_cycle(),
            status: system::Reading::default(),
            notices: system::Reading::default(),
            outputs: Vec::new(),
            next_output: 0,
            caps_lock: false,
            metrics: Metrics::of(&env.text),
            env,
            pictures: ScreenPictures::new(),
            clock: Clock::default(),
            timer: Timer::default(),
            user,
            inhibitor: None,
            logind: false,
            followed: Vec::new(),
            idled: false,
            locks: 0,
            lock: None,
            consent: None,
            failed: FailedAttempts::default(),
        };
        model.follow_outputs();
        let mut commands = system::start(&model.heard.0);
        commands.push(listen(&model.heard.1));
        commands.push(cupertino_kit::task::io(
            screen::SETTINGS_DEADLINE,
            ScreenSettings::load,
            Message::Settings,
        ));
        commands.push(cupertino_kit::task::io(
            screen::SETTINGS_DEADLINE,
            screen::session_hour_cycle,
            Message::Hours,
        ));
        commands.push(cupertino_kit::task::io(
            PICTURE_DEADLINE,
            || {
                crate::user::current_picture()
                    .map_err(|e| eprintln!("cupertino-login: {e}"))
                    .ok()
                    .flatten()
            },
            Message::Picture,
        ));
        // the login screen shows this session's wallpaper after it, as the compositor's settings name it now
        commands.push(write_login_wallpaper());
        let heard = model.heard.0.clone();
        commands.extend(screen::watch(
            cupertino_defaults::compositor::CompositorSettings::path()
                .into_iter()
                .collect(),
            "the login screen's wallpaper",
            move || drop(heard.try_send(Heard::WallpaperChanged)),
        ));
        // the input source the status item shows
        let heard = model.heard.0.clone();
        commands.extend(screen::watch(
            cupertino_defaults::keyboard::KeyboardSettings::path()
                .into_iter()
                .collect(),
            "the input item",
            move || drop(heard.try_send(Heard::StatusDue)),
        ));
        // the session is locked as this process starts: its lock screen went away without unlocking (it crashed and
        // the server restarted this process), and the outputs stay black until this one takes the lock
        if session::locked() {
            eprintln!(
                "cupertino-login: the session is locked without a lock screen: taking the lock"
            );
            let now = model.now();
            model.request_lock(now, false, &mut commands);
        }
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let now = model.now();
        let mut commands = Vec::new();
        let result = match message {
            Message::Heard(heard) => {
                commands.push(listen(&model.heard.1));
                model.heard(now, heard, &mut commands)
            }
            Message::Shell(shell) => model.shell(now, shell, &mut commands),
            Message::Input(input) => {
                model.input(now, input, &mut commands);
                Ok(())
            }
            Message::Timer(id) => {
                model.timer.fired(id);
                Ok(())
            }
            Message::Settings(settings) => {
                model.settings = settings;
                commands.extend(model.read_status());
                Ok(())
            }
            Message::Hours(hours) => {
                model.hours = hours;
                Ok(())
            }
            Message::Picture(picture) => {
                model.user.picture = picture;
                Ok(())
            }
            Message::Wallpaper { lock, source } => {
                if let Some(held) = model.lock.as_mut().filter(|l| l.number == lock) {
                    held.wallpaper = Some(source);
                }
                Ok(())
            }
            Message::Status(items) => {
                model.status.value = items;
                if model.status.read() {
                    commands.extend(model.read_status());
                }
                Ok(())
            }
            Message::Notices(list) => {
                // a list that comes while the session is not locked is a late one: it never shows
                model.notices.value = if model.lock.is_some() {
                    list
                } else {
                    Vec::new()
                };
                if model.notices.read() {
                    commands.extend(model.read_notices());
                }
                Ok(())
            }
            Message::ConsentIcons { run, icons } => {
                if let Some(panel) = model.consent.as_mut().filter(|p| p.run == run) {
                    panel.set_icons(icons);
                }
                Ok(())
            }
            Message::Made(made) => {
                let env = model.env.clone();
                commands.extend(model.pictures.made(&env, made, Message::Made));
                Ok(())
            }
            Message::Done => Ok(()),
            other => model.bus(other, &mut commands),
        };
        match result.and_then(|()| model.housekeeping(now, &mut commands)) {
            Ok(()) => {
                commands.extend(model.want_pictures(now));
                let wait = model.next_wakeup(now);
                commands.extend(model.timer.schedule(now, wait, Message::Timer));
                commands
            }
            Err(e) => screen::end(Err(e.to_string())),
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let now = model.now();
        let mut windows = Vec::new();
        if let Some(lock) = model
            .lock
            .as_ref()
            .filter(|l| l.screen.phase() != Phase::Unlocked)
        {
            for output in &model.outputs {
                let content = view::screen(
                    &model.screen_of(lock, output, now),
                    LOCKED_TITLE,
                    Message::Input,
                );
                windows.push(view::full_screen(
                    output.key.clone(),
                    format!("{LOCK_APP_ID}.{}", output.number),
                    LOCKED_TITLE,
                    output.size,
                    content,
                    None,
                ));
            }
        }
        if model.lock.is_none() {
            if let Some(panel) = model.consent.as_ref().filter(|p| p.shown()) {
                windows.push(view::desktop_control(
                    consent::key(),
                    panel.request(),
                    Message::Input,
                ));
            }
        }
        windows
    }
}

impl Program for LockScreenProgram {
    fn surface(model: &Model, window: &WindowKey) -> Surface {
        let role = match model.outputs.iter().find(|o| &o.key == window) {
            Some(output) => Role::Lock(output.id),
            None if *window == consent::key() => {
                Role::Layer(consent::layer(model.consent.as_ref()))
            }
            None => Role::Window,
        };
        Surface {
            role,
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        let shell = match event {
            Event::Session(Change::Locked(locked)) => Shell::Locked(locked),
            Event::Session(Change::LockRequested { explicit }) => Shell::LockRequested { explicit },
            Event::Session(Change::Idle(idle)) => Shell::Idle(idle),
            Event::Appearance(appearance) => Shell::Appearance(appearance),
            Event::Outputs => Shell::Outputs,
            Event::Lock(lock::Event::Locked) => Shell::Lock(true),
            Event::Lock(lock::Event::Finished) => Shell::Lock(false),
            Event::Consent(event) => Shell::Consent(event),
            Event::CapsLock(on) => Shell::CapsLock(on),
            Event::Closed(key) => Shell::Closed(key),
            _ => return None,
        };
        Some(Message::Shell(shell))
    }
}

impl Model {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// The lock screen of `lock` on `output` at `now`.
    fn screen_of<'a>(&'a self, lock: &'a Lock, output: &LockOutput, now: f64) -> Screen<'a> {
        let size = output.size;
        let screen = &lock.screen;
        let help = self
            .failed
            .help_shown(&self.user.login, self.options.retries_until_hint);
        // nothing of the interface shows before its wallpaper does
        let shown = lock.shown.is_some();
        Screen {
            size,
            metrics: self.metrics,
            clock: &self.clock,
            user: Some(&self.user),
            field: Field::Password {
                generation: screen.field(),
                // nothing is typed while an attempt is checked
                enabled: screen.phase() != Phase::Unlocking && !screen.checking(),
                busy: screen.checking(),
            },
            name: None,
            typed: screen.typed(),
            caps_lock: self.caps_lock,
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
            lock_message: self.settings.lock_message.as_deref(),
            back: false,
            locked: false,
            hide_user: self.settings.hide_user_avatar_and_name,
            access: self.access,
            large_clock: self.settings.large_clock,
            avatar_from: None,
            power: None,
            status: Status {
                items: &self.status.value,
                access: None,
            },
            notices: &self.notices.value,
            wallpaper: lock.wallpaper.as_ref().unwrap_or(&WAITING),
            dim: lock.dim(now),
            pictures: self.pictures.shown(&output.key),
        }
    }

    /// Name what every lock surface shows to the pictures, which make what is missing on the CPU pool. Nothing while the
    /// session is unlocked.
    fn want_pictures(&mut self, now: f64) -> Vec<Command<Message>> {
        let screens: Vec<(WindowKey, Wants)> = match &self.lock {
            Some(lock) if lock.screen.phase() != Phase::Unlocked => self
                .outputs
                .iter()
                .map(|output| {
                    let wants = self.screen_of(lock, output, now).wants(output.scale);
                    (output.key.clone(), wants)
                })
                .collect(),
            _ => Vec::new(),
        };
        if screens.is_empty() && self.lock.is_none() {
            return Vec::new();
        }
        let env = self.env.clone();
        self.pictures.want(&env, screens, Message::Made)
    }

    /// The outputs the server describes now, each keeping its window while it stays.
    fn follow_outputs(&mut self) {
        let mut outputs = Vec::new();
        for output in outputs::all() {
            let Some(geometry) = output.geometry else {
                continue;
            };
            let number = match self.outputs.iter().find(|o| o.id == output.id) {
                Some(known) => known.number,
                None => {
                    self.next_output += 1;
                    self.next_output
                }
            };
            outputs.push(LockOutput {
                id: output.id,
                key: WindowKey(Key::from(format!("lock-{number}"))),
                number,
                size: geometry.size,
                scale: output.scale,
            });
        }
        self.outputs = outputs;
    }

    /// Seconds until the program next has something to do by itself: a frame of a fade, the shake or the spinner,
    /// the screen's next change or the clock's minute.
    fn next_wakeup(&self, now: f64) -> Option<f64> {
        self.lock.as_ref().map(|lock| {
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
        })
    }

    fn heard(
        &mut self,
        now: f64,
        heard: Heard,
        commands: &mut Vec<Command<Message>>,
    ) -> Result<()> {
        match heard {
            Heard::Lock => self.request_lock(now, false, commands),
            Heard::Unlock => match &mut self.lock {
                Some(lock) => {
                    eprintln!("cupertino-login: unlocked by a fast user switch");
                    lock.screen.unlock_by_switch(now);
                }
                None => eprintln!("cupertino-login: unlock ignored: the session is not locked"),
            },
            Heard::ManagerRestarted => {
                commands.extend(self.announce(" again to the restarted session manager"))
            }
            Heard::StatusDue => commands.extend(self.read_status()),
            Heard::WallpaperChanged => commands.push(write_login_wallpaper()),
            Heard::NoticesDue => commands.extend(self.read_notices()),
            Heard::Notice(text) => {
                if let Some(lock) = &mut self.lock {
                    lock.screen.notify(text);
                }
            }
            Heard::Outcome(outcome) => self.handle_auth(now, outcome)?,
            Heard::Following(stream) => commands.extend(self.following(stream)),
            Heard::Ended(stream, why) => return system::ended(stream, &why),
        }
        Ok(())
    }

    fn shell(
        &mut self,
        now: f64,
        shell: Shell,
        commands: &mut Vec<Command<Message>>,
    ) -> Result<()> {
        match shell {
            // a lock this process does not hold: the lock screen before it went away without unlocking
            Shell::Locked(true) if self.lock.is_none() => self.request_lock(now, false, commands),
            Shell::Locked(_) => {}
            Shell::LockRequested { explicit } => self.request_lock(now, explicit, commands),
            Shell::Idle(idle) => {
                self.idled = idle;
                // the display sleeps or wakes: what was typed does not wait for whoever comes
                if let Some(lock) = &mut self.lock {
                    lock.screen.clear_field();
                }
            }
            Shell::Appearance(appearance) => {
                self.reduce_motion = appearance.reduce_motion;
                self.access = Access {
                    increase_contrast: appearance.increase_contrast,
                    reduce_transparency: appearance.reduce_transparency,
                };
                if let Some(lock) = &mut self.lock {
                    lock.screen.set_reduce_motion(appearance.reduce_motion);
                }
                let changed = self.dark != appearance.dark;
                self.dark = appearance.dark;
                // the lock screen takes the wallpaper of the new appearance; the old one shows until it is read
                if let (true, Some(lock)) = (changed, &self.lock) {
                    commands.push(self.read_wallpaper(lock.number));
                }
            }
            Shell::Outputs => self.follow_outputs(),
            Shell::Lock(locked) => {
                if let Some(lock) = &mut self.lock {
                    lock.answer = Some(locked);
                }
            }
            Shell::Consent(event) => commands.extend(consent::heard(self, event)),
            Shell::CapsLock(on) => self.caps_lock = on,
            Shell::Closed(key) if self.outputs.iter().any(|o| o.key == key) => {
                return Err("the server closed a surface of the lock screen".into())
            }
            Shell::Closed(_) => {}
        }
        Ok(())
    }

    /// What the person did on the lock screen or the consent panel.
    fn input(&mut self, now: f64, input: Input, commands: &mut Vec<Command<Message>>) {
        if let Input::DesktopControl(choice) = input {
            consent::answer(self, choice);
            return;
        }
        let Some(lock) = &mut self.lock else {
            return;
        };
        match input {
            Input::Activity => lock.screen.activity(now),
            Input::Edited(typed) => lock.screen.edit_to(now, typed),
            Input::Help => lock.help_open = !lock.help_open,
            Input::Escape => lock.screen.escape(now),
            Input::Submit(typed) => {
                if let Some(password) = lock.screen.submit(now, typed.secret()) {
                    let login = self.user.login.clone();
                    let heard = self.heard.0.clone();
                    // the notices and the outcome go the same way, so they arrive in the order PAM gave them
                    commands.push(cupertino_kit::task::io(
                        auth::ATTEMPT_DEADLINE,
                        move || {
                            let notices = heard.clone();
                            let outcome = auth::authenticate(&login, password, &|text| {
                                drop(notices.try_send(Heard::Notice(text)))
                            });
                            drop(heard.try_send(Heard::Outcome(outcome)));
                        },
                        |()| Message::Done,
                    ));
                }
            }
            // the greeter's and the panels' inputs
            _ => {}
        }
    }

    /// Carry out what the timers ask for and follow the lock.
    fn housekeeping(&mut self, now: f64, commands: &mut Vec<Command<Message>>) -> Result<()> {
        if std::mem::take(&mut self.idled) {
            self.request_lock(now, false, commands);
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
        // the screens show on their wallpaper whole: once it is read and every lock surface's pictures are made, or
        // once the wait for them is over
        let ready =
            lock.wallpaper.is_some() && self.outputs.iter().all(|o| self.pictures.ready(&o.key));
        if lock.shown.is_none() && (ready || now - lock.asked >= WALLPAPER_WAIT) {
            lock.shown = Some(now);
            lock.screen.show_from(now);
        }
        lock.screen.tick(now);
        if lock.screen.phase() == Phase::Unlocked {
            return self.unlock(commands);
        }
        Ok(())
    }

    /// Read the wallpaper of the lock `number` for the session's appearance on the I/O pool: it comes back as
    /// [`Message::Wallpaper`].
    fn read_wallpaper(&self, number: u64) -> Command<Message> {
        let dark = self.dark;
        cupertino_kit::task::io(
            WALLPAPER_DEADLINE,
            move || {
                let settings = cupertino_defaults::compositor::CompositorSettings::path()
                    .and_then(|path| {
                        cupertino_defaults::compositor::CompositorSettings::load(&path, None)
                            .map_err(|e| eprintln!("cupertino-login: {}: {e}", path.display()))
                            .ok()
                    })
                    .map(|(settings, _)| settings)
                    .unwrap_or_default();
                wallpaper::for_session(&settings, dark, |dark| {
                    cupertino_assets::Assets::discover().wallpaper_path(dark)
                })
            },
            move |source| Message::Wallpaper {
                lock: number,
                source,
            },
        )
    }

    /// Lock the session: the session lock is asked for and every output gets its lock surface. `explicit`: the
    /// person asked, so the lock screen hides after 5 s without input instead of 30 s. When the session is locked
    /// already, the field starts empty again (the system is about to sleep, or it was locked again).
    fn request_lock(&mut self, now: f64, explicit: bool, commands: &mut Vec<Command<Message>>) {
        if let Some(lock) = &mut self.lock {
            lock.screen.clear_field();
            return;
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
        let settled = cupertino_kit_shell::test_clock::motion_settled();
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
        lock::lock();
        commands.push(self.read_wallpaper(self.locks));
        commands.extend(self.read_notices());
        if let Ok(clock) = screen::clock(self.hours) {
            self.clock = clock;
        }
    }

    fn handle_auth(&mut self, now: f64, outcome: AuthOutcome) -> Result<()> {
        let lock = self
            .lock
            .as_mut()
            .ok_or("an authentication result arrived while the session is not locked")?;
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
        Ok(())
    }

    /// End the lock after an accepted attempt and hold sleep back again. The lock windows go with the next view, and
    /// the session lock with them.
    fn unlock(&mut self, commands: &mut Vec<Command<Message>>) -> Result<()> {
        let lock = self.lock.take().expect("only a locked session unlocks");
        // nothing of this lock's list stays for the next one, and the screens' memory is given back
        self.notices.value.clear();
        self.pictures.forget();
        if lock.answer != Some(true) {
            return Err("the session unlocked before the server confirmed the lock".into());
        }
        lock::unlock();
        eprintln!("cupertino-login: unlocked");
        commands.extend(self.inhibit());
        Ok(())
    }
}
