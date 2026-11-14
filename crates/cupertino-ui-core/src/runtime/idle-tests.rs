//! A window's idle time: a node that asked for it in its paint or its measure is called once, between frames and never
//! inside one, in slices that end before the next frame is due and before a wake; the first event that arrives ends it,
//! work left goes on in a later slice, and once it is done the loop waits as it did without it. What the work shapes
//! the next frame's layout finds in the text system's cache, as it would have shaped it.

use super::idle::{FrameClock, FRAME_MARGIN_NS, IDLE_SLICE_NS};
use super::*;
use crate::test_support::{options, FakeClock, FakeSystem};
use crate::{Change, EventCx, IdleCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_platform::{Configure, DecorationMode, WindowStates, WmCapabilities};
use cupertino_ui_text::{AttributedText, FontIndex, ParagraphStyle, TextStyle, TextWork};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::Ordering;

const FONT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cupertino-assets/fonts/InterVariable-subset.ttf"
));
const MS: u64 = 1_000_000;

/// One call of a node's idle work: which node, when it began and when its slice ends.
#[derive(Clone, Debug, PartialEq)]
struct Call {
    node: &'static str,
    at: u64,
    until: u64,
    /// How many times the runtime had looked at the window system before it.
    looks: usize,
}

/// The window system's record of the timeout of each of its waits.
type Waits = Rc<RefCell<Vec<Option<Duration>>>>;

thread_local! {
    static CALLS: RefCell<Vec<Call>> = RefCell::default();
    /// The test's clock, which a piece of idle work moves on by its cost.
    static CLOCK: RefCell<Option<Arc<FakeClock>>> = const { RefCell::new(None) };
    /// The window system's record of its waits and the events that arrive while the runtime is busy.
    static WAITS: RefCell<Option<Waits>> = const { RefCell::new(None) };
    static ARRIVING: RefCell<Option<Rc<RefCell<VecDeque<PlatformEvent>>>>> = const { RefCell::new(None) };
}

fn now() -> u64 {
    CLOCK.with(|c| c.borrow().as_ref().unwrap().0.load(Ordering::SeqCst))
}

fn set_now(at: u64) {
    CLOCK.with(|c| c.borrow().as_ref().unwrap().0.store(at, Ordering::SeqCst));
}

fn calls() -> Vec<Call> {
    CALLS.with(|c| c.borrow().clone())
}

fn style() -> TextStyle {
    TextStyle::new(13.0)
}

/// What the window shows: a label for each of `labels`, and a node that shapes `ahead` in idle time.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shown {
    labels: Vec<String>,
    ahead: Vec<String>,
    /// How long shaping one of `ahead` takes, on the test's clock.
    cost: u64,
    /// The node asks from its measure, else from its paint.
    from_measure: bool,
    /// Bumped to make the node paint and measure again.
    revision: u32,
    /// After its first piece, the node's work sends the window an event.
    interrupts: bool,
    /// The node is in the view.
    ahead_shown: bool,
}

struct Idle;

impl Application for Idle {
    type Model = Shown;
    type Message = Shown;

    fn init() -> (Shown, Vec<Command<Shown>>) {
        (Shown::default(), Vec::new())
    }

    fn update(model: &mut Shown, message: Shown) -> Vec<Command<Shown>> {
        *model = message;
        Vec::new()
    }

    fn view(model: &Shown) -> Vec<Window<Shown>> {
        // the node first, so the labels never move it
        let mut children: Vec<Element<Shown>> = Vec::new();
        if model.ahead_shown {
            children.push(
                Element::new(Ahead {
                    texts: model.ahead.clone(),
                    cost: model.cost,
                    from_measure: model.from_measure,
                    revision: model.revision,
                    interrupts: model.interrupts,
                })
                .key("ahead"),
            );
        }
        children.extend(
            model.labels.iter().enumerate().map(|(i, text)| {
                Element::new(Label { text: text.clone() }).key(format!("label-{i}"))
            }),
        );
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Idle".into(),
                app_id: "test.idle".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            popup: None,
            background: WindowBackground::None,
            content: Element::new(Column { children }),
            menus: None,
        }]
    }
}

struct Column {
    children: Vec<Element<Shown>>,
}

