//! The banners and the notification centre as CupertinoKit draws them, on the headless window system. Each golden
//! is the Inspector dump of every open window after a scripted run of the model, with what a screen reader was told
//! to announce on the way. `CUPERTINO_UPDATE_GOLDENS=1` rewrites the goldens.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    AccessibilityAdapter, AccessibilityUpdate, ActionRequest, Application, Command,
    PlaceholderIcons, RuntimeOptions, Window, WindowKey,
};
use cupertino_kit::geometry::Point;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_notifications::view::{self, Input, Ui, SHADOW_MARGIN};
use cupertino_notifications::{CenterConfig, NotificationCenter, NotifyInput, Urgency};
use cupertino_theme::Theme;
use cupertino_ui_test::{Inspector, ManualClock, TestDriver};

/// What the test does to the model between frames.
#[derive(Clone, Debug)]
enum Step {
    Notify(Box<NotifyInput>),
    /// The model's clock, in seconds.
    At(f64),
    OpenCentre,
}

#[derive(Clone, Debug)]
enum Message {
    Step(Step),
    Input(Input),
}

thread_local! {
    /// The steps of this thread's run, taken by [`Scripted::init`].
    static STEPS: RefCell<Option<async_channel::Receiver<Step>>> = const { RefCell::new(None) };
}

struct Model {
    center: RefCell<NotificationCenter>,
    ui: Ui,
    now: f64,
    steps: async_channel::Receiver<Step>,
}

fn next(steps: &async_channel::Receiver<Step>) -> Command<Message> {
    let steps = steps.clone();
    Command::Perform(Box::pin(async move {
        Message::Step(steps.recv().await.expect("the test holds the sender"))
    }))
}

/// The view of the process on a model the test scripts.
struct Scripted;

impl Application for Scripted {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (Model, Vec<Command<Message>>) {
        let steps = STEPS.with_borrow_mut(Option::take).expect("a scripted run");
        let first = next(&steps);
        let model = Model {
            center: RefCell::new(NotificationCenter::new(CenterConfig::default())),
            ui: Ui::default(),
            now: 0.0,
            steps,
        };
        (model, vec![first])
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Step(step) => {
                match step {
                    Step::Notify(input) => {
                        model.center.get_mut().notify(*input, model.now);
                    }
                    Step::At(t) => model.now = t,
                    Step::OpenCentre => model.ui.centre_open = true,
                }
                model.center.get_mut().tick(model.now);
                vec![next(&model.steps)]
            }
            Message::Input(input) => {
                view::apply(model.center.get_mut(), &mut model.ui, input, model.now);
                Vec::new()
            }
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        view::windows(
            &mut model.center.borrow_mut(),
            &model.ui,
            model.now,
            Message::Input,
        )
    }
}

/// Writes down what a screen reader would announce: each live node, when it first reaches the tree and whenever
/// its name or description changes.
#[derive(Clone, Default)]
struct Announcements {
    said: Arc<Mutex<Vec<String>>>,
    live: Arc<Mutex<BTreeMap<String, String>>>,
}

impl AccessibilityAdapter for Announcements {
    fn is_active(&self) -> bool {
        true
    }

    fn window_focus(&mut self, _: WindowId, _: bool) {}

    fn update(&mut self, update: &AccessibilityUpdate) {
        for (id, node) in &update.nodes {
            let Some(live) = node.live else {
                continue;
            };
            let text = format!(
                "{live:?} {:?} {:?}",
                node.name.as_deref().unwrap_or(""),
                node.description.as_deref().unwrap_or("")
            );
            let key = format!("{:?}/{id:?}", update.window);
            let mut seen = self.live.lock().unwrap();
            if seen.get(&key) != Some(&text) {
                seen.insert(key, text.clone());
                self.said.lock().unwrap().push(text);
            }
        }
    }

    fn drain_actions(&mut self, _: &mut Vec<ActionRequest>) {}
}

fn theme() -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
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
    announcements: Announcements,
    configured: Vec<WindowId>,
}

impl Run {
    fn new() -> Self {
        let (steps, receiver) = async_channel::unbounded();
        STEPS.set(Some(receiver));
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let announcements = Announcements::default();
        let options = RuntimeOptions {
            theme: Arc::new(theme()),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
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
            accessibility: Some(Box::new(announcements.clone())),
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
            announcements,
            configured: Vec::new(),
        };
        run.settle();
        run
    }

