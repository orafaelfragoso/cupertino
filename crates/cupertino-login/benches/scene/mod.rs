//! The lock screen on the headless window system with the CPU renderer, at the size and scale a bench asks for, on a
//! picture made for it: its first frame, which makes the screen's picture, and the frame of one typed character.

mod fixture;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    Application, Command, Key, PlaceholderIcons, RuntimeOptions, Window, WindowKey,
};
use cupertino_kit::geometry::Size;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, Timestamp, WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_login::model::LockScreen;
use cupertino_login::user::User;
use cupertino_login::view::{
    self, Access, Clock, Field, Input, Metrics, Screen, LOCKED_TITLE, LOCK_APP_ID,
};
use cupertino_login::wallpaper::Source;
use cupertino_ui_test::{ManualClock, TestDriver};
use std::cell::Cell;
use std::num::NonZeroU32;
use std::sync::{Arc, OnceLock};

thread_local! {
    /// The output the next lock screen of this thread fills, in points.
    static OUTPUT: Cell<Size> = const { Cell::new(Size::new(2560.0, 1440.0)) };
}

/// The wallpaper of the bench's lock screens: the fixture picture at 3840 by 2160, made once.
fn wallpaper() -> &'static Source {
    static WALLPAPER: OnceLock<Source> = OnceLock::new();
    WALLPAPER.get_or_init(|| Source::Picture(Arc::new(fixture::fixture_picture(3840, 2160, true))))
}

pub struct Model {
    size: Size,
    screen: LockScreen,
    clock: Clock,
    user: User,
    metrics: Metrics,
}

pub struct Lock;

fn text_system() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    Arc::new(TextSystem::new(fonts, "Inter Variable"))
}

impl Application for Lock {
    type Model = Model;
    type Message = Input;

    fn init() -> (Model, Vec<Command<Input>>) {
        let mut screen = LockScreen::new(0.0, 3600.0);
        screen.settle_interface();
        let model = Model {
            size: OUTPUT.get(),
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
            metrics: Metrics::of(&text_system()),
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
        let s = &model.screen;
        let content = view::screen(
            &Screen {
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
                notices: &[],
                wallpaper: wallpaper(),
                dim: 1.0,
            },
            LOCKED_TITLE,
            |input| input,
        );
        vec![view::full_screen(
            WindowKey(Key::from("lock")),
            LOCK_APP_ID.into(),
            LOCKED_TITLE,
            model.size,
            content,
            None,
        )]
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
        OUTPUT.set(size);
        view::forget_pictures();
        wallpaper();
        let text = text_system();
        let glyphs = Arc::clone(&text);
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let theme = ThemeTokens::cupertino(
            &cupertino_theme::Theme::from_dir(data).unwrap(),
            ThemePreferences {
                appearance: Appearance::Dark,
                reduce_transparency: false,
                reduce_motion: false,
            },
        )
        .unwrap();
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 1000,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 25,
                        glyph_cache_bytes: 1 << 22,
                        image_cache_bytes: 1 << 22,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: None,
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let mut driver = TestDriver::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(turns()).unwrap();
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
