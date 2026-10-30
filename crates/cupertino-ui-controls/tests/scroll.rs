mod support;

use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, ScrollView, Scroller, Stack, Text, TextRole,
};
use cupertino_ui_core::{
    AccessibleValue, Application, Change, Command, Element, Event, EventCx, MeasureCx, PaintCx,
    Propagation, Role, SemanticsCx, Widget, Window,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, LayoutSpec, Measured, ScrollState, SizeSpec,
};
use cupertino_ui_platform::{PointerEvent, ScrollSource, Timestamp, WindowEvent};
use cupertino_ui_render::{PaintOp, Shape};
use cupertino_ui_text::{AttributedText, TextStyle};
use std::cell::Cell;
use std::num::NonZeroU32;
use support::{point, theme, window, Harness, TAB};

const DOWN: u32 = 0xff54;
const PAGE_DOWN: u32 = 0xff56;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;
const MS: u64 = 1_000_000;

#[derive(Clone, Copy, Default)]
struct Setup {
    scroller: Scroller,
    /// The rows are buttons, so the view holds Tab stops.
    buttons: bool,
    /// The view sits 60 tall in an outer view.
    nested: bool,
}

thread_local! {
    static SETUP: Cell<Setup> = Cell::new(Setup::default());
}

const START: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Inner(ScrollState),
    Outer(ScrollState),
    Press(usize),
}

struct Model {
    inner: ScrollState,
    outer: ScrollState,
    pressed: Vec<usize>,
}

/// Ten rows of 24 in a scroll view, as buttons or as text; nested, the view is 60 tall above 200 of space in an outer
/// view.
struct Scrolled;

impl Application for Scrolled {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (
            Model {
                inner: START,
                outer: START,
                pressed: Vec::new(),
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Inner(state) => model.inner = state,
            Message::Outer(state) => model.outer = state,
            Message::Press(i) => model.pressed.push(i),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let setup = SETUP.get();
        let rows = (0..10)
            .map(|i| {
                let row = if setup.buttons {
                    Element::new(Button {
                        label: format!("Row {i}"),
                        role: ButtonRole::Normal,
                        style: ButtonStyle::Bordered,
                        enabled: true,
                        busy: false,
                        on_press: Some(Message::Press(i)),
                    })
                } else {
                    Element::new(Text {
                        content: AttributedText::plain(format!("Row {i}"), TextStyle::new(13.0)),
                        role: TextRole::Body,
                        wrap: false,
                        secondary: false,
                        selected: false,
                    })
                };
                Element::new(Tall(24.0, Some(row))).key(format!("row {i}"))
            })
            .collect();
        let inner = Element::from(ScrollView {
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: 0.0,
                align: Align::Stretch,
                children: rows,
            }),
            state: model.inner,
            horizontal: false,
            vertical: true,
            scroller: setup.scroller,
            on_scroll: Box::new(Message::Inner),
        });
        if !setup.nested {
            return window(inner);
        }
        window(Element::from(ScrollView {
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: 0.0,
                align: Align::Stretch,
                children: vec![
                    Element::new(Tall(60.0, Some(inner))),
                    Element::new(Tall(200.0, None)),
                ],
            }),
            state: model.outer,
            horizontal: false,
            vertical: true,
            scroller: Scroller::Overlay,
            on_scroll: Box::new(Message::Outer),
        }))
    }
}

/// A box of a fixed height around an optional child.
struct Tall(f64, Option<Element<Message>>);

