//! The greeter: the login screen of the session manager's greeter session, a CupertinoKit application. One full-output
//! layer surface window per output with exclusive keyboard (there is no session to lock), the session manager in place
//! of PAM, and three screens ([`view`]): the user picker, the name typed for "Other…", and the password screen of the
//! lock screen's state machine. The manager owns the accounts, the settings and the authentication; this process draws
//! and relays what the user types. It never ends by itself: the server restarts it if it fails.
//!
//! The authentication hand-off is [`GreeterApp::answer`]: the password the state machine hands out on submission goes,
//! zeroed after sending, into the manager's `Answer` for the conversation it asked about; the manager runs PAM (and
//! anything a PAM stack does with the password, such as unlocking a keyring) for the session it starts.

use crate::app::listen;
use crate::roles::{self, Heard, LockWish, LoginRoles};
use crate::screen::{self, Result, Timer};
use cupertino_defaults::login::ScreenSettings;
use cupertino_keyboard::modifiers::Profile;
use cupertino_kit::controls::FrameClock;
use cupertino_kit::core::{Application, Command, Element, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_login::hint::FailedAttempts;
use cupertino_login::manager::{Connection, Message as ManagerMessage, SOCKET_VARIABLE};
use cupertino_login::model::{AuthOutcome, LockScreen, Password, Phase, HIDE_AFTER};
use cupertino_login::picker::{self, NameEntry, Picker, Tile};
use cupertino_login::power::{Action, Menu};
use cupertino_login::status::{self, StatusItems};
use cupertino_login::user::User;
use cupertino_login::view::{
    self, Access, AccessOption, Clock, Field, Input, Metrics, NameField, PickerScreen, Screen,
    Status, GREETER_APP_ID, GREETER_TITLE,
};
use cupertino_login::wallpaper::{self, Pictures, Source, Stamp, Want};
use cupertino_login_ipc::{
    Account, Event as ManagerEvent, Outcome, PowerAction, PowerMenu, PowerRefusal, Request, Secret,
    UserRow, VERSION,
};
use cupertino_shell_client::LayerShell;
use cupertino_ui_wayland::WaylandWindowSystem;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The connection to the manager is tried once a second for this long when the process starts, then the process gives up and the
/// server restarts it (approximate, ours). The manager starts this screen only after it listens, but it may be restarting.
const CONNECT_ATTEMPTS: u32 = 30;
const CONNECT_INTERVAL: Duration = Duration::from_secs(1);

/// How long the manager may take to answer `Hello` with `Welcome` (approximate, ours), and how often the start looks.
const WELCOME_TIMEOUT: Duration = Duration::from_secs(10);
const WELCOME_POLL: Duration = Duration::from_millis(10);

/// The longest the screens wait for the first user's wallpaper before they show on the fallback colour, seconds
/// (approximate, ours): until then nothing is shown, and the display stays as black as the server starts it.
const WALLPAPER_WAIT: f64 = 1.0;

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

#[derive(Clone, Debug)]
pub enum Message {
    Manager(Arc<ManagerEvent>),
    /// A user's wallpaper was read.
    Wallpaper(
        PathBuf,
        Stamp,
        Arc<std::result::Result<wallpaper::Picture, String>>,
    ),
    /// The connection to the manager broke.
    Broken(String),
    /// A reading of the status items.
    Status(StatusItems),
    Heard(Heard),
    Input(Input),
    Timer(u64),
    /// The compositor gave a window a frame while something moves by itself (the dimming coming in, the switch between
    /// the screens, a fade, the shake or the spinner): the next step is drawn.
    Frame,
    /// The server closed a greeter surface.
    Closed,
    Unheard(&'static str),
}

pub struct GreeterApp {
    connection: Connection,
    manager: async_channel::Receiver<Message>,
    heard: async_channel::Receiver<Heard>,
    failure: Rc<RefCell<Option<String>>>,
    /// The password of the attempt that is open, until the manager asks for it.
    pending: Option<Password>,
    start: Instant,
    timer: Timer,
    outputs: Vec<(u32, Size)>,
    caps_lock: bool,
    metrics: Metrics,
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
    /// The screens show nothing yet: the first user's wallpaper is being read, for at most [`WALLPAPER_WAIT`].
    waiting: bool,
    /// For the threads that read a wallpaper.
    sender: async_channel::Sender<Message>,
    /// When the screens first showed (their dimming comes in), and when the picker and the password screen last
    /// switched; `None`: at once, for a fixed test clock.
    shown: Option<f64>,
    switched: Option<f64>,
    /// What `login.conf` says the screens show.
    settings: ScreenSettings,
    /// The display options chosen on the accessibility item.
    access: Access,
    /// The last reading of the status items.
    status: StatusItems,
    /// The accessibility item's menu is open.
    access_open: bool,
    /// Reduce Motion, chosen on the accessibility item: no shake and no movement in the switch.
    reduce_motion: bool,
    /// The screen's language reads right to left.
    rtl: bool,
    /// The account the manager said is locked, while its password screen shows.
    locked: Option<String>,
    /// The clock's hour cycle: the locale's.
    hours: cupertino_defaults::menubar::HourCycle,
}

/// The power item a special name in the name field stands for: `>restart`, `>sleep`, and `>power` or `>shutdown`
/// for shutting down.
fn special_name(name: &str) -> Option<Action> {
    match name {
        ">restart" => Some(Action::Restart),
        ">sleep" => Some(Action::Sleep),
        ">power" | ">shutdown" => Some(Action::ShutDown),
        _ => None,
    }
}

/// The users' wallpapers by user id in `rows`.
fn wallpapers(rows: &[UserRow]) -> HashMap<u32, PathBuf> {
    rows.iter()
        .filter_map(|row| Some((row.uid, PathBuf::from(row.wallpaper.as_ref()?))))
        .collect()
}

/// What the manager's `Welcome` tells the screen.
struct Welcome {
    users: Vec<UserRow>,
    show_other: bool,
    power: PowerMenu,
    will_prompt: bool,
    retries_until_hint: u32,
}

/// Wait for the manager's `Hello` and `Welcome`, the two messages that answer ours.
fn await_welcome(messages: &async_channel::Receiver<Message>) -> Result<Welcome> {
    let deadline = Instant::now() + WELCOME_TIMEOUT;
    let mut greeted = false;
    loop {
        let message = match messages.try_recv() {
            Ok(message) => message,
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(WELCOME_POLL);
                continue;
            }
            Err(async_channel::TryRecvError::Empty) => {
                return Err("the session manager did not answer in time".into())
            }
            Err(async_channel::TryRecvError::Closed) => {
                return Err("the connection to the session manager closed".into())
            }
        };
        let event = match message {
            Message::Manager(event) => event,
            Message::Broken(e) => return Err(e.into()),
            _ => continue,
        };
        match &*event {
            &ManagerEvent::Hello { version } if version == VERSION && !greeted => greeted = true,
            &ManagerEvent::Hello { version } => {
                return Err(format!(
                    "the session manager speaks version {version}, this screen {VERSION}"
                )
                .into())
            }
            ManagerEvent::Welcome {
                users,
                show_other,
                power,
                will_prompt,
                retries_until_hint,
            } if greeted => {
                return Ok(Welcome {
                    users: users.clone(),
                    show_other: *show_other,
                    power: *power,
                    will_prompt: *will_prompt,
                    retries_until_hint: *retries_until_hint,
                })
            }
            ManagerEvent::Refused { reason } => {
                return Err(
                    format!("the session manager refused the connection: {reason:?}").into(),
                )
            }
            _ => return Err("the session manager sent a message out of order".into()),
        }
    }
}

/// Connect to the manager's socket, trying once a second for [`CONNECT_ATTEMPTS`] seconds.
fn connect(socket: &str) -> Result<UnixStream> {
    let mut attempt = 1;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok(stream),
            Err(e) if attempt >= CONNECT_ATTEMPTS => {
                return Err(format!(
                    "connecting to the session manager at {socket} failed {attempt} times: {e}"
                )
                .into())
            }
            Err(_) => {
                attempt += 1;
                std::thread::sleep(CONNECT_INTERVAL);
            }
        }
    }
}