impl Widget<Shown> for Column {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: cupertino_ui_layout::Axis::Vertical,
                spacing: 0.0,
                align: cupertino_ui_layout::Align::Start,
                justify: cupertino_ui_layout::Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Shown>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Shown>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A line of text, measured as its paragraph lays out.
struct Label {
    text: String,
}

impl Widget<Shown> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.text != old.text {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let text = AttributedText::plain(self.text.clone(), style());
        let size = cx
            .env()
            .text
            .measure(
                &text,
                &ParagraphStyle::default(),
                Some(constraints.width.max() as f32),
            )
            .unwrap();
        Measured {
            size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Shown>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Shapes `texts` in idle time, one a piece while its slice has time, each piece moving the clock on by `cost`; it
/// asks again for what is left.
struct Ahead {
    texts: Vec<String>,
    cost: u64,
    from_measure: bool,
    revision: u32,
    interrupts: bool,
}

impl Widget<Shown> for Ahead {
    /// The next of `texts` to shape.
    type State = Cell<usize>;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(10.0),
            height: SizeSpec::Fixed(10.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> Cell<usize> {
        Cell::new(0)
    }

    fn update(&self, old: &Self, _: &mut Cell<usize>) -> Change {
        if self.revision != old.revision {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Cell<usize>, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        if self.from_measure {
            cx.request_idle();
        }
        Measured {
            size: Size::new(10.0, 10.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut Cell<usize>, _: &Event, _: &mut EventCx<'_, Shown>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &Cell<usize>, cx: &mut PaintCx<'_>) {
        if !self.from_measure {
            cx.request_idle();
        }
    }

    fn semantics(&self, _: &Cell<usize>, _: &mut SemanticsCx<'_>) {}

    fn idle(&self, next: &Cell<usize>, cx: &mut IdleCx<'_>) {
        let looks = WAITS.with(|w| w.borrow().as_ref().unwrap().borrow().len());
        CALLS.with(|c| {
            c.borrow_mut().push(Call {
                node: "ahead",
                at: now(),
                until: cx.until().0,
                looks,
            })
        });
        while next.get() < self.texts.len() {
            if !cx.has_time() {
                cx.request_idle();
                return;
            }
            let text = AttributedText::plain(self.texts[next.get()].clone(), style());
            cx.env()
                .text
                .paragraph(&text, &ParagraphStyle::default())
                .unwrap();
            next.set(next.get() + 1);
            set_now(now() + self.cost);
            if self.interrupts && next.get() == 1 {
                ARRIVING.with(|a| {
                    a.borrow()
                        .as_ref()
                        .unwrap()
                        .borrow_mut()
                        .push_back(PlatformEvent::Window {
                            window: WindowId(1),
                            event: WindowEvent::Focus(true),
                        })
                });
            }
        }
    }
}

type Rt = Runtime<Idle, FakeSystem>;

fn steps() -> NonZeroU32 {
    NonZeroU32::new(16).unwrap()
}

fn text_system() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(FONT.to_vec())).unwrap();
    Arc::new(TextSystem::new(fonts, "Inter"))
}

/// The window configured and its first frame presented, showing `shown`, at time 0.
fn start(shown: Shown) -> (Rt, WindowId) {
    CALLS.with(|c| c.borrow_mut().clear());
    let clock = Arc::new(FakeClock::default());
    CLOCK.with(|c| *c.borrow_mut() = Some(Arc::clone(&clock)));
    let mut options = options(&Rc::default());
    options.clock = clock;
    options.text = text_system();
    let system = FakeSystem::default();
    WAITS.with(|w| *w.borrow_mut() = Some(Rc::clone(&system.waits)));
    ARRIVING.with(|a| *a.borrow_mut() = Some(Rc::clone(&system.arriving)));
    let mut rt = Rt::new(system, options);
    rt.messages.push_back(shown);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    rt.window_system_mut().send(
        window,
        WindowEvent::Configure(Configure {
            id: ConfigureId(1),
            size: Some(Size::new(200.0, 100.0)),
            states: WindowStates {
                activated: true,
                ..WindowStates::default()
            },
            bounds: None,
            server_decorations: true,
            wm_capabilities: WmCapabilities {
                window_menu: true,
                maximize: true,
                fullscreen: true,
                minimize: true,
            },
        }),
    );
    rt.step(steps()).unwrap();
    assert!(!rt.windows[0].frame_ready, "the first frame is presented");
    (rt, window)
}

/// The compositor is ready for the next frame at `at`; the window draws `shown` in it.
fn frame(rt: &mut Rt, window: WindowId, at: u64, shown: Option<Shown>) {
    set_now(at);
    rt.messages.extend(shown);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(at)));
    rt.step(steps()).unwrap();
}

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("name number {i}")).collect()
}

#[test]
fn idle_work_runs_between_frames_never_inside_one_and_once_for_each_request() {
    let shown = Shown {
        ahead: names(3),
        ahead_shown: true,
        ..Shown::default()
    };
    let (mut rt, window) = start(shown.clone());
    assert!(calls().is_empty(), "the frame that asked ran no idle work");
    assert!(rt.has_idle_work());

    // an event that arrived goes first: the next frame is drawn before any idle work
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(0)));
    rt.run_idle().unwrap();
    assert!(calls().is_empty(), "nothing runs while an event waits");
    let again = Shown {
        revision: 1,
        ..shown.clone()
    };
    rt.messages.push_back(again.clone());
    rt.step(steps()).unwrap();
    assert!(calls().is_empty(), "a frame runs no idle work");