impl Widget<Message> for Tall {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            height: SizeSpec::Fixed(self.0),
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        self.1.take().into_iter().collect()
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

fn step(h: &mut Harness<Scrolled>) {
    h.rt.step(NonZeroU32::new(64).unwrap()).unwrap();
}

/// The compositor is ready for the frame the window asked for, if it asked.
fn next_frame(h: &mut Harness<Scrolled>) {
    let system = h.rt.window_system_mut();
    if system.frame_requested(h.window).unwrap() {
        system.frame_ready(h.window, Timestamp(0)).unwrap();
    }
    step(h);
}

fn harness(setup: Setup) -> Harness<Scrolled> {
    SETUP.set(setup);
    Harness::new()
}

fn wheel(h: &mut Harness<Scrolled>, at: Point, dy: f64, source: ScrollSource, stop: bool) {
    h.pointer(PointerEvent::Scroll {
        position: at,
        delta: Point::new(0.0, dy),
        value120: None,
        source,
        stop,
    });
}

/// The overlay thumb's rect, from the window's display list, if it paints.
fn thumb(h: &Harness<Scrolled>) -> Option<(Rect, f32)> {
    h.snapshot()
        .display_list
        .nodes
        .iter()
        .flat_map(|n| n.ops.iter())
        .find_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, radius },
                paint: cupertino_ui_render::Paint::Solid(color),
            } if *radius == 6.0 && rect.size.width == 12.0 => Some((*rect, color.a)),
            _ => None,
        })
}

#[test]
fn a_scroll_view_reports_its_size_and_the_wheel_moves_it_within_its_content() {
    let mut h = harness(Setup::default());
    let state = h.model().inner;
    assert_eq!(state.viewport, Size::new(200.0, 100.0));
    assert_eq!(state.content.height, 240.0);
    wheel(&mut h, point(50.0, 50.0), 30.0, ScrollSource::Wheel, false);
    assert_eq!(h.model().inner.offset.y, 30.0);
    assert_eq!(
        h.rect("row 0").origin.y,
        -30.0,
        "the content moved with the offset"
    );
    wheel(
        &mut h,
        point(50.0, 50.0),
        10_000.0,
        ScrollSource::Wheel,
        false,
    );
    assert_eq!(
        h.model().inner.offset.y,
        140.0,
        "the offset stops at the end"
    );
}

#[test]
fn keys_from_a_focused_child_scroll_by_a_line_a_page_and_to_the_ends() {
    let mut h = harness(Setup {
        buttons: true,
        ..Setup::default()
    });
    h.tap(TAB);
    h.tap(DOWN);
    assert_eq!(h.model().inner.offset.y, 40.0);
    h.tap(PAGE_DOWN);
    assert_eq!(h.model().inner.offset.y, 100.0, "a page leaves 40 in view");
    h.tap(END);
    assert_eq!(h.model().inner.offset.y, 140.0);
    h.tap(HOME);
    assert_eq!(h.model().inner.offset.y, 0.0);
}

fn focused_role(h: &Harness<Scrolled>) -> Role {
    let focus = h.snapshot().focus.unwrap();
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(id, _)| *id == focus)
        .unwrap()
        .1
        .role
}

#[test]
fn a_view_is_a_tab_stop_only_when_nothing_in_it_is() {
    let mut h = harness(Setup::default());
    h.tap(TAB);
    assert_eq!(focused_role(&h), Role::ScrollView);

    let mut h = harness(Setup {
        buttons: true,
        ..Setup::default()
    });
    h.tap(TAB);
    assert_eq!(focused_role(&h), Role::Button);
    h.tap(TAB);
    assert_eq!(focused_role(&h), Role::Button, "the view itself is no stop");
}

#[test]
fn a_gesture_stays_with_the_view_that_took_it_and_a_new_one_moves_the_outer_view() {
    let mut h = harness(Setup {
        nested: true,
        ..Setup::default()
    });
    let inside = point(50.0, 30.0);
    wheel(&mut h, inside, 1_000.0, ScrollSource::Wheel, false);
    assert_eq!(h.model().inner.offset.y, 240.0 - 60.0);
    wheel(&mut h, inside, 50.0, ScrollSource::Wheel, false);
    assert_eq!(
        h.model().outer.offset.y,
        0.0,
        "the burst keeps going to the inner view at its end"
    );
    h.clock().advance_to(Timestamp(301 * MS)).unwrap();
    h.settle();
    wheel(&mut h, inside, 50.0, ScrollSource::Wheel, false);
    assert_eq!(h.model().outer.offset.y, 50.0, "a new burst passes on");
}

