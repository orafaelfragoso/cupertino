//! The lock screen and the login screen drawn to PNGs for look checks:
//! `cargo run -p cupertino-login --example login_capture -- SURFACE [flags]` (crates/cupertino-kit/README.md).
//!
//! Surfaces: `lock` (the lock screen), `picker` (the login screen's accounts) and `password` (the login screen asking
//! the chosen account's password). `--field empty|typed|help` shows the password field empty, with a password typed
//! (its dots and the submit button), or after failed attempts (the help button pressed, its line shown). On the picker,
//! `--selection none|shown` shows no selection (as it opens) or the first tile selected (after an arrow key or the
//! pointer), and `--users three|one` three accounts and "Other…", or one account alone. `consent` is the panel that
//! asks whether an app may control the desktop, for an app that names its desktop entry. Each screen
//! stands on a wallpaper made for the capture, lighter for the light appearance; the accounts, the clock and the
//! dimming are fixed, so a command gives the same pixels on every run. The fonts, symbols and theme are the system's,
//! as an application's.

#[path = "../benches/scene/fixture.rs"]
mod fixture;

use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_kit::core::{Application, Command, Environment, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{KeyEvent, Keysym, Mods, SystemAppearance, Timestamp, WindowId};
use cupertino_kit::test::capture::{presented, App, Look, Pixmap, Request, State};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowEvent, WindowStates};
use cupertino_kit::theme::ThemeTokens;
use cupertino_login::model::LockScreen;
use cupertino_login::picker::{self, Picker};
use cupertino_login::power::{Action, Menu};
use cupertino_login::status::{BatteryState, StatusItems, WifiState};

/// The lock message the captures show.
const LOCK_MESSAGE: &str = "If found, please return to the front desk.";
use cupertino_login::user::User;
use cupertino_login::view::{
    self, Access, Clock, ControlRequest, Field, Input, Made, Metrics, PickerScreen, RequestingApp,
    Screen, ScreenPictures, Shown, Status, GREETER_APP_ID, GREETER_TITLE, LOCKED_TITLE,
    LOCK_APP_ID,
};
use cupertino_login::wallpaper::Source;

const TURNS: u32 = 10_000;
const FRAMES: usize = 16;
/// The screens' size in points when `--size` gives none.
const OUTPUT: Size = Size::new(2560.0, 1440.0);
/// What is typed into the password field for `--field typed`.
const TYPED: &str = "secret";
/// The wallpaper's pixels, a common picture's.
const WALLPAPER: (u32, u32) = (3840, 2160);
/// How long a capture waits for its screen's pictures, made on the CPU pool.
const PATIENCE: Duration = Duration::from_secs(60);

