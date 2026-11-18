//! A small application on the headless window system: a column of probe widgets that show what the runtime did with
//! them, driven as a window system would drive it.

use cupertino_assets::BUILTIN_FONT;
use cupertino_geometry::Size;
use cupertino_theme::ui::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_ui_core::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_core::platform::{
    Capabilities, DecorationMode, KeyEvent, Keysym, Mods, Timestamp, WindowEvent, WindowId,
    WindowSpec, WindowStates,
};
use cupertino_ui_core::{
    Application, Change, Command, CommandRegistry, Element, Event, EventCx, MeasureCx, PaintCx,
    PlaceholderIcons, Propagation, Role, RuntimeOptions, SemanticsCx, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_test::{ManualClock, TestDriver};
use cupertino_ui_text::{FontIndex, GlyphRaster, TextSystem};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub const TAB: Keysym = Keysym(0xff09);
pub const LEFT_TAB: Keysym = Keysym(0xfe20);
pub const ENTER: Keysym = Keysym(0xff0d);

/// What the probes and the application tell the test.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// The view becomes these probes, in order: an optional key, the label and whether the probe takes Enter.
    Show(Vec<Row>),
    /// The windows the view holds, by key.
    Windows(Vec<&'static str>),
    /// A probe took Enter.
    Took(&'static str),
    /// A window's shortcut.
    Shortcut(&'static str),
    Closed(&'static str),
    Exit,
    /// The next message the test queued: the only way into the application from outside.
    Next,
}

thread_local! {
    static QUEUED: RefCell<VecDeque<Msg>> = const { RefCell::new(VecDeque::new()) };
}

pub const F12: Keysym = Keysym(0xffc9);

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: Option<&'static str>,
    pub label: &'static str,
    pub takes_enter: bool,
}

pub fn row(key: Option<&'static str>, label: &'static str) -> Row {
    Row {
        key,
        label,
        takes_enter: false,
    }
}

#[derive(Default)]
pub struct Model {
    pub rows: Vec<Row>,
    pub windows: Vec<&'static str>,
    pub log: Vec<Msg>,
}

pub struct Lab;

impl Application for Lab {
    type Model = Model;
    type Message = Msg;

    fn init() -> (Model, Vec<Command<Msg>>) {
        let model = Model {
            windows: vec!["main"],
            ..Model::default()
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Msg) -> Vec<Command<Msg>> {
        if message == Msg::Next {
            let queued = QUEUED.with(|q| q.borrow_mut().pop_front());
            return queued.map_or_else(Vec::new, |m| Lab::update(model, m));
        }
        match &message {
            Msg::Show(rows) => model.rows = rows.clone(),
            Msg::Windows(keys) => model.windows = keys.clone(),
            Msg::Exit => return vec![Command::Exit],
            _ => {}
        }
        model.log.push(message);
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Msg>> {
        model
            .windows
            .iter()
            .map(|&key| Window {
                key: WindowKey(key.into()),
                spec: WindowSpec {
                    title: key.into(),
                    app_id: "org.cupertino.tests.Lab".into(),
                    size: Size::new(200.0, 200.0),
                    min_size: None,
                    max_size: None,
                    decorations: DecorationMode::None,
                    foreign_parent: None,
                    input_region: None,
                },
                shortcuts: CommandRegistry::new()
                    .with(
                        cupertino_ui_core::Shortcut {
                            mods: Mods::NONE,
                            keysym: ENTER,
                        },
                        Msg::Shortcut("enter"),
                    )
                    .with(
                        cupertino_ui_core::Shortcut {
                            mods: Mods::PRIMARY,
                            keysym: Keysym(0x73),
                        },
                        Msg::Shortcut("save"),
                    )
                    .with(
                        cupertino_ui_core::Shortcut {
                            mods: Mods::NONE,
                            keysym: F12,
                        },
                        Msg::Next,
                    ),
                on_close: (key != "last").then_some(Msg::Closed(key)),
                background: WindowBackground::None,
                popup: None,
                content: Element::new(Column(Some(
                    model
                        .rows
                        .iter()
                        .map(|r| {
                            let probe = Element::new(Probe {
                                label: r.label,
                                takes_enter: r.takes_enter,
                            });
                            match r.key {
                                Some(k) => probe.key(k),
                                None => probe,
                            }
                        })
                        .collect(),
                ))),
                menus: None,
            })
            .collect()
    }
}

/// A vertical stack of its children.
pub struct Column(Option<Vec<Element<Msg>>>);

impl Widget<Msg> for Column {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Start,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<Msg>> {
        self.0.take().unwrap_or_default()
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Msg>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

static SERIALS: AtomicU32 = AtomicU32::new(0);

/// A focusable leaf whose accessible name is its label and the serial of the state its node was made with, so a test
/// can tell a node that kept its state from one made anew.
pub struct Probe {
    pub label: &'static str,
    pub takes_enter: bool,
}

impl Widget<Msg> for Probe {
    type State = u32;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> u32 {
        SERIALS.fetch_add(1, Ordering::Relaxed)
    }

    fn update(&self, _: &Self, _: &mut u32) -> Change {
        Change::Paint
    }

    fn measure(&self, _: &u32, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(80.0, 20.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut u32, event: &Event, cx: &mut EventCx<'_, Msg>) -> Propagation {
        match event {
            Event::Key(key, _) if key.pressed && key.keysym == ENTER && self.takes_enter => {
                cx.emit(Msg::Took(self.label));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &u32, _: &mut PaintCx<'_>) {}

    fn semantics(&self, serial: &u32, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(format!("{}#{serial}", self.label));
        cx.focusable();
    }
}

pub fn text_system() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    Arc::new(TextSystem::new(fonts, "Inter Variable"))
}

pub fn options(node_limit: usize) -> RuntimeOptions {
    let theme = ThemeTokens::neutral(ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: true,
    })
    .unwrap();
    let text = text_system();
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(theme),
        text,
        icons: Arc::new(PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit,
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

pub fn turns() -> NonZeroU32 {
    NonZeroU32::new(1000).unwrap()
}

/// The lab on the headless window system with its first window configured and active.
pub struct Shown {
    pub driver: TestDriver<Lab>,
    pub window: WindowId,
    pub at: u64,
}

const FRAME_STEP: u64 = 100_000_000;

impl Shown {
    pub fn open() -> Shown {
        Shown::open_with(1000)
    }

    pub fn open_with(node_limit: usize) -> Shown {
        let driver = TestDriver::new(
            Capabilities::default(),
            options(node_limit),
            ManualClock::new(Timestamp(0)),
        );
        let mut shown = Shown {
            driver,
            window: WindowId(0),
            at: 0,
        };
        shown.driver.run_until_idle(turns()).unwrap();
        shown.window = shown.driver.runtime().windows()[0].1;
        shown.configure(shown.window);
        shown
    }

    /// Configures `window` as active and draws what it asks for.
    pub fn configure(&mut self, window: WindowId) {
        let states = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        self.driver
            .window_system_mut()
            .configure(window, None, states)
            .unwrap();
        self.settle();
    }

    /// Hands every window the frames it asks for until none asks for one.
    pub fn settle(&mut self) {
        for _ in 0..64 {
            self.driver.run_until_idle(turns()).unwrap();
            let waiting: Vec<WindowId> = self
                .driver
                .window_system_mut()
                .windows()
                .into_iter()
                .filter(|w| {
                    self.driver
                        .window_system_mut()
                        .frame_requested(*w)
                        .unwrap_or(false)
                })
                .collect();
            if waiting.is_empty() {
                return;
            }
            self.at += FRAME_STEP;
            self.driver.advance_to(Timestamp(self.at)).unwrap();
            for window in waiting {
                self.driver
                    .window_system_mut()
                    .frame_ready(window, Timestamp(self.at))
                    .unwrap();
            }
        }
        panic!("the windows keep asking for frames");
    }

    /// The application says `message`, as a task finishing would, and the windows settle.
    pub fn send(&mut self, message: Msg) {
        let window = self.queue(message);
        self.press(window, F12, Mods::NONE);
    }

    /// Queues `message` for the application and returns the window whose F12 delivers it.
    pub fn queue(&mut self, message: Msg) -> WindowId {
        QUEUED.with(|q| q.borrow_mut().push_back(message));
        self.driver.runtime().windows()[0].1
    }

    pub fn model(&self) -> &Model {
        self.driver.runtime().model()
    }

    /// Presses and releases a key in `window`.
    pub fn press(&mut self, window: WindowId, keysym: Keysym, mods: Mods) {
        self.send_key(window, keysym, mods);
        self.settle();
    }

    /// Queues the press and release of a key without running the runtime.
    pub fn send_key(&mut self, window: WindowId, keysym: Keysym, mods: Mods) {
        for pressed in [true, false] {
            let event = KeyEvent {
                code: 0,
                keysym,
                text: None,
                pressed,
                repeat: false,
                mods,
            };
            self.driver
                .window_system_mut()
                .enqueue_window(window, WindowEvent::Key(event, Timestamp(self.at)))
                .unwrap();
        }
    }

    /// The accessible name of the node holding the focus in `window`.
    pub fn focused(&self, window: WindowId) -> Option<String> {
        let snapshot = self.driver.runtime().snapshot(window)?;
        let focus = snapshot.focus?;
        snapshot
            .accessibility
            .into_iter()
            .find(|(id, _)| *id == focus)
            .and_then(|(_, node)| node.name)
    }

    /// The names of the probes of `window`, in the tree's order.
    pub fn names(&self, window: WindowId) -> Vec<String> {
        let snapshot = self.driver.runtime().snapshot(window).unwrap();
        let named: HashMap<_, _> = snapshot.accessibility.into_iter().collect();
        snapshot
            .nodes
            .iter()
            .filter_map(|node| named.get(&node.id)?.name.clone())
            .collect()
    }
}
