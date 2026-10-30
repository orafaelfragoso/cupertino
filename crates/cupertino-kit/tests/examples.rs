//! The examples' applications driven by input on the headless window system, checked against Inspector dumps in
//! `tests/goldens/`. `CUPERTINO_UPDATE_GOLDENS=1` rewrites the dumps.

#[path = "../examples/apps/counter.rs"]
mod counter;
#[path = "../examples/apps/forms.rs"]
mod forms;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{Application, RuntimeOptions};
use cupertino_kit::geometry::Point;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{Inspector, ManualClock, TestDriver};
use std::num::NonZeroU32;
use std::sync::Arc;

const TAB: u32 = 0xff09;
const RETURN: u32 = 0xff0d;
const SPACE: u32 = 0x20;

/// The repository's Cupertino theme in Light, so the dumps never depend on installed theme data.
fn theme() -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
}

struct Run<A: Application> {
    driver: TestDriver<A>,
    window: WindowId,
}

impl<A: Application> Run<A> {
    fn new() -> Self {
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme()),
            text,
            icons: Arc::new(cupertino_ui_core::PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 1000,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 22,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
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
        driver.run_until_idle(budget()).unwrap();
        let window = driver.runtime().windows()[0].1;
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(window, None, active)
            .unwrap();
        let mut run = Self { driver, window };
        run.settle();
        run
    }

    fn settle(&mut self) {
        for _ in 0..8 {
            self.driver.run_until_idle(budget()).unwrap();
            let system = self.driver.window_system_mut();
            if !system.frame_requested(self.window).unwrap() {
                return;
            }
            system.frame_ready(self.window, Timestamp(0)).unwrap();
        }
        panic!("the window keeps asking for frames");
    }

    fn send(&mut self, event: WindowEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(self.window, event)
            .unwrap();
        self.settle();
    }

    fn key(&mut self, keysym: u32, text: Option<char>) {
        for pressed in [true, false] {
            self.send(WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(keysym),
                    text: text.filter(|_| pressed),
                    pressed,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ));
        }
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(c as u32, Some(c));
        }
    }

    fn click(&mut self, key: &'static str) {
        let runtime = self.driver.runtime();
        let id = runtime.find(self.window, &key.into()).unwrap();
        let snapshot = runtime.snapshot(self.window).unwrap();
        let rect = snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect;
        let position = Point::new(
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        );
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        self.send(pointer(PointerEvent::Motion { position }));
        for pressed in [true, false] {
            self.send(pointer(PointerEvent::Button {
                position,
                button: MouseButton::Left,
                pressed,
                serial: 1,
            }));
        }
    }

    fn model(&self) -> &A::Model {
        self.driver.runtime().model()
    }

    /// Compares the window's dump with `tests/goldens/<name>.txt` and returns it.
    fn golden(&self, name: &str) -> String {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let dump = Inspector::default().dump(&snapshot);
        let path = format!("{}/tests/goldens/{name}.txt", env!("CARGO_MANIFEST_DIR"));
        if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
            std::fs::write(&path, &dump).unwrap();
        }
        let expected = std::fs::read_to_string(&path).unwrap();
        if let Some(difference) = Inspector::first_difference(&expected, &dump) {
            panic!("{name} differs from its golden: {difference}");
        }
        dump
    }
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

#[test]
fn the_counter_counts_clicks_and_keys() {
    let mut run = Run::<counter::Counter>::new();
    run.click("more");
    run.click("more");
    run.key(TAB, None);
    run.key(SPACE, Some(' '));
    assert_eq!(*run.model(), 1);
    run.golden("counter");
}

#[test]
fn the_form_takes_a_name_a_password_and_a_choice() {
    let mut run = Run::<forms::Forms>::new();
    run.key(TAB, None);
    run.type_text("Ada");
    run.key(TAB, None);
    run.type_text("hunter2");
    run.key(RETURN, None);
    run.key(TAB, None);
    run.key(SPACE, Some(' '));
    let account = run.model();
    assert_eq!(
        (account.name.as_str(), account.signed_in, account.remember),
        ("Ada", Some(7), true)
    );
    let dump = run.golden("forms");
    assert!(!dump.contains("hunter2"));
}
