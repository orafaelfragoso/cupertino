//! The login screen of the session manager's greeter session, a CupertinoKit program on `cupertino-kit-shell` (`run`):
//! one full-output layer surface per output with the keyboard (there is no session to lock), the session manager in
//! place of PAM, and three screens ([`view`]): the user picker, the name typed for "Other…", and the password screen
//! of the lock screen's state machine. The manager owns the accounts, the settings and the authentication; this program
//! draws and relays what the user types. It never ends by itself: the server starts it again if it fails.
//!
//! The authentication hand-off is in [`talk`]: the password the state machine hands out on submission goes, zeroed
//! after sending, into the manager's `Answer` for the conversation it asked about; the manager runs PAM (and anything a
//! PAM stack does with the password, such as unlocking a keyring) for the session it starts. Power goes to the manager
//! too (`Request::Power`), which asks logind.
//!
//! Nothing waits on the UI thread: the manager's socket is a future of the loop ([`crate::manager::Pump`]), the status
//! items and the wallpapers are read on Kit's I/O pool.

mod talk;

use crate::hint::FailedAttempts;
use crate::manager::{Connection, SOCKET_VARIABLE};
use crate::model::{LockScreen, Password, Phase, HIDE_AFTER};
use crate::picker::{self, NameEntry, Picker, Tile};
use crate::power::Menu;
use crate::screen::{self, Result, Timer};
use crate::status::{self, StatusItems};
use crate::user::User;
use crate::view::{
    self, Access, Clock, Field, Input, Made, Metrics, NameField, PickerScreen, Screen,
    ScreenPictures, Status, Wants, GREETER_APP_ID, GREETER_TITLE,
};
use crate::wallpaper::{self, Pictures, Source, Stamp, Want};
use cupertino_defaults::login::ScreenSettings;
use cupertino_defaults::menubar::HourCycle;
use cupertino_kit::bus::{self as kit_bus, Bus};
use cupertino_kit::controls::FrameClock;
use cupertino_kit::core::{
    Application, Command, Element, Environment, Key, RendererClass, Window, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit_shell::outputs::{self, OutputId};
use cupertino_kit_shell::{
    Anchor, Event, Keyboard, Layer, LayerSpec, Margin, Program, Role, Surface,
};
use cupertino_login_ipc::{Event as ManagerEvent, PowerAction};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The connection to the manager is tried once a second for this long when the program starts, then it gives up and
/// the server starts it again (approximate, ours). The manager starts this screen only after it listens, but it may be
/// restarting.
const CONNECT_ATTEMPTS: u32 = 30;
const CONNECT_INTERVAL: Duration = Duration::from_secs(1);

/// How long the manager may take to answer `Hello` with `Welcome` (approximate, ours).
const WELCOME_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest the screens wait for the first user's wallpaper before they show on the fallback colour, seconds after
/// the manager's `Welcome` (approximate, ours): until then nothing is shown, and the display stays as black as the
/// server starts it.
const WALLPAPER_WAIT: f64 = 1.0;

/// How long reading a user's wallpaper should take on the I/O pool: a picture of up to the manager's limit
/// (`MAX_WALLPAPER_BYTES`) on a local disk, decoded.
const WALLPAPER_DEADLINE: Duration = Duration::from_secs(10);

/// How long looking at a wallpaper file should take on the I/O pool: its metadata.
const STAMP_DEADLINE: Duration = Duration::from_secs(2);

/// The layer namespace of the screens.
const NAMESPACE: &str = "login-greeter";

/// The screen the greeter shows.
enum Stage {
    /// The tiles of the picker.
    Picking,
    /// The password of `user`; with `name`, the name is typed above it (the "Other…" tile, `show-full-name`, an
    /// administrator's authorisation) and `user` is who it names at the next attempt.
    Password {
        user: User,
        screen: Box<LockScreen>,
        name: Option<NameEntry>,
    },
}

/// What the connection to the manager and the status items tell the greeter.
#[derive(Clone, Debug)]
pub enum Heard {
    /// The connection to the manager is up; its events follow.
    Connected(Connection),
    Manager(Arc<ManagerEvent>),
    /// A reading of the status items is out of date.
    StatusDue,
}

#[derive(Clone, Debug)]
pub enum Message {
    Heard(Heard),
    /// The connection to the manager could not be made, or broke.
    Broken(String),
    /// The manager did not answer `Hello` in time, unless it has.
    WelcomeTimeout,
    System(Option<zbus::Connection>),
    Settings(ScreenSettings),
    /// What a user's wallpaper file is now: `None` when there is none.
    Stamped(PathBuf, Option<Stamp>),
    /// A user's wallpaper was read.
    Wallpaper(
        PathBuf,
        Stamp,
        Arc<std::result::Result<wallpaper::Picture, String>>,
    ),
    Status(StatusItems),
    Outputs,
    CapsLock(bool),
    Closed(WindowKey),
    Input(Input),
    Timer(u64),
    /// The compositor gave a window a frame while something moves by itself (the dimming coming in, the switch between
    /// the screens, a fade, the shake or the spinner): the next step is drawn.
    Frame,
    /// A picture of the screens was made on the CPU pool.
    Made(Made),
    /// A task that has nothing to report ended.
    Done,
}

/// One output and the screen it shows.
#[derive(Clone, Debug)]
struct GreeterOutput {
    id: OutputId,
    key: WindowKey,
    number: u64,
    size: Size,
    /// Device pixels per point.
    scale: f64,
}

/// What a screen of the greeter shows: the picker or the password screen.
enum Shows<'a> {
    Picker(PickerScreen<'a>),
    Password(Screen<'a>),
}

/// The login screen program.
pub struct Greeter;

/// What the login screen holds.
pub struct Model {
    heard: (async_channel::Sender<Heard>, async_channel::Receiver<Heard>),
    connection: Option<Connection>,
    system: Option<zbus::Connection>,
    /// The manager's `Hello` came; its `Welcome` has not yet.
    greeted: bool,
    /// When the manager's `Welcome` came: the screens may show from then on.
    welcomed: Option<f64>,
    /// `login.conf` was read.
    configured: bool,
    /// The password of the attempt that is open, until the manager asks for it.
    pending: Option<Password>,
    start: Instant,
    timer: Timer,
    outputs: Vec<GreeterOutput>,
    next_output: u64,
    caps_lock: bool,
    env: Environment,
    metrics: Metrics,
    /// The pictures the screens stand on, made on the CPU pool.
    screen_pictures: ScreenPictures,
    clock: Clock,
    picker: Picker,
    /// Whether the picker ends with "Other…", from the manager's `Welcome`.
    show_other: bool,
    stage: Stage,
    /// The power item; `None` when the preferences hide every item.
    power: Option<Menu>,
    /// The power action an administrator is being asked to authorise on the name and password screens: Restart or
    /// Shut Down chosen while a user session is open.
    authorising: Option<PowerAction>,
    /// `greeter ready` has been logged.
    announced: Cell<bool>,
    /// Wrong passwords typed at this greeter.
    failed: FailedAttempts,
    /// `RetriesUntilHint`, see [`FailedAttempts::help_shown`].
    retries_until_hint: u32,
    /// The password help is open.
    help_open: bool,
    /// The users' wallpapers by user id, as the manager names them, those read so far, and what the screens show.
    wallpapers: HashMap<u32, PathBuf>,
    pictures: Pictures,
    wallpaper: Source,
    /// The wallpaper file the screens want, and what it was when last looked at (`None` inside: no such file).
    wanted: Option<(PathBuf, Option<Option<Stamp>>)>,
    /// The screens show nothing yet: the manager has not welcomed the screen, or the first user's wallpaper is being
    /// read, for at most [`WALLPAPER_WAIT`].
    waiting: bool,
    /// When the screens first showed (their dimming comes in), and when the picker and the password screen last
    /// switched; `None`: at once, for a fixed test clock.
    shown: Option<f64>,
    switched: Option<f64>,
    /// What `login.conf` says the screens show.
    settings: ScreenSettings,
    /// The display options chosen on the accessibility item.
    access: Access,
    status: StatusItems,
    /// A reading of the status items runs, and another is due after it.
    status_reading: (bool, bool),
    /// The accessibility item's menu is open.
    access_open: bool,
    /// Reduce Motion, chosen on the accessibility item: no shake and no movement in the switch.
    reduce_motion: bool,
    /// The screen's language reads right to left.
    rtl: bool,
    /// The account the manager said is locked, while its password screen shows.
    locked: Option<String>,
    /// The clock's hour cycle: the locale's.
    hours: HourCycle,
}

impl Drop for Model {
    fn drop(&mut self) {
        screen::close([self.system.take()]);
    }
}

/// Connect to the session manager and the server and run until the screen fails: it never ends by itself.
pub fn run() -> std::result::Result<(), String> {
    let environment = cupertino_kit::app::environment().map_err(|e| e.to_string())?;
    cupertino_kit_shell::run::<Greeter>(environment).map_err(|e| e.to_string())?;
    screen::ended("the greeter")
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

/// Connect to the manager's socket at `path`, trying once a second for [`CONNECT_ATTEMPTS`] seconds, then hand its
/// events over until it breaks.
fn connect(path: PathBuf, heard: async_channel::Sender<Heard>) -> Command<Message> {
    Command::Perform(Box::pin(async move {
        let mut attempt = 1;
        let (connection, pump) = loop {
            match Connection::connect(&path).await {
                Ok(connected) => break connected,
                Err(e) if attempt >= CONNECT_ATTEMPTS => {
                    return Message::Broken(format!(
                        "connecting to the session manager at {} failed {attempt} times: {e}",
                        path.display()
                    ))
                }
                Err(_) => {
                    attempt += 1;
                    async_io::Timer::after(CONNECT_INTERVAL).await;
                }
            }
        };
        drop(heard.try_send(Heard::Connected(connection)));
        let broke = pump
            .run(|event| drop(heard.try_send(Heard::Manager(Arc::new(event)))))
            .await;
        Message::Broken(broke.to_string())
    }))
}

impl Application for Greeter {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The login screen must always draw, whatever becomes of the GPU, and it is mostly at rest.
    const RENDERER: RendererClass = RendererClass::Cpu;

    fn init() -> (Model, Vec<Command<Message>>) {
        let heard = async_channel::unbounded();
        let env = cupertino_kit_shell::environment();
        let mut model = Model {
            heard,
            connection: None,
            system: None,
            greeted: false,
            welcomed: None,
            configured: false,
            pending: None,
            start: Instant::now(),
            timer: Timer::default(),
            outputs: Vec::new(),
            next_output: 0,
            caps_lock: false,
            metrics: Metrics::of(&env.text),
            env,
            screen_pictures: ScreenPictures::new(),
            clock: screen::clock(crate::user::locale_hour_cycle()).unwrap_or_default(),
            hours: crate::user::locale_hour_cycle(),
            // until the manager's `Welcome` lists the users
            picker: Picker::new(Vec::new(), true).expect("the \"Other…\" tile alone is a picker"),
            show_other: false,
            stage: Stage::Picking,
            power: None,
            authorising: None,
            announced: Cell::new(false),
            failed: FailedAttempts::default(),
            retries_until_hint: 0,
            help_open: false,
            wallpapers: HashMap::new(),
            pictures: Pictures::default(),
            wallpaper: Source::Solid(wallpaper::LOGIN_FALLBACK),
            wanted: None,
            waiting: true,
            shown: (!cupertino_kit_shell::test_clock::motion_settled()).then_some(0.0),
            switched: None,
            settings: ScreenSettings::default(),
            access: Access::default(),
            status: StatusItems::default(),
            status_reading: (false, false),
            access_open: false,
            reduce_motion: false,
            rtl: crate::user::locale_is_rtl(),
            locked: None,
        };
        model.follow_outputs();
        let mut commands = vec![listen(&model.heard.1)];
        match std::env::var_os(SOCKET_VARIABLE) {
            Some(path) => commands.push(connect(PathBuf::from(path), model.heard.0.clone())),
            None => {
                return (
                    model,
                    screen::end(Err(format!("{SOCKET_VARIABLE} is not set"))),
                )
            }
        }
        commands.push(Command::Perform(Box::pin(async {
            async_io::Timer::after(WELCOME_TIMEOUT).await;
            Message::WelcomeTimeout
        })));
        commands.push(cupertino_kit::task::io(
            screen::SETTINGS_DEADLINE,
            ScreenSettings::load,
            Message::Settings,
        ));
        commands.extend(kit_bus::connect(
            Bus::System,
            |c| Message::System(c.ok()),
            Message::System(None),
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
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let now = model.now();
        let mut commands = Vec::new();
        let result = match message {
            Message::Heard(heard) => {
                commands.push(listen(&model.heard.1));
                match heard {
                    Heard::Connected(connection) => {
                        model.connection = Some(connection);
                        Ok(())
                    }
                    Heard::Manager(event) => model.manager_event(now, &event),
                    Heard::StatusDue => {
                        commands.extend(model.read_status());
                        Ok(())
                    }
                }
            }
            Message::Broken(e) => Err(e.into()),
            Message::WelcomeTimeout if model.welcomed.is_none() => {
                Err("the session manager did not answer in time".into())
            }
            Message::WelcomeTimeout => Ok(()),
            Message::System(system) => {
                model.system_connected(system, &mut commands);
                Ok(())
            }
            Message::Settings(settings) => {
                model.configure(now, settings);
                commands.extend(model.read_status());
                Ok(())
            }
            Message::Stamped(path, stamp) => {
                if let Some((wanted, known)) = &mut model.wanted {
                    if *wanted == path {
                        *known = Some(stamp);
                    }
                }
                Ok(())
            }
            Message::Wallpaper(path, stamp, picture) => {
                let picture = Arc::try_unwrap(picture).unwrap_or_else(|shared| match &*shared {
                    Ok(_) => Err("the picture was shared".into()),
                    Err(e) => Err(e.clone()),
                });
                model.pictures.read(path, stamp, picture);
                Ok(())
            }
            Message::Status(items) => {
                model.status = items;
                let again = std::mem::take(&mut model.status_reading.1);
                model.status_reading.0 = false;
                if again {
                    commands.extend(model.read_status());
                }
                Ok(())
            }
            Message::Outputs => {
                model.follow_outputs();
                Ok(())
            }
            Message::CapsLock(on) => {
                model.caps_lock = on;
                Ok(())
            }
            Message::Closed(key) if model.outputs.iter().any(|o| o.key == key) => {
                Err("the server closed a surface of the greeter".into())
            }
            Message::Made(made) => {
                let env = model.env.clone();
                commands.extend(model.screen_pictures.made(&env, made, Message::Made));
                Ok(())
            }
            Message::Closed(_) | Message::Frame | Message::Done => Ok(()),
            Message::Input(input) => model.input(now, input, &mut commands),
            Message::Timer(id) => {
                model.timer.fired(id);
                Ok(())
            }
        };
        let result = result.and_then(|()| {
            if let Stage::Password { screen, .. } = &mut model.stage {
                screen.tick(now);
            }
            model.clock = screen::clock(model.hours)?;
            commands.extend(model.follow_wallpaper());
            if model.waiting && model.welcomed.is_some_and(|at| now - at >= WALLPAPER_WAIT) {
                eprintln!("cupertino-login: the wallpaper was not read in time; the screens show without it");
                model.appear(now);
            }
            Ok(())
        });
        match result {
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
        // no surface until the first wallpaper is in: the screens appear on it whole
        if model.waiting {
            return Vec::new();
        }
        let now = model.now();
        let mut windows = Vec::new();
        // what moves by itself steps once per frame the compositor gives the screens, not on a timer between them
        let on_frame = model.animating(now).then_some(Message::Frame);
        for output in &model.outputs {
            let content = Element::from(FrameClock {
                content: model.screen_view(output, now),
                on_frame: on_frame.clone(),
                on_layers_settled: None,
            });
            windows.push(view::full_screen(
                output.key.clone(),
                format!("{GREETER_APP_ID}.{}", output.number),
                GREETER_TITLE,
                output.size,
                content,
                None,
            ));
            if let Some(menu) = &model.power {
                windows.extend(view::power_menu(output.key.clone(), menu, Message::Input));
            }
            windows.extend(view::access_menu(
                output.key.clone(),
                model.access_open,
                model.access,
                model.reduce_motion,
                Message::Input,
            ));
        }
        if !windows.is_empty() && !model.announced.replace(true) {
            eprintln!("cupertino-login: greeter ready");
        }
        windows
    }
}

impl Program for Greeter {
    fn surface(model: &Model, window: &WindowKey) -> Surface {
        let role = match model.outputs.iter().find(|o| &o.key == window) {
            Some(output) => Role::Layer(LayerSpec {
                namespace: NAMESPACE,
                output: Some(output.id),
                layer: Layer::Overlay,
                anchor: Anchor {
                    top: true,
                    bottom: true,
                    left: true,
                    right: true,
                },
                size: Size::new(0.0, 0.0),
                margin: Margin::default(),
                exclusive_zone: -1,
                keyboard: Keyboard::Exclusive,
            }),
            // the power and accessibility menus: popups of a screen
            None => Role::Window,
        };
        Surface {
            role,
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(event: Event) -> Option<Message> {
        match event {
            Event::Outputs => Some(Message::Outputs),
            Event::CapsLock(on) => Some(Message::CapsLock(on)),
            Event::Closed(key) => Some(Message::Closed(key)),
            _ => None,
        }
    }
}

impl Model {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
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
            outputs.push(GreeterOutput {
                id: output.id,
                key: WindowKey(Key::from(format!("greeter-{number}"))),
                number,
                size: geometry.size,
                scale: output.scale,
            });
        }
        self.outputs = outputs;
    }

    /// `login.conf` was read: the screens follow it, and with `show-full-name` the name and password screen is the
    /// first one.
    fn configure(&mut self, now: f64, settings: ScreenSettings) {
        let first = !self.configured;
        self.configured = true;
        self.settings = settings;
        if first && self.settings.show_full_name && matches!(self.stage, Stage::Picking) {
            self.stage = self.credentials(now);
        }
    }

    /// The system bus is there, or not: the status items follow the network and the power daemon on it.
    fn system_connected(
        &mut self,
        system: Option<zbus::Connection>,
        commands: &mut Vec<Command<Message>>,
    ) {
        let Some(system) = system else {
            if self.system.take().is_some() {
                eprintln!(
                    "cupertino-login: the system bus closed: the status items stay as they are"
                );
            }
            return;
        };
        self.system = Some(system.clone());
        let heard = self.heard.0.clone();
        commands.push(Command::Perform(Box::pin(async move {
            let why = status::follow(&system, move || drop(heard.try_send(Heard::StatusDue))).await;
            eprintln!("cupertino-login: the status items are no longer followed: {why}");
            Message::Done
        })));
        commands.extend(self.read_status());
    }

    /// Read the status items on the I/O pool, one reading at a time.
    fn read_status(&mut self) -> Option<Command<Message>> {
        if !self.configured {
            return None;
        }
        if self.status_reading.0 {
            self.status_reading.1 = true;
            return None;
        }
        self.status_reading.0 = true;
        let (system, host, input) = (
            self.system.clone(),
            self.settings.host_info,
            self.settings.show_input_menu,
        );
        Some(cupertino_kit::task::io(
            status::READ_DEADLINE,
            move || status::read(system, host, input),
            Message::Status,
        ))
    }

    /// What the stage shows on `output` at `now`.
    fn screen_of<'a>(&'a self, output: &GreeterOutput, now: f64) -> Shows<'a> {
        let size = output.size;
        let pictures = self.screen_pictures.shown(&output.key);
        let dim = self.shown.map_or(1.0, |at| wallpaper::dimming_at(now - at));
        // with Reduce Motion the switch is at once
        let block = self
            .switched
            .filter(|_| !self.reduce_motion)
            .map_or(1.0, |at| picker::switched(now - at));
        // the chosen tile's avatar moves between its row and the password screen during the switch
        let moving = block < 1.0;
        let status = Status {
            items: &self.status,
            access: Some(self.access_open),
        };
        match &self.stage {
            Stage::Picking => Shows::Picker(PickerScreen {
                size,
                metrics: self.metrics,
                clock: &self.clock,
                picker: &self.picker,
                max_per_page: self.max_per_page(),
                rtl: self.rtl,
                large_clock: self.settings.large_clock,
                avatar_from: moving.then(|| {
                    (
                        self.picker.selected_index(),
                        view::user_avatar(size, self.metrics),
                    )
                }),
                power: self.power.as_ref(),
                status,
                opacity: 1.0,
                block,
                wallpaper: &self.wallpaper,
                dim,
                pictures,
            }),
            Stage::Password { user, screen, name } => {
                let help = self.failed.help_shown(&user.login, self.retries_until_hint);
                Shows::Password(Screen {
                    size,
                    metrics: self.metrics,
                    clock: &self.clock,
                    user: name.is_none().then_some(user),
                    field: Field::Password {
                        generation: screen.field(),
                        // nothing is typed while an attempt is checked
                        enabled: !matches!(screen.phase(), Phase::Unlocking | Phase::Unlocked)
                            && !screen.checking(),
                        busy: screen.checking(),
                    },
                    name: name.as_ref().map(|entry| NameField {
                        text: entry.text(),
                        revision: entry.revision(),
                    }),
                    typed: screen.typed(),
                    caps_lock: self.caps_lock,
                    spinner: screen.spinner(now),
                    denied: screen.failure_outline(),
                    denied_said: screen.denied_said(),
                    notice: screen.notice(),
                    help,
                    help_open: self.help_open && help,
                    opacity: screen.opacity(now),
                    block,
                    shake: screen.shake(now),
                    hidden: screen.hidden(),
                    lock_message: self.settings.lock_message.as_deref(),
                    // nothing to go back to where the name and password screen is the first one
                    back: name.is_none()
                        || !self.settings.show_full_name
                        || self.authorising.is_some(),
                    locked: self.locked.as_deref() == Some(user.login.as_str()),
                    hide_user: self.settings.hide_user_avatar_and_name,
                    access: self.access,
                    large_clock: self.settings.large_clock,
                    avatar_from: moving
                        .then(|| {
                            view::tile_avatar(
                                size,
                                self.metrics,
                                &self.picker,
                                self.max_per_page(),
                                self.rtl,
                                self.picker.selected_index(),
                            )
                        })
                        .flatten(),
                    power: self.power.as_ref(),
                    status,
                    notices: &[],
                    wallpaper: &self.wallpaper,
                    dim,
                    pictures,
                })
            }
        }
    }

    /// The screen of the stage on `output` at `now`.
    fn screen_view(&self, output: &GreeterOutput, now: f64) -> Element<Message> {
        match self.screen_of(output, now) {
            Shows::Picker(screen) => view::picker(&screen, Message::Input),
            Shows::Password(screen) => view::screen(&screen, GREETER_TITLE, Message::Input),
        }
    }

    /// Name what every screen shows to the pictures, which make what is missing on the CPU pool; nothing before the
    /// screens show.
    fn want_pictures(&mut self, now: f64) -> Vec<Command<Message>> {
        let screens: Vec<(WindowKey, Wants)> = if self.waiting {
            Vec::new()
        } else {
            self.outputs
                .iter()
                .map(|output| {
                    let wants = match self.screen_of(output, now) {
                        Shows::Picker(screen) => screen.wants(output.scale),
                        Shows::Password(screen) => screen.wants(output.scale),
                    };
                    (output.key.clone(), wants)
                })
                .collect()
        };
        let env = self.env.clone();
        self.screen_pictures.want(&env, screens, Message::Made)
    }

    /// Whether something on the screens moves by itself at `now`, a step each frame: the dimming coming in, the switch
    /// between the screens, and the password screen's fades, shake or spinner.
    fn animating(&self, now: f64) -> bool {
        let dimming = self.shown.is_some_and(|at| now - at < wallpaper::DIM_FADE);
        let switching = self
            .switched
            .is_some_and(|at| now - at < picker::SWITCH_DURATION);
        let moving = match &self.stage {
            Stage::Picking => false,
            Stage::Password { screen, .. } => {
                let opacity = screen.opacity(now);
                (opacity > 0.0 && opacity < 1.0)
                    || matches!(screen.phase(), Phase::Failed { .. } | Phase::Unlocking)
                    || screen.spinner(now).is_some()
            }
        };
        dimming || switching || moving
    }

    /// Seconds until the screen next changes by itself, besides the frames of what moves ([`Self::animating`]): the
    /// password screen's next change, the clock's minute, or the end of the wait for the first wallpaper.
    fn next_wakeup(&self, now: f64) -> Option<f64> {
        let mut wait = screen::to_next_minute().unwrap_or(60.0);
        if let (true, Some(at)) = (self.waiting, self.welcomed) {
            wait = wait.min((at + WALLPAPER_WAIT - now).max(0.0));
        }
        if let Stage::Password { screen, .. } = &self.stage {
            if let Some(next) = screen.next_wakeup(now) {
                wait = wait.min(next);
            }
        }
        Some(wait)
    }

    /// The screens show from `now`, their dimming coming in: the first wallpaper is in, or the wait for it is over.
    fn appear(&mut self, now: f64) {
        self.waiting = false;
        if self.shown.is_some() {
            self.shown = Some(now);
        }
    }

    /// Show the selected user's wallpaper: look at its file on the I/O pool when it is another one than the last (or the
    /// manager listed the users again), read it there when it is not read yet or changed, and keep showing the last one
    /// until it is in. The screens appear once the manager welcomed them, their settings are read and the first
    /// wallpaper is in, or at once when there is none to read.
    fn follow_wallpaper(&mut self) -> Option<Command<Message>> {
        if self.welcomed.is_none() || !self.configured {
            return None;
        }
        let uid = match &self.stage {
            Stage::Picking => match self.picker.selected() {
                Tile::Account(user) => user.uid,
                Tile::Other => None,
            },
            Stage::Password { user, .. } => user.uid,
        };
        let Some(path) = uid.and_then(|uid| self.wallpapers.get(&uid)).cloned() else {
            self.wanted = None;
            let want = self.pictures.want(None, None);
            return self.show(want);
        };
        match &self.wanted {
            Some((wanted, Some(stamp))) if *wanted == path => {
                let want = self.pictures.want(Some(&path), *stamp);
                self.show(want)
            }
            // being looked at
            Some((wanted, None)) if *wanted == path => None,
            _ => {
                self.wanted = Some((path.clone(), None));
                let looked = path.clone();
                Some(cupertino_kit::task::io(
                    STAMP_DEADLINE,
                    move || Stamp::of(&looked),
                    move |stamp| Message::Stamped(path, stamp),
                ))
            }
        }
    }

    /// What the cache says of the wanted wallpaper: shown, or read on the I/O pool.
    fn show(&mut self, want: Want) -> Option<Command<Message>> {
        match want {
            Want::Ready(source) => {
                self.wallpaper = source;
                if self.waiting {
                    let now = self.now();
                    self.appear(now);
                }
                None
            }
            Want::Reading => None,
            Want::Read(path, stamp) => Some(cupertino_kit::task::io(
                WALLPAPER_DEADLINE,
                {
                    let path = path.clone();
                    move || {
                        wallpaper::decode(
                            &path,
                            cupertino_login_ipc::message::MAX_WALLPAPER_BYTES as u64,
                        )
                    }
                },
                move |picture| Message::Wallpaper(path, stamp, Arc::new(picture)),
            )),
        }
    }

    /// The screens switch between the picker and the password screen at `now`.
    fn switch(&mut self, now: f64, to: Stage) {
        let between = matches!(self.stage, Stage::Picking) != matches!(to, Stage::Picking);
        if between && !cupertino_kit_shell::test_clock::motion_settled() {
            self.switched = Some(now);
        }
        self.help_open = false;
        self.stage = to;
    }

    /// The tiles per page, from the first output the server has described.
    fn max_per_page(&self) -> usize {
        self.outputs
            .first()
            .map_or(picker::max_per_page(0.0, 0.0), |output| {
                picker::max_per_page(output.size.width, output.size.height)
            })
    }

    /// Act on the selected tile: the password screen of its account, or the name screen of "Other…".
    fn choose(&mut self, now: f64) {
        let to = match self.picker.selected() {
            Tile::Account(user) => Stage::Password {
                user: user.clone(),
                screen: Box::new(self.password_screen(now)),
                name: None,
            },
            Tile::Other => self.credentials(now),
        };
        self.switch(now, to);
    }

    /// The screen with a name and a password field.
    fn credentials(&self, now: f64) -> Stage {
        Stage::Password {
            user: User::typed(String::new()),
            screen: Box::new(self.password_screen(now)),
            name: Some(NameEntry::default()),
        }
    }

    /// The state of a password screen that starts at `now`. Its interface (the clock, the status bar, the power item)
    /// is shown from the start: the screens it replaces showed it, and the switch fades in only the block that changes.
    fn password_screen(&self, now: f64) -> LockScreen {
        let mut screen = LockScreen::new(now, HIDE_AFTER);
        screen.set_reduce_motion(self.reduce_motion);
        screen.settle_interface();
        screen
    }

    /// The screen the greeter starts on and goes back to: the picker, or the name and password screen with
    /// `show-full-name`.
    fn home(&self, now: f64) -> Stage {
        if self.settings.show_full_name {
            self.credentials(now)
        } else {
            Stage::Picking
        }
    }

    /// The connection to the manager, once it is up.
    fn manager(&self) -> Result<&Connection> {
        self.connection
            .as_ref()
            .ok_or_else(|| "the session manager is not connected".into())
    }
}
