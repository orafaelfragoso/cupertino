//! Runs an example's application on the headless window system and compares its Inspector dump with a golden. A
//! [`Traced`] application's golden starts with its trace: each input, then each message it caused and the model after
//! that message, in order.

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    AccessibilityAdapter, Application, Command, FrameSnapshot, Key, RuntimeOptions, Window,
};
use cupertino_kit::geometry::Point;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, ScrollSource, Timestamp,
    WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{Inspector, ManualClock, TestDriver};
use std::cell::RefCell;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::sync::Arc;

pub const TAB: u32 = 0xff09;
pub const RETURN: u32 = 0xff0d;
pub const SPACE: u32 = 0x20;
pub const DOWN: u32 = 0xff54;
pub const PAGE_UP: u32 = 0xff55;
pub const END: u32 = 0xff57;
pub const LEFT: u32 = 0xff51;

thread_local! {
    /// The trace of this thread's [`Traced`] application, `None` while none has started.
    static TRACE: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn trace(line: impl FnOnce() -> String) {
    TRACE.with_borrow_mut(|trace| {
        if let Some(trace) = trace {
            trace.push_str(&line());
            trace.push('\n');
        }
    });
}

/// `A`, writing each message it handles and its model after that message to the trace. A key typed into a protected
/// field is traced without its content.
pub struct Traced<A>(PhantomData<A>);

impl<A: Application> Application for Traced<A>
where
    A::Model: Debug,
    A::Message: Debug,
{
    type Model = A::Model;
    type Message = A::Message;

    fn init() -> (A::Model, Vec<Command<A::Message>>) {
        let (model, commands) = A::init();
        TRACE.set(Some(format!("init {model:?}\n")));
        (model, commands)
    }

    fn update(model: &mut A::Model, message: A::Message) -> Vec<Command<A::Message>> {
        let handled = format!("  {message:?}");
        let commands = A::update(model, message);
        trace(|| format!("{handled}\n    = {model:?}"));
        commands
    }

    fn view(model: &A::Model) -> Vec<Window<A::Message>> {
        A::view(model)
    }
}

fn key_name(keysym: u32, text: Option<char>) -> String {
    match (keysym, text) {
        (_, Some(c)) => format!("{c:?}"),
        (TAB, None) => "Tab".into(),
        (RETURN, None) => "Return".into(),
        (DOWN, None) => "Down".into(),
        (PAGE_UP, None) => "Page_Up".into(),
        (END, None) => "End".into(),
        (LEFT, None) => "Left".into(),
        _ => format!("{keysym:#x}"),
    }
}

/// The evdev code of a named key, which the text keymap resolves keys by; 0 for a key that types a character.
fn code(keysym: u32) -> u32 {
    match keysym {
        TAB => 15,
        RETURN => 28,
        SPACE => 57,
        LEFT => 105,
        DOWN => 108,
        PAGE_UP => 104,
        END => 107,
        _ => 0,
    }
}

/// The repository's Cupertino theme in Light, so the dumps never depend on installed theme data.
fn theme() -> ThemeTokens {
    repository_theme(Appearance::Light)
}

/// The repository's Cupertino theme in `appearance`.
pub fn repository_theme(appearance: Appearance) -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
}

pub struct Run<A: Application> {
    pub driver: TestDriver<A>,
    pub window: WindowId,
    /// The driver's clock: the runtime's time.
    pub clock: ManualClock,
}

/// A display frame at 60 Hz, in nanoseconds.
const FRAME_NS: u64 = 16_666_667;

impl<A: Application> Run<A> {
    pub fn new() -> Self {
        Self::with_theme(theme())
    }

    /// The application with `theme` instead of the repository's.
    pub fn with_theme(theme: ThemeTokens) -> Self {
        Self::with(theme, LayoutDirection::Ltr)
    }

    /// The application with `theme`, laid out in `direction`.
    pub fn with(theme: ThemeTokens, direction: LayoutDirection) -> Self {
        Self::build(theme, direction, None)
    }

    /// A run whose runtime publishes to `accessibility` and takes its actions.
    pub fn with_accessibility(accessibility: Option<Box<dyn AccessibilityAdapter>>) -> Self {
        Self::build(theme(), LayoutDirection::Ltr, accessibility)
    }

    fn build(
        theme: ThemeTokens,
        direction: LayoutDirection,
        accessibility: Option<Box<dyn AccessibilityAdapter>>,
    ) -> Self {
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(cupertino_ui_core::PlaceholderIcons::default()),
            direction,
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
            accessibility,
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let clock = ManualClock::new(Timestamp(0));
        let mut driver = TestDriver::new(Capabilities::default(), options, clock.clone());
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
        let mut run = Self {
            driver,
            window,
            clock,
        };
        run.settle();
        run
    }