    /// Runs until nothing is left to do: new windows are configured and every frame asked for is given.
    fn settle(&mut self) {
        for _ in 0..16 {
            self.driver.run_until_idle(budget()).unwrap();
            let mut busy = false;
            for (_, window) in self.driver.runtime().windows() {
                let system = self.driver.window_system_mut();
                if !self.configured.contains(&window) {
                    self.configured.push(window);
                    system
                        .configure(window, None, WindowStates::default())
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

    fn notify(&mut self, input: NotifyInput) {
        self.step(Step::Notify(Box::new(input)));
    }

    fn step(&mut self, step: Step) {
        self.steps.try_send(step).unwrap();
        self.settle();
    }

    fn windows(&self) -> Vec<(WindowKey, WindowId)> {
        self.driver.runtime().windows()
    }

    fn only_window(&self) -> WindowId {
        let windows = self.windows();
        assert_eq!(windows.len(), 1, "{windows:?}");
        windows[0].1
    }

    fn pointer(&mut self, window: WindowId, event: PointerEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(window, WindowEvent::Pointer(event, Timestamp(0)))
            .unwrap();
        self.settle();
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

    fn model_x(&self) -> Vec<f64> {
        let model = self.driver.runtime().model();
        let width = view::strip_width(
            &model.center.borrow().config.layout,
            &model.center.borrow().config.swipe,
        );
        model
            .center
            .borrow_mut()
            .placements(
                cupertino_kit::geometry::Size::new(width, 0.0),
                &mut view::CardMetrics,
                model.now,
            )
            .iter()
            .map(|p| p.rect.x())
            .collect()
    }

    /// Every open window's dump, in the order they opened, under a heading.
    fn frame(&self, heading: &str) -> String {
        let mut out = format!("== {heading}\n");
        for (key, window) in self.windows() {
            let snapshot = self.driver.runtime().snapshot(window).unwrap();
            out.push_str(&format!("-- {key:?}\n"));
            out.push_str(&unnumbered(&Inspector::default().dump(&snapshot)));
        }
        out
    }

    fn announced(&self) -> String {
        let said = self.announcements.said.lock().unwrap();
        let mut out = String::from("== announced\n");
        for line in said.iter() {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

/// The dump without the numbers of image resources, which the process's tests share and take in any order.
fn unnumbered(dump: &str) -> String {
    let mut out = String::with_capacity(dump.len());
    let mut rest = dump;
    while let Some(at) = rest.find("ResourceId(") {
        let start = at + "ResourceId(".len();
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        out.push('_');
        rest = &rest[digits..];
    }
    out.push_str(rest);
    out
}

fn golden(name: &str, dump: &str) {
    let path = format!("{}/tests/goldens/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::create_dir_all(format!("{}/tests/goldens", env!("CARGO_MANIFEST_DIR"))).unwrap();
        std::fs::write(&path, dump).unwrap();
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{path}: {e} (run with CUPERTINO_UPDATE_GOLDENS=1)"));
    if let Some(difference) = Inspector::first_difference(&expected, dump) {
        panic!("{name} differs from its golden: {difference}");
    }
}

fn mail(summary: &str, body: &str) -> NotifyInput {
    let mut input = NotifyInput::new("Mail", summary, body);
    input.hints.desktop_entry = Some("org.cupertino.Mail".into());
    input
}

#[test]
fn a_new_banner_slides_in_and_is_announced() {
    let mut run = Run::new();
    assert!(run.windows().is_empty(), "no banner, no window");
    run.notify(mail("Lunch", "Are you free at <b>noon</b>?").with_action("reply", "Reply"));
    let arriving = run.frame("arriving, t = 0");
    run.step(Step::At(1.0));
    run.only_window();
    assert_eq!(
        run.model_x(),
        vec![388.0 - 16.0 - 344.0],
        "at rest inside its strip"
    );
    let rest = run.frame("at rest, t = 1");
    // a critical one is assertive; the first gives way after its minimum second on screen
    run.notify(
        NotifyInput::new("Battery", "Battery low", "5% remaining").with_urgency(Urgency::Critical),
    );
    run.step(Step::At(1.1));
    assert_eq!(run.windows().len(), 2, "one leaving, one arriving");
    let crossing = run.frame("a critical banner takes over, t = 1.1");
    let dump = format!("{arriving}{rest}{crossing}{}", run.announced());
    assert!(
        dump.contains("Polite \"Mail: Lunch\" \"Are you free at noon?\""),
        "{dump}"
    );
    assert!(
        dump.contains("Assertive \"Battery: Battery low\""),
        "{dump}"
    );
    golden("banner", &dump);
}

#[test]
fn the_centre_stacks_each_application_s_notifications() {
    let mut run = Run::new();
    for (t, summary) in [(0.0, "First"), (1.0, "Second"), (2.0, "Third")] {
        run.step(Step::At(t));
        run.notify(mail(summary, "From the inbox"));
    }
    run.step(Step::At(3.0));
    run.notify(NotifyInput::new("Calendar", "Stand-up", "In 5 minutes"));
    run.step(Step::At(4.0));
    run.step(Step::OpenCentre);
    let (key, _) = run.windows()[0].clone();
    assert_eq!(key, view::centre_key(), "the centre replaces the banners");
    let dump = format!("{}{}", run.frame("centre"), run.announced());
    golden("centre-stacked", &dump);
}

#[test]
fn a_swipe_drags_the_banner_and_a_release_past_the_distance_dismisses_it() {
    let mut run = Run::new();
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    let window = run.only_window();
    let rest = run.model_x()[0];
    let press = Point::new(rest + 100.0, SHADOW_MARGIN + 20.0);
    run.pointer(window, PointerEvent::Enter { position: press });
    run.button(window, press, true);
    run.pointer(
        window,
        PointerEvent::Motion {
            position: Point::new(press.x + 60.0, press.y),
        },
    );
    assert_eq!(
        run.model_x(),
        vec![rest + 60.0],
        "the card follows the pointer"
    );
    let dragged = run.frame("dragged 60 pt to the right");
    run.button(window, Point::new(press.x + 60.0, press.y), false);
    // mid-way through the dismissal, the card is still on its way out
    run.step(Step::At(1.05));
    let x = run.model_x();
    assert_eq!(x.len(), 1, "the banner is leaving, not gone");
    assert!(x[0] > rest + 60.0, "it moves on to the right: {x:?}");
    let leaving = run.frame("released, t = 1.05");
    run.step(Step::At(3.0));
    assert!(
        run.windows().is_empty(),
        "the dismissed banner's window closed"
    );
    golden("swipe-dismiss", &format!("{dragged}{leaving}"));
}
