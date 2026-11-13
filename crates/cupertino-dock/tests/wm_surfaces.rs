//! The window-manager surfaces of the dock process, the overview's chrome and the application switcher's panel, as
//! CupertinoKit draws them on the headless window system: what the pointer, the keys and assistive technology do on
//! them, the geometry the toolkit lays out at output scales 1, 1.25, 1.5 and 2, and the fades. The models are the
//! dock's (`overview_ui`, `switcher`); the server's events are scripted.
//!
//! With `CUPERTINO_LOOK_DIR` set, `look_captures` also writes light and dark pictures of both surfaces at 1× and 2×
//! into that folder (the overview over a stand-in for the windows the server draws).

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use cupertino_assets::BUILTIN_FONT;
use cupertino_dock::overview_kit::{OverviewInput, OverviewRequest, OverviewSurface};
use cupertino_dock::overview_ui::{Mode, WindowEntry, Workspace};
use cupertino_dock::switcher::SwitcherLayout;
use cupertino_dock::switcher_kit::{self, SwitcherEntry, SwitcherInput, SwitcherSurface};
use cupertino_dock::wm_kit::{ImageSource, PointerInput};
use cupertino_kit::controls::Canvas;
use cupertino_kit::core::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, Action, ActionRequest,
    Application, Command, Element, FrameSnapshot, Key, Live, NodeId, PlaceholderIcons, Role,
    RuntimeOptions, Window, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowStates,
};
use cupertino_kit::render::{
    CpuConfig, CpuRenderer, ImageResource, Paint, PaintOp, PixelFormat, ResourceId, Shape,
};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, Color, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{describe, ManualClock, TestDriver};
use cupertino_wm::geometry::Rect as WmRect;

/// What the server (or the clock) does to the model between frames.
#[derive(Clone, Debug)]
enum Step {
    /// The model's clock, in seconds.
    At(f64),
    Overview {
        mode: Mode,
        area: WmRect,
        windows: Vec<WindowEntry>,
        workspaces: Vec<Workspace>,
    },
    OverviewClosed,
    Switcher {
        entries: Vec<SwitcherEntry>,
        selected: usize,
    },
    SwitcherClosed(bool),
    SwitcherLost,
    ReduceMotion(bool),
    /// Input given to the overview's model as its window would (for the pictures, whose stand-in is the target).
    OverviewInput(OverviewInput),
}

#[derive(Clone, Debug)]
enum Message {
    Step(Step),
    Overview(OverviewInput),
    Switcher(SwitcherInput),
}

thread_local! {
    static STEPS: RefCell<Option<(async_channel::Receiver<Step>, Size, bool)>> = const { RefCell::new(None) };
}

struct Model {
    overview: OverviewSurface,
    switcher: SwitcherSurface,
    now: f64,
    output: Size,
    /// Draw a stand-in for the windows the server draws under the overview (for the pictures).
    stand_in: bool,
    steps: async_channel::Receiver<Step>,
    /// What the surfaces asked the server for, in order.
    requests: Vec<String>,
    /// The frames the overview told the server last.
    frames: Vec<(u32, WmRect)>,
}

impl Model {
    /// A request of the overview: a window's new frame updates [`Model::frames`], the pictures and a drag's frames
    /// are the server's drawing, and everything else is noted in [`Model::requests`].
    fn follow(&mut self, r: OverviewRequest) {
        match r {
            OverviewRequest::Frame(id, f) => {
                if let Some(frame) = self.frames.iter_mut().find(|x| x.0 == id) {
                    frame.1 = f;
                }
            }
            OverviewRequest::Picture(..) | OverviewRequest::Drag(..) => {}
            r => self.requests.push(format!("{r:?}")),
        }
    }
}

fn next(steps: &async_channel::Receiver<Step>) -> Command<Message> {
    let steps = steps.clone();
    Command::Perform(Box::pin(async move {
        Message::Step(steps.recv().await.expect("the test holds the sender"))
    }))
}

struct Scripted;