/// The user of a row. A picture that cannot be decoded is left out and logged: the screen shows initials.
fn user_of(row: &UserRow) -> User {
    User::from_row(row).unwrap_or_else(|e| {
        eprintln!("cupertino-login: {e}");
        User {
            login: row.login.clone(),
            display: row.display_name.clone(),
            picture: None,
            uid: Some(row.uid),
            logged_in: row.logged_in,
        }
    })
}

/// The power items the manager says to show, in menu order.
fn power_actions(menu: &PowerMenu) -> Vec<Action> {
    [
        (Action::Restart, menu.restart),
        (Action::ShutDown, menu.shut_down),
        (Action::Sleep, menu.sleep),
    ]
    .into_iter()
    .filter(|&(_, shown)| shown)
    .map(|(action, _)| action)
    .collect()
}

/// The journal line that says which titles the power menu shows (the ellipsis marks an item that asks for an administrator).
fn log_power_items(menu: Option<&Menu>) {
    let items = menu.map_or_else(String::new, |menu| {
        menu.items()
            .iter()
            .map(|action| action.title(menu.will_prompt()))
            .collect::<Vec<_>>()
            .join(", ")
    });
    eprintln!("cupertino-login: power items: {items}");
}

/// What the name and password screens say while an administrator is asked to authorise `action`. approximate, ours.
fn authorisation_notice(action: PowerAction) -> &'static str {
    match action {
        PowerAction::Restart => {
            "Other users are logged in. To restart, enter an administrator's name and password."
        }
        PowerAction::ShutDown => {
            "Other users are logged in. To shut down, enter an administrator's name and password."
        }
        PowerAction::Sleep => unreachable!("sleep asks for no administrator"),
    }
}

