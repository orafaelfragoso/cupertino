use crate::support;

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
use cupertino_ui_platform::{Mods, PointerEvent, ScrollSource, Timestamp, WindowEvent};
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
    /// Three rows instead of ten: everything fits.
    short: bool,
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
        let rows = (0..if setup.short { 3 } else { 10 })
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
        inverted: (false, false),
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
            } if *radius == 3.0 && rect.size.width == 6.0 => Some((*rect, color.a)),
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

#[test]
fn a_key_held_with_a_command_modifier_is_left_for_the_shortcuts() {
    let mut h = harness(Setup {
        buttons: true,
        ..Setup::default()
    });
    h.tap(TAB);
    for mods in [Mods::PRIMARY, Mods::CTRL, Mods::ALT] {
        h.key_event(DOWN, true, false, mods);
        h.key_event(DOWN, false, false, mods);
        h.key_event(END, true, false, mods);
        h.key_event(END, false, false, mods);
    }
    assert_eq!(h.model().inner.offset.y, 0.0);
    h.key_event(DOWN, true, false, Mods::SHIFT);
    assert_eq!(
        h.model().inner.offset.y,
        40.0,
        "Shift is no command modifier"
    );
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
    // in the bar's 17 at the trailing edge, 6 wide and 3 in from it: track 96 from y 2; thumb 96 × 100 / 240 = 40,
    // half way down its 56 of travel
    assert_eq!(rect, Rect::new(8.0, 30.0, 6.0, 40.0));
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
    // the track takes 17 beside the content
    assert_eq!(h.model().inner.viewport, Size::new(183.0, 100.0));
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
                        inverted: (false, false),
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

/// Sends a pointer event and steps once, without settling: the content may be moving on its own.
fn send_now(h: &mut Harness<Scrolled>, event: PointerEvent) {
    h.rt.window_system_mut()
        .enqueue_window(h.window, WindowEvent::Pointer(event, Timestamp(0)))
        .unwrap();
    step(h);
}

/// A finger on the touchpad moving the content by `dy` at `ms`, or lifting.
fn finger(h: &mut Harness<Scrolled>, at: Point, ms: u64, dy: f64, stop: bool) {
    h.clock().advance_to(Timestamp(ms * MS)).unwrap();
    send_now(
        h,
        PointerEvent::Scroll {
            position: at,
            delta: Point::new(0.0, dy),
            value120: None,
            source: ScrollSource::Finger,
            stop,
            inverted: (false, false),
        },
    );
}

/// Animation frames every 16 ms from `from` to `to`.
fn frames(h: &mut Harness<Scrolled>, from: u64, to: u64) {
    let mut ms = from;
    while ms <= to {
        h.clock().advance_to(Timestamp(ms * MS)).unwrap();
        next_frame(h);
        ms += 16;
    }
}

/// Where the content is drawn: the first row's top, which is 0 at rest at the start.
fn drawn(h: &Harness<Scrolled>) -> f64 {
    -h.rect("row 0").origin.y
}

#[test]
fn a_finger_pulling_past_the_start_stretches_the_content_and_the_stretch_returns_after_the_lift() {
    let mut h = harness(Setup::default());
    let at = point(50.0, 50.0);
    for i in 1..=8 {
        finger(&mut h, at, i * 10, -30.0, false);
    }
    let stretch = -drawn(&h);
    assert_eq!(
        h.model().inner.offset.y,
        0.0,
        "the offset stays at the edge"
    );
    // eight pulls of 30 past a viewport of 100 show as 100 × (1 − 1 / (1 + 0.07 × 240 / 100)) ≈ 14.4
    assert!(
        (stretch - 14.4).abs() < 0.2,
        "drawn {stretch} past the edge"
    );
    assert!(
        !h.snapshot().pending.animating,
        "a held stretch does not move on its own"
    );

    // the finger rests, then lifts: the stretch returns to the edge without overshooting it
    finger(&mut h, at, 200, 0.0, true);
    frames(&mut h, 216, 280);
    let returning = -drawn(&h);
    assert!(returning > 0.0 && returning < stretch, "{returning}");
    frames(&mut h, 296, 600);
    assert_eq!(drawn(&h), 0.0);
    assert_eq!(h.model().inner.offset.y, 0.0);
    assert!(!h.snapshot().pending.animating, "the return ends");
}

#[test]
fn a_finger_moving_back_unwinds_the_stretch_before_it_moves_the_content() {
    let mut h = harness(Setup::default());
    let at = point(50.0, 50.0);
    for i in 1..=4 {
        finger(&mut h, at, i * 10, -30.0, false);
    }
    let stretch = -drawn(&h);
    assert!(stretch > 0.0);
    // a little back: still stretched, by less
    finger(&mut h, at, 50, 30.0, false);
    let less = -drawn(&h);
    assert!(less > 0.0 && less < stretch, "{less} of {stretch}");
    // back past the edge: the movement that reaches it stops there, the next moves the content
    finger(&mut h, at, 60, 200.0, false);
    assert_eq!(drawn(&h), 0.0);
    finger(&mut h, at, 70, 20.0, false);
    assert_eq!(h.model().inner.offset.y, 20.0);
    assert_eq!(drawn(&h), 20.0);
}

#[test]
fn a_wheel_steps_inside_the_range_with_no_stretch_and_no_inertia() {
    let mut h = harness(Setup::default());
    let at = point(50.0, 50.0);
    for i in 1..=5 {
        h.clock().advance_to(Timestamp(i * 10 * MS)).unwrap();
        wheel(&mut h, at, -30.0, ScrollSource::Wheel, false);
    }
    assert_eq!(drawn(&h), 0.0, "no stretch at the edge");
    // a new burst: the first one, which could not move the content, was nobody's
    for i in 40..=44 {
        h.clock().advance_to(Timestamp(i * 10 * MS)).unwrap();
        wheel(&mut h, at, 20.0, ScrollSource::Wheel, false);
    }
    assert_eq!(h.model().inner.offset.y, 100.0);
    assert!(
        !h.snapshot().pending.animating,
        "a wheel's movement is complete as it arrives"
    );
    h.clock().advance_to(Timestamp(800 * MS)).unwrap();
    h.settle();
    assert_eq!(h.model().inner.offset.y, 100.0);
}

#[test]
fn a_flick_reaching_the_end_carries_past_it_and_returns() {
    let mut h = harness(Setup::default());
    let at = point(50.0, 50.0);
    // 25 points every 10 ms, 2 500 points a second, from 100 of 140
    for i in 1..=4 {
        finger(&mut h, at, i * 10, 25.0, false);
    }
    assert_eq!(h.model().inner.offset.y, 100.0);
    finger(&mut h, at, 40, 0.0, true);
    let mut past = 0.0_f64;
    let mut ms = 56;
    while ms <= 1_500 {
        frames(&mut h, ms, ms);
        past = past.max(drawn(&h) - 140.0);
        ms += 16;
    }
    assert!(past > 1.0, "the content carried {past} past the end");
    assert_eq!(h.model().inner.offset.y, 140.0);
    assert_eq!(drawn(&h), 140.0, "and returned to it");
    assert!(!h.snapshot().pending.animating);
}

#[test]
fn a_new_finger_catches_coasting_content_where_it_is() {
    let mut h = harness(Setup::default());
    let at = point(50.0, 50.0);
    for i in 1..=4 {
        finger(&mut h, at, i * 10, 8.0, false);
    }
    finger(&mut h, at, 40, 0.0, true);
    frames(&mut h, 56, 104);
    let caught = h.model().inner.offset.y;
    assert!(caught > 32.0, "coasting to {caught}");
    finger(&mut h, at, 104, 1.0, false);
    let held = h.model().inner.offset.y;
    assert!((held - caught - 1.0).abs() < 1e-6, "{held} after {caught}");
    frames(&mut h, 120, 400);
    assert_eq!(
        h.model().inner.offset.y,
        held,
        "the content stays under the finger"
    );
}

/// Where a flick of five movements of 2 points 10 ms apart, then a lift, leaves the content on frames from 66 ms to
/// 1 s: delivered one at a time as they happen, or (`batched`) all at once when the window wakes at the lift, each
/// with the time it happened.
fn flick_trace(batched: bool) -> Vec<f64> {
    let mut h = harness(Setup::default());
    let at = point(50.0, 50.0);
    let movement = |ms: u64, dy: f64, stop: bool| {
        WindowEvent::Pointer(
            PointerEvent::Scroll {
                position: at,
                delta: Point::new(0.0, dy),
                value120: None,
                source: ScrollSource::Finger,
                stop,
                inverted: (false, false),
            },
            Timestamp(ms * MS),
        )
    };
    let mut events: Vec<(u64, WindowEvent)> = (1..=5)
        .map(|i| (i * 10, movement(i * 10, 2.0, false)))
        .collect();
    events.push((50, movement(50, 0.0, true)));
    if batched {
        h.clock().advance_to(Timestamp(50 * MS)).unwrap();
    }
    for (ms, event) in events {
        if !batched {
            h.clock().advance_to(Timestamp(ms * MS)).unwrap();
        }
        h.rt.window_system_mut()
            .enqueue_window(h.window, event)
            .unwrap();
        if !batched {
            step(&mut h);
        }
    }
    if batched {
        step(&mut h);
    }
    assert_eq!(h.model().inner.offset.y, 10.0);
    let mut trace = Vec::new();
    let mut ms = 66;
    while ms <= 1_000 {
        frames(&mut h, ms, ms);
        trace.push(drawn(&h));
        ms += 16;
    }
    trace
}

#[test]
fn a_flick_delivered_in_one_batch_coasts_as_one_delivered_as_it_happened() {
    let spread = flick_trace(false);
    let batched = flick_trace(true);
    let rest = *spread.last().unwrap();
    // 200 points a second coasts about 100 points
    assert!(rest > 80.0, "the spread flick coasts to {rest}");
    for (i, (s, b)) in spread.iter().zip(&batched).enumerate() {
        let travel = s - 10.0;
        assert!(
            (b - s).abs() <= 0.02 * travel.abs().max(1.0),
            "frame {i}: batched at {b}, spread at {s}"
        );
    }
}

/// Where a touch dragging the content up by 2 points every 10 ms five times, then lifting, leaves it on frames from
/// 66 ms to 1 s: delivered one at a time as it happens, or (`batched`) all at once at the lift, each with its own time.
fn touch_flick_trace(batched: bool) -> Vec<f64> {
    use cupertino_ui_platform::TouchEvent;
    let mut h = harness(Setup::default());
    let at = |i: u64| point(50.0, 80.0 - 2.0 * i as f64);
    let mut events = vec![(
        0,
        TouchEvent::Down {
            id: 1,
            position: at(0),
        },
    )];
    events.extend((1..=5).map(|i| {
        (
            i * 10,
            TouchEvent::Motion {
                id: 1,
                position: at(i),
            },
        )
    }));
    events.push((50, TouchEvent::Up { id: 1 }));
    if batched {
        h.clock().advance_to(Timestamp(50 * MS)).unwrap();
    }
    for (ms, event) in events {
        if !batched {
            h.clock().advance_to(Timestamp(ms * MS)).unwrap();
        }
        h.rt.window_system_mut()
            .enqueue_window(h.window, WindowEvent::Touch(event, Timestamp(ms * MS)))
            .unwrap();
        if !batched {
            step(&mut h);
        }
    }
    if batched {
        step(&mut h);
    }
    assert_eq!(h.model().inner.offset.y, 10.0);
    let mut trace = Vec::new();
    let mut ms = 66;
    while ms <= 1_000 {
        frames(&mut h, ms, ms);
        trace.push(drawn(&h));
        ms += 16;
    }
    trace
}

#[test]
fn a_touch_flick_delivered_in_one_batch_coasts_as_one_delivered_as_it_happened() {
    let spread = touch_flick_trace(false);
    let batched = touch_flick_trace(true);
    assert!(*spread.last().unwrap() > 80.0, "the touch flick coasts");
    for (i, (s, b)) in spread.iter().zip(&batched).enumerate() {
        assert!(
            (b - s).abs() <= 0.02 * (s - 10.0).abs().max(1.0),
            "frame {i}: batched at {b}, spread at {s}"
        );
    }
}

#[test]
fn a_cancelled_touch_stops_the_drag_and_the_stretch_returns() {
    use cupertino_ui_platform::TouchEvent;
    let mut h = harness(Setup::default());
    let send = |h: &mut Harness<Scrolled>, ms: u64, event| {
        h.clock().advance_to(Timestamp(ms * MS)).unwrap();
        h.rt.window_system_mut()
            .enqueue_window(h.window, WindowEvent::Touch(event, Timestamp(0)))
            .unwrap();
        step(h);
    };
    send(
        &mut h,
        0,
        TouchEvent::Down {
            id: 1,
            position: point(50.0, 20.0),
        },
    );
    for i in 1..=4 {
        let position = point(50.0, 20.0 + 30.0 * i as f64);
        send(&mut h, i * 10, TouchEvent::Motion { id: 1, position });
    }
    assert!(
        drawn(&h) < 0.0,
        "the touch pulled the content down past the start"
    );
    send(&mut h, 50, TouchEvent::Cancel);
    frames(&mut h, 66, 600);
    assert_eq!(drawn(&h), 0.0);
    assert!(!h.snapshot().pending.animating);
}

/// A finger on an inner view at its end, pushing on past it, stretches the inner view; the outer view, which could
/// still move, stays where it is.
#[test]
fn a_finger_on_an_inner_view_at_its_end_stretches_it_and_leaves_the_outer_view() {
    let mut h = harness(Setup {
        nested: true,
        ..Setup::default()
    });
    let inside = point(50.0, 30.0);
    wheel(&mut h, inside, 1_000.0, ScrollSource::Wheel, false);
    assert_eq!(h.model().inner.offset.y, 240.0 - 60.0);
    let end = drawn(&h);
    // the wheel's burst is over
    h.clock().advance_to(Timestamp(400 * MS)).unwrap();
    h.settle();
    for i in 1..=4 {
        finger(&mut h, inside, 400 + i * 10, 30.0, false);
    }
    assert_eq!(h.model().outer.offset.y, 0.0, "the outer view stays");
    assert_eq!(h.model().inner.offset.y, 240.0 - 60.0);
    // four pushes of 30 past a viewport of 60 show as 60 × (1 − 1 / (1 + 0.07 × 120 / 60)) ≈ 7.4
    let stretch = drawn(&h) - end;
    assert!(
        (stretch - 7.4).abs() < 0.2,
        "the inner content is drawn {stretch} past its end"
    );
    finger(&mut h, inside, 450, 0.0, true);
    frames(&mut h, 466, 900);
    assert_eq!(drawn(&h), end, "the stretch returns");
    assert_eq!(h.model().outer.offset.y, 0.0);
}

#[test]
fn coasting_content_keeps_moving_while_the_pointer_scrolls_another_view() {
    let mut h = harness(Setup {
        nested: true,
        ..Setup::default()
    });
    let inside = point(50.0, 30.0);
    for i in 1..=4 {
        finger(&mut h, inside, i * 10, 8.0, false);
    }
    finger(&mut h, inside, 40, 0.0, true);
    frames(&mut h, 56, 88);
    let coasting = h.model().inner.offset.y;
    // below the inner view, in the outer one
    let outside = point(50.0, 80.0);
    send_now(&mut h, PointerEvent::Motion { position: outside });
    h.clock().advance_to(Timestamp(96 * MS)).unwrap();
    send_now(
        &mut h,
        PointerEvent::Scroll {
            position: outside,
            delta: Point::new(0.0, 20.0),
            value120: None,
            source: ScrollSource::Wheel,
            stop: false,
            inverted: (false, false),
        },
    );
    assert_eq!(
        h.model().outer.offset.y,
        20.0,
        "the wheel moves the view under it"
    );
    frames(&mut h, 104, 200);
    assert!(
        h.model().inner.offset.y > coasting,
        "the inner view coasts on from {coasting} to {}",
        h.model().inner.offset.y
    );
}

fn scroll_bar_roles(h: &Harness<Scrolled>) -> usize {
    h.snapshot()
        .accessibility
        .into_iter()
        .filter(|(_, n)| n.role == Role::ScrollBar)
        .count()
}

#[test]
fn an_always_shown_track_gives_its_gutter_back_while_everything_fits() {
    let mut h = harness(Setup {
        scroller: Scroller::Always,
        short: true,
        ..Setup::default()
    });
    assert_eq!(h.model().inner.content.height, 72.0);
    assert_eq!(
        h.model().inner.viewport,
        Size::new(200.0, 100.0),
        "no gutter for a track with nothing to scroll"
    );
    assert_eq!(scroll_bar_roles(&h), 0);
    // taller than the view: the track comes back and takes its 17
    h.resize(Size::new(200.0, 50.0));
    assert_eq!(h.model().inner.viewport, Size::new(183.0, 50.0));
    assert_eq!(scroll_bar_roles(&h), 1);
}

#[test]
fn the_session_style_overrides_the_view_s_own() {
    use cupertino_ui_platform::{PlatformEvent, ScrollBars};
    let mut h = harness(Setup::default());
    assert_eq!(
        h.model().inner.viewport.width,
        200.0,
        "the view's overlay bars"
    );
    h.rt.window_system_mut()
        .enqueue(PlatformEvent::ScrollBars(ScrollBars::Always))
        .unwrap();
    h.settle();
    assert_eq!(
        h.model().inner.viewport.width,
        183.0,
        "the session keeps scroll bars shown beside the content"
    );
    assert_eq!(scroll_bar_roles(&h), 1);

    SETUP.set(Setup {
        scroller: Scroller::Always,
        ..Setup::default()
    });
    let mut h = Harness::<Scrolled>::new();
    h.rt.window_system_mut()
        .enqueue(PlatformEvent::ScrollBars(ScrollBars::WhenScrolling))
        .unwrap();
    h.settle();
    assert_eq!(
        h.model().inner.viewport.width,
        200.0,
        "the session shows bars over the content while it scrolls"
    );
    assert_eq!(scroll_bar_roles(&h), 0);
}

#[test]
fn an_overlay_thumb_stays_while_the_pointer_is_on_it_and_fades_after_it_leaves() {
    let mut h = harness(Setup::default());
    wheel(&mut h, point(50.0, 50.0), 70.0, ScrollSource::Wheel, false);
    assert!(thumb(&h).is_some());
    // on the thumb's strip, wider than the thumb as drawn: x 185 is in the strip, left of the 6 drawn
    h.pointer(PointerEvent::Motion {
        position: point(185.0, 50.0),
    });
    h.clock().advance_to(Timestamp(3_000 * MS)).unwrap();
    h.settle();
    let (_, alpha) = thumb(&h).expect("held by the pointer");
    assert!(alpha > 0.0);

    // off the thumb: shown a second longer, then it fades
    h.pointer(PointerEvent::Motion {
        position: point(50.0, 50.0),
    });
    h.clock().advance_to(Timestamp(3_900 * MS)).unwrap();
    h.settle();
    assert!(
        thumb(&h).is_some(),
        "shown for a second after the pointer left"
    );
    h.clock().advance_to(Timestamp(4_400 * MS)).unwrap();
    step(&mut h);
    next_frame(&mut h);
    h.settle();
    assert_eq!(thumb(&h), None);
}

#[test]
fn dragging_an_overlay_thumb_keeps_it_shown_until_the_release() {
    let mut h = harness(Setup::default());
    wheel(&mut h, point(50.0, 50.0), 70.0, ScrollSource::Wheel, false);
    // the thumb is 40 long from y 30
    h.pointer(PointerEvent::Motion {
        position: point(191.0, 40.0),
    });
    h.button(point(191.0, 40.0), true);
    h.pointer(PointerEvent::Motion {
        position: point(191.0, 54.0),
    });
    let y = h.model().inner.offset.y;
    // 14 of the thumb's 56 of travel is 35 of the content's 140
    assert!((y - 105.0).abs() < 0.5, "{y}");
    h.clock().advance_to(Timestamp(5_000 * MS)).unwrap();
    h.settle();
    assert!(thumb(&h).is_some(), "held while dragged");
    h.button(point(191.0, 54.0), false);
    h.pointer(PointerEvent::Motion {
        position: point(50.0, 50.0),
    });
    h.clock().advance_to(Timestamp(6_400 * MS)).unwrap();
    step(&mut h);
    next_frame(&mut h);
    h.settle();
    assert_eq!(
        thumb(&h),
        None,
        "gone a second and a fade after the release"
    );
}