impl Application for Scripted {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (Model, Vec<Command<Message>>) {
        let (steps, output, stand_in) = STEPS.with_borrow_mut(Option::take).expect("a run");
        let first = next(&steps);
        let model = Model {
            overview: OverviewSurface::new(),
            switcher: SwitcherSurface::new(),
            now: 0.0,
            output,
            stand_in,
            steps,
            requests: Vec::new(),
            frames: Vec::new(),
        };
        (model, vec![first])
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let now = model.now;
        match message {
            Message::Step(step) => {
                match step {
                    Step::At(t) => model.now = t,
                    Step::Overview {
                        mode,
                        area,
                        windows,
                        workspaces,
                    } => {
                        model.frames = model
                            .overview
                            .changed(mode, area, &windows, workspaces, now);
                    }
                    Step::OverviewClosed => model.overview.closed(now),
                    Step::Switcher { entries, selected } => model
                        .switcher
                        .changed(
                            7,
                            (model.output.width, model.output.height),
                            entries,
                            selected,
                            now,
                        )
                        .unwrap(),
                    Step::SwitcherClosed(committed) => model.switcher.closed(committed, now),
                    Step::SwitcherLost => model.switcher.lost(),
                    Step::ReduceMotion(on) => model.overview.set_reduce_motion(on),
                    Step::OverviewInput(input) => {
                        model.overview.input(input, now);
                    }
                }
                for r in model.overview.tick(model.now) {
                    model.follow(r);
                }
                model.switcher.tick(model.now);
                vec![next(&model.steps)]
            }
            Message::Overview(input) => {
                for r in model.overview.input(input, now) {
                    model.follow(r);
                }
                Vec::new()
            }
            Message::Switcher(input) => {
                for r in model.switcher.input(input, now) {
                    model.requests.push(format!("{r:?}"));
                }
                Vec::new()
            }
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let mut windows = Vec::new();
        if let Some(mut window) = model
            .overview
            .window(model.now, model.output, Message::Overview)
        {
            if model.stand_in {
                window.content = stand_in(window.content, model.overview.thumbnails(model.now));
            }
            windows.push(window);
        }
        windows.extend(model.switcher.window(
            model.now,
            &|app| icon(model.stand_in, app),
            Message::Switcher,
        ));
        windows
    }
}

/// For the pictures, a plain square tinted from the application's id in place of its icon; no renditions otherwise.
fn icon(stand_in: bool, app: &str) -> Vec<ImageSource> {
    if !stand_in {
        return Vec::new();
    }
    let side = 128u32;
    let hue = app
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let tint = [
        (hue % 160 + 60) as u8,
        (hue / 160 % 160 + 60) as u8,
        (hue / 25600 % 160 + 60) as u8,
        255,
    ];
    let pixels: Vec<u8> = (0..side * side).flat_map(|_| tint).collect();
    vec![ImageSource {
        id: ResourceId(u64::from(hue) + 1),
        image: ImageResource {
            revision: 1,
            width: side,
            height: side,
            stride: side * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: pixels.into(),
        },
    }]
}

/// The overview's dim and a gray card per window, as the server draws them under the chrome, for the pictures.
fn stand_in(chrome: Element<Message>, frames: Vec<(u32, WmRect)>) -> Element<Message> {
    Element::new(Canvas {
        size: None,
        label: String::new(),
        paint: Box::new(move |cx| {
            let size = cx.size();
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(Color::new(0.18, 0.2, 0.26, 1.0)),
            });
            for (_, f) in &frames {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(f.x, f.y, f.w, f.h),
                        radius: 10.0,
                    },
                    paint: Paint::Solid(Color::new(0.85, 0.86, 0.88, 1.0)),
                });
            }
        }),
        children: vec![chrome],
    })
}

/// Hands assistive technology's requests to the runtime and keeps the last tree it was given.
#[derive(Clone, Default)]
struct Assistive {
    pending: Arc<Mutex<Vec<ActionRequest>>>,
}

impl AccessibilityAdapter for Assistive {
    fn is_active(&self) -> bool {
        true
    }

    fn window_focus(&mut self, _: WindowId, _: bool) {}

    fn update(&mut self, _: &AccessibilityUpdate) {}

    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>) {
        out.append(&mut self.pending.lock().unwrap());
    }
}

fn theme(appearance: Appearance, reduce_transparency: bool) -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

