//! The lock screen on Kit's headless window system, drawn as an application's windows are, at the size and scale a
//! bench asks for, on a picture made for it: its first frame, which waits for the screen's pictures made on the CPU
//! pool, and the frame of one typed character.

mod fixture;

use cupertino_kit::core::{Application, Command, Environment, Key, Window, WindowKey};
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{KeyEvent, Keysym, Mods, Timestamp, WindowId};
use cupertino_kit::test::{Capabilities, ManualClock, TestDriver, WindowEvent, WindowStates};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_login::model::LockScreen;
use cupertino_login::user::User;
use cupertino_login::view::{
    self, Access, Clock, Field, Input, Made, Metrics, Screen, ScreenPictures, Shown, Status,
    LOCKED_TITLE, LOCK_APP_ID,
};
use cupertino_login::wallpaper::Source;
use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// How long a bench waits for the screen's pictures, made on the CPU pool.
const PATIENCE: Duration = Duration::from_secs(60);

thread_local! {
    /// The output the next lock screen of this thread fills, in points, and the device scale of its pictures.
    static OUTPUT: Cell<(Size, f64)> = const { Cell::new((Size::new(2560.0, 1440.0), 1.0)) };
    /// The environment the next lock screen of this thread measures with and makes its pictures in.
    static ENVIRONMENT: RefCell<Option<Environment>> = const { RefCell::new(None) };
}

/// The key of the lock screen's window.
fn window_key() -> WindowKey {
    WindowKey(Key::from("lock"))
}

/// What the lock screen hears: what the person does, and the pictures made for it.
#[derive(Clone, Debug)]
pub enum Message {
    Input(Input),
    Made(Made),
}

/// The wallpaper of the bench's lock screens: the fixture picture at 3840 by 2160, made once.
fn wallpaper() -> &'static Source {
    static WALLPAPER: OnceLock<Source> = OnceLock::new();
    WALLPAPER.get_or_init(|| Source::Picture(Arc::new(fixture::fixture_picture(3840, 2160, true))))
}

pub struct Model {
    size: Size,
    scale: f64,
    screen: LockScreen,
    clock: Clock,
    user: User,
    metrics: Metrics,
    pictures: ScreenPictures,
    env: Environment,
}

pub struct Lock;

impl Application for Lock {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let mut screen = LockScreen::new(0.0, 3600.0);
        screen.settle_interface();
        let (size, scale) = OUTPUT.get();
        let env = ENVIRONMENT
            .take()
            .expect("the bench sets the environment first");
        let mut model = Model {
            size,
            scale,
            screen,
            clock: Clock {
                date: "Mon Nov 16".into(),
                time: "10:24".into(),
            },
            user: User {
                login: "ada".into(),
                display: "Ada Lovelace".into(),
                picture: None,
                uid: None,
                logged_in: false,
            },
            metrics: Metrics::of(&env.text),
            pictures: ScreenPictures::new(),
            env,
        };
        let commands = want(&mut model);
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Input(Input::Edited(typed)) => model.screen.edit_to(1.0, typed),
            Message::Input(_) => {}
            Message::Made(made) => return model.pictures.made(&model.env, made, Message::Made),
        }
        want(model)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let content = lock_screen(model, model.pictures.shown(&window_key()), |screen| {
            view::screen(screen, LOCKED_TITLE, Message::Input)
        });
        vec![view::full_screen(
            window_key(),
            LOCK_APP_ID.into(),
            LOCKED_TITLE,
            model.size,
            content,
            None,
        )]
    }
}

/// What the lock screen's pictures are made from, handed to its pictures after each update.
fn want(model: &mut Model) -> Vec<Command<Message>> {
    let scale = model.scale;
    let wants = lock_screen(model, model.pictures.shown(&window_key()), |screen| {
        screen.wants(scale)
    });
    model
        .pictures
        .want(&model.env, vec![(window_key(), wants)], Message::Made)
}

