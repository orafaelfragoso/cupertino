//! A control in a window on the headless window system, driven by the pointer, the keys and a clock that moves only
//! when a test moves it, as a program's runtime drives it. A test gives the window's content and what its messages
//! do to the state as a [`Scene`].

use std::marker::PhantomData;
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
    AccessibilityNode, Application, Command, CommandRegistry, Element, Key, PlaceholderIcons, Role,
    RuntimeOptions, Window, WindowBackground, WindowKey,
};
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_test::{ManualClock, TestDriver};
use cupertino_ui_text::{FontIndex, GlyphRaster, TextSystem};

/// The work the runtime may do between two looks at it.
const TURNS: u32 = 1000;

pub const RETURN: Keysym = Keysym(0xff0d);
pub const ESCAPE: Keysym = Keysym(0xff1b);
pub const TAB: Keysym = Keysym(0xff09);
pub const BACKSPACE: Keysym = Keysym(0xff08);
pub const HOME: Keysym = Keysym(0xff50);
pub const LEFT: Keysym = Keysym(0xff51);
pub const UP: Keysym = Keysym(0xff52);
pub const RIGHT: Keysym = Keysym(0xff53);
pub const DOWN: Keysym = Keysym(0xff54);
pub const END: Keysym = Keysym(0xff57);
pub const SPACE: Keysym = Keysym(0x20);

/// What a test's window shows, and what its messages do to its state.
pub trait Scene: 'static {
    type State: 'static;
    type Msg: Clone + Send + 'static;

    fn state() -> Self::State;
    fn update(state: &mut Self::State, message: Self::Msg);
    fn view(state: &Self::State) -> Element<Self::Msg>;

    /// The popup windows the scene declares besides its window, such as an open menu.
    fn popups(_: &Self::State) -> Vec<Window<Self::Msg>> {
        Vec::new()
    }
}

/// The size the window of a scene is shown at.
pub const SIZE: Size = Size::new(400.0, 300.0);

pub struct Probe<S>(PhantomData<S>);

impl<S: Scene> Application for Probe<S> {
    type Model = S::State;
    type Message = S::Msg;

    fn init() -> (S::State, Vec<Command<S::Msg>>) {
        (S::state(), Vec::new())
    }

    fn update(state: &mut S::State, message: S::Msg) -> Vec<Command<S::Msg>> {
        S::update(state, message);
        Vec::new()
    }