struct Run {
    driver: TestDriver<Scripted>,
    steps: async_channel::Sender<Step>,
    assistive: Assistive,
    configured: Vec<WindowId>,
    scale: f64,
}

impl Run {
    fn new(output: Size) -> Self {
        Self::with(output, Appearance::Dark, false, false)
    }

    fn with(
        output: Size,
        appearance: Appearance,
        reduce_transparency: bool,
        stand_in: bool,
    ) -> Self {
        let (steps, receiver) = async_channel::unbounded();
        STEPS.set(Some((receiver, output, stand_in)));
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let assistive = Assistive::default();
        let options = RuntimeOptions {
            theme: Arc::new(theme(appearance, reduce_transparency)),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 1000,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 24,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: Some(Box::new(assistive.clone())),
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let driver = TestDriver::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        let mut run = Run {
            driver,
            steps,
            assistive,
            configured: Vec::new(),
            scale: 1.0,
        };
        run.settle();
        run
    }

    /// Runs until every window rests, then checks that a screen reader could read every window open.
    fn settle(&mut self) {
        self.settle_frames();
        self.driver.assert_accessible("Window manager surfaces");
    }

    /// Runs until nothing is left to do: a new window is configured at the size it asked for, at the run's scale, and
    /// every frame asked for is given.
    fn settle_frames(&mut self) {
        for _ in 0..16 {
            self.driver.run_until_idle(budget()).unwrap();
            let mut busy = false;
            for (_, window) in self.driver.runtime().windows() {
                let system = self.driver.window_system_mut();
                if !self.configured.contains(&window) {
                    self.configured.push(window);
                    let size = system.spec(window).unwrap().size;
                    system
                        .enqueue_window(window, WindowEvent::Scale(self.scale))
                        .unwrap();
                    system
                        .configure(window, Some(size), WindowStates::default())
                        .unwrap();
                    busy = true;
                } else if system.frame_requested(window).unwrap() {
                    system.frame_ready(window, Timestamp(0)).unwrap();
                    busy = true;
                }
            }
            if !busy {
                return;
            }
        }
        panic!("the windows keep asking for frames");
    }

    fn step(&mut self, step: Step) {
        self.steps.try_send(step).unwrap();
        self.settle();
    }

    fn at(&mut self, t: f64) {
        self.step(Step::At(t));
    }

    fn model(&self) -> &Model {
        self.driver.runtime().model()
    }

    fn requests(&self) -> Vec<String> {
        self.model().requests.clone()
    }

    fn window(&self, prefix: &str) -> Option<WindowId> {
        self.driver
            .runtime()
            .windows()
            .into_iter()
            .find(|(WindowKey(k), _)| matches!(k, Key::Str(s) if s.starts_with(prefix)))
            .map(|(_, id)| id)
    }

    fn snapshot(&self, window: WindowId) -> FrameSnapshot {
        self.driver.runtime().snapshot(window).unwrap()
    }

    /// The rectangle of the node `key` names in `window`, in window points.
    fn rect(&self, window: WindowId, key: &str) -> Option<Rect> {
        let id = self
            .driver
            .runtime()
            .find(window, &Key::from(key.to_owned()))?;
        self.snapshot(window)
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.rect)
    }

    /// Every node of `window` whose key starts with `prefix`, with its rectangle.
    fn rects(&self, window: WindowId, prefix: &str) -> Vec<(String, Rect)> {
        self.snapshot(window)
            .nodes
            .iter()
            .filter_map(|n| match &n.key {
                Some(Key::Str(s)) if s.starts_with(prefix) => Some((s.to_string(), n.rect)),
                _ => None,
            })
            .collect()
    }

    fn pointer(&mut self, window: WindowId, event: PointerEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(window, WindowEvent::Pointer(event, Timestamp(0)))
            .unwrap();
        self.settle();
    }

    fn moved(&mut self, window: WindowId, position: Point) {
        self.pointer(window, PointerEvent::Motion { position });
    }

    fn button(&mut self, window: WindowId, position: Point, pressed: bool) {
        self.pointer(
            window,
            PointerEvent::Button {
                position,
                button: MouseButton::Left,
                pressed,
                serial: 1,
                mods: Mods::NONE,
            },
        );
    }

    fn click(&mut self, window: WindowId, position: Point) {
        self.moved(window, position);
        self.button(window, position, true);
        self.button(window, position, false);
    }