    rt.run_idle().unwrap();
    assert_eq!(calls().len(), 1, "called once after the frame");
    assert!(!rt.has_idle_work(), "done, it asked for no more");
    rt.run_idle().unwrap();
    assert_eq!(calls().len(), 1, "the request is cleared by the call");

    // a frame whose paint asks again calls it again; one that does not paint it, not
    frame(
        &mut rt,
        window,
        16 * MS,
        Some(Shown {
            revision: 2,
            ..shown.clone()
        }),
    );
    rt.run_idle().unwrap();
    assert_eq!(calls().len(), 2);
    frame(
        &mut rt,
        window,
        32 * MS,
        Some(Shown {
            labels: vec!["a".into()],
            revision: 2,
            ..shown
        }),
    );
    rt.run_idle().unwrap();
    assert_eq!(calls().len(), 2, "its paint was kept, so it did not ask");
}

#[test]
fn a_node_asks_from_its_measure_and_one_that_went_away_is_not_called() {
    let shown = Shown {
        ahead: names(2),
        from_measure: true,
        ahead_shown: true,
        ..Shown::default()
    };
    let (mut rt, window) = start(shown.clone());
    rt.run_idle().unwrap();
    assert_eq!(calls().len(), 1, "asked from its measure");

    frame(
        &mut rt,
        window,
        16 * MS,
        Some(Shown {
            revision: 1,
            ..shown.clone()
        }),
    );
    assert!(rt.has_idle_work());
    // gone before its idle time came
    frame(
        &mut rt,
        window,
        32 * MS,
        Some(Shown {
            ahead_shown: false,
            ..shown
        }),
    );
    rt.run_idle().unwrap();
    assert_eq!(calls().len(), 1);
    assert!(!rt.has_idle_work());
}

#[test]
fn slices_end_before_the_next_frame_is_due_and_before_a_wake_and_work_left_goes_on_after_the_frame()
{
    const PERIOD: u64 = 16 * MS;
    let shown = Shown {
        ahead: names(120),
        cost: MS / 4,
        ahead_shown: true,
        ..Shown::default()
    };
    let (mut rt, window) = start(shown.clone());
    // frames a period apart: the frame clock learns the period
    for (i, at) in [PERIOD, 2 * PERIOD].into_iter().enumerate() {
        let revision = i as u32 + 1;
        frame(
            &mut rt,
            window,
            at,
            Some(Shown {
                revision,
                ..shown.clone()
            }),
        );
    }
    assert_eq!(rt.windows[0].frame_clock.period(), Some(PERIOD));
    let due = 3 * PERIOD;
    // the frame took 3 ms
    set_now(2 * PERIOD + 3 * MS);
    rt.run_idle().unwrap();
    let slices = calls();
    assert!(slices.len() > 1, "{slices:?}");
    for call in &slices {
        assert!(
            call.until <= call.at + IDLE_SLICE_NS,
            "a slice is short: {call:?}"
        );
        assert!(
            call.until <= due - FRAME_MARGIN_NS,
            "it ends before the frame is due: {call:?}"
        );
    }
    for pair in slices.windows(2) {
        assert!(
            pair[1].looks > pair[0].looks,
            "the window system is looked at between slices"
        );
    }
    assert!(
        now() <= due - FRAME_MARGIN_NS + MS / 4,
        "stopped by {}",
        now()
    );
    assert!(rt.has_idle_work(), "work is left for after the frame");
    assert_eq!(
        rt.idle_resume(),
        Some(Timestamp(due + PERIOD)),
        "a frame a period late no longer holds the work back"
    );

    // the frame came and drew: the work goes on after it, up to the frame after
    frame(
        &mut rt,
        window,
        due,
        Some(Shown {
            revision: 3,
            ..shown.clone()
        }),
    );
    let before = calls().len();
    rt.run_idle().unwrap();
    let after = calls();
    assert!(after.len() > before);
    assert!(after[before..]
        .iter()
        .all(|c| c.at >= due && c.until <= due + PERIOD - FRAME_MARGIN_NS));
    assert!(rt.has_idle_work());
    // a frame a period late, held back as for a window not shown: the work no longer waits for it
    set_now(due + 2 * PERIOD);
    rt.run_idle().unwrap();
    assert!(!rt.has_idle_work(), "done");

    // a wake ends a slice too
    let mut asleep = start(shown).0;
    let root = asleep.windows[0].root.unwrap();
    asleep.windows[0].wakes.insert(root, Timestamp(MS / 2));
    asleep.run_idle().unwrap();
    let woke = calls();
    assert_eq!(woke.len(), 1, "{woke:?}");
    assert_eq!(woke[0].until, MS / 2, "the slice ends at the wake");
    assert!(asleep.has_idle_work());
}

