//! A file view in one window on the headless window system, driven by the pointer, the keys and a clock that moves
//! only when a test moves it, as a program's runtime drives it.

use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_geometry::{Point, Rect, Size};
use cupertino_theme::ui::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_ui_core::layout::LayoutDirection;
use cupertino_ui_core::platform::{
    Capabilities, DecorationMode, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp,
    WindowEvent, WindowId, WindowSpec, WindowStates,
};
use cupertino_ui_core::{
    Application, CommandRegistry, Element, Key, PlaceholderIcons, Role, RuntimeOptions, Window,
    WindowBackground, WindowKey,
};
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_test::{ManualClock, TestDriver};
use cupertino_ui_text::{FontIndex, GlyphRaster, TextSystem};

/// The work the runtime may do between two looks at it.
const TURNS: u32 = 1000;

pub const RETURN: Keysym = Keysym(0xff0d);
pub const TAB: Keysym = Keysym(0xff09);
pub const LEFT: Keysym = Keysym(0xff51);
pub const RIGHT: Keysym = Keysym(0xff53);
pub const DOWN: Keysym = Keysym(0xff54);
pub const HOME: Keysym = Keysym(0xff50);
pub const END: Keysym = Keysym(0xff57);

/// Options that draw on the CPU with the built-in font, in the neutral light theme, left to right.
fn options() -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let theme = ThemeTokens::neutral(ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: true,
    })
    .unwrap();
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(theme),
        text,
        icons: Arc::new(PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 10_000,
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
    }
}

/// The one window of a test's application, `size` points large, holding `content`.
pub fn window<M: Clone + 'static>(size: Size, content: Element<M>) -> Window<M> {
    Window {
        key: WindowKey(Key::from("view")),
        spec: WindowSpec {
            title: "View".into(),
            app_id: "org.cupertino.tests.FileView".into(),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Theme,
        popup: None,
        content,
        menus: None,
    }
}

/// Application `A` run on the headless window system, its window `size` points large and active.
pub struct View<A: Application> {
    pub driver: TestDriver<A>,
    pub window: WindowId,
    /// The clock, in nanoseconds.
    now: u64,
}

impl<A: Application> View<A> {
    pub fn open(size: Size) -> View<A> {
        let mut driver = TestDriver::<A>::new(
            Capabilities::default(),
            options(),
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(turns()).unwrap();
        let window = driver.runtime().windows()[0].1;
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(window, Some(size), active)
            .unwrap();
        let mut view = View {
            driver,
            window,
            now: 0,
        };
        view.settle();
        view
    }

    pub fn model(&self) -> &A::Model {
        self.driver.runtime().model()
    }

    /// The bounds of the node named `name`, of role `role` when one is given, the one furthest along when several are.
    pub fn bounds_of(&self, role: Option<Role>, name: &str) -> Option<Rect> {
        let snapshot = self.driver.runtime().snapshot(self.window)?;
        snapshot
            .accessibility
            .iter()
            .filter(|(_, n)| role.is_none_or(|r| n.role == r) && n.name.as_deref() == Some(name))
            .map(|(_, n)| n.bounds)
            .max_by(|a, b| a.min_x().total_cmp(&b.min_x()))
    }

    /// The middle of the node [`View::bounds_of`] finds.
    pub fn center_of(&self, role: Option<Role>, name: &str) -> Option<Point> {
        self.bounds_of(role, name).map(|bounds| bounds.center())
    }

    /// Run the window, handing it the frames it asks for, until it rests.
    pub fn settle(&mut self) {
        for _ in 0..64 {
            self.driver.run_until_idle(turns()).unwrap();
            let system = self.driver.window_system_mut();
            if !system.frame_requested(self.window).unwrap() {
                return;
            }
            system
                .frame_ready(self.window, Timestamp(self.now))
                .unwrap();
        }
    }

    /// Move the clock on by `seconds`, waking what waits for it.
    pub fn wait(&mut self, seconds: f64) {
        self.now += (seconds * 1e9) as u64;
        self.driver.advance_to(Timestamp(self.now)).unwrap();
        self.settle();
    }

    fn send(&mut self, event: WindowEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(self.window, event)
            .unwrap();
        self.settle();
    }

    pub fn pointer(&mut self, event: PointerEvent) {
        self.send(WindowEvent::Pointer(event, Timestamp(self.now)));
    }

    /// The pointer at `at`.
    pub fn move_to(&mut self, at: Point) {
        self.pointer(PointerEvent::Motion { position: at });
    }

    /// `button` going down at `at`, with `mods` held, the pointer moved there first.
    pub fn press(&mut self, at: Point, button: MouseButton, mods: Mods) {
        self.move_to(at);
        self.pointer(PointerEvent::Button {
            position: at,
            button,
            pressed: true,
            serial: 1,
            mods,
        });
    }

    /// `button` let go at `at`.
    pub fn release(&mut self, at: Point, button: MouseButton) {
        self.pointer(PointerEvent::Button {
            position: at,
            button,
            pressed: false,
            serial: 1,
            mods: Mods::NONE,
        });
    }

    /// A press of the main button at `at` with `mods` held, and its release.
    pub fn click(&mut self, at: Point, mods: Mods) {
        self.press(at, MouseButton::Left, mods);
        self.release(at, MouseButton::Left);
    }

    /// The key of `keysym`, which types `text`, pressed and let go with `mods` held.
    pub fn key(&mut self, keysym: Keysym, text: Option<char>, mods: Mods) {
        for pressed in [true, false] {
            let key = KeyEvent {
                code: 0,
                keysym,
                text: text.filter(|_| pressed),
                pressed,
                repeat: false,
                mods,
            };
            self.send(WindowEvent::Key(key, Timestamp(self.now)));
        }
    }
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(TURNS).unwrap()
}