    fn key(&mut self, window: WindowId, keysym: u32, mods: Mods) {
        for pressed in [true, false] {
            self.driver
                .window_system_mut()
                .enqueue_window(
                    window,
                    WindowEvent::Key(
                        KeyEvent {
                            code: 0,
                            keysym: Keysym(keysym),
                            text: None,
                            pressed,
                            repeat: false,
                            mods,
                        },
                        Timestamp(0),
                    ),
                )
                .unwrap();
            self.settle();
        }
    }

    /// What a screen reader reads of `window`: one line per node it announces.
    fn reading(&self, window: WindowId) -> Vec<String> {
        self.snapshot(window)
            .accessibility
            .iter()
            .map(|(_, node)| describe(node))
            .collect()
    }

    fn node(&self, window: WindowId, name: &str) -> (NodeId, AccessibilityNode) {
        self.snapshot(window)
            .accessibility
            .into_iter()
            .find(|(_, n)| n.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no node named {name:?}: {:?}", self.reading(window)))
    }

    /// Assistive technology clicks the node named `name`.
    fn press_named(&mut self, window: WindowId, name: &str) {
        let (target, node) = self.node(window, name);
        assert!(
            node.actions.contains(&Action::Click),
            "{name} offers no click"
        );
        self.assistive.pending.lock().unwrap().push(ActionRequest {
            target,
            action: Action::Click,
            data: None,
        });
        // the request reaches the runtime on its next turn
        self.at(self.model().now);
    }
}

fn centre(r: Rect) -> Point {
    Point::new(
        r.origin.x + r.size.width / 2.0,
        r.origin.y + r.size.height / 2.0,
    )
}

fn bottom(r: Rect) -> f64 {
    r.origin.y + r.size.height
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

const MENU_BAR: f64 = 24.0;
const DOCK_ZONE: f64 = 70.0;

/// The usable area of an output of `size` points: below the menu bar, above the dock.
fn area(size: Size) -> WmRect {
    // the overview lays out over the whole height down to the dock: the menu bar gives way to it
    WmRect::new(0.0, 0.0, size.width, size.height - DOCK_ZONE)
}

/// Three windows of different sizes, one small, at desktop places of an output of `size` points.
fn three_windows(size: Size) -> Vec<WindowEntry> {
    let (w, h) = (size.width, size.height);
    [
        (
            1,
            "Documents",
            WmRect::new(w * 0.35, MENU_BAR + 20.0, w * 0.55, h * 0.55),
        ),
        (
            2,
            "Settings",
            WmRect::new(w * 0.05, h * 0.3, w * 0.42, h * 0.6),
        ),
        (
            3,
            "Forms",
            WmRect::new(w * 0.38, h * 0.22, w * 0.16, h * 0.17),
        ),
    ]
    .into_iter()
    .map(|(id, title, frame)| WindowEntry {
        id,
        frame,
        title: title.into(),
        minimized: false,
    })
    .collect()
}

fn workspaces(n: u32) -> Vec<Workspace> {
    (1..=n)
        .map(|id| Workspace {
            id: 100 + id,
            current: id == 1,
        })
        .collect()
}

fn open_overview(run: &mut Run, output: Size, windows: Vec<WindowEntry>, n: u32) -> WindowId {
    run.step(Step::Overview {
        mode: Mode::All,
        area: area(output),
        windows,
        workspaces: workspaces(n),
    });
    run.window("overview").expect("the chrome's window opened")
}

const OUTPUTS: [(f64, f64); 4] = [
    (1280.0, 800.0),
    (1280.0, 720.0),
    (1440.0, 900.0),
    (1920.0, 1080.0),
];
const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];

/// The issue's case on the migrated chrome: three windows of different sizes, one small, at every output size and
/// scale. The toolkit lays the windows out where the server is told they go, at least 39 pt below the closed desktops
/// band, clear of every name and of each other, centred across the area, with no title under them, and a click on a
/// window selects it.
#[test]
fn three_windows_clear_the_strip_and_each_other_at_every_size_and_scale() {
    for (w, h) in OUTPUTS {
        for scale in SCALES {
            let output = Size::new(w, h);
            let case = format!("{w}×{h} pt at {scale}");
            let mut run = Run::new(output);
            run.scale = scale;
            let window = open_overview(&mut run, output, three_windows(output), 3);
            let frames = run.model().frames.clone();
            assert_eq!(frames.len(), 3, "{case}");
            let slots: Vec<Rect> = run
                .rects(window, "slot-")
                .into_iter()
                .map(|(_, r)| r)
                .chain(run.rect(window, "add"))
                .collect();
            assert_eq!(slots.len(), 4, "{case}: three names and the add button");
            let band = run.rect(window, "band").expect("the band");
            assert_eq!(band.size.height, 72.0, "{case}: the band is closed");
            let strip = bottom(band);
            let thumbnails: Vec<Rect> = frames
                .iter()
                .map(|(id, f)| {
                    let drawn = run.rect(window, &format!("window-{id}")).unwrap();
                    assert_eq!(
                        drawn,
                        Rect::new(f.x, f.y, f.w, f.h),
                        "{case}: drawn where the server is told"
                    );
                    drawn
                })
                .collect();
            for (i, t) in thumbnails.iter().enumerate() {
                assert!(
                    t.origin.y >= strip + 39.0 - 1e-6,
                    "{case}: window {i} at {} within 39 pt of the band at {strip}",
                    t.origin.y
                );
                for s in &slots {
                    assert!(
                        !intersects(*t, *s),
                        "{case}: window {i} {t:?} over slot {s:?}"
                    );
                }
                for (j, u) in thumbnails.iter().enumerate().skip(i + 1) {
                    assert!(!intersects(*t, *u), "{case}: windows {i} and {j} overlap");
                }
                assert!(
                    bottom(*t) <= area(output).bottom(),
                    "{case}: window {i} in the area"
                );
                assert!(
                    run.rect(window, &format!("title-{}", frames[i].0))
                        .is_none(),
                    "{case}: no title under window {i}"
                );
            }
            let left = thumbnails
                .iter()
                .map(|t| t.origin.x)
                .fold(f64::MAX, f64::min);
            let right = thumbnails
                .iter()
                .map(|t| t.origin.x + t.size.width)
                .fold(0.0, f64::max);
            assert!(
                ((left + right) / 2.0 - w / 2.0).abs() < 1e-6,
                "{case}: the arrangement is centred ({left}..{right})"
            );
            let (id, _) = frames[2];
            let small = run.rect(window, &format!("window-{id}")).unwrap();
            run.click(window, centre(small));
            assert_eq!(
                run.requests(),
                [format!("Select({id})")],
                "{case}: the small window is hit"
            );
        }
    }
}

#[test]
fn the_keys_walk_the_windows_choose_one_and_escape_leaves() {
    let output = Size::new(1280.0, 800.0);
    let mut run = Run::new(output);
    let window = open_overview(&mut run, output, three_windows(output), 2);
    const TAB: u32 = 0xff09;
    const RIGHT: u32 = 0xff53;
    const RETURN: u32 = 0xff0d;
    const ESCAPE: u32 = 0xff1b;
    run.key(window, TAB, Mods::NONE);
    let first = run
        .model()
        .overview
        .ui()
        .unwrap()
        .selection()
        .expect("Tab selects");
    let title = run
        .model()
        .overview
        .ui()
        .unwrap()
        .title(first)
        .unwrap()
        .to_owned();
    let (_, node) = run.node(window, &title);
    assert_eq!(node.role, Role::Button);
    assert!(
        node.states.focused && node.states.selected,
        "the selection is the screen reader's focus"
    );
    run.key(window, RIGHT, Mods::NONE);
    let second = run.model().overview.ui().unwrap().selection().unwrap();
    assert_ne!(first, second, "an arrow moves the selection");
    run.key(window, RETURN, Mods::NONE);
    run.key(window, ESCAPE, Mods::NONE);
    assert_eq!(
        run.requests(),
        [format!("Select({second})"), "Close".into()]
    );
}

#[test]
fn the_strip_switches_adds_removes_and_reorders_workspaces() {
    let output = Size::new(1280.0, 800.0);
    let mut run = Run::new(output);
    let window = open_overview(&mut run, output, three_windows(output), 2);
    let second = run.rect(window, "slot-102").unwrap();
    run.click(window, centre(second));
    run.click(window, centre(run.rect(window, "add").unwrap()));
    // the pointer at the top opens the band; a second's rest on a desktop's picture shows its remove button
    run.at(1.0);
    let picture = run
        .rect(window, "picture-102")
        .expect("the open band shows the pictures");
    assert!(run.rect(window, "band").unwrap().size.height > 160.0);
    run.moved(window, centre(picture));
    assert!(run.rect(window, "remove-102").is_none(), "not at once");
    run.at(2.1);
    run.moved(window, centre(picture));
    let remove = run
        .rect(window, "remove-102")
        .expect("the remove button after the rest");
    assert_eq!(remove.size.width, 28.0);
    run.click(window, centre(remove));
    // dragging the second name before the first
    let first = run.rect(window, "slot-101").unwrap();
    let second = run.rect(window, "slot-102").unwrap();
    run.moved(window, centre(second));
    run.button(window, centre(second), true);
    let target = Point::new(first.origin.x + 2.0, centre(first).y);
    run.moved(window, target);
    run.button(window, target, false);
    // a press on the background leaves
    run.click(window, Point::new(5.0, output.height - DOCK_ZONE - 5.0));
    assert_eq!(
        run.requests(),
        [
            "SwitchWorkspace(102)",
            "AddWorkspace",
            "RemoveWorkspace(102)",
            "MoveWorkspace { id: 102, index: 0 }",
            "Close"
        ]
    );
}

#[test]
fn assistive_technology_reads_and_presses_the_chrome() {
    let output = Size::new(1280.0, 800.0);
    let mut run = Run::new(output);
    let window = open_overview(&mut run, output, three_windows(output), 2);
    let reading = run.reading(window);
    for line in [
        "Group \"Overview\"",
        "Button \"Desktop 1\" focusable selected",
        "Button \"Desktop 2\" focusable",
        "Button \"Add Desktop\" focusable",
        "Button \"Documents\" focusable",
        "Button \"Settings\" focusable",
        "Button \"Forms\" focusable",
    ] {
        assert!(
            reading.iter().any(|l| l == line),
            "{line:?} not in {reading:#?}"
        );
    }
    run.press_named(window, "Forms");
    run.press_named(window, "Desktop 2");
    run.press_named(window, "Add Desktop");
    assert_eq!(
        run.requests(),
        ["Select(3)", "SwitchWorkspace(102)", "AddWorkspace"]
    );
}

#[test]
fn closing_fades_the_chrome_gives_the_keyboard_back_and_ignores_input() {
    let output = Size::new(1280.0, 800.0);
    let mut run = Run::new(output);
    let window = open_overview(&mut run, output, three_windows(output), 1);
    assert!(run.model().overview.wants_keyboard());
    run.at(1.0);
    run.step(Step::OverviewClosed);
    assert!(
        !run.model().overview.wants_keyboard(),
        "the keyboard goes back as the fade starts"
    );
    assert!(
        run.model().overview.next_wake(1.0).is_some(),
        "the fade needs frames"
    );
    let small = run.rect(window, "window-3").unwrap();
    run.click(window, centre(small));
    assert!(run.requests().is_empty(), "a fading chrome takes no input");
    run.at(1.0 + 0.25);
    assert!(
        run.window("overview").is_none(),
        "the window goes when the fade ends"
    );
    assert!(run.model().overview.is_done(1.25));
}

#[test]
fn a_new_window_set_reflows_and_tells_the_server_every_frame() {
    let output = Size::new(1280.0, 800.0);
    let mut run = Run::new(output);
    let mut windows = three_windows(output);
    let window = open_overview(&mut run, output, windows[..2].to_vec(), 1);
    windows.truncate(3);
    run.step(Step::Overview {
        mode: Mode::All,
        area: area(output),
        windows: windows.clone(),
        workspaces: workspaces(1),
    });
    assert_eq!(
        run.window("overview"),
        Some(window),
        "the same overview keeps its window"
    );
    let ids: Vec<u32> = run.model().frames.iter().map(|f| f.0).collect();
    assert_eq!(ids, [1, 2, 3]);
}

#[test]
fn under_reduce_motion_the_highlight_goes_straight_to_the_selection() {
    let output = Size::new(1280.0, 800.0);
    let mut run = Run::new(output);
    run.step(Step::ReduceMotion(true));
    let window = open_overview(&mut run, output, three_windows(output), 1);
    const TAB: u32 = 0xff09;
    run.key(window, TAB, Mods::NONE);
    run.at(0.5);
    run.key(window, TAB, Mods::NONE);
    let ui = run.model().overview.ui().unwrap();
    let selected = ui.selection().unwrap();
    let frame = ui.frames().into_iter().find(|f| f.0 == selected).unwrap().1;
    let highlight = run
        .snapshot(window)
        .nodes
        .iter()
        .find(|n| n.widget.contains("Part") && n.key.is_none() && n.rect.size.width > frame.w)
        .map(|n| n.rect)
        .expect("the highlight is drawn");
    let ring = OverviewSurface::ring(frame);
    assert_eq!(
        highlight,
        Rect::new(ring.x, ring.y, ring.w, ring.h),
        "no glide from the last selection"
    );
}

/// The colours a node of `window` fills with.
fn fills(run: &Run, window: WindowId) -> Vec<Color> {
    run.snapshot(window)
        .display_list
        .nodes
        .iter()
        .flat_map(|n| n.ops.iter())
        .filter_map(|op| match op {
            PaintOp::Fill {
                paint: Paint::Solid(c),
                ..
            } => Some(*c),
            _ => None,
        })
        .collect()
}

#[test]
fn reduce_transparency_and_increase_contrast_draw_the_slots_opaque() {
    let output = Size::new(1280.0, 800.0);
    for (appearance, reduce_transparency) in [
        (Appearance::Dark, true),
        (Appearance::Light, true),
        (Appearance::DarkHighContrast, false),
    ] {
        let mut run = Run::with(output, appearance, reduce_transparency, false);
        let window = open_overview(&mut run, output, three_windows(output), 2);
        let fills = fills(&run, window);
        assert!(
            fills.len() >= 2 && fills.iter().all(|c| c.a == 1.0 || c.a > 0.84),
            "{appearance:?}: {fills:?}"
        );
    }
    let mut run = Run::new(output);
    let window = open_overview(&mut run, output, three_windows(output), 2);
    assert!(
        fills(&run, window).iter().any(|c| c.a < 0.5),
        "translucent slots otherwise"
    );
}

fn entries(names: &[&str]) -> Vec<SwitcherEntry> {
    names
        .iter()
        .map(|n| SwitcherEntry {
            app_id: format!("org.example.{n}"),
            name: (*n).to_owned(),
        })
        .collect()
}

#[test]
fn the_switcher_shows_after_its_delay_and_the_pointer_selects_and_activates() {
    let output = Size::new(1280.0, 800.0);
    let mut run = Run::new(output);
    run.step(Step::Switcher {
        entries: entries(&["Files", "Settings", "Forms"]),
        selected: 1,
    });
    assert!(
        run.window("switcher").is_none(),
        "no panel before the delay"
    );
    assert_eq!(run.model().switcher.next_wake(0.0), Some(0.15));
    run.at(0.15);
    let window = run
        .window("switcher")
        .expect("the panel shows after its delay");
    let spec = run.driver.window_system_mut().spec(window).unwrap().clone();
    assert_eq!(spec.app_id, switcher_kit::APP_ID);
    let layout = SwitcherLayout::new(3, output.width, output.height);
    assert_eq!(spec.size, Size::new(layout.panel.w, layout.panel.h));
    let item = |i: usize| {
        let r = layout.item(i);
        Point::new(r.cx() - layout.panel.x, r.cy() - layout.panel.y)
    };
    // the first position arms nothing; past the hover box the entry under the pointer is selected
    run.moved(window, item(0));
    assert!(
        run.requests().is_empty(),
        "hovering selects only once the pointer moved"
    );
    run.moved(window, Point::new(item(0).x + 10.0, item(0).y));
    run.button(window, item(2), true);
    run.button(window, item(2), false);
    run.button(window, Point::new(2.0, 2.0), false);
    assert_eq!(
        run.requests(),
        ["Select(0)", "Select(2)", "Activate(2)", "Cancel"]
    );
    let reading = run.reading(window);
    for line in [
        "List \"Application Switcher\" 3 items",
        "ListItem \"Files\" focusable",
        "ListItem \"Settings\" focusable selected",
        "ListItem \"Forms\" focusable",
        "StaticText \"Settings\"",
    ] {
        assert!(
            reading.iter().any(|l| l == line),
            "{line:?} not in {reading:#?}"
        );
    }
    let label = run
        .snapshot(window)
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::StaticText)
        .unwrap()
        .1;
    assert_eq!(label.live, Some(Live::Polite), "each step is announced");
    run.press_named(window, "Forms");
    assert_eq!(run.requests().last().unwrap(), "Activate(2)");
}