#[test]
fn an_overlay_thumb_shows_on_scroll_fades_after_a_second_and_lets_clicks_through_while_hidden() {
    let mut h = harness(Setup {
        buttons: true,
        ..Setup::default()
    });
    assert_eq!(thumb(&h), None);
    h.click(point(196.0, 12.0));
    assert_eq!(
        h.model().pressed,
        [0],
        "the hidden bar's strip is the row's"
    );

    wheel(&mut h, point(50.0, 50.0), 70.0, ScrollSource::Wheel, false);
    let (rect, alpha) = thumb(&h).expect("a scroll shows the thumb");
    // in the bar's 16 at the trailing edge: track 96 from y 2; thumb 96 × 100 / 240 = 40, half way down its 56 of
    // travel
    assert_eq!(rect, Rect::new(2.0, 30.0, 12.0, 40.0));
    let full = alpha;

    h.clock().advance_to(Timestamp(1_150 * MS)).unwrap();
    step(&mut h);
    next_frame(&mut h);
    let (_, fading) = thumb(&h).expect("half way through the fade");
    assert!((fading - full * 0.5).abs() < 0.01, "{fading} of {full}");

    h.clock().advance_to(Timestamp(1_400 * MS)).unwrap();
    next_frame(&mut h);
    h.settle();
    assert_eq!(thumb(&h), None);
}

#[test]
fn with_reduce_motion_an_overlay_thumb_hides_at_once_after_its_second() {
    let mut reduced = theme();
    reduced.motion.reduce = true;
    SETUP.set(Setup::default());
    let mut h = Harness::<Scrolled>::with_theme(reduced);
    wheel(&mut h, point(50.0, 50.0), 70.0, ScrollSource::Wheel, false);
    assert!(thumb(&h).is_some());
    h.clock().advance_to(Timestamp(1_000 * MS)).unwrap();
    h.settle();
    assert_eq!(thumb(&h), None);
    assert!(!h.snapshot().pending.animating);
}

#[test]
fn an_always_shown_track_pages_on_a_click_and_its_thumb_drags() {
    let mut h = harness(Setup {
        scroller: Scroller::Always,
        ..Setup::default()
    });
    assert_eq!(h.model().inner.viewport, Size::new(185.0, 100.0));
    h.click(point(192.0, 90.0));
    assert_eq!(h.model().inner.offset.y, 60.0, "a page leaves 40 in view");
    // thumb 100 × 100 / 240 ≈ 41.7 tall over 58.3 of travel: at 60 of 140 it starts near 25
    h.button(point(192.0, 35.0), true);
    h.pointer(PointerEvent::Motion {
        position: point(192.0, 35.0 + 58.3 / 2.0),
    });
    let y = h.model().inner.offset.y;
    assert!((y - 130.0).abs() < 1.0, "{y}");
    h.button(point(192.0, 64.0), false);
    let bar = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::ScrollBar)
        .unwrap()
        .1;
    assert!(matches!(bar.value, Some(AccessibleValue::Number { max, .. }) if max == 140.0));
}

#[test]
fn a_touchpad_flick_coasts_after_the_lift_unless_motion_is_reduced() {
    for reduce in [false, true] {
        let mut theme = theme();
        theme.motion.reduce = reduce;
        SETUP.set(Setup::default());
        let mut h = Harness::<Scrolled>::with_theme(theme);
        for i in 1..=5 {
            h.clock().advance_to(Timestamp(i * 10 * MS)).unwrap();
            wheel(&mut h, point(50.0, 50.0), 8.0, ScrollSource::Finger, false);
        }
        assert_eq!(h.model().inner.offset.y, 40.0);
        h.rt.window_system_mut()
            .enqueue_window(
                h.window,
                WindowEvent::Pointer(
                    PointerEvent::Scroll {
                        position: point(50.0, 50.0),
                        delta: Point::ZERO,
                        value120: None,
                        source: ScrollSource::Finger,
                        stop: true,
                    },
                    Timestamp(0),
                ),
            )
            .unwrap();
        step(&mut h);
        h.clock().advance_to(Timestamp(200 * MS)).unwrap();
        next_frame(&mut h);
        let after = h.model().inner.offset.y;
        if reduce {
            assert_eq!(after, 40.0, "no coasting under Reduce Motion");
        } else {
            assert!(after > 40.0, "the content coasts on, to {after}");
        }
    }
}