    /// Steps until the window has drawn everything it asked to draw. A window still asking after 8 frames at the same
    /// time is in a transition (the colours of a window made active again): the clock then moves a display frame per
    /// frame, for up to 2 seconds, until the transition ends.
    pub fn settle(&mut self) {
        for round in 0..8 + 120 {
            self.driver.run_until_idle(budget()).unwrap();
            let system = self.driver.window_system_mut();
            if !system.frame_requested(self.window).unwrap() {
                return;
            }
            if round >= 8 {
                self.clock.advance_by(FRAME_NS);
            }
            let now = cupertino_kit::core::Clock::now(&self.clock);
            self.driver
                .window_system_mut()
                .frame_ready(self.window, now)
                .unwrap();
        }
        panic!("the window keeps asking for frames");
    }

    pub fn send(&mut self, event: WindowEvent) {
        trace(|| format!("> {event:?}"));
        self.deliver(event);
    }

    fn deliver(&mut self, event: WindowEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(self.window, event)
            .unwrap();
        self.settle();
    }

    pub fn key(&mut self, keysym: u32, text: Option<char>) {
        self.chord(keysym, text, Mods::NONE);
    }

    /// Presses and releases a key with `mods` held.
    pub fn chord(&mut self, keysym: u32, text: Option<char>, mods: Mods) {
        let protected = self.driver.runtime().protected_focus(self.window).unwrap();
        trace(|| match (protected, mods) {
            (true, _) => "> key, protected".into(),
            (false, Mods::NONE) => format!("> key {}", key_name(keysym, text)),
            (false, mods) => format!("> key {} {mods:?}", key_name(keysym, text)),
        });
        for pressed in [true, false] {
            self.deliver(WindowEvent::Key(
                KeyEvent {
                    code: code(keysym),
                    keysym: Keysym(keysym),
                    text: text.filter(|_| pressed),
                    pressed,
                    repeat: false,
                    mods,
                },
                Timestamp(0),
            ));
        }
    }

    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(c as u32, Some(c));
        }
    }

    pub fn click(&mut self, key: impl Into<Key>) {
        let key = key.into();
        trace(|| format!("> click {key:?}"));
        let runtime = self.driver.runtime();
        let id = runtime.find(self.window, &key).unwrap();
        let snapshot = runtime.snapshot(self.window).unwrap();
        let rect = snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect;
        let position = Point::new(
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        );
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        self.deliver(pointer(PointerEvent::Motion { position }));
        for pressed in [true, false] {
            self.deliver(pointer(PointerEvent::Button {
                position,
                button: MouseButton::Left,
                pressed,
                serial: 1,
                mods: Mods::NONE,
            }));
        }
    }

    /// One wheel detent's worth of movement, `dy` points down, at `position`.
    pub fn wheel(&mut self, position: Point, dy: f64) {
        trace(|| format!("> wheel {dy} at ({}, {})", position.x, position.y));
        self.deliver(WindowEvent::Pointer(
            PointerEvent::Scroll {
                position,
                delta: Point::new(0.0, dy),
                value120: None,
                source: ScrollSource::Wheel,
                stop: false,
                inverted: (false, false),
            },
            Timestamp(0),
        ));
    }

    pub fn snapshot(&self) -> FrameSnapshot {
        self.driver.runtime().snapshot(self.window).unwrap()
    }

    pub fn model(&self) -> &A::Model {
        self.driver.runtime().model()
    }

    /// The trace so far, then the window's dump.
    pub fn dump(&self) -> String {
        let snapshot = self.driver.runtime().snapshot(self.window).unwrap();
        let dump = Inspector::default().dump(&snapshot);
        match TRACE.with_borrow(Clone::clone) {
            Some(trace) => format!("{trace}\n{dump}"),
            None => dump,
        }
    }

    /// Compares [`Self::dump`] with `tests/goldens/<name>.txt` and returns it.
    pub fn golden(&self, name: &str) -> String {
        if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
            std::fs::write(golden_path(name), self.dump()).unwrap();
        }
        self.compare(name)
            .unwrap_or_else(|difference| panic!("{name} differs from its golden: {difference}"))
    }

    /// [`Self::dump`] if it matches `tests/goldens/<name>.txt`, otherwise the first line where it differs.
    pub fn compare(&self, name: &str) -> Result<String, String> {
        let dump = self.dump();
        let expected = std::fs::read_to_string(golden_path(name)).unwrap();
        match Inspector::first_difference(&expected, &dump) {
            Some(difference) => Err(difference),
            None => Ok(dump),
        }
    }
}

fn golden_path(name: &str) -> String {
    format!("{}/tests/goldens/{name}.txt", env!("CARGO_MANIFEST_DIR"))
}

pub fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}