#[test]
fn a_step_moves_the_selection_and_a_commit_fades_the_panel_out() {
    let output = Size::new(1280.0, 800.0);
    let mut run = Run::new(output);
    let list = entries(&["Files", "Settings"]);
    run.step(Step::Switcher {
        entries: list.clone(),
        selected: 1,
    });
    run.at(0.2);
    let window = run.window("switcher").unwrap();
    run.step(Step::Switcher {
        entries: list.clone(),
        selected: 0,
    });
    assert_eq!(
        run.window("switcher"),
        Some(window),
        "a step keeps the panel"
    );
    assert_eq!(run.model().switcher.selected(), Some(0));
    run.step(Step::SwitcherClosed(true));
    assert!(
        run.window("switcher").is_some(),
        "the panel fades out after a commit"
    );
    assert_eq!(
        run.model().switcher.next_wake(0.2),
        Some(0.2 + switcher_kit::FRAME)
    );
    run.at(0.2 + 0.25);
    assert!(run.window("switcher").is_none());
    // a cancel removes the panel at once
    run.step(Step::Switcher {
        entries: list.clone(),
        selected: 1,
    });
    run.at(0.8);
    assert!(run.window("switcher").is_some());
    run.step(Step::SwitcherClosed(false));
    assert!(
        run.window("switcher").is_none(),
        "a cancel removes the panel at once"
    );
}