/// The key of the capture's window, which its pictures are made for.
fn window_key() -> WindowKey {
    WindowKey(Key::from("capture"))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Surface {
    Lock,
    Picker,
    Password,
    Consent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum FieldState {
    Empty,
    Typed,
    Help,
}

/// What the next window of this thread shows.
#[derive(Clone, Copy)]
struct Shot {
    surface: Surface,
    field: FieldState,
    /// The picker's first tile shows as selected.
    selected: bool,
    /// The picker offers one account alone.
    one_user: bool,
    size: Size,
    /// The device scale the screen's pictures are made at.
    scale: f64,
}

thread_local! {
    static SHOT: Cell<Shot> = const {
        Cell::new(Shot {
            surface: Surface::Lock,
            field: FieldState::Empty,
            selected: false,
            one_user: false,
            size: OUTPUT,
            scale: 1.0,
        })
    };
    /// The wallpaper of the next window of this thread and the environment its metrics and pictures come from.
    static GROUND: RefCell<Option<(Source, Environment)>> = const { RefCell::new(None) };
}

struct Model {
    shot: Shot,
    screen: LockScreen,
    clock: Clock,
    users: Vec<User>,
    picker: Picker,
    /// The login screen's power item, closed.
    power: Option<Menu>,
    metrics: Metrics,
    wallpaper: Source,
    /// The screen's pictures, made on the CPU pool.
    pictures: ScreenPictures,
    env: Environment,
}

/// What the capture's window hears: what the person does, and the pictures made for it.
#[derive(Clone, Debug)]
enum Message {
    Input(Input),
    Made(Made),
}

/// The capture's window, showing [`SHOT`].
struct Showing;

fn user(login: &str, display: &str) -> User {
    User {
        login: login.into(),
        display: display.into(),
        picture: None,
        uid: None,
        logged_in: false,
    }
}

impl Application for Showing {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let shot = SHOT.get();
        let (wallpaper, env) = GROUND.take().expect("the capture sets the wallpaper first");
        let mut screen = LockScreen::new(0.0, 3600.0);
        screen.settle_interface();
        let users = vec![
            user("ada", "Ada Lovelace"),
            user("grace", "Grace Hopper"),
            user("alan", "Alan Turing"),
        ];
        let accounts = if shot.one_user {
            users[..1].to_vec()
        } else {
            users.clone()
        };
        let mut picker = Picker::new(accounts, !shot.one_user).expect("accounts to offer");
        if shot.selected {
            picker.select(0);
        }
        let model = Model {
            shot,
            screen,
            clock: Clock {
                date: "Mon Nov 16".into(),
                time: "10:24".into(),
            },
            users,
            picker,
            power: Menu::new(vec![Action::Sleep, Action::Restart, Action::ShutDown]),
            metrics: Metrics::of(&env.text),
            wallpaper,
            pictures: ScreenPictures::new(),
            env,
        };
        let mut model = model;
        let commands = want(&mut model);
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Input(Input::Edited(typed)) => model.screen.edit_to(1.0, typed),
            Message::Input(_) => {}
            Message::Made(made) => {
                return model.pictures.made(&model.env, made, Message::Made);
            }
        }
        want(model)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let shot = model.shot;
        if shot.surface == Surface::Consent {
            return vec![view::desktop_control(
                window_key(),
                &consent_request(),
                Message::Input,
            )];
        }
        let (title, app_id) = match shot.surface {
            Surface::Lock => (LOCKED_TITLE, LOCK_APP_ID.to_string()),
            _ => (GREETER_TITLE, format!("{GREETER_APP_ID}.capture")),
        };
        let content = shown(
            model,
            model.pictures.shown(&window_key()),
            |picker| view::picker(picker, Message::Input),
            |screen| view::screen(screen, title, Message::Input),
        );
        vec![view::full_screen(
            window_key(),
            app_id,
            title,
            shot.size,
            content,
            None,
        )]
    }
}

/// What the screen's pictures are made from, handed to its pictures after each update.
fn want(model: &mut Model) -> Vec<Command<Message>> {
    if model.shot.surface == Surface::Consent {
        return Vec::new();
    }
    let scale = model.shot.scale;
    let wants = shown(
        model,
        model.pictures.shown(&window_key()),
        |picker| picker.wants(scale),
        |screen| screen.wants(scale),
    );
    model
        .pictures
        .want(&model.env, vec![(window_key(), wants)], Message::Made)
}

