//! The banners and the notification centre as CupertinoKit draws them, on the headless window system. Each golden
//! is the Inspector dump of every open window after a scripted run of the model, with what a screen reader was told
//! to announce on the way. `CUPERTINO_UPDATE_GOLDENS=1` rewrites the goldens. The other tests drive the windows with
//! the pointer, the wheel, the touchpad and the keyboard and check the geometry of what they show.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::FrameSnapshot;
use cupertino_kit::core::{
    Action, Application, Command, Key, Live, PlaceholderIcons, Role, RuntimeOptions, Window,
    WindowKey,
};
use cupertino_kit::geometry::Point;
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, MaterialRegion, Mods, MouseButton, PointerEvent, ScrollSource,
    Timestamp, WindowEvent, WindowId, WindowRequest, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_notifications::view::{
    self, CardMetrics, Fold, HostAction, Input, Ui, SHADOW_MARGIN,
};
use cupertino_notifications::{CenterConfig, NotificationCenter, NotifyInput, Urgency};
use cupertino_theme::Theme;
use cupertino_ui_test::{AssistiveTechnology, Inspector, ManualClock, TestDriver};

/// What the test does to the model between frames.
#[derive(Clone, Debug)]
enum Step {
    Notify(Box<NotifyInput>),
    /// The model's clock, in seconds.
    At(f64),
    OpenCentre,
    /// The session locks (`true`) or unlocks, as the process learns it from the server.
    Lock(bool),
}

#[derive(Clone, Debug)]
enum Message {
    Step(Step),
    Input(Input),
    /// Focus the node with this key, once the view shows it.
    Focus(Key),
}

thread_local! {
    /// The steps of this thread's run and its cards' metrics, taken by [`Scripted::init`].
    static STEPS: RefCell<Option<(async_channel::Receiver<Step>, CardMetrics)>> = const { RefCell::new(None) };
}

struct Model {
    center: RefCell<NotificationCenter>,
    ui: Ui,
    now: f64,
    steps: async_channel::Receiver<Step>,
    metrics: CardMetrics,
    /// What the windows left to the host, in order.
    host: Vec<HostAction>,
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
        let (steps, metrics) = STEPS.with_borrow_mut(Option::take).expect("a scripted run");
        let first = next(&steps);
        let model = Model {
            center: RefCell::new(NotificationCenter::new(CenterConfig::default())),
            ui: Ui::default(),
            now: 0.0,
            steps,
            metrics,
            host: Vec::new(),
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
                    // the list is in place at once: these tests look at the open centre, not its slide
                    Step::OpenCentre => model.ui.open_centre(model.now, true),
                    Step::Lock(locked) => {
                        model.center.get_mut().set_screen_locked(locked, model.now)
                    }
                }
                // cleared cards that have left go from the model, as the application's housekeeping does
                model.ui.finish_removals(model.center.get_mut(), model.now);
                model.center.get_mut().tick(model.now);
                vec![next(&model.steps)]
            }
            Message::Input(input) => {
                let action = view::apply(model.center.get_mut(), &mut model.ui, input, model.now);
                model.host.extend(action.clone());
                match action {
                    Some(HostAction::Focus(key)) => {
                        vec![Command::Perform(Box::pin(
                            async move { Message::Focus(key) },
                        ))]
                    }
                    _ => Vec::new(),
                }
            }
            Message::Focus(key) => vec![Command::Focus(key)],
        }
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        view::windows(
            &mut model.center.borrow_mut(),
            &model.ui,
            &model.metrics,
            model.now,
            Message::Input,
        )
    }
}

fn theme(reduce_transparency: bool, increase_contrast: bool) -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance: if increase_contrast {
            Appearance::LightHighContrast
        } else {
            Appearance::Light
        },
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
    /// The stand-in screen reader: the trees it was sent, what live regions said as the AT-SPI bridge announces
    /// them (a live node's name, as it appears or is renamed), and the actions a test asks for.
    reader: AssistiveTechnology,
    configured: Vec<WindowId>,
    size: Option<Size>,
    /// The runtime's clock, moved only while a window animates.
    clock: ManualClock,
}

/// One frame at 60 Hz, in nanoseconds.
const FRAME_NS: u64 = 16_666_667;

/// How a run's window system and theme are set up.
#[derive(Clone, Copy, Default)]
struct Setup {
    /// The desktop draws materials under windows.
    effects: bool,
    reduce_transparency: bool,
    increase_contrast: bool,
    /// The size the windows are configured at instead of the one they ask for.
    size: Option<Size>,
}

impl Run {
    fn new() -> Self {
        Self::with(Setup::default())
    }

