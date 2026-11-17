//! The lock screen and the login screen drawn to PNGs for look checks: `ci/capture.sh login/SURFACE [flags]`
//! (crates/cupertino-capture/README.md).
//!
//! Surfaces: `lock` (the lock screen), `picker` (the login screen's accounts) and `password` (the login screen asking
//! the chosen account's password). `--field empty|typed|help` shows the password field empty, with a password typed
//! (its dots and the submit button), or after failed attempts (the help button pressed, its line shown). Each screen
//! stands on a wallpaper made for the capture, lighter for the light appearance; the accounts, the clock and the
//! dimming are fixed, so a command gives the same pixels on every run. The fonts, symbols and theme are the system's,
//! as an application's.

#[path = "../benches/scene/fixture.rs"]
mod fixture;

use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_capture::{App, Look, Request, State};
use cupertino_kit::core::{Application, Command, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, SystemAppearance, Timestamp, WindowEvent, WindowId,
    WindowStates,
};
use cupertino_kit::render::PixelFormat;
use cupertino_kit::text::TextSystem;
use cupertino_kit::theme::ThemeTokens;
use cupertino_login::model::LockScreen;
use cupertino_login::picker::{self, Picker};
use cupertino_login::power::{Action, Menu};
use cupertino_login::status::{BatteryState, StatusItems, WifiState};

/// The lock message the captures show.
const LOCK_MESSAGE: &str = "If found, please return to the front desk.";
use cupertino_login::user::User;
use cupertino_login::view::{
    self, Access, Clock, Field, Input, Metrics, PickerScreen, Screen, Status, GREETER_APP_ID,
    GREETER_TITLE, LOCKED_TITLE, LOCK_APP_ID,
};
use cupertino_login::wallpaper::Source;
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, TestDriver};
use tiny_skia::{IntSize, Pixmap};

const TURNS: u32 = 10_000;
const FRAMES: usize = 16;
/// The screens' size in points when `--size` gives none.
const OUTPUT: Size = Size::new(2560.0, 1440.0);
/// What is typed into the password field for `--field typed`.
const TYPED: &str = "secret";
/// The wallpaper's pixels, a common picture's.
const WALLPAPER: (u32, u32) = (3840, 2160);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Surface {
    Lock,
    Picker,
    Password,
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
    size: Size,
}

thread_local! {
    static SHOT: Cell<Shot> = const {
        Cell::new(Shot {
            surface: Surface::Lock,
            field: FieldState::Empty,
            size: OUTPUT,
        })
    };
    /// The wallpaper of the next window of this thread and the text system its metrics come from.
    static GROUND: RefCell<Option<(Source, Arc<TextSystem>)>> = const { RefCell::new(None) };
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
}

struct Shown;

fn user(login: &str, display: &str) -> User {
    User {
        login: login.into(),
        display: display.into(),
        picture: None,
        uid: None,
        logged_in: false,
    }
}

impl Application for Shown {
    type Model = Model;
    type Message = Input;

    fn init() -> (Model, Vec<Command<Input>>) {
        let shot = SHOT.get();
        let (wallpaper, text) = GROUND.take().expect("the capture sets the wallpaper first");
        let mut screen = LockScreen::new(0.0, 3600.0);
        screen.settle_interface();
        let users = vec![
            user("ada", "Ada Lovelace"),
            user("grace", "Grace Hopper"),
            user("alan", "Alan Turing"),
        ];
        let picker = Picker::new(users.clone(), true).expect("accounts to offer");
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
            metrics: Metrics::of(&text),
            wallpaper,
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, input: Input) -> Vec<Command<Input>> {
        if let Input::Edited(typed) = input {
            model.screen.edit_to(1.0, typed);
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Input>> {
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
        let (title, app_id) = match shot.surface {
            Surface::Lock => (LOCKED_TITLE, LOCK_APP_ID.to_string()),
            _ => (GREETER_TITLE, format!("{GREETER_APP_ID}.capture")),
        };
        let content = match shot.surface {
            Surface::Picker => view::picker(
                &PickerScreen {
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
                },
                |input| input,
            ),
            Surface::Lock | Surface::Password => view::screen(
                &Screen {
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
                },
                title,
                |input| input,
            ),
        };
        vec![view::full_screen(
            WindowKey(Key::from("capture")),
            app_id,
            title,
            size,
            content,
            None,
        )]
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
    const STATE_FLAGS: &'static [&'static str] = &["field"];

    fn surfaces(&self) -> Vec<String> {
        ["lock", "picker", "password"].map(String::from).to_vec()
    }

    fn sheet_states(&self, surface: &str) -> Vec<State> {
        if surface == "picker" {
            return vec![State::new()];
        }
        ["empty", "typed", "help"]
            .map(|v| vec![("field".to_string(), v.to_string())])
            .to_vec()
    }

    fn capture(&mut self, request: &Request<'_>) -> Result<Pixmap, String> {
        let surface = match request.surface {
            "lock" => Surface::Lock,
            "picker" => Surface::Picker,
            _ => Surface::Password,
        };
        let field = state(request)?;
        if surface == Surface::Picker && field != FieldState::Empty {
            return Err("login/picker has no password field".into());
        }
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
            size,
        });
        draw(request.look, request.scale, wallpaper, field)
    }
}

fn draw(look: Look, scale: f32, wallpaper: Source, field: FieldState) -> Result<Pixmap, String> {
    let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
    let preferences = cupertino_kit::app::preferences(&SystemAppearance {
        dark: look.dark,
        increase_contrast: look.increase_contrast,
        reduce_transparency: look.reduce_transparency,
        reduce_motion: false,
        steady_caret: true,
    });
    let theme = Theme::load().map_err(|e| e.to_string())?;
    let tokens = ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?;
    options.theme = Arc::new(tokens);
    GROUND.set(Some((wallpaper, Arc::clone(&options.text))));
    view::forget_pictures();
    let turns = NonZeroU32::new(TURNS).expect("a turn budget");
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let mut driver = TestDriver::<Shown>::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    );
    // the theme of either appearance, as the application's runner makes it: the screens draw in the dark one
    driver
        .runtime_mut()
        .set_theme_for(Box::new(move |appearance: &SystemAppearance| {
            let preferences = cupertino_kit::app::preferences(appearance);
            Arc::new(ThemeTokens::cupertino(&theme, preferences).expect("the theme loaded once"))
        }));
    driver.run_until_idle(turns).map_err(|e| fail(&e))?;
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
    let settle = |driver: &mut TestDriver<Shown>| -> Result<(), String> {
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
    let system = driver.runtime().window_system();
    let frame = system
        .frames(window)
        .map_err(|e| fail(&e))?
        .last()
        .ok_or("the window presented no frame")?;
    let mut pixels = match system.composited(window) {
        Ok(Some(composited)) => composited,
        _ => frame.pixels.clone(),
    };
    if frame.format == PixelFormat::Rgba8 {
        for px in pixels.as_chunks_mut::<4>().0 {
            let a = u16::from(px[3]);
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    let size = IntSize::from_wh(frame.width, frame.height).ok_or("the frame is empty")?;
    Pixmap::from_vec(pixels, size).ok_or_else(|| "the frame's pixels do not fill it".to_string())
}

fn main() -> ExitCode {
    cupertino_capture::run(|| {
        cupertino_capture::assets()?;
        Ok(Login {
            wallpapers: [None, None],
        })
    })
}