/// The notice for a power action the manager did not carry out. The words are approximate, ours.
fn power_refusal_text(reason: PowerRefusal) -> &'static str {
    match reason {
        PowerRefusal::OtherUsersLoggedIn => "Other users are logged in.",
        PowerRefusal::Inhibited => "A program is preventing this.",
        PowerRefusal::Disabled => "This is turned off in the login settings.",
        PowerRefusal::Failed => "This did not work. See the system journal.",
    }
}

/// The notice for an attempt that ended without a wrong password. A module's own text (`pam_faillock` says how long the
/// lock-out lasts) is better than ours when PAM sent one. The words are approximate, ours: the design's are not in the
/// strings read.
fn refusal_text(outcome: Outcome, pam_notice: Option<&str>) -> String {
    let ours = match outcome {
        Outcome::Locked => "This account is locked.",
        Outcome::Expired => "The password for this account has expired.",
        Outcome::Refused => "This account is not allowed to log in.",
        Outcome::Busy => "Too many attempts. Try again in a moment.",
        Outcome::Internal => "Signing in failed. See the system journal.",
        Outcome::Accepted | Outcome::Denied => unreachable!("the callers handle these"),
    };
    match outcome {
        Outcome::Locked | Outcome::Expired | Outcome::Refused => {
            pam_notice.unwrap_or(ours).to_owned()
        }
        _ => ours.to_owned(),
    }
}