    fn with(setup: Setup) -> Self {
        let (steps, receiver) = async_channel::unbounded();
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let theme = Arc::new(theme(setup.reduce_transparency, setup.increase_contrast));
        STEPS.set(Some((
            receiver,
            CardMetrics::new(Arc::clone(&theme), Arc::clone(&text)),
        )));
        let glyphs = Arc::clone(&text);
        let reader = AssistiveTechnology::new();
        let options = RuntimeOptions {
            theme,
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
            accessibility: Some(reader.adapter()),
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let clock = ManualClock::new(Timestamp(0));
        let driver = TestDriver::new(
            Capabilities {
                app_effects: setup.effects,
                ..Capabilities::default()
            },
            options,
            clock.clone(),
        );
        let mut run = Run {
            driver,
            steps,
            reader,
            configured: Vec::new(),
            size: setup.size,
            clock,
        };
        run.settle();
        run
    }

    /// Runs until every window rests, then checks that a screen reader could read every window open.
    fn settle(&mut self) {
        self.settle_frames();
        self.driver.assert_accessible("Notifications");
    }

    /// Runs until nothing is left to do: new windows are configured and every frame asked for is given.
    /// A window that asks again for a frame at the same time is animating: the clock then moves one 60 Hz frame.
    fn settle_frames(&mut self) {
        let mut framed: Vec<(WindowId, Timestamp)> = Vec::new();
        for _ in 0..16 + 120 {
            self.driver.run_until_idle(budget()).unwrap();
            let mut busy = false;
            for (_, window) in self.driver.runtime().windows() {
                let system = self.driver.window_system_mut();
                if !self.configured.contains(&window) {
                    self.configured.push(window);
                    system
                        .configure(window, self.size, WindowStates::default())
                        .unwrap();
                    busy = true;
                } else if system.frame_requested(window).unwrap() {
                    let mut now = cupertino_kit::core::Clock::now(&self.clock);
                    if framed.contains(&(window, now)) {
                        self.clock.advance_by(FRAME_NS);
                        now = cupertino_kit::core::Clock::now(&self.clock);
                    }
                    framed.push((window, now));
                    let system = self.driver.window_system_mut();
                    system.frame_ready(window, now).unwrap();
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

    /// Where a banner's card rests in its strip: its leading edge, and its top below the room for its shadow.
    fn banner_rest(&self) -> Point {
        let model = self.driver.runtime().model();
        let center = model.center.borrow();
        let layout = &center.config.layout;
        let width = view::strip_width(layout, &center.config.swipe);
        Point::new(
            width - layout.right_margin - layout.width,
            layout.top_inset.min(SHADOW_MARGIN),
        )
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
            .placements(Size::new(width, 0.0), &mut model.metrics.clone(), model.now)
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

    /// What the stand-in reader heard, oldest first, one line each: how urgently, and the words.
    fn announced(&self) -> String {
        let mut out = String::from("== announced\n");
        for said in self.reader.announcements() {
            out.push_str(&format!("{:?} {:?}\n", said.live, said.text));
        }
        out
    }

    /// What the stand-in reader heard since the last call: how urgently, and the words.
    fn heard(&self) -> Vec<(Live, String)> {
        self.reader
            .take_announcements()
            .into_iter()
            .map(|said| (said.live, said.text))
            .collect()
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
        vec![run.banner_rest().x],
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
        dump.contains("Polite \"Mail: Lunch, Are you free at noon?\""),
        "{dump}"
    );
    assert!(
        dump.contains("Assertive \"Battery: Battery low, 5% remaining\""),
        "{dump}"
    );
    golden("banner", &dump);
}

#[test]
fn a_screen_reader_s_click_on_a_banner_s_action_button_invokes_its_action() {
    let mut run = Run::new();
    run.notify(mail("Lunch", "Are you free at noon?").with_action("reply", "Reply"));
    run.step(Step::At(1.0));
    let window = run.only_window();
    let reader = run.reader.clone();
    let tree = reader.tree(window).expect("the reader holds the banner");
    let reply = tree
        .nodes
        .iter()
        .find(|(_, n)| n.role == Role::Button && n.name.as_deref() == Some("Reply"))
        .map(|(id, _)| *id)
        .expect("the reader reads the action button");
    run.driver.ask(&reader, reply, Action::Click, None);
    run.settle();
    assert!(
        matches!(&run.host()[..], [HostAction::Invoke(_, key)] if key == "reply"),
        "{:?}",
        run.host()
    );
}

/// The words of every node of every tree the reader holds.
fn readable(run: &Run) -> String {
    let mut out = String::new();
    for (_, window) in run.windows() {
        for (_, node) in run.reader.tree(window).map(|t| t.nodes).unwrap_or_default() {
            for words in [&node.name, &node.description].into_iter().flatten() {
                out.push_str(words);
                out.push('\n');
            }
        }
    }
    out
}

#[test]
fn a_new_banner_says_its_application_its_title_and_its_body_but_not_its_buttons() {
    let mut run = Run::new();
    run.notify(
        mail("Lunch", "Are you free\nat <b>noon</b>?")
            .with_action("reply", "Reply")
            .with_action("later", "Later"),
    );
    run.step(Step::At(1.0));
    assert_eq!(
        run.heard(),
        [(
            Live::Polite,
            "Mail: Lunch, Are you free at noon?".to_owned()
        )],
        "said once, as it arrives"
    );
    // the buttons and the close button keep their own names, outside the banner's
    let window = run.only_window();
    let tree = run
        .reader
        .tree(window)
        .expect("the reader holds the banner");
    let buttons: Vec<_> = tree
        .nodes
        .iter()
        .filter(|(_, n)| n.role == Role::Button)
        .filter_map(|(_, n)| n.name.clone())
        .collect();
    for label in ["Reply", "Later"] {
        assert!(buttons.iter().any(|b| b == label), "{buttons:?}");
    }
    let live: Vec<_> = tree
        .nodes
        .iter()
        .filter(|(_, n)| n.live.is_some())
        .collect();
    assert_eq!(live.len(), 1, "one live node: the card");
    assert_eq!(live[0].1.role, Role::Group);
    // the pointer over it shows its close button: nothing new is said
    run.pointer(
        window,
        PointerEvent::Motion {
            position: Point::new(200.0, 40.0),
        },
    );
    run.step(Step::At(2.0));
    assert_eq!(run.heard(), []);
}

#[test]
fn a_banner_s_close_button_on_its_corner_stays_under_the_pointer_and_dismisses_it() {
    let mut run = Run::new();
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    let window = run.only_window();
    let card = run.banner_rest();
    assert_eq!(run.model_x(), [card.x]);
    run.pointer(
        window,
        PointerEvent::Enter {
            position: Point::new(card.x + 100.0, card.y + 20.0),
        },
    );
    let snapshot = run.snapshot(window);
    let close = keyed(&snapshot, "close")
        .expect("the pointer over the card shows its close button")
        .rect;
    assert_eq!(
        close.center(),
        view::close_rect(Rect::new(card.x, card.y, 0.0, 0.0)).center(),
        "centred on the card's corner curve"
    );
    // past the card, over the button's outer half, the banner is still hovered and the button still there
    let outer = Point::new(close.min_x() + 3.0, close.center().y);
    assert!(outer.x < card.x);
    run.click(window, outer);
    assert_eq!(run.host(), [], "the button took the press, not the card");
    run.step(Step::At(1.1));
    assert!(
        run.model_x()[0] > card.x,
        "the banner slides out: {:?}",
        run.model_x()
    );
}

#[test]
fn a_banner_updated_in_place_is_said_again() {
    let mut run = Run::new();
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    run.heard();
    let mut update = mail("Lunch", "Can we make it one o'clock?");
    update.replaces_id = 1;
    run.notify(update.clone());
    run.step(Step::At(1.5));
    assert_eq!(
        run.heard(),
        [(
            Live::Polite,
            "Mail: Lunch, Can we make it one o'clock?".to_owned()
        )]
    );
    let mut longer = mail(
        "Lunch moved",
        "Can we make it one o'clock? Or later?\n\nI have a call at noon.",
    );
    longer.replaces_id = 1;
    run.notify(longer);
    run.step(Step::At(2.0));
    assert_eq!(
        run.heard(),
        [(
            Live::Polite,
            "Mail: Lunch moved, Can we make it one o'clock? Or later? I have a call at noon."
                .to_owned()
        )],
        "a taller card is a new window, said as it appears"
    );
    // the same words again show nothing new, and say nothing new
    let mut same = mail(
        "Lunch moved",
        "Can we make it one o'clock? Or later?\n\nI have a call at noon.",
    );
    same.replaces_id = 1;
    run.notify(same);
    run.step(Step::At(2.5));
    assert_eq!(run.heard(), []);
    assert_eq!(run.windows().len(), 1);
}

#[test]
fn the_same_words_from_a_new_notification_are_said_again() {
    let mut run = Run::new();
    let words = (
        Live::Polite,
        "Mail: Lunch, Are you free at noon?".to_owned(),
    );
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    assert_eq!(run.heard(), std::slice::from_ref(&words));
    // a second notification with the same words takes the screen from the first: a new banner, said as it shows
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.5));
    assert_eq!(run.heard(), std::slice::from_ref(&words));
    // and again once the screen is empty
    for t in [7.0, 8.0] {
        run.step(Step::At(t));
    }
    assert!(run.windows().is_empty());
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(8.5));
    assert_eq!(run.heard(), [words]);
}

#[test]
fn a_banner_is_neither_said_nor_readable_while_the_screen_is_locked() {
    let mut run = Run::new();
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    run.heard();
    // the lock screen hides the banner on screen: its window goes
    run.step(Step::Lock(true));
    assert!(run.windows().is_empty());
    run.notify(mail("Bank", "Your sign-in code is 482913"));
    run.step(Step::At(1.5));
    assert!(run.windows().is_empty(), "nothing shows on the lock screen");
    assert_eq!(run.heard(), [], "nothing is said while locked");
    assert!(!readable(&run).contains("482913"));
    // at the unlock the banner that was on screen comes back; what arrived while locked is in the centre
    run.step(Step::Lock(false));
    run.step(Step::At(1.6));
    assert_eq!(
        run.windows().len(),
        1,
        "only the banner from before the lock"
    );
    assert!(readable(&run).contains("Lunch"));
    assert!(
        !readable(&run).contains("482913"),
        "the new one went to the centre"
    );
    assert_eq!(
        run.heard(),
        [],
        "nothing is said again at the unlock, and nothing that arrived while locked"
    );
}

#[test]
fn a_banner_back_at_the_unlock_is_not_said_again_but_its_next_update_is() {
    let mut run = Run::new();
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    assert_eq!(run.heard().len(), 1);
    run.step(Step::Lock(true));
    run.step(Step::At(30.0));
    run.step(Step::Lock(false));
    run.step(Step::At(30.1));
    assert_eq!(run.windows().len(), 1, "it kept the rest of its lifetime");
    assert_eq!(run.heard(), [], "said once, as it arrived");
    // an update after the unlock is news
    let mut update = mail("Lunch", "Can we make it one o'clock?");
    update.replaces_id = 1;
    run.notify(update);
    run.step(Step::At(30.5));
    assert_eq!(
        run.heard(),
        [(
            Live::Polite,
            "Mail: Lunch, Can we make it one o'clock?".to_owned()
        )]
    );
    // a new arrival after the unlock is said as it shows
    run.step(Step::At(40.0));
    run.notify(mail("Dinner", "At eight?"));
    run.step(Step::At(40.5));
    assert_eq!(
        run.heard(),
        [(Live::Polite, "Mail: Dinner, At eight?".to_owned())]
    );
}

#[test]
fn a_banner_of_an_application_without_previews_shows_and_says_only_notification() {
    use cupertino_defaults::notifications::{AppOverride, NotificationSettings, Previews};
    let mut run = Run::new();
    let mut policy = NotificationSettings::UNSET;
    policy.set_app(
        "org.cupertino.Mail",
        AppOverride {
            show_previews: Some(Previews::Never),
            ..AppOverride::default()
        },
    );
    run.driver
        .runtime()
        .model()
        .center
        .borrow_mut()
        .set_policy(policy);
    run.notify(mail("Bank", "Your sign-in code is 482913").with_action("copy", "Copy 482913"));
    run.step(Step::At(1.0));
    assert_eq!(
        run.heard(),
        [(Live::Polite, "Mail: Notification".to_owned())]
    );
    let words = readable(&run);
    assert!(
        !words.contains("482913") && !words.contains("Bank"),
        "{words}"
    );
    let banner = run.only_window();
    run.driver
        .look(banner, "banner-without-previews")
        .expect("with CUPERTINO_LOOK_DIR set, the banner is written");
    run.step(Step::OpenCentre);
    let words = readable(&run);
    assert!(words.contains("Mail: Notification"), "{words}");
    assert!(
        !words.contains("482913") && !words.contains("Bank"),
        "{words}"
    );
}

#[test]
fn announcing_a_banner_never_takes_the_focus() {
    let mut run = Run::new();
    run.notify(mail("Lunch", "Are you free at noon?").with_action("reply", "Reply"));
    run.step(Step::At(1.0));
    run.notify(
        NotifyInput::new("Battery", "Battery low", "5% remaining").with_urgency(Urgency::Critical),
    );
    run.step(Step::At(1.1));
    let said = run.heard();
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!(said[1].0, Live::Assertive);
    assert_eq!(
        run.reader.focused_window(),
        None,
        "no banner takes the keyboard"
    );
    for (_, window) in run.windows() {
        assert_eq!(run.reader.focus(window), None);
        assert_eq!(run.reader.focus_moves(window), 0);
        let tree = run.reader.tree(window).unwrap();
        assert!(
            tree.nodes
                .iter()
                .all(|(_, n)| n.live.is_none() || !n.states.focusable),
            "a banner's card is no Tab stop"
        );
    }
}

#[test]
fn banners_arriving_together_are_each_said_as_they_show() {
    let mut run = Run::new();
    // one on screen and one waiting, which the third replaces and sends to the centre: each banner a sighted user
    // sees is said as it shows, and the one that never shows as a banner is read in the centre
    for app in ["Mail", "Calendar", "Messages"] {
        run.notify(NotifyInput::new(app, "Hello", "Are you there?"));
    }
    let mut said = Vec::new();
    for t in [0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 6.0, 8.0] {
        run.step(Step::At(t));
        said.extend(run.heard());
    }
    assert_eq!(
        said,
        [
            (Live::Polite, "Mail: Hello, Are you there?".to_owned()),
            (Live::Polite, "Messages: Hello, Are you there?".to_owned()),
        ]
    );
    run.step(Step::OpenCentre);
    assert_eq!(run.heard(), [], "the centre's cards are not live");
    assert!(readable(&run).contains("Calendar: Hello"));
}

#[test]
fn the_centre_s_cards_hold_the_whole_body_for_a_reader_moving_through_the_list() {
    let mut run = Run::new();
    run.notify(mail("Release notes draft", RELEASE_NOTES));
    run.step(Step::At(1.0));
    run.notify(NotifyInput::new("Calendar", "Stand-up", "In 5 minutes"));
    run.step(Step::At(2.5));
    run.step(Step::OpenCentre);
    let window = run.only_window();
    let tree = run.reader.tree(window).unwrap();
    let mut items: Vec<_> = tree
        .nodes
        .iter()
        .filter(|(_, n)| n.role == Role::ListItem)
        .map(|(_, n)| (n.item_index, n.name.clone(), n.description.clone(), n.live))
        .collect();
    items.sort_by_key(|item| item.0);
    assert_eq!(
        items,
        [
            (
                Some(0),
                Some("Calendar: Stand-up".to_owned()),
                Some("In 5 minutes".to_owned()),
                None
            ),
            (
                Some(1),
                Some("Mail: Release notes draft".to_owned()),
                Some(RELEASE_NOTES.to_owned()),
                None
            ),
        ],
        "named for moving through the list, the whole body read on focus"
    );
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
fn a_click_expands_a_group_into_its_cards_and_show_less_collapses_it() {
    let mut run = Run::new();
    for (t, summary) in [(0.0, "First"), (1.0, "Second"), (2.0, "Third")] {
        run.step(Step::At(t));
        run.notify(mail(summary, "From the inbox"));
    }
    run.step(Step::At(3.0));
    run.notify(NotifyInput::new("Calendar", "Stand-up", "In 5 minutes"));
    run.step(Step::At(4.0));
    run.step(Step::OpenCentre);
    let window = run.only_window();
    let items = |run: &Run| -> Vec<(Option<String>, Option<bool>)> {
        let snapshot = run.snapshot(window);
        let mut items: Vec<_> = snapshot
            .accessibility
            .iter()
            .filter(|(_, a)| a.role == Role::ListItem)
            .map(|(_, a)| (a.item_index, a.name.clone(), a.states.expanded))
            .collect();
        items.sort();
        items
            .into_iter()
            .map(|(_, name, expanded)| (name, expanded))
            .collect()
    };
    assert_eq!(
        items(&run),
        [
            (Some("Calendar: Stand-up".into()), None),
            (Some("Mail: Third".into()), Some(false))
        ],
        "a collapsed group reads as its front card, not expanded"
    );
    let front = rect(&run.snapshot(window), "card-3");
    run.click(window, front.center());
    assert!(run.host().is_empty(), "the click expands the group");
    // half-way the cards are on their way out of the stack
    run.step(Step::At(4.08));
    let moving = rect(&run.snapshot(window), "card-2");
    run.step(Step::At(6.0));
    let settled = rect(&run.snapshot(window), "card-2");
    assert!(
        front.min_y() < moving.min_y()
            && moving.min_y() < settled.min_y()
            && moving.width() < settled.width(),
        "the card comes out from behind the front one, narrower until it is in place: {front:?} {moving:?} \
         {settled:?}"
    );
    assert_eq!(
        items(&run),
        [
            (Some("Calendar: Stand-up".into()), None),
            (Some("Mail: Third".into()), None),
            (Some("Mail: Second".into()), None),
            (Some("Mail: First".into()), None)
        ],
        "an expanded group lists each of its cards"
    );
    let dump = format!("{}{}", run.frame("expanded"), run.announced());
    golden("centre-expanded", &dump);
    let show_less = rect(&run.snapshot(window), "show-less");
    run.click(window, show_less.center());
    run.step(Step::At(8.0));
    assert_eq!(items(&run).len(), 2, "Show Less collapses the group");
}

#[test]
fn a_swipe_drags_the_banner_and_a_release_past_the_distance_dismisses_it() {
    let mut run = Run::new();
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    let window = run.only_window();
    let rest = run.model_x()[0];
    let press = Point::new(rest + 100.0, run.banner_rest().y + 20.0);
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

impl Run {
    fn snapshot(&self, window: WindowId) -> FrameSnapshot {
        self.driver.runtime().snapshot(window).unwrap()
    }

    fn send(&mut self, window: WindowId, event: WindowEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(window, event)
            .unwrap();
        self.settle();
    }

    fn key(&mut self, window: WindowId, keysym: u32) {
        for pressed in [true, false] {
            self.send(
                window,
                WindowEvent::Key(
                    KeyEvent {
                        code: 1,
                        keysym: Keysym(keysym),
                        text: None,
                        pressed,
                        repeat: false,
                        mods: Mods::NONE,
                    },
                    Timestamp(0),
                ),
            );
        }
    }

    fn click(&mut self, window: WindowId, at: Point) {
        self.pointer(window, PointerEvent::Motion { position: at });
        self.button(window, at, true);
        self.button(window, at, false);
    }

    fn scroll(&mut self, window: WindowId, at: Point, scroll: PointerEvent) {
        self.pointer(window, PointerEvent::Motion { position: at });
        self.pointer(window, scroll);
    }

    fn ui(&self) -> Ui {
        self.driver.runtime().model().ui.clone()
    }

    fn host(&self) -> Vec<HostAction> {
        self.driver.runtime().model().host.clone()
    }

    fn materials(&self, window: WindowId) -> Vec<Vec<MaterialRegion>> {
        self.driver
            .runtime()
            .window_system()
            .requests(window)
            .unwrap()
            .iter()
            .filter_map(|r| match r {
                WindowRequest::Materials(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }
}

/// The rect of the node with `key`, in window coordinates.
fn rect(snapshot: &FrameSnapshot, key: &str) -> Rect {
    keyed(snapshot, key)
        .unwrap_or_else(|| panic!("no node {key:?}"))
        .rect
}

fn keyed<'a>(
    snapshot: &'a FrameSnapshot,
    key: &str,
) -> Option<&'a cupertino_kit::core::NodeSnapshot> {
    snapshot
        .nodes
        .iter()
        .find(|n| n.key == Some(cupertino_kit::core::Key::from(key.to_owned())))
}

/// Every node under `root`, itself included.
fn subtree(
    snapshot: &FrameSnapshot,
    root: cupertino_kit::core::NodeId,
) -> Vec<&cupertino_kit::core::NodeSnapshot> {
    let mut out = Vec::new();
    let mut todo = vec![root];
    while let Some(id) = todo.pop() {
        if let Some(node) = snapshot.nodes.iter().find(|n| n.id == id) {
            todo.extend(node.children.iter().copied());
            out.push(node);
        }
    }
    out
}

fn inside(outer: Rect, inner: Rect) -> bool {
    const EPSILON: f64 = 0.01;
    inner.min_x() >= outer.min_x() - EPSILON
        && inner.min_y() >= outer.min_y() - EPSILON
        && inner.max_x() <= outer.max_x() + EPSILON
        && inner.max_y() <= outer.max_y() + EPSILON
}

const RELEASE_NOTES: &str = "Here is the full draft of the release notes so you can review it before the meeting. \
The first section covers the new notification centre, which now scrolls with the wheel, the touchpad and the \
keyboard, and wraps long messages instead of cutting them off after one line. The second section lists the fixes to \
the dock and the menu bar, and the third explains the changes to the settings app. Please read it all the way to the \
end, where the known issues and the upgrade notes are, and send me your comments by tomorrow afternoon.";

/// Fourteen applications' notifications, the last one with a body far longer than a card shows.
fn many(run: &mut Run) -> Vec<u32> {
    let mut ids = Vec::new();
    for i in 0..13 {
        run.step(Step::At(i as f64 * 0.1));
        run.notify(NotifyInput::new(
            &format!("App {i}"),
            &format!("Message {i}"),
            "A short body on one line.",
        ));
        ids.push(run.driver.runtime().model().center.borrow().history()[0].id);
    }
    run.notify(mail("Release notes draft", RELEASE_NOTES).with_action("reply", "Reply"));
    ids.push(run.driver.runtime().model().center.borrow().history()[0].id);
    run.step(Step::At(10.0));
    ids
}

#[test]
fn a_banner_s_long_body_wraps_inside_its_card_and_stops_at_its_line_limit() {
    let mut run = Run::new();
    run.notify(mail("Release notes draft", RELEASE_NOTES).with_action("reply", "Reply"));
    run.step(Step::At(1.0));
    let window = run.only_window();
    let snapshot = run.snapshot(window);
    let card = keyed(&snapshot, "card-1").expect("the banner's card");
    let spec = run
        .driver
        .runtime()
        .window_system()
        .spec(window)
        .unwrap()
        .clone();
    assert_eq!(
        spec.size.height,
        card.rect.max_y() + SHADOW_MARGIN,
        "the window is as tall as the card and its shadow"
    );
    let model = run.driver.runtime().model();
    let n = model.center.borrow().get(1).unwrap().clone();
    let layout = model.metrics.card(&n, card.rect.width(), Fold::Banner);
    assert_eq!(card.rect.height(), layout.height);
    let shown = layout.body.as_ref().unwrap().content.text.clone();
    assert!(shown.ends_with('\u{2026}'));
    let speech = view::banner_speech(&n, &layout);
    let content = card.rect.outset(-view::PADDING);
    for node in subtree(&snapshot, card.id).into_iter().skip(1) {
        if node.key.as_ref().is_some_and(|k| *k == "close".into()) {
            continue;
        }
        if node.widget.contains("::Place<") {
            continue;
        }
        assert!(
            inside(content, node.rect),
            "{} at {:?} leaves the card's content box {content:?}",
            node.widget,
            node.rect
        );
    }
    // the banner says what it shows, cut where the card cuts it; the centre holds the whole message
    let flat = shown.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(speech, format!("Mail: Release notes draft, {flat}"));
    assert_eq!(
        run.heard(),
        [(Live::Polite, speech.clone())],
        "the banner is said as it shows"
    );
    let semantics = &snapshot.accessibility;
    let named = semantics
        .iter()
        .find(|(_, a)| a.name.as_deref() == Some(speech.as_str()))
        .expect("the card is named after what it shows");
    assert_eq!(
        named.1.description, None,
        "the body is in the name, not said twice"
    );
}

#[test]
fn the_centre_scrolls_with_the_wheel_the_touchpad_and_the_keyboard_and_keeps_its_hit_targets() {
    let config = CenterConfig::default();
    let width = view::strip_width(&config.layout, &config.swipe);
    let mut run = Run::with(Setup {
        size: Some(Size::new(width, 480.0)),
        ..Setup::default()
    });
    let ids = many(&mut run);
    run.step(Step::OpenCentre);
    let window = run.only_window();
    run.send(window, WindowEvent::Focus(true));
    let top = run
        .driver
        .runtime()
        .model()
        .center
        .borrow()
        .config
        .layout
        .top_inset;
    let snapshot = run.snapshot(window);
    let list = rect(&snapshot, "list");
    assert_eq!(rect(&snapshot, "header-row").min_y(), top);
    assert_eq!(list.min_y(), top + view::HEADER_HEIGHT);
    assert_eq!(
        list.max_y(),
        480.0 - view::BOTTOM_MARGIN,
        "the list ends above the bottom edge"
    );
    let scroll = run.ui().scroll;
    assert!(
        scroll.content.height > scroll.viewport.height + 400.0,
        "{scroll:?}"
    );
    assert_eq!(scroll.offset.y, 0.0);

    let at = Point::new(list.center().x, list.min_y() + 100.0);
    run.scroll(
        window,
        at,
        PointerEvent::Scroll {
            position: at,
            delta: Point::ZERO,
            value120: Some((0, 120)),
            source: ScrollSource::Wheel,
            stop: false,
            inverted: (false, false),
        },
    );
    assert_eq!(
        run.ui().scroll.offset.y,
        40.0,
        "a wheel detent moves a line"
    );
    run.scroll(
        window,
        at,
        PointerEvent::Scroll {
            position: at,
            delta: Point::new(0.0, 25.4),
            value120: None,
            source: ScrollSource::Finger,
            stop: false,
            inverted: (false, false),
        },
    );
    assert_eq!(
        run.ui().scroll.offset.y,
        65.0,
        "the touchpad moves by its fingers, in whole points"
    );

    // the first card has the focus; the keys it does not use move the list
    let end = 0xff57;
    run.key(window, end);
    let scroll = run.ui().scroll;
    assert_eq!(
        scroll.offset.y,
        scroll.content.height - scroll.viewport.height
    );
    let snapshot = run.snapshot(window);
    let last = rect(&snapshot, &format!("card-{}", ids[0]));
    assert!(
        inside(list, last),
        "the oldest card shows at the end: {last:?} in {list:?}"
    );
    // a click lands on the card that shows there now
    run.click(window, Point::new(last.center().x, last.center().y));
    assert_eq!(run.host(), vec![HostAction::Activate(ids[0])]);
    let header = rect(&snapshot, "header-row");
    let newest = rect(&snapshot, &format!("card-{}", ids[13]));
    assert!(
        newest.max_y() <= header.max_y(),
        "the newest card scrolled away under the header"
    );

    run.key(window, 0xff50);
    assert_eq!(run.ui().scroll.offset.y, 0.0, "Home goes back to the top");
    run.key(window, 0xff56);
    let scroll = run.ui().scroll;
    assert_eq!(
        scroll.offset.y,
        scroll.viewport.height - 40.0,
        "Page Down keeps a line in view"
    );
}

#[test]
fn a_centre_card_unfolds_its_long_body_with_its_disclosure_button() {
    let mut run = Run::new();
    run.notify(mail("Release notes draft", RELEASE_NOTES));
    run.step(Step::At(1.0));
    run.step(Step::OpenCentre);
    let window = run.only_window();
    let snapshot = run.snapshot(window);
    let folded = rect(&snapshot, "card-1");
    let disclosure = rect(&snapshot, "disclosure");
    assert!(inside(folded, disclosure), "{folded:?} {disclosure:?}");
    run.click(window, disclosure.center());
    assert!(run.ui().unfolded.contains(&1));
    assert!(
        run.host().is_empty(),
        "the disclosure button does not activate the card"
    );
    let snapshot = run.snapshot(window);
    let unfolded = rect(&snapshot, "card-1");
    assert!(
        unfolded.height() > folded.height() + 30.0,
        "{folded:?} → {unfolded:?}"
    );
    let node = keyed(&snapshot, "disclosure").unwrap().id;
    let semantics = &snapshot
        .accessibility
        .iter()
        .find(|(id, _)| *id == node)
        .unwrap()
        .1;
    assert_eq!(semantics.states.expanded, Some(true));
    run.click(window, rect(&snapshot, "disclosure").center());
    assert!(run.ui().unfolded.is_empty());
}

#[test]
fn the_keyboard_activates_and_clears_the_centre_s_cards_and_its_close_button_clears_one() {
    let mut run = Run::new();
    for (t, app) in [
        (0.0, "Calendar"),
        (0.1, "Weather"),
        (0.2, "Backup"),
        (0.3, "Backup"),
    ] {
        run.step(Step::At(t));
        run.notify(NotifyInput::new(app, "Summary", "Body"));
    }
    run.step(Step::At(1.0));
    run.step(Step::OpenCentre);
    let window = run.only_window();
    run.send(window, WindowEvent::Focus(true));
    let history = |run: &Run| -> Vec<u32> {
        let model = run.driver.runtime().model();
        let ids = model
            .center
            .borrow()
            .history()
            .iter()
            .map(|n| n.id)
            .collect();
        ids
    };
    let focused = |run: &Run| -> Option<String> {
        let snapshot = run.snapshot(window);
        let node = snapshot
            .nodes
            .iter()
            .find(|n| Some(n.id) == snapshot.focus)?;
        node.key.as_ref().map(|k| format!("{k:?}"))
    };
    let key_of = |id: u32| Some(format!("{:?}", view::card_key(id)));
    let snapshot = run.snapshot(window);
    let first = keyed(&snapshot, "card-4").unwrap().id;
    assert_eq!(
        snapshot.focus,
        Some(first),
        "the newest card takes the focus"
    );
    let list = snapshot
        .accessibility
        .iter()
        .find(|(_, a)| a.role == cupertino_kit::core::Role::List)
        .expect("the cards are a list");
    assert_eq!(list.1.children.len(), 3, "one card for each application");
    // the newest card fronts a collapsed group of two: Return expands the group, and then activates the card
    run.key(window, 0xff0d);
    assert!(run.host().is_empty(), "Return expands the group");
    assert_eq!(run.ui().expanded.len(), 1);
    assert_eq!(focused(&run), key_of(4), "the focus stays on the card");
    run.key(window, 0xff0d);
    assert_eq!(
        run.host(),
        vec![HostAction::Activate(4)],
        "Return activates it"
    );
    // a cleared card leaves the model once it has slid away
    run.key(window, 0xffff);
    run.step(Step::At(1.5));
    assert_eq!(history(&run), vec![3, 2, 1], "Delete clears it");
    assert_eq!(
        focused(&run),
        key_of(3),
        "the focus goes to the group's next card, which the view built after the update"
    );
    run.key(window, 0xffff);
    run.step(Step::At(2.0));
    assert_eq!(history(&run), vec![2, 1]);
    assert_eq!(
        focused(&run),
        key_of(2),
        "and then to the next group's card"
    );

    // the pointer over a card reveals its close button
    let card = rect(&run.snapshot(window), "card-1");
    run.pointer(
        window,
        PointerEvent::Motion {
            position: card.center(),
        },
    );
    assert_eq!(run.ui().revealed, Some(1));
    let snapshot = run.snapshot(window);
    let close = subtree(&snapshot, keyed(&snapshot, "card-1").unwrap().id)
        .into_iter()
        .find(|n| n.key.as_ref().is_some_and(|k| *k == "close".into()))
        .unwrap()
        .rect;
    assert_eq!(
        (close.min_x() - card.min_x(), close.min_y() - card.min_y()),
        (-view::CLOSE_REACH_X, -view::CLOSE_REACH_Y),
        "centred on the card's top-leading corner curve"
    );
    assert!(
        close.min_y() >= rect(&snapshot, "list-content").min_y(),
        "the first card leaves its close button room under the header"
    );
    run.click(window, close.center());
    run.step(Step::At(2.5));
    assert_eq!(history(&run), vec![2]);
    assert_eq!(
        focused(&run),
        key_of(2),
        "a card cleared with the pointer leaves the focus where it was"
    );
    assert_eq!(
        run.host(),
        vec![
            HostAction::Activate(4),
            HostAction::Focus(view::card_key(3)),
            HostAction::Focus(view::card_key(2)),
        ],
        "the close button activates nothing"
    );

    // the last card cleared, Escape still closes the empty centre
    run.key(window, 0xffff);
    run.step(Step::At(3.0));
    assert!(history(&run).is_empty());
    // a press only: the centre's window closes on it
    run.send(
        window,
        WindowEvent::Key(
            KeyEvent {
                code: 1,
                keysym: Keysym(0xff1b),
                text: None,
                pressed: true,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        ),
    );
    assert!(!run.ui().centre_open);
}

/// The strokes along a card's edge in the banner's window: the outline and the rim the card lays over its glass.
fn card_edge_strokes(effects: bool, increase_contrast: bool) -> usize {
    let mut run = Run::with(Setup {
        effects,
        increase_contrast,
        ..Setup::default()
    });
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    let window = run.only_window();
    let snapshot = run.snapshot(window);
    let card = rect(&snapshot, "card-1");
    snapshot
        .display_list
        .nodes
        .iter()
        .flat_map(|n| n.ops.iter())
        .filter(|op| {
            matches!(op, cupertino_kit::render::PaintOp::Stroke { shape: cupertino_kit::render::Shape::RoundedRect { rect, .. }, .. }
                if (rect.size.width - card.size.width).abs() < 2.0 && (rect.size.height - card.size.height).abs() < 2.0)
        })
        .count()
}

#[test]
fn a_card_on_glass_paints_no_edge_at_rest_and_its_rim_with_increase_contrast() {
    assert_eq!(
        card_edge_strokes(true, false),
        0,
        "the compositor's glass marks the card's edge at rest"
    );
    assert!(
        card_edge_strokes(true, true) > 0,
        "with Increase Contrast the card lays its rim over the glass"
    );
    assert!(
        card_edge_strokes(false, false) > 0,
        "without the desktop's glass the card strokes its outline"
    );
}

#[test]
fn each_card_shows_the_compositor_s_material_where_the_desktop_draws_it_and_its_fill_elsewhere() {
    let mut run = Run::with(Setup {
        effects: true,
        ..Setup::default()
    });
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    let window = run.only_window();
    let card = rect(&run.snapshot(window), "card-1");
    let shown = run
        .materials(window)
        .last()
        .cloned()
        .expect("materials were asked for");
    assert_eq!(
        shown,
        vec![MaterialRegion {
            material: "CardGlass".into(),
            rect: card,
            radius: view::CORNER_RADIUS,
            alpha: 1.0,
            glass: Some(cupertino_kit::platform::GlassRegion {
                style: cupertino_kit::platform::GlassStyle::Card,
                container: 0,
                spacing: 0.0,
            }),
            raised: false,
        }],
        "the card glass under the banner's card and nowhere else in its strip"
    );
    run.notify(NotifyInput::new("Calendar", "Stand-up", "In 5 minutes"));
    run.notify(NotifyInput::new("Calendar", "Review", "In 10 minutes"));
    run.step(Step::At(2.0));
    run.step(Step::OpenCentre);
    let window = run.only_window();
    let snapshot = run.snapshot(window);
    let regions = run.materials(window).last().cloned().unwrap();
    let rects: Vec<Rect> = regions.iter().map(|r| r.rect).collect();
    assert_eq!(
        rects[1..],
        [rect(&snapshot, "card-3"), rect(&snapshot, "card-1")],
        "each front card is glass, and nothing behind the header"
    );
    let container = |r: &MaterialRegion| r.glass.map(|g| (g.container, g.spacing));
    assert!(
        rects.len() == 3
            && container(&regions[0]) == container(&regions[1])
            && container(&regions[0]).is_some_and(|(c, spacing)| c != 0 && spacing == 0.0)
            && container(&regions[2]) != container(&regions[0]),
        "the sheet behind a group's front card joins its glass in one shape: {regions:?}"
    );

    let mut run = Run::with(Setup {
        effects: true,
        reduce_transparency: true,
        ..Setup::default()
    });
    run.notify(mail("Lunch", "Are you free at noon?"));
    run.step(Step::At(1.0));
    let window = run.only_window();
    let shown = run.materials(window).last().cloned().unwrap_or_default();
    assert!(
        shown.len() == 1 && shown[0].glass.is_some(),
        "Reduce Transparency keeps the card's glass, drawn with the compositor's recipe for it: {shown:?}"
    );
    let fills = run
        .snapshot(window)
        .display_list
        .nodes
        .iter()
        .flat_map(|n| n.ops.iter())
        .filter(|op| {
            matches!(op, cupertino_kit::render::PaintOp::Fill { shape: cupertino_kit::render::Shape::RoundedRect { rect, .. }, .. } if rect.size == card.size)
        })
        .count();
    assert_eq!(
        fills, 1,
        "the card's shape is filled once, as the clipped source of its shadow outside the glass"
    );
}

#[test]
fn past_the_glass_one_window_shows_the_lower_groups_sheets_go_opaque_and_every_card_keeps_its_glass(
) {
    let mut run = Run::with(Setup {
        effects: true,
        ..Setup::default()
    });
    // fourteen collapsed groups of three: a card and two sheets each, more glass than one window shows
    let apps: Vec<String> = (0..14).map(|n| format!("App {n}")).collect();
    let mut t = 0.0;
    for app in &apps {
        for n in 0..3 {
            t += 0.01;
            run.step(Step::At(t));
            run.notify(NotifyInput::new(app, &format!("Summary {n}"), "Body"));
        }
    }
    run.step(Step::At(t + 1.0));
    run.step(Step::OpenCentre);
    let window = run.only_window();
    let snapshot = run.snapshot(window);
    let regions = run.materials(window).last().cloned().unwrap();
    assert!(
        regions.len() <= cupertino_kit::core::MAX_MATERIALS,
        "{} glass regions",
        regions.len()
    );
    let model = run.driver.runtime().model();
    let fronts: Vec<u32> = model
        .center
        .borrow()
        .groups()
        .iter()
        .map(|g| g.notifications[0].id)
        .collect();
    assert_eq!(fronts.len(), 14);
    for id in fronts {
        let card = rect(&snapshot, &format!("card-{id}"));
        assert!(
            regions.iter().any(|r| r.rect == card),
            "card {id} at {card:?} keeps its glass: {regions:?}"
        );
    }
    // the sheets of the groups at the top keep theirs, a whole group at a time
    let sheets = regions.len() - 14;
    assert!(sheets > 0 && sheets % 2 == 0, "{sheets} sheets of glass");
}

#[test]
fn the_keyboard_reveals_the_card_it_is_on_and_tab_reaches_its_buttons_whatever_the_pointer_does() {
    let mut run = Run::new();
    for (t, app) in [(0.0, "Calendar"), (0.1, "Weather")] {
        run.step(Step::At(t));
        let mut input = NotifyInput::new(app, "Summary", "Body");
        input.actions = vec!["open".into(), "Open".into()];
        run.notify(input);
    }
    run.step(Step::At(1.0));
    run.step(Step::OpenCentre);
    let window = run.only_window();
    run.send(window, WindowEvent::Focus(true));
    let focused_key = |run: &Run| -> Option<String> {
        let snapshot = run.snapshot(window);
        let node = snapshot
            .nodes
            .iter()
            .find(|n| Some(n.id) == snapshot.focus)?;
        node.key.as_ref().map(|k| format!("{k:?}"))
    };
    let buttons_of = |run: &Run, id: u32| -> Vec<String> {
        let snapshot = run.snapshot(window);
        let card = keyed(&snapshot, &format!("card-{id}")).unwrap().id;
        let mut keys: Vec<String> = subtree(&snapshot, card)
            .into_iter()
            .filter(|n| Some(n.id) != Some(card))
            .filter_map(|n| n.key.as_ref().map(|k| format!("{k:?}")))
            .filter(|k| k.contains("close") || k.contains("action"))
            .collect();
        keys.sort();
        keys
    };
    // Tab goes from card to card until one shows its ring; that card shows its buttons
    let mut on = None;
    for _ in 0..4 {
        run.key(window, 0xff09);
        if let Some(id) = run.ui().focused {
            on = Some(id);
            break;
        }
    }
    let id = on.expect("the keyboard reveals a card");
    assert_eq!(focused_key(&run), Some(format!("{:?}", view::card_key(id))));
    assert!(
        !buttons_of(&run, id).is_empty(),
        "the card the keyboard is on shows its buttons"
    );
    let other = if id == 1 { 2 } else { 1 };
    assert!(buttons_of(&run, other).is_empty(), "and only that card");
    // the pointer moving over no card, then over the other card, takes nothing from the keyboard's card
    let card = rect(&run.snapshot(window), &format!("card-{id}"));
    run.pointer(
        window,
        PointerEvent::Motion {
            position: Point::new(card.center().x, card.max_y() + 200.0),
        },
    );
    assert_eq!(run.ui().focused, Some(id));
    assert!(!buttons_of(&run, id).is_empty());
    let over = rect(&run.snapshot(window), &format!("card-{other}"));
    run.pointer(
        window,
        PointerEvent::Motion {
            position: over.center(),
        },
    );
    assert_eq!(
        run.ui().revealed,
        Some(other),
        "hover reveals the other card"
    );
    assert!(
        !buttons_of(&run, id).is_empty(),
        "and the keyboard's keeps its buttons"
    );
    // Tab goes on into the card's buttons, and the card stays revealed while the keyboard is in it
    run.key(window, 0xff09);
    let inside = focused_key(&run).expect("a focused node");
    assert!(
        buttons_of(&run, id).contains(&inside),
        "{inside} is one of card {id}'s buttons {:?}",
        buttons_of(&run, id)
    );
    assert_eq!(run.ui().focused, Some(id));
}