#[test]
fn the_first_event_that_arrives_ends_the_idle_time() {
    let shown = Shown {
        ahead: names(20),
        cost: MS / 4,
        interrupts: true,
        ahead_shown: true,
        ..Shown::default()
    };
    let (mut rt, _) = start(shown);
    rt.run_idle().unwrap();
    assert_eq!(calls().len(), 1, "the slice ran on, the next did not start");
    assert!(rt.ready(), "the event waits to be handled");
    assert!(rt.has_idle_work());
}

#[test]
fn once_the_work_is_done_the_loop_waits_as_it_does_without_it() {
    let blocking = |shown: Shown| {
        let (mut rt, _) = start(shown);
        rt.window_system_mut().close_on_wait = true;
        let waits = Rc::clone(&rt.window_system().waits);
        let first = waits.borrow().len();
        rt.run().unwrap();
        let waits = waits.borrow()[first..].to_vec();
        let at = waits
            .iter()
            .position(|w| *w != Some(Duration::ZERO))
            .unwrap();
        (waits[at], first + at)
    };
    let (without, _) = blocking(Shown::default());
    assert_eq!(without, None, "nothing to do: the loop waits for an event");
    let (with, at) = blocking(Shown {
        ahead: names(12),
        cost: MS / 2,
        ahead_shown: true,
        ..Shown::default()
    });
    assert_eq!(with, without, "the work adds no wakeup");
    let slices = calls();
    assert!(slices.len() > 1, "{slices:?}");
    assert!(
        // `looks` is how many waits came before the call; the wait at `at` comes after it
        slices.iter().all(|c| c.looks <= at),
        "the work ran before the loop waited: {slices:?}, waited at {at}"
    );
}

#[test]
fn what_idle_work_shapes_the_next_frame_lays_out_without_shaping_and_as_it_shapes_itself() {
    let texts = names(8);
    let shown = Shown {
        ahead: texts.clone(),
        ahead_shown: true,
        ..Shown::default()
    };
    let (mut rt, window) = start(shown.clone());
    let before = TextWork::current().shaped;
    rt.run_idle().unwrap();
    assert_eq!(TextWork::current().shaped - before, texts.len() as u64);

    let laid = TextWork::current().shaped;
    frame(
        &mut rt,
        window,
        16 * MS,
        Some(Shown {
            labels: texts.clone(),
            ..shown
        }),
    );
    assert_eq!(
        TextWork::current().shaped - laid,
        0,
        "the frame found every name shaped"
    );
    // the sizes laid out are those of shaping each name anew
    let fresh = text_system();
    let snapshot = rt.snapshot(window).unwrap();
    for (i, text) in texts.iter().enumerate() {
        let id = rt.find(window, &Key::from(format!("label-{i}"))).unwrap();
        let rect = snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect;
        let size = fresh
            .measure(
                &AttributedText::plain(text.clone(), style()),
                &ParagraphStyle::default(),
                Some(200.0),
            )
            .unwrap();
        assert_eq!(rect.size, size, "{text}");
    }
}

#[test]
fn the_frame_clock_takes_the_shortest_recent_period_and_lets_a_late_frame_go() {
    let mut clock = FrameClock::default();
    assert_eq!(clock.due(Timestamp(0)), None, "no frame yet");
    clock.frame(Timestamp(0));
    assert_eq!(clock.period(), None, "one frame gives no period");
    clock.frame(Timestamp(16 * MS));
    clock.frame(Timestamp(48 * MS));
    assert_eq!(
        clock.period(),
        Some(16 * MS),
        "a skipped refresh is not the period"
    );
    assert_eq!(clock.due(Timestamp(50 * MS)), Some(Timestamp(64 * MS)));
    assert_eq!(clock.held_back(), Some(Timestamp(80 * MS)));
    assert_eq!(
        clock.due(Timestamp(80 * MS)),
        None,
        "a period late: held back"
    );
    // a pause and frames closer than a refresh say nothing of the period
    clock.frame(Timestamp(1_000 * MS));
    clock.frame(Timestamp(1_000 * MS + MS));
    assert_eq!(clock.period(), Some(16 * MS));
    // the shortest of the last few: a faster display shows within them
    for i in 1..=8 {
        clock.frame(Timestamp(2_000 * MS + i * 8 * MS));
    }
    assert_eq!(clock.period(), Some(8 * MS));
    for i in 0..=8 {
        clock.frame(Timestamp(3_000 * MS + i * 16 * MS));
    }
    assert_eq!(clock.period(), Some(16 * MS), "older periods are let go");
}
