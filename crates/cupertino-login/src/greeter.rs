//! The greeter: the login screen of the session manager's greeter session. One full-output layer surface per output with
//! exclusive keyboard (there is no session to lock), the session manager in place of PAM, and three screens: the user
//! picker, the name typed for "Other…", and the password screen of the lock screen's state machine. The manager owns the
//! accounts, the settings and the authentication; this process draws and relays what the user types. It never ends by
//! itself: the server restarts it if it fails.

use crate::screen::{self, FrameKey, Notifier, OutputSurface, PickerKey, Result, Scene};
use cupertino_assets::Assets;
use cupertino_geometry::Point;
use cupertino_login::hint::FailedAttempts;
use cupertino_login::layout::Layout;
use cupertino_login::manager::{Connection, Message, SOCKET_VARIABLE};
use cupertino_login::model::{AuthOutcome, LockScreen, Password, Phase, HIDE_AFTER};
use cupertino_login::picker::{self, Direction, Grid, NameEntry, NameKey, Picker, Tile};
use cupertino_login::power::{Action, Menu, MenuKey};
use cupertino_login::render::Renderer;
use cupertino_login::user::User;
use cupertino_login_ipc::{
    Account, Event as ManagerEvent, Outcome, PowerAction, PowerMenu, PowerRefusal, Request, Secret,
    UserRow, VERSION,
};
use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurfaceConfig, Margin, ShellEvent,
};
use cupertino_theme::Theme;
use cupertino_wayland_client::{
    key, Button, Connection as Wayland, Event, KeyboardEvent, Modifiers, PointerEvent, SurfaceId,
};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// The connection to the manager is tried once a second for this long when the process starts, then the process gives up and the
/// server restarts it (approximate, ours). The manager starts this screen only after it listens, but it may be restarting.
const CONNECT_ATTEMPTS: u32 = 30;
const CONNECT_INTERVAL: Duration = Duration::from_secs(1);

/// How long the manager may take to answer `Hello` with `Welcome` (approximate, ours).
const WELCOME_TIMEOUT: Duration = Duration::from_secs(10);

/// Namespace of the greeter's layer surfaces for the server.
const NAMESPACE: &str = "login-greeter";

/// The screen the greeter shows.
enum Stage {
    /// The tiles of the picker.
    Picking,
    /// A name is typed for the "Other…" tile.
    Naming(NameEntry),
    /// The password of `user`.
    Password { user: User, screen: LockScreen },
}

/// What a frame shows: when it differs from the last frame drawn, redraw.
#[derive(PartialEq)]
enum Drawn {
    Field(FrameKey),
    Picker(PickerKey),
}