/// Connect to the session manager and the server and run: the screen never ends by itself.
pub fn run() -> Result<std::convert::Infallible> {
    let socket = std::env::var(SOCKET_VARIABLE).map_err(|e| format!("{SOCKET_VARIABLE}: {e}"))?;
    let stream = connect(&socket)?;
    let mut system = WaylandWindowSystem::connect_as(Profile::cupertino(), GreeterApp::RENDERER)
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let conn = system.connection();
    let layer_shell = LayerShell::bind(conn).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let outputs = roles::outputs(conn);
    let (sender, messages) = async_channel::unbounded();
    let wallpaper_sender = sender.clone();
    let status_sender = sender.clone();
    let connection = Connection::spawn(stream, move |message| {
        let message = match message {
            ManagerMessage::Event(event) => Message::Manager(Arc::new(event)),
            ManagerMessage::Broken(e) => Message::Broken(e.to_string()),
        };
        // the receiver lives as long as the application
        let _ = sender.send_blocking(message);
    })?;
    let welcome = await_welcome(&messages)?;

    let picker = Picker::new(
        welcome.users.iter().map(user_of).collect(),
        welcome.show_other,
    )?;
    eprintln!(
        "cupertino-login: picker: {} users, other tile: {}, {} logged in",
        welcome.users.len(),
        if welcome.show_other { "yes" } else { "no" },
        welcome.users.iter().filter(|row| row.logged_in).count()
    );
    let mut power = Menu::new(power_actions(&welcome.power));
    if let Some(menu) = &mut power {
        menu.set_will_prompt(welcome.will_prompt);
    }
    log_power_items(power.as_ref());

    let (heard, heard_messages) = async_channel::unbounded();
    // the greeter never locks: there is no session
    let roles = LoginRoles::new(
        layer_shell,
        None,
        None,
        Rc::new(RefCell::new(LockWish::default())),
        heard,
    );
    let system = system.with_roles(roles);
    let failure = Rc::new(RefCell::new(None));
    let ended = Rc::clone(&failure);
    cupertino_kit::app::run_on_with::<GreeterApp>(system, move |runtime| {
        let app = GreeterApp {
            connection,
            manager: messages,
            heard: heard_messages,
            failure: ended,
            pending: None,
            start: Instant::now(),
            timer: Timer::default(),
            outputs,
            caps_lock: false,
            metrics: Metrics::of(&runtime.text),
            clock: screen::clock(cupertino_login::user::locale_hour_cycle()).unwrap_or_default(),
            hours: cupertino_login::user::locale_hour_cycle(),
            picker,
            show_other: welcome.show_other,
            stage: Stage::Picking,
            power,
            authorising: None,
            announced: Cell::new(false),
            failed: FailedAttempts::default(),
            retries_until_hint: welcome.retries_until_hint,
            help_open: false,
            wallpapers: wallpapers(&welcome.users),
            pictures: Pictures::default(),
            wallpaper: Source::Solid(cupertino_login::wallpaper::LOGIN_FALLBACK),
            waiting: true,
            sender: wallpaper_sender,
            shown: (!cupertino_shell_client::test_clock::motion_settled()).then_some(0.0),
            switched: None,
            settings: ScreenSettings::load(),
            access: Access::default(),
            status: StatusItems::default(),
            access_open: false,
            reduce_motion: false,
            rtl: cupertino_login::user::locale_is_rtl(),
            locked: None,
        };
        let mut app = app;
        if app.settings.show_full_name {
            app.stage = app.credentials(0.0);
        }
        // the first user's wallpaper is read before anything shows: the screens appear on it, not on the fallback
        let now = app.now();
        app.follow_wallpaper(now);
        // the status items, read on a thread of their own; the screen goes on without them
        if let Err(e) = status::poll(
            app.settings.host_info,
            app.settings.show_input_menu,
            move |items| status_sender.send_blocking(Message::Status(items)).is_ok(),
        ) {
            eprintln!("cupertino-login: the status items: {e}");
        }
        let mut commands = vec![
            listen(
                &app.manager,
                std::convert::identity,
                Message::Unheard("the session manager's reader"),
            ),
            listen(
                &app.heard,
                Message::Heard,
                Message::Unheard("the surface roles"),
            ),
        ];
        // the end of the wait for the wallpaper, if nothing else comes first
        let wait = app.next_wakeup(now);
        commands.extend(app.timer.schedule(now, wait, Message::Timer));
        (app, commands)
    })?;
    let reason = failure.borrow_mut().take();
    Err(reason
        .unwrap_or_else(|| "the greeter ended".to_owned())
        .into())
}

impl Application for GreeterApp {
    type Model = GreeterApp;
    type Message = Message;
    const RESIDENT: bool = true;
    /// The login screen must always draw, whatever becomes of the GPU, and it is mostly at rest.
    const RENDERER: cupertino_kit::core::RendererClass = cupertino_kit::core::RendererClass::Cpu;

    fn init() -> (GreeterApp, Vec<Command<Message>>) {
        unreachable!("the greeter starts only through `run`, which hands the model to the runtime")
    }