#[test]
fn a_switch_whose_window_the_server_closed_resumes_in_a_new_one() {
    let output = Size::new(1280.0, 800.0);
    let mut run = Run::new(output);
    let list = entries(&["Files", "Settings", "Forms"]);
    run.step(Step::Switcher {
        entries: list.clone(),
        selected: 1,
    });
    run.at(0.2);
    let first = run.window("switcher").unwrap();
    run.step(Step::SwitcherLost);
    assert!(run.window("switcher").is_none());
    run.step(Step::Switcher {
        entries: list,
        selected: 2,
    });
    let second = run
        .window("switcher")
        .expect("the next step shows the switch at once, past its delay");
    assert_ne!(first, second);
    assert_eq!(run.model().switcher.selected(), Some(2));
}

/// Pictures for a person to look at: the overview with three windows and the switcher, light and dark, at 1× and
/// 2×, into `CUPERTINO_LOOK_DIR` when it is set; nothing otherwise.
#[test]
fn look_captures() {
    let Some(dir) = std::env::var_os("CUPERTINO_LOOK_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let output = Size::new(1280.0, 720.0);
    for scale in [1.0, 2.0] {
        let mut run = Run::with(output, Appearance::Light, false, true);
        let window = open_overview(&mut run, output, three_windows(output), 2);
        // the pointer over the first window selects it
        let (_, first) = run.model().frames[0];
        run.step(Step::OverviewInput(OverviewInput::Pointer(
            PointerInput::Moved(Point::new(first.cx(), first.cy())),
        )));
        run.at(2.0);
        run.driver
            .look_into(
                &dir,
                window,
                "overview-three-windows",
                scale,
                &[Appearance::Light, Appearance::Dark],
            )
            .unwrap();
        run.step(Step::Switcher {
            entries: entries(&["Files", "Settings", "Forms"]),
            selected: 1,
        });
        run.at(2.5);
        let panel = run.window("switcher").unwrap();
        run.driver
            .look_into(
                &dir,
                panel,
                "switcher",
                scale,
                &[Appearance::Light, Appearance::Dark],
            )
            .unwrap();
    }
}