    fn view(state: &S::State) -> Vec<Window<S::Msg>> {
        let mut windows = vec![Window {
            key: WindowKey(Key::from("probe")),
            spec: WindowSpec {
                title: "Probe".into(),
                app_id: "org.cupertino.tests.Controls".into(),
                size: SIZE,
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
            content: S::view(state),
            menus: None,
        }];
        windows.extend(S::popups(state));
        windows
    }
}

/// Options that draw on the CPU with the built-in font, in the neutral light theme, left to right, with Reduce
/// Transparency on or off.
fn options(reduce_transparency: bool) -> RuntimeOptions {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let theme = ThemeTokens::neutral(ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency,
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

/// Scene `S` in its window, active, with the pointer and the keys of a person.
pub struct View<S: Scene> {
    pub driver: TestDriver<Probe<S>>,
    pub window: WindowId,
    /// The popups the keyboard was given to, the last of them still shown having it: the pointer and the keys go to
    /// it and its nodes are read, else the scene's window has it.
    aimed: Vec<WindowId>,
    /// The clock, in nanoseconds.
    now: u64,
}

impl<S: Scene> View<S> {
    pub fn open() -> View<S> {
        View::open_with(Capabilities::default(), false)
    }

    /// [`View::open`] on a window system of `capabilities`, with Reduce Transparency on or off.
    pub fn open_with(capabilities: Capabilities, reduce_transparency: bool) -> View<S> {
        let mut driver = TestDriver::<Probe<S>>::new(
            capabilities,
            options(reduce_transparency),
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
            .configure(window, Some(SIZE), active)
            .unwrap();
        let mut view = View {
            driver,
            window,
            aimed: Vec::new(),
            now: 0,
        };
        view.settle();
        view
    }

    /// The window of the popup the scene declares, if it declares one.
    pub fn popup(&self) -> Option<WindowId> {
        let main = self.window;
        self.driver
            .runtime()
            .windows()
            .into_iter()
            .map(|(_, id)| id)
            .rfind(|id| *id != main)
    }

    /// Give `window`, a popup, the keyboard, as a compositor does a menu that takes it; the pointer and the keys go
    /// to it and its nodes are read, until it goes.
    pub fn aim(&mut self, window: WindowId) {
        self.aimed.push(window);
        self.send(WindowEvent::Focus(true));
    }

    /// The window the pointer and the keys go to: the last popup aimed at that is still shown, else the scene's.
    fn current(&self) -> WindowId {
        let shown = self.driver.runtime().windows();
        self.aimed
            .iter()
            .rev()
            .copied()
            .find(|window| shown.iter().any(|(_, id)| id == window))
            .unwrap_or(self.window)
    }

    pub fn state(&self) -> &S::State {
        self.driver.runtime().model()
    }

    /// The accessibility nodes of the window.
    pub fn nodes(&self) -> Vec<AccessibilityNode> {
        self.driver
            .runtime()
            .snapshot(self.current())
            .map(|snapshot| snapshot.accessibility.into_iter().map(|(_, n)| n).collect())
            .unwrap_or_default()
    }

    /// The node of role `role` named `name`, the one furthest along when several are.
    pub fn node(&self, role: Role, name: &str) -> Option<AccessibilityNode> {
        self.nodes()
            .into_iter()
            .filter(|n| n.role == role && n.name.as_deref() == Some(name))
            .max_by(|a, b| a.bounds.min_x().total_cmp(&b.bounds.min_x()))
    }

    pub fn bounds_of(&self, role: Role, name: &str) -> Rect {
        self.node(role, name)
            .unwrap_or_else(|| panic!("no {role:?} named {name:?}"))
            .bounds
    }

    /// The middle of the node of role `role` named `name`.
    pub fn center_of(&self, role: Role, name: &str) -> Point {
        self.bounds_of(role, name).center()
    }

    /// Run the window, handing it the frames it asks for, until it rests.
    pub fn settle(&mut self) {
        for _ in 0..64 {
            self.driver.run_until_idle(turns()).unwrap();
            let windows: Vec<WindowId> = self
                .driver
                .runtime()
                .windows()
                .into_iter()
                .map(|(_, id)| id)
                .collect();
            let system = self.driver.window_system_mut();
            let mut asked = false;
            for window in windows {
                if system.frame_requested(window).unwrap() {
                    system.frame_ready(window, Timestamp(self.now)).unwrap();
                    asked = true;
                }
            }
            if !asked {
                return;
            }
        }
    }

    /// Move the clock on by `seconds`, waking what waits for it.
    pub fn wait(&mut self, seconds: f64) {
        self.now += (seconds * 1e9) as u64;
        self.driver.advance_to(Timestamp(self.now)).unwrap();
        self.settle();
    }

    fn send(&mut self, event: WindowEvent) {
        let window = self.current();
        self.driver
            .window_system_mut()
            .enqueue_window(window, event)
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
        self.key_down(keysym, text, mods);
        self.key_up(keysym, mods);
    }

    pub fn key_down(&mut self, keysym: Keysym, text: Option<char>, mods: Mods) {
        self.send(WindowEvent::Key(
            KeyEvent {
                code: evdev(keysym),
                keysym,
                text,
                pressed: true,
                repeat: false,
                mods,
            },
            Timestamp(self.now),
        ));
    }

    pub fn key_up(&mut self, keysym: Keysym, mods: Mods) {
        self.send(WindowEvent::Key(
            KeyEvent {
                code: evdev(keysym),
                keysym,
                text: None,
                pressed: false,
                repeat: false,
                mods,
            },
            Timestamp(self.now),
        ));
    }

    /// Each character of `text` typed in turn.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(Keysym(c as u32), Some(c), Mods::NONE);
        }
    }
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(TURNS).unwrap()
}

impl<S: Scene> View<S> {
    /// Tab until the node of role `role` named `name` has the keyboard.
    pub fn focus(&mut self, role: Role, name: &str) {
        for _ in 0..16 {
            if self.node(role, name).is_some_and(|n| n.states.focused) {
                return;
            }
            self.key(TAB, None, Mods::NONE);
        }
        panic!("Tab never reached {role:?} {name:?}");
    }
}

/// The evdev code of the key that gives `keysym`, as a keyboard sends it, for the keys the tests press; 0 for any
/// other.
fn evdev(keysym: Keysym) -> u32 {
    const LETTERS: &str = "qwertyuiop";
    const HOME_ROW: &str = "asdfghjkl";
    const BOTTOM_ROW: &str = "zxcvbnm";
    match keysym.0 {
        0xff08 => 14,
        0xff09 => 15,
        0xff0d => 28,
        0xff1b => 1,
        0x20 => 57,
        0xff50 => 102,
        0xff52 => 103,
        0xff55 => 104,
        0xff51 => 105,
        0xff53 => 106,
        0xff57 => 107,
        0xff54 => 108,
        0xff56 => 109,
        0xffff => 111,
        c => char::from_u32(c)
            .filter(char::is_ascii_lowercase)
            .and_then(|c| {
                [(LETTERS, 16), (HOME_ROW, 30), (BOTTOM_ROW, 44)]
                    .iter()
                    .find_map(|(row, first)| row.find(c).map(|i| first + i as u32))
            })
            .unwrap_or(0),
    }
}