/// The lock screen on `pictures`, given to `f`.
fn lock_screen<R>(model: &Model, pictures: &Shown, f: impl FnOnce(&Screen<'_>) -> R) -> R {
    {
        let s = &model.screen;
        f(&Screen {
            size: model.size,
            metrics: model.metrics,
            clock: &model.clock,
            user: Some(&model.user),
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
            denied_said: false,
            notice: None,
            help: false,
            help_open: false,
            opacity: 1.0,
            block: 1.0,
            shake: 0.0,
            hidden: false,
            lock_message: None,
            back: false,
            locked: false,
            hide_user: false,
            access: Access::default(),
            large_clock: true,
            avatar_from: None,
            power: None,
            status: Status::NONE,
            notices: &[],
            wallpaper: wallpaper(),
            dim: 1.0,
            pictures,
        })
    }
}

pub struct Run {
    driver: TestDriver<Lock>,
    window: WindowId,
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(10_000).unwrap()
}

impl Run {
    /// The lock screen's window on an output of `size` points at `scale`, opened and configured, its first frame not
    /// drawn yet.
    /// The screen's picture is made anew for its first frame; the wallpaper is ready before.
    pub fn open(size: Size, scale: f64) -> Run {
        OUTPUT.set((size, scale));
        wallpaper();
        // the system's fonts and symbols, as an application's, in the theme of the repository's data
        let mut options = cupertino_kit::app::options().unwrap();
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let theme = ThemeTokens::cupertino(
            &cupertino_assets::theme::from_dir(data).unwrap(),
            ThemePreferences {
                appearance: Appearance::Dark,
                reduce_transparency: false,
                reduce_motion: false,
            },
        )
        .unwrap();
        options.theme = Arc::new(theme);
        ENVIRONMENT.set(Some(Environment {
            theme: Arc::clone(&options.theme),
            text: Arc::clone(&options.text),
            direction: options.direction,
            scale,
            icons: Arc::clone(&options.icons),
        }));
        let mut driver = TestDriver::<Lock>::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        // the screen's pictures, made on the CPU pool, before its first frame
        let end = Instant::now() + PATIENCE;
        loop {
            driver.run_until_idle(turns()).unwrap();
            if driver.runtime().model().pictures.ready(&window_key()) {
                break;
            }
            let left = end.saturating_duration_since(Instant::now());
            assert!(
                driver.window_system_mut().wait_for_wake(left),
                "the screen's pictures were not made within {PATIENCE:?}"
            );
        }
        let window = driver.runtime().windows()[0].1;
        let system = driver.window_system_mut();
        system.set_keep_pixels(false);
        system
            .enqueue_window(window, WindowEvent::Scale(scale))
            .unwrap();
        system
            .configure(
                window,
                None,
                WindowStates {
                    activated: true,
                    ..WindowStates::default()
                },
            )
            .unwrap();
        Run { driver, window }
    }

    /// Runs until the window rests: every frame it asks for drawn.
    pub fn settle(&mut self) {
        for _ in 0..64 {
            self.driver.run_until_idle(turns()).unwrap();
            let system = self.driver.window_system_mut();
            if !system.frame_requested(self.window).unwrap() {
                return;
            }
            system.frame_ready(self.window, Timestamp(0)).unwrap();
        }
        panic!("the window keeps asking for frames");
    }

    /// Types `c` into the focused password field, pressed and released.
    pub fn type_char(&mut self, c: char) {
        for pressed in [true, false] {
            self.driver
                .window_system_mut()
                .enqueue_window(
                    self.window,
                    WindowEvent::Key(
                        KeyEvent {
                            code: 0,
                            keysym: Keysym(c as u32),
                            text: pressed.then_some(c),
                            pressed,
                            repeat: false,
                            mods: Mods::NONE,
                        },
                        Timestamp(0),
                    ),
                )
                .unwrap();
        }
    }
}