    fn update(app: &mut GreeterApp, message: Message) -> Vec<Command<Message>> {
        let now = app.now();
        let mut commands = Vec::new();
        let result = match message {
            Message::Manager(event) => {
                commands.push(listen(
                    &app.manager,
                    std::convert::identity,
                    Message::Unheard("the session manager's reader"),
                ));
                app.manager_event(now, &event)
            }
            Message::Wallpaper(path, stamp, picture) => {
                // it came on the manager's channel: listen on for what comes next
                commands.push(listen(
                    &app.manager,
                    std::convert::identity,
                    Message::Unheard("the session manager's reader"),
                ));
                let picture = Arc::try_unwrap(picture).unwrap_or_else(|shared| match &*shared {
                    Ok(_) => Err("the picture was shared".into()),
                    Err(e) => Err(e.clone()),
                });
                app.pictures.read(path, stamp, picture);
                Ok(())
            }
            Message::Broken(e) => Err(e.into()),
            Message::Status(items) => {
                commands.push(listen(
                    &app.manager,
                    std::convert::identity,
                    Message::Unheard("the session manager's reader"),
                ));
                app.status = items;
                Ok(())
            }
            Message::Heard(heard) => {
                commands.push(listen(
                    &app.heard,
                    Message::Heard,
                    Message::Unheard("the surface roles"),
                ));
                match heard {
                    Heard::Outputs(outputs) => app.outputs = outputs,
                    Heard::CapsLock(on) => app.caps_lock = on,
                    _ => {}
                }
                Ok(())
            }
            Message::Input(input) => app.input(now, input, &mut commands),
            Message::Timer(id) => {
                app.timer.fired(id);
                Ok(())
            }
            Message::Frame => Ok(()),
            Message::Closed => Err("the server closed a surface of the greeter".into()),
            Message::Unheard(what) => Err(format!("{what} stopped").into()),
        };
        let result = result.and_then(|()| {
            if let Stage::Password { screen, .. } = &mut app.stage {
                screen.tick(now);
            }
            app.clock = screen::clock(app.hours)?;
            app.follow_wallpaper(now);
            if app.waiting && now >= WALLPAPER_WAIT {
                eprintln!("cupertino-login: the wallpaper was not read in time; the screens show without it");
                app.appear(now);
            }
            Ok(())
        });
        match result {
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

    fn view(app: &GreeterApp) -> Vec<Window<Message>> {
        // no surface until the first wallpaper is in: the screens appear on it whole
        if app.waiting {
            return Vec::new();
        }
        let now = app.now();
        let mut windows = Vec::new();
        // what moves by itself steps once per frame the compositor gives the screens, not on a timer between them
        let on_frame = app.animating(now).then_some(Message::Frame);
        let dim = app.shown.map_or(1.0, |at| wallpaper::dimming_at(now - at));
        // with Reduce Motion the switch is at once
        let block = app
            .switched
            .filter(|_| !app.reduce_motion)
            .map_or(1.0, |at| picker::switched(now - at));
        // the chosen tile's avatar moves between its row and the password screen during the switch
        let moving = block < 1.0;
        let tile_avatar = |size| {
            view::tile_avatar(
                size,
                app.metrics,
                &app.picker,
                app.max_per_page(),
                app.rtl,
                app.picker.selected_index(),
            )
        };
        for &(output, size) in &app.outputs {
            let key = WindowKey(Key::from(format!("greeter-{output}")));
            let content = match &app.stage {
                Stage::Picking => view::picker(
                    &PickerScreen {
                        size,
                        metrics: app.metrics,
                        clock: &app.clock,
                        picker: &app.picker,
                        max_per_page: app.max_per_page(),
                        rtl: app.rtl,
                        large_clock: app.settings.large_clock,
                        avatar_from: moving.then(|| {
                            (
                                app.picker.selected_index(),
                                view::user_avatar(size, app.metrics),
                            )
                        }),
                        power: app.power.as_ref(),
                        status: Status {
                            items: &app.status,
                            access: Some(app.access_open),
                        },
                        opacity: 1.0,
                        block,
                        wallpaper: &app.wallpaper,
                        dim,
                    },
                    Message::Input,
                ),
                Stage::Password { user, screen, name } => view::screen(
                    &Screen {
                        size,
                        metrics: app.metrics,
                        clock: &app.clock,
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
                        caps_lock: app.caps_lock,
                        spinner: screen.spinner(now),
                        denied: screen.failure_outline(),
                        denied_said: screen.denied_said(),
                        notice: screen.notice(),
                        help: app.failed.help_shown(&user.login, app.retries_until_hint),
                        help_open: app.help_open
                            && app.failed.help_shown(&user.login, app.retries_until_hint),
                        opacity: screen.opacity(now),
                        block,
                        shake: screen.shake(now),
                        hidden: screen.hidden(),
                        lock_message: app.settings.lock_message.as_deref(),
                        // nothing to go back to where the name and password screen is the first one
                        back: name.is_none()
                            || !app.settings.show_full_name
                            || app.authorising.is_some(),
                        locked: app.locked.as_deref() == Some(user.login.as_str()),
                        hide_user: app.settings.hide_user_avatar_and_name,
                        access: app.access,
                        large_clock: app.settings.large_clock,
                        avatar_from: moving.then(|| tile_avatar(size)).flatten(),
                        power: app.power.as_ref(),
                        status: Status {
                            items: &app.status,
                            access: Some(app.access_open),
                        },
                        notices: &[],
                        wallpaper: &app.wallpaper,
                        dim,
                    },
                    GREETER_TITLE,
                    Message::Input,
                ),
            };
            let content = Element::from(FrameClock {
                content,
                on_frame: on_frame.clone(),
                on_layers_settled: None,
            });
            windows.push(view::full_screen(
                key.clone(),
                format!("{GREETER_APP_ID}.{output}"),
                GREETER_TITLE,
                size,
                content,
                Some(Message::Closed),
            ));
            if let Some(menu) = &app.power {
                windows.extend(view::power_menu(key.clone(), menu, Message::Input));
            }
            windows.extend(view::access_menu(
                key,
                app.access_open,
                app.access,
                app.reduce_motion,
                Message::Input,
            ));
        }
        if !windows.is_empty() && !app.announced.replace(true) {
            eprintln!("cupertino-login: greeter ready");
        }
        windows
    }
}

impl GreeterApp {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
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
        if self.waiting {
            wait = wait.min((WALLPAPER_WAIT - now).max(0.0));
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

    /// Show the selected user's wallpaper: read it on a thread of its own when it is not read yet, and keep showing the
    /// last one until it is in. The screens appear once the first one is in, or at once when there is none to read.
    fn follow_wallpaper(&mut self, now: f64) {
        let uid = match &self.stage {
            Stage::Picking => match self.picker.selected() {
                Tile::Account(user) => user.uid,
                Tile::Other => None,
            },
            Stage::Password { user, .. } => user.uid,
        };
        let path = uid.and_then(|uid| self.wallpapers.get(&uid));
        match self.pictures.want(path.map(PathBuf::as_path)) {
            Want::Ready(source) => {
                self.wallpaper = source;
                if self.waiting {
                    self.appear(now);
                }
            }
            Want::Reading => {}
            Want::Read(path, stamp) => {
                let sender = self.sender.clone();
                let asked = path.clone();
                let spawned = std::thread::Builder::new()
                    .name("login wallpaper".into())
                    .spawn(move || {
                        let picture = wallpaper::decode(
                            &path,
                            cupertino_login_ipc::message::MAX_WALLPAPER_BYTES as u64,
                        );
                        // the receiver lives as long as the application
                        let _ = sender.send_blocking(Message::Wallpaper(
                            path,
                            stamp,
                            Arc::new(picture),
                        ));
                    });
                if let Err(e) = spawned {
                    eprintln!("cupertino-login: reading a wallpaper: {e}");
                    // the read is over: the screen keeps its fallback, and a later change of the file reads it again
                    self.pictures.read(asked, stamp, Err(e.to_string()));
                }
            }
        }
    }

    /// The screens switch between the picker and the password screen at `now`.
    fn switch(&mut self, now: f64, to: Stage) {
        let between = matches!(self.stage, Stage::Picking) != matches!(to, Stage::Picking);
        if between && !cupertino_shell_client::test_clock::motion_settled() {
            self.switched = Some(now);
        }
        self.help_open = false;
        self.stage = to;
    }

    /// The tiles per page, from the first output the server has described.
    fn max_per_page(&self) -> usize {
        self.outputs
            .first()
            .map_or(picker::max_per_page(0.0, 0.0), |(_, size)| {
                picker::max_per_page(size.width, size.height)
            })
    }

    /// What the person did on a screen.
    fn input(
        &mut self,
        now: f64,
        input: Input,
        commands: &mut Vec<Command<Message>>,
    ) -> Result<()> {
        match input {
            Input::TogglePower => {
                if let Some(menu) = &mut self.power {
                    menu.toggle();
                }
                return Ok(());
            }
            Input::ToggleAccess => {
                self.access_open = !self.access_open;
                return Ok(());
            }
            Input::CloseAccess => {
                self.access_open = false;
                return Ok(());
            }
            Input::AccessOption(option) => {
                match option {
                    AccessOption::IncreaseContrast => {
                        self.access.increase_contrast = !self.access.increase_contrast
                    }
                    AccessOption::ReduceTransparency => {
                        self.access.reduce_transparency = !self.access.reduce_transparency
                    }
                    AccessOption::ReduceMotion => self.reduce_motion = !self.reduce_motion,
                }
                self.access_open = false;
                if let Stage::Password { screen, .. } = &mut self.stage {
                    screen.set_reduce_motion(self.reduce_motion);
                }
                return Ok(());
            }
            Input::ClosePower => {
                if let Some(menu) = &mut self.power {
                    menu.close();
                }
                return Ok(());
            }
            Input::Power(row) => {
                let action = self.power.as_mut().and_then(|menu| menu.activate(row));
                return match action {
                    Some(action) => self.perform(action),
                    None => Ok(()),
                };
            }
            _ => {}
        }
        match (&mut self.stage, input) {
            (Stage::Picking, Input::Select(index)) => self.picker.select(index),
            (Stage::Picking, Input::Focused(index)) => self.picker.focus(index),
            (Stage::Picking, Input::Choose(index)) => {
                self.picker.select(index);
                self.choose(now);
            }
            (Stage::Picking, Input::Step(direction)) => {
                let max_per_page = self.max_per_page();
                self.picker.step(direction, max_per_page);
                commands.push(Command::Focus(view::tile_key(self.picker.selected_index())));
            }
            (Stage::Picking, Input::Jump(jump)) => {
                let max_per_page = self.max_per_page();
                self.picker.jump(jump, max_per_page);
                commands.push(Command::Focus(view::tile_key(self.picker.selected_index())));
            }
            // Option+Return: the name and password screen in place of the picker, and back
            (Stage::Picking, Input::ToggleNames) => {
                let to = self.credentials(now);
                self.switch(now, to);
            }
            (Stage::Password { name: Some(_), .. }, Input::ToggleNames) => {
                self.authorising = None;
                self.switch(now, Stage::Picking);
            }
            (
                Stage::Password {
                    name: Some(entry), ..
                },
                Input::Name(change),
            ) => entry.set(&change.text),
            (
                Stage::Password {
                    name: Some(entry),
                    screen,
                    ..
                },
                Input::SubmitName,
            ) => {
                if let Some(action) = special_name(entry.text()) {
                    return self.special(action);
                }
                if !entry.text().is_empty() {
                    commands.push(Command::Focus(Key::from(format!(
                        "password-{}",
                        screen.field()
                    ))));
                }
            }
            (Stage::Password { screen, .. }, Input::Escape) => {
                // Escape goes back to the tiles unless an attempt is on its way to a decision
                let deciding = matches!(
                    screen.phase(),
                    Phase::Authenticating { .. } | Phase::Unlocking | Phase::Unlocked
                );
                if deciding {
                    screen.escape(now);
                } else {
                    self.authorising = None;
                    let home = self.home(now);
                    self.switch(now, home);
                }
            }
            (Stage::Password { screen, .. }, Input::Activity) => screen.activity(now),
            (Stage::Password { screen, .. }, Input::Edited(typed)) => screen.edit_to(now, typed),
            (Stage::Password { .. }, Input::Help) => self.help_open = !self.help_open,
            (Stage::Password { user, screen, name }, Input::Submit(typed)) => {
                // with a name field: a name first, and the special names do what they name
                if let Some(entry) = name {
                    if let Some(action) = special_name(entry.text()) {
                        return self.special(action);
                    }
                    if entry.text().is_empty() {
                        commands.push(Command::Focus(Key::from("name")));
                        return Ok(());
                    }
                    *user = User::typed(entry.text().to_owned());
                }
                if let Some(password) = screen.submit(now, typed.secret()) {
                    let request = match self.authorising {
                        Some(action) => Request::AuthenticatePower {
                            name: user.login.clone(),
                            action,
                        },
                        None => Request::Authenticate {
                            account: match user.uid {
                                Some(uid) => Account::Listed { uid },
                                None => Account::Typed {
                                    name: user.login.clone(),
                                },
                            },
                        },
                    };
                    // the manager asks for the password with a prompt; until then it waits here
                    self.pending = Some(password);
                    self.connection.send(&request)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Carry out a menu item. Restart and Shut Down while a user session is open would end it: they ask for an administrator's
    /// name and password first (the ellipsis of the title), on the name and password screens; the manager checks the
    /// password and then does the action. Any other item goes to the manager at once; a refusal comes back as `PowerRefused`
    /// and the greeter stays up.
    fn perform(&mut self, action: Action) -> Result<()> {
        eprintln!("cupertino-login: power action: {}", action.log_name());
        let action = match action {
            Action::Restart => PowerAction::Restart,
            Action::ShutDown => PowerAction::ShutDown,
            Action::Sleep => PowerAction::Sleep,
        };
        if action != PowerAction::Sleep && self.power.as_ref().is_some_and(Menu::will_prompt) {
            // an attempt that is open (a password being checked) ends: the dialog starts its own
            self.connection.send(&Request::Cancel {})?;
            self.pending = None;
            self.authorising = Some(action);
            self.stage = self.credentials(0.0);
            if let Stage::Password { screen, .. } = &mut self.stage {
                screen.notify(authorisation_notice(action).to_owned());
            }
            return Ok(());
        }
        self.connection.send(&Request::Power { action })?;
        Ok(())
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

    /// A special name typed in the name field: the power item it names, when the menu offers it.
    fn special(&mut self, action: Action) -> Result<()> {
        if self
            .power
            .as_ref()
            .is_some_and(|menu| menu.items().contains(&action))
        {
            return self.perform(action);
        }
        eprintln!(
            "cupertino-login: power action not offered: {}",
            action.log_name()
        );
        Ok(())
    }

    /// The authentication hand-off: answer the manager's prompt with the password of the open attempt. The password is
    /// moved into the request's secret, which is zeroed once the request is written.
    fn answer(&mut self, conversation: u64) -> Result<bool> {
        match self.pending.take() {
            Some(password) => {
                self.connection.send(&Request::Answer {
                    conversation,
                    secret: Secret::new(password).map_err(|e| e.to_string())?,
                })?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Something the session manager said. Everything but `Reset`, `Users` and the refusals concerns the attempt on the
    /// password screen.
    fn manager_event(&mut self, now: f64, event: &ManagerEvent) -> Result<()> {
        match event {
            ManagerEvent::Reset {} => {
                eprintln!("cupertino-login: reset");
                self.pending = None;
                self.authorising = None;
                let home = self.home(now);
                self.switch(now, home);
                return Ok(());
            }
            ManagerEvent::Users { users, will_prompt } => {
                // the list changed (a session started or ended): the tiles are built again; a screen being typed on stays
                self.picker = Picker::new(users.iter().map(user_of).collect(), self.show_other)?;
                self.wallpapers = wallpapers(users);
                if let Some(menu) = &mut self.power {
                    menu.set_will_prompt(*will_prompt);
                }
                log_power_items(self.power.as_ref());
                eprintln!(
                    "cupertino-login: picker: {} users, other tile: {}, {} logged in",
                    users.len(),
                    if self.show_other { "yes" } else { "no" },
                    users.iter().filter(|row| row.logged_in).count()
                );
                return Ok(());
            }
            ManagerEvent::Refused { reason } => {
                return Err(
                    format!("the session manager refused the connection: {reason:?}").into(),
                )
            }
            &ManagerEvent::PowerRefused { reason } => {
                let text = power_refusal_text(reason);
                eprintln!("cupertino-login: power action refused: {text}");
                if let Stage::Password { screen, .. } = &mut self.stage {
                    screen.notify(text.to_owned());
                }
                return Ok(());
            }
            _ => {}
        }
        if !matches!(self.stage, Stage::Password { .. }) {
            return Err("the session manager answered while no password was asked".into());
        }
        match event {
            ManagerEvent::Prompt {
                conversation,
                echo,
                text,
            } => {
                if *echo {
                    if let Stage::Password { screen, .. } = &mut self.stage {
                        screen.notify(text.clone());
                    }
                }
                // the password is spent: a second question is more than this screen can answer
                if !self.answer(*conversation)? {
                    self.connection.send(&Request::Cancel {})?;
                    if let Stage::Password { screen, .. } = &mut self.stage {
                        screen.finish_auth(
                            now,
                            AuthOutcome::Error("authentication needs more than a password".into()),
                        )?;
                    }
                }
            }
            ManagerEvent::Notice { text, .. } => {
                if let Stage::Password { screen, .. } = &mut self.stage {
                    screen.notify(text.clone());
                }
            }
            ManagerEvent::Result {
                outcome: Outcome::Accepted,
            } if self.authorising.is_some() => {
                // the machine goes down, or the manager refuses and shows the screen again (`PowerRefused`, `Reset`)
                eprintln!("cupertino-login: power action authorised");
            }
            ManagerEvent::Result {
                outcome: Outcome::Accepted,
            } => {
                // the spinner goes on until the manager shows the screen again (`Reset`)
                eprintln!("cupertino-login: session starting");
            }
            &ManagerEvent::Result { outcome } => {
                let Stage::Password { user, screen, .. } = &mut self.stage else {
                    unreachable!("checked above");
                };
                if outcome == Outcome::Denied {
                    self.failed.record(&user.login);
                    eprintln!("cupertino-login: authentication failed");
                    screen.finish_auth(now, AuthOutcome::Denied)?;
                } else {
                    if outcome == Outcome::Locked {
                        self.locked = Some(user.login.clone());
                    }
                    let reason = refusal_text(outcome, screen.notice());
                    eprintln!("cupertino-login: authentication error: {reason}");
                    screen.finish_auth(now, AuthOutcome::Error(reason))?;
                }
            }
            ManagerEvent::SessionStarting { .. } => {}
            ManagerEvent::Hello { .. } | ManagerEvent::Welcome { .. } => {
                return Err("the session manager sent a message out of order".into())
            }
            ManagerEvent::Reset {}
            | ManagerEvent::Users { .. }
            | ManagerEvent::Refused { .. }
            | ManagerEvent::PowerRefused { .. } => unreachable!("handled above"),
        }
        Ok(())
    }
}