/// What the capture's screen shows on `pictures`, given to `picker` or `screen` by its surface.
fn shown<R>(
    model: &Model,
    pictures: &Shown,
    picker: impl FnOnce(&PickerScreen<'_>) -> R,
    screen: impl FnOnce(&Screen<'_>) -> R,
) -> R {
    {
        let shot = model.shot;
        let size = shot.size;
        let help = shot.field == FieldState::Help;
        let s = &model.screen;
        // the lock screen has no power item
        let power = model
            .power
            .as_ref()
            .filter(|_| shot.surface != Surface::Lock);
        // a battery, a joined network and the input source in the status bar; the login screen's accessibility item
        let items = StatusItems {
            wifi: Some(WifiState::Connected { bars: 3 }),
            battery: Some(BatteryState {
                percentage: 76.0,
                charging: false,
            }),
            input: Some("US".to_owned()),
            host: None,
        };
        let status = Status {
            items: &items,
            access: (shot.surface != Surface::Lock).then_some(false),
        };
        match shot.surface {
            Surface::Picker => picker(&PickerScreen {
                size,
                metrics: model.metrics,
                clock: &model.clock,
                picker: &model.picker,
                max_per_page: picker::max_per_page(size.width, size.height),
                rtl: false,
                large_clock: true,
                avatar_from: None,
                power,
                status,
                opacity: 1.0,
                block: 1.0,
                wallpaper: &model.wallpaper,
                dim: 1.0,
                pictures,
            }),
            Surface::Lock | Surface::Password | Surface::Consent => screen(&Screen {
                size,
                metrics: model.metrics,
                clock: &model.clock,
                user: model.users.first(),
                field: Field::Password {
                    generation: s.field(),
                    enabled: true,
                    busy: false,
                },
                name: None,
                typed: s.typed(),
                caps_lock: false,
                spinner: None,
                denied: false,
                denied_said: help,
                notice: None,
                help,
                help_open: help,
                opacity: 1.0,
                block: 1.0,
                shake: 0.0,
                hidden: false,
                lock_message: Some(LOCK_MESSAGE),
                back: shot.surface == Surface::Password,
                locked: false,
                hide_user: false,
                access: Access::default(),
                large_clock: true,
                avatar_from: None,
                power,
                status,
                notices: &[],
                wallpaper: &model.wallpaper,
                dim: 1.0,
                pictures,
            }),
        }
    }
}

/// What the consent capture asks: an app that names its desktop entry asks to observe the desktop and to use the
/// pointer and the keyboard, its answer to be remembered.
fn consent_request() -> ControlRequest {
    ControlRequest {
        output: "DP-1".into(),
        capabilities: 1 | 4 | 8,
        app: Some(RequestingApp {
            name: "Terminal".into(),
            identity: "org.example.Terminal".into(),
            desktop: true,
            services: 1,
            remembered: true,
            icon: Vec::new(),
        }),
    }
}

struct Login {
    /// The wallpapers made so far, light and dark.
    wallpapers: [Option<Source>; 2],
}

fn state(request: &Request<'_>) -> Result<FieldState, String> {
    match request.state.iter().find(|(flag, _)| flag == "field") {
        None => Ok(FieldState::Empty),
        Some((_, value)) => match value.as_str() {
            "empty" => Ok(FieldState::Empty),
            "typed" => Ok(FieldState::Typed),
            "help" => Ok(FieldState::Help),
            other => Err(format!("--field {other}: empty, typed or help")),
        },
    }
}

impl App for Login {
    const NAME: &'static str = "login";
    const STATE_FLAGS: &'static [&'static str] = &["field", "selection", "users"];

    fn surfaces(&self) -> Vec<String> {
        ["lock", "picker", "password", "consent"]
            .map(String::from)
            .to_vec()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        if surface == "consent" {
            return vec![State::new()];
        }
        if surface == "picker" {
            return ["none", "shown"]
                .map(|v| vec![("selection".to_string(), v.to_string())])
                .to_vec();
        }
        ["empty", "typed", "help"]
            .map(|v| vec![("field".to_string(), v.to_string())])
            .to_vec()
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let surface = match request.surface {
            "lock" => Surface::Lock,
            "picker" => Surface::Picker,
            "consent" => Surface::Consent,
            _ => Surface::Password,
        };
        if surface == Surface::Consent && !request.state.is_empty() {
            return Err("login/consent has no states".into());
        }
        let field = state(request)?;
        if surface == Surface::Picker && field != FieldState::Empty {
            return Err("login/picker has no password field".into());
        }
        let picked = |flag: &str, values: [&str; 2]| -> Result<bool, String> {
            match request.state.iter().find(|(f, _)| f == flag) {
                None => Ok(false),
                Some(_) if surface != Surface::Picker => {
                    Err(format!("--{flag} is a state of login/picker"))
                }
                Some((_, v)) if v == values[0] => Ok(false),
                Some((_, v)) if v == values[1] => Ok(true),
                Some((_, v)) => Err(format!("--{flag} {v}: {} or {}", values[0], values[1])),
            }
        };
        let selected = picked("selection", ["none", "shown"])?;
        let one_user = picked("users", ["three", "one"])?;
        let size = request.size.map_or(OUTPUT, |(w, h)| Size::new(w, h));
        let dark = request.look.dark;
        let wallpaper = self.wallpapers[usize::from(dark)]
            .get_or_insert_with(|| {
                Source::Picture(Arc::new(fixture::fixture_picture(
                    WALLPAPER.0,
                    WALLPAPER.1,
                    dark,
                )))
            })
            .clone();
        SHOT.set(Shot {
            surface,
            field,
            selected,
            one_user,
            size,
            scale: f64::from(request.scale),
        });
        draw(request.look, request.scale, wallpaper, field)
    }
}

// a capture loads the theme of the appearance it draws, outside any UI loop
#[allow(clippy::disallowed_methods)]
fn draw(look: Look, scale: f32, wallpaper: Source, field: FieldState) -> Result<Pixmap, String> {
    let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
    let preferences = cupertino_kit::test::preferences(&SystemAppearance {
        dark: look.dark,
        increase_contrast: look.increase_contrast,
        reduce_transparency: look.reduce_transparency,
        reduce_motion: false,
        steady_caret: true,
    });
    let theme = cupertino_assets::theme::load().map_err(|e| e.to_string())?;
    let tokens = ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?;
    options.theme = Arc::new(tokens);
    let env = Environment {
        theme: Arc::clone(&options.theme),
        text: Arc::clone(&options.text),
        direction: options.direction,
        scale: f64::from(scale),
        icons: Arc::clone(&options.icons),
    };
    GROUND.set(Some((wallpaper, env)));
    let turns = NonZeroU32::new(TURNS).expect("a turn budget");
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let mut driver = TestDriver::<Showing>::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    );
    // the theme of either appearance, as the application's runner makes it: the screens draw in the dark one
    driver
        .runtime_mut()
        .set_theme_for(Box::new(move |appearance: &SystemAppearance| {
            let preferences = cupertino_kit::test::preferences(appearance);
            Arc::new(ThemeTokens::cupertino(&theme, preferences).expect("the theme loaded once"))
        }));
    // the screen's pictures, made on the CPU pool, before its first frame
    let end = Instant::now() + PATIENCE;
    loop {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let model = driver.runtime().model();
        if model.shot.surface == Surface::Consent || model.pictures.ready(&window_key()) {
            break;
        }
        let left = end.saturating_duration_since(Instant::now());
        if !driver.window_system_mut().wait_for_wake(left) {
            return Err("the screen's pictures were not made in time".into());
        }
    }
    let window: WindowId = driver.runtime().windows()[0].1;
    let system = driver.window_system_mut();
    system
        .enqueue_window(window, WindowEvent::Scale(f64::from(scale)))
        .map_err(|e| fail(&e))?;
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    system
        .configure(window, None, active)
        .map_err(|e| fail(&e))?;
    let settle = |driver: &mut TestDriver<Showing>| -> Result<(), String> {
        for _ in 0..FRAMES {
            driver.run_until_idle(turns).map_err(|e| fail(&e))?;
            let system = driver.window_system_mut();
            if !system.frame_requested(window).map_err(|e| fail(&e))? {
                return Ok(());
            }
            system
                .frame_ready(window, Timestamp(0))
                .map_err(|e| fail(&e))?;
        }
        Err("the window keeps asking for frames".into())
    };
    settle(&mut driver)?;
    if field == FieldState::Typed {
        for c in TYPED.chars() {
            for pressed in [true, false] {
                let key = KeyEvent {
                    code: 0,
                    keysym: Keysym(c as u32),
                    text: pressed.then_some(c),
                    pressed,
                    repeat: false,
                    mods: Mods::NONE,
                };
                driver
                    .window_system_mut()
                    .enqueue_window(window, WindowEvent::Key(key, Timestamp(0)))
                    .map_err(|e| fail(&e))?;
            }
        }
        settle(&mut driver)?;
    }
    presented(driver.runtime().window_system(), window)
}

fn main() -> ExitCode {
    cupertino_kit::test::capture::run(|| {
        cupertino_kit::test::capture::assets()?;
        Ok(Login {
            wallpapers: [None, None],
        })
    })
}