struct App {
    wayland: Wayland,
    layer_shell: LayerShell,
    renderer: Renderer,
    connection: Connection,
    /// The password of the attempt that is open, until the manager asks for it.
    pending: Option<Password>,
    messages: Receiver<Message>,
    start: Instant,
    picker: Picker,
    /// Whether the picker ends with "Other…", from the manager's `Welcome`.
    show_other: bool,
    stage: Stage,
    /// The power item; `None` when the preferences hide every item.
    power: Option<Menu>,
    /// The power action an administrator is being asked to authorise on the name and password screens: Restart or
    /// Shut Down chosen while a user session is open.
    authorising: Option<PowerAction>,
    /// The surface of each output.
    surfaces: Vec<OutputSurface>,
    /// The last frame drawn.
    drawn: Option<Drawn>,
    /// `greeter ready` has been logged.
    announced: bool,
    /// Wrong passwords typed at this greeter.
    failed: FailedAttempts,
    /// `RetriesUntilHint`, see [`FailedAttempts::help_shown`].
    retries_until_hint: u32,
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
fn await_welcome(messages: &Receiver<Message>) -> Result<Welcome> {
    let mut greeted = false;
    loop {
        match messages
            .recv_timeout(WELCOME_TIMEOUT)
            .map_err(|_| "the session manager did not answer in time")?
        {
            Message::Event(ManagerEvent::Hello { version }) if version == VERSION && !greeted => {
                greeted = true
            }
            Message::Event(ManagerEvent::Hello { version }) => {
                return Err(format!(
                    "the session manager speaks version {version}, this screen {VERSION}"
                )
                .into())
            }
            Message::Event(ManagerEvent::Welcome {
                users,
                show_other,
                power,
                will_prompt,
                retries_until_hint,
            }) if greeted => {
                return Ok(Welcome {
                    users,
                    show_other,
                    power,
                    will_prompt,
                    retries_until_hint,
                })
            }
            Message::Event(ManagerEvent::Refused { reason }) => {
                return Err(
                    format!("the session manager refused the connection: {reason:?}").into(),
                )
            }
            Message::Event(_) => {
                return Err("the session manager sent a message out of order".into())
            }
            Message::Broken(e) => return Err(e.into()),
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
    let assets = Assets::discover();
    let theme = Theme::load()?;
    let renderer = Renderer::new(theme.tokens.typography(), &assets)?;

    let socket = std::env::var(SOCKET_VARIABLE).map_err(|e| format!("{SOCKET_VARIABLE}: {e}"))?;
    let stream = connect(&socket)?;
    let mut wayland = Wayland::connect()?;
    let layer_shell =
        LayerShell::bind(&mut wayland).ok_or("the server lacks zwlr_layer_shell_v1")?;
    let (sender, messages) = mpsc::channel();
    let notifier = Notifier::new(sender, wayland.waker());
    let connection = Connection::spawn(stream, move |message| notifier.send(message))?;
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

    let start = Instant::now();
    let mut app = App {
        wayland,
        layer_shell,
        renderer,
        connection,
        pending: None,
        messages,
        start,
        picker,
        show_other: welcome.show_other,
        stage: Stage::Picking,
        power,
        authorising: None,
        surfaces: Vec::new(),
        drawn: None,
        announced: false,
        failed: FailedAttempts::default(),
        retries_until_hint: welcome.retries_until_hint,
    };
    app.arrange_surfaces()?;
    loop {
        app.present()?;
        let timeout = app.timeout()?;
        for event in app.wayland.wait(timeout)? {
            app.handle(event)?;
        }
        app.housekeeping()?;
    }
}

impl App {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// How long the loop may sleep: until the password screen next changes by itself or the clock's minute turns.
    fn timeout(&self) -> Result<Option<Duration>> {
        let minute = Duration::from_secs(60 - screen::unix_seconds()?.rem_euclid(60) as u64);
        let by_screen = match &self.stage {
            Stage::Password { screen, .. } => screen.next_wakeup(self.now()),
            Stage::Picking | Stage::Naming(_) => None,
        };
        Ok(Some(by_screen.map_or(minute, |s| {
            Duration::from_secs_f64(s).min(minute)
        })))
    }

    /// The tiles per page, from the first output the server has described.
    fn max_per_page(&self) -> Result<usize> {
        let geometry = self
            .wayland
            .outputs()
            .into_iter()
            .find_map(|o| o.geometry)
            .ok_or("the server has no output")?;
        Ok(picker::max_per_page(
            f64::from(geometry.width),
            f64::from(geometry.height),
        ))
    }

    /// The frame the current stage shows now.
    fn current(&self, now: f64) -> Result<Drawn> {
        let menu = self.menu();
        Ok(match &self.stage {
            Stage::Picking => {
                Drawn::Picker(PickerKey::new(&self.picker, self.max_per_page()?, menu)?)
            }
            Stage::Naming(entry) => Drawn::Field(FrameKey::name(
                entry,
                self.authorising.map(authorisation_notice),
                menu,
            )?),
            Stage::Password { user, screen } => Drawn::Field(FrameKey::new(
                &Scene {
                    screen,
                    caps_lock: self.wayland.modifiers().caps_lock,
                    help: self.failed.help_shown(&user.login, self.retries_until_hint),
                    power: menu,
                },
                now,
            )?),
        })
    }

    fn menu(&self) -> Option<&Menu> {
        self.power.as_ref()
    }

    fn handle(&mut self, event: Event) -> Result<()> {
        let now = self.now();
        match event {
            Event::Frame(frame) => {
                if let Some(s) = self.surface_mut(frame.surface) {
                    s.presenter.frame_done();
                }
            }
            Event::Keyboard {
                event:
                    KeyboardEvent::Key {
                        code,
                        pressed: true,
                        mods,
                        character,
                        ..
                    },
                ..
            } => self.press(now, code, mods, character)?,
            Event::Pointer { surface, event } => self.pointer(now, surface, event)?,
            Event::OutputsChanged => self.arrange_surfaces()?,
            Event::Extension(event) => match event.downcast_ref::<ShellEvent>() {
                Some(&ShellEvent::Configure { surface, size }) => {
                    // of a surface destroyed since
                    if let Some(s) = self.surface_mut(surface) {
                        s.configured(size)?;
                    }
                }
                Some(ShellEvent::Closed(_)) => {
                    return Err("the server closed a surface of the greeter".into())
                }
                _ => {}
            },
            // the screens have one fixed look and no popups
            _ => {}
        }
        Ok(())
    }

    fn surface_mut(&mut self, surface: SurfaceId) -> Option<&mut OutputSurface> {
        self.surfaces.iter_mut().find(|s| s.surface() == surface)
    }

    /// Draw every surface that can take a frame.
    fn present(&mut self) -> Result<()> {
        for surface in screen::ready(&self.wayland, &mut self.surfaces)? {
            self.draw(surface)?;
        }
        Ok(())
    }

    /// A key went down on the screen of the current stage.
    fn press(
        &mut self,
        now: f64,
        code: u32,
        mods: Modifiers,
        character: Option<char>,
    ) -> Result<()> {
        if self.power_key(code)? {
            return Ok(());
        }
        let typed = screen::typed_key(code, mods, character);
        match &mut self.stage {
            Stage::Picking => {
                let max_per_page = self.max_per_page()?;
                let picker = &mut self.picker;
                match code {
                    key::LEFT => picker.step(Direction::Left, max_per_page),
                    key::RIGHT => picker.step(Direction::Right, max_per_page),
                    key::UP => picker.step(Direction::Up, max_per_page),
                    key::DOWN => picker.step(Direction::Down, max_per_page),
                    key::ENTER | key::KEYPAD_ENTER => self.choose(now)?,
                    _ => {}
                }
            }
            Stage::Naming(entry) => {
                let Some(typed) = typed else { return Ok(()) };
                match entry.press(typed) {
                    NameKey::Typing => {}
                    NameKey::Submit(name) => {
                        let mut screen = LockScreen::new(now, HIDE_AFTER);
                        if let Some(action) = self.authorising {
                            screen.notify(authorisation_notice(action).to_owned());
                        }
                        self.stage = Stage::Password {
                            user: User::typed(name),
                            screen,
                        };
                    }
                    NameKey::Cancel => {
                        self.authorising = None;
                        self.stage = Stage::Picking;
                    }
                }
            }
            Stage::Password { user, screen } => {
                // Escape goes back to the tiles unless an attempt is on its way to a decision
                let deciding = matches!(
                    screen.phase(),
                    Phase::Authenticating { .. } | Phase::Unlocking | Phase::Unlocked
                );
                if code == key::ESCAPE && !deciding {
                    self.authorising = None;
                    self.stage = Stage::Picking;
                    return Ok(());
                }
                match typed {
                    Some(typed) => {
                        if let Some(password) = screen.press(now, typed) {
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
                    None => screen.activity(now),
                }
            }
        }
        Ok(())
    }

    /// Pointer input: the power item takes the clicks on it and on its open menu; otherwise over the tiles the pointer
    /// selects and a click chooses, and on the other screens it is activity.
    fn pointer(&mut self, now: f64, surface: SurfaceId, event: PointerEvent) -> Result<()> {
        if self.power_pointer(surface, event)? {
            return Ok(());
        }
        match (&mut self.stage, event) {
            (
                Stage::Password { screen, .. },
                PointerEvent::Motion(_) | PointerEvent::Button { .. },
            ) => screen.activity(now),
            (Stage::Picking, PointerEvent::Motion(position)) => {
                if let Some(index) = self.tile_at(surface, position)? {
                    self.picker.select(index);
                }
            }
            (
                Stage::Picking,
                PointerEvent::Button {
                    button: Button::Left,
                    pressed: true,
                    position,
                },
            ) => {
                if let Some(index) = self.tile_at(surface, position)? {
                    self.picker.select(index);
                    self.choose(now)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The layout of the output `surface` covers.
    fn layout_of(&self, surface: SurfaceId) -> Result<Layout> {
        let output = self
            .surfaces
            .iter()
            .find(|s| s.surface() == surface)
            .map(|s| s.output)
            .ok_or("a pointer event on an unknown surface")?;
        let geometry = self
            .wayland
            .outputs()
            .into_iter()
            .find(|o| o.id == output)
            .and_then(|o| o.geometry)
            .ok_or("the output of a surface has no geometry")?;
        Ok(self
            .renderer
            .layout(f64::from(geometry.width), f64::from(geometry.height)))
    }

    /// Pointer input for the power item: a click on the glyph opens or closes the menu, a click on a row does its item,
    /// a click elsewhere closes the menu, and the pointer over a row selects it. Returns whether the event was taken.
    fn power_pointer(&mut self, surface: SurfaceId, event: PointerEvent) -> Result<bool> {
        let Some(menu) = &self.power else {
            return Ok(false);
        };
        let layout = self.layout_of(surface)?;
        let rows = layout.power_menu(menu.items().len()).1;
        let row_at = |position: Point| rows.iter().position(|row| row.contains(position));
        let open = menu.open().is_some();
        let menu = self.power.as_mut().ok_or("the power item is missing")?;
        match event {
            PointerEvent::Motion(position) if open => {
                if let Some(row) = row_at(position) {
                    menu.select(row);
                }
                Ok(true)
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } if open => {
                match row_at(position) {
                    Some(row) => {
                        let action = menu.activate(row);
                        self.perform(action.ok_or("a menu row without an item")?)?;
                    }
                    None => menu.close(),
                }
                Ok(true)
            }
            PointerEvent::Button {
                button: Button::Left,
                pressed: true,
                position,
            } if layout.power_item.contains(position) => {
                menu.toggle();
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// A key while the power item exists: Tab opens or closes the menu, and an open menu takes the arrows, Return and
    /// Escape. Returns whether the key was taken.
    fn power_key(&mut self, code: u32) -> Result<bool> {
        let Some(menu) = &mut self.power else {
            return Ok(false);
        };
        if code == key::TAB {
            menu.toggle();
            return Ok(true);
        }
        if menu.open().is_none() {
            return Ok(false);
        }
        let menu_key = match code {
            key::UP => Some(MenuKey::Up),
            key::DOWN => Some(MenuKey::Down),
            key::ENTER | key::KEYPAD_ENTER => Some(MenuKey::Return),
            key::ESCAPE => Some(MenuKey::Escape),
            _ => None,
        };
        if let Some(action) = menu_key.and_then(|k| menu.press(k)) {
            self.perform(action)?;
        }
        // an open menu keeps the keyboard
        Ok(true)
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
            self.stage = Stage::Naming(NameEntry::default());
            return Ok(());
        }
        self.connection.send(&Request::Power { action })?;
        Ok(())
    }

    /// The index of the tile under `position` (points on `surface`), if there is one.
    fn tile_at(&self, surface: SurfaceId, position: Point) -> Result<Option<usize>> {
        let picker = &self.picker;
        let max_per_page = self.max_per_page()?;
        let page = picker.page(max_per_page);
        let layout = self.layout_of(surface)?;
        let frames = layout.tiles(
            Grid::for_count(max_per_page.min(picker.tiles().len())),
            page.len(),
        );
        Ok(frames
            .iter()
            .position(|frame| frame.contains(position))
            .map(|i| page.start + i))
    }

    /// Act on the selected tile: the password screen of its account, or the name screen of "Other…".
    fn choose(&mut self, now: f64) -> Result<()> {
        self.stage = match &self.picker.selected() {
            Tile::Account(user) => Stage::Password {
                user: user.clone(),
                screen: LockScreen::new(now, HIDE_AFTER),
            },
            Tile::Other => Stage::Naming(NameEntry::default()),
        };
        Ok(())
    }

    /// Take what the session manager said, follow the screen's timers and redraw what changed.
    fn housekeeping(&mut self) -> Result<()> {
        let now = self.now();
        while let Ok(message) = self.messages.try_recv() {
            match message {
                Message::Event(event) => self.manager_event(now, event)?,
                Message::Broken(e) => return Err(e.into()),
            }
        }
        if let Stage::Password { screen, .. } = &mut self.stage {
            screen.tick(now);
        }
        if self.drawn.as_ref() != Some(&self.current(now)?) {
            for s in &mut self.surfaces {
                s.presenter.request_redraw();
            }
        }
        Ok(())
    }

    /// Something the session manager said. Everything but `Reset` concerns the attempt on the password screen.
    fn manager_event(&mut self, now: f64, event: ManagerEvent) -> Result<()> {
        if let ManagerEvent::Reset = event {
            eprintln!("cupertino-login: reset");
            self.pending = None;
            self.authorising = None;
            self.stage = Stage::Picking;
            return Ok(());
        }
        if let ManagerEvent::Users { users, will_prompt } = event {
            // the list changed (a session started or ended): the tiles are built again; a screen being typed on stays
            self.picker = Picker::new(users.iter().map(user_of).collect(), self.show_other)?;
            if let Some(menu) = &mut self.power {
                menu.set_will_prompt(will_prompt);
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
        if let ManagerEvent::Refused { reason } = event {
            return Err(format!("the session manager refused the connection: {reason:?}").into());
        }
        if let ManagerEvent::PowerRefused { reason } = event {
            let text = power_refusal_text(reason);
            eprintln!("cupertino-login: power action refused: {text}");
            if let Stage::Password { screen, .. } = &mut self.stage {
                screen.notify(text.to_owned());
            }
            return Ok(());
        }
        let Stage::Password { user, screen } = &mut self.stage else {
            return Err("the session manager answered while no password was asked".into());
        };
        match event {
            ManagerEvent::Prompt {
                conversation,
                echo,
                text,
            } => {
                if echo {
                    screen.notify(text);
                }
                match self.pending.take() {
                    Some(password) => self.connection.send(&Request::Answer {
                        conversation,
                        secret: Secret::new(password).map_err(|e| e.to_string())?,
                    })?,
                    // the password is spent: a second question is more than this screen can answer
                    None => {
                        self.connection.send(&Request::Cancel {})?;
                        screen.finish_auth(
                            now,
                            AuthOutcome::Error("authentication needs more than a password".into()),
                        )?;
                    }
                }
            }
            ManagerEvent::Notice { text, .. } => screen.notify(text),
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
            ManagerEvent::Result {
                outcome: Outcome::Denied,
            } => {
                self.failed.record(&user.login);
                eprintln!("cupertino-login: authentication failed");
                screen.finish_auth(now, AuthOutcome::Denied)?;
            }
            ManagerEvent::Result { outcome } => {
                let reason = refusal_text(outcome, screen.notice());
                eprintln!("cupertino-login: authentication error: {reason}");
                screen.finish_auth(now, AuthOutcome::Error(reason))?;
            }
            ManagerEvent::SessionStarting { .. } => {}
            ManagerEvent::Hello { .. } | ManagerEvent::Welcome { .. } => {
                return Err("the session manager sent a message out of order".into())
            }
            ManagerEvent::Reset
            | ManagerEvent::Users { .. }
            | ManagerEvent::Refused { .. }
            | ManagerEvent::PowerRefused { .. } => unreachable!("handled above"),
        }
        Ok(())
    }

    /// Keep exactly one surface per output the server has described.
    fn arrange_surfaces(&mut self) -> Result<()> {
        let layer_shell = &self.layer_shell;
        screen::arrange_surfaces(&mut self.wayland, &mut self.surfaces, |wayland, output| {
            let layer = layer_shell.layer_surface(
                wayland,
                &LayerSurfaceConfig {
                    namespace: NAMESPACE.to_string(),
                    output: Some(output),
                    layer: Layer::Overlay,
                    anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
                    size: (0, 0),
                    margin: Margin::default(),
                    exclusive_zone: -1,
                    keyboard: KeyboardInteractivity::Exclusive,
                },
            )?;
            Ok((layer.surface(), Some(layer)))
        })
    }

    fn draw(&mut self, surface: SurfaceId) -> Result<()> {
        // of a surface destroyed since
        let Some(i) = self.surfaces.iter().position(|s| s.surface() == surface) else {
            return Ok(());
        };
        let drawn = self.current(self.now())?;
        let menu = self.power.as_ref();
        let presenter = &mut self.surfaces[i].presenter;
        match (&drawn, &self.stage) {
            (Drawn::Picker(key), _) => screen::draw_picker(
                &mut self.wayland,
                presenter,
                &mut self.renderer,
                key,
                &self.picker,
                menu,
            )?,
            (Drawn::Field(key), Stage::Password { user, .. }) => screen::draw(
                &mut self.wayland,
                presenter,
                &mut self.renderer,
                key,
                Some(user),
                menu,
            )?,
            (Drawn::Field(key), _) => screen::draw(
                &mut self.wayland,
                presenter,
                &mut self.renderer,
                key,
                None,
                menu,
            )?,
        }
        self.drawn = Some(drawn);
        if !self.announced {
            self.announced = true;
            eprintln!("cupertino-login: greeter ready");
        }
        Ok(())
    }
}
