//! The controls' own transitions, frame by frame on the test clock: each starts at its first step, runs over its
//! duration and settles, the window asking for frames only while one runs; with Reduce Motion each is at its end at
//! once.

use crate::support;

use cupertino_geometry::{Insets, Point, Rect};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, SegmentedControl, Slider, SliderStyle, Stack, Toggle,
};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{Mods, MouseButton, PointerEvent, Timestamp, WindowEvent};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::ThemeTokens;
use support::{window, Harness};

fn pointer(event: PointerEvent) -> WindowEvent {
    WindowEvent::Pointer(event, Timestamp(0))
}

fn button(position: Point, pressed: bool) -> WindowEvent {
    pointer(PointerEvent::Button {
        position,
        button: MouseButton::Left,
        pressed,
        serial: 1,
        mods: Mods::NONE,
    })
}

/// Presses at `at` (the pointer first moves there), drawing each step at once.
fn press<A: Application>(h: &mut Harness<A>, at: Point) {
    h.send_now(pointer(PointerEvent::Motion { position: at }));
    h.send_now(button(at, true));
}

fn reduced() -> ThemeTokens {
    let mut theme = support::theme();
    theme.motion.reduce = true;
    theme.preferences.reduce_motion = true;
    theme
}

/// The rectangles a node's paint clips to: a lens, or a segment's gliding mark.
fn clips(ops: &[PaintOp]) -> Vec<Rect> {
    ops.iter()
        .filter_map(|op| match op {
            PaintOp::PushClip(Shape::RoundedRect { rect, .. } | Shape::Rect(rect)) => Some(*rect),
            _ => None,
        })
        .collect()
}

/// The opacities of the plain layers a node pushes: a lens or a knob fading.
fn fades(ops: &[PaintOp]) -> Vec<f32> {
    ops.iter()
        .filter_map(|op| match op {
            PaintOp::PushLayer { opacity, effects } if effects.is_empty() => Some(*opacity),
            _ => None,
        })
        .collect()
}

/// The rounded fills a node paints outside any clip or layer: a track, a plain thumb or knob.
fn plain_fills(ops: &[PaintOp]) -> Vec<Rect> {
    let (mut clip, mut layer, mut out) = (0, 0, Vec::new());
    for op in ops {
        match op {
            PaintOp::PushClip(_) => clip += 1,
            PaintOp::PopClip => clip -= 1,
            PaintOp::PushLayer { .. } => layer += 1,
            PaintOp::PopLayer => layer -= 1,
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, .. },
                ..
            } if clip == 0 && layer == 0 => out.push(*rect),
            _ => {}
        }
    }
    out
}

// The switch.

struct Switch;

impl Application for Switch {
    type Model = bool;
    type Message = bool;

    fn init() -> (bool, Vec<Command<bool>>) {
        (false, Vec::new())
    }

    fn update(on: &mut bool, new: bool) -> Vec<Command<bool>> {
        *on = new;
        Vec::new()
    }

    fn view(on: &bool) -> Vec<Window<bool>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(Toggle {
                label: "Wi-Fi".into(),
                on: *on,
                enabled: true,
                on_toggle: Some(Box::new(|on| on)),
            })
            .key("toggle")],
        }))
    }
}

/// The switch's track, and the centre of its thumb: the lens's while it shows, else the plain thumb's.
fn switch_thumb(h: &Harness<Switch>) -> (Rect, f64, Option<Rect>) {
    let ops = h.ops("toggle");
    let fills = plain_fills(&ops);
    let track = fills[0];
    let lens = clips(&ops).first().copied();
    let x = match lens {
        Some(lens) => lens.center().x,
        None => {
            fills
                .iter()
                .rev()
                .find(|r| r.height() < track.height())
                .expect("a plain thumb")
                .center()
                .x
        }
    };
    (track, x, lens)
}

#[test]
fn a_clicked_switch_swells_into_its_lens_travels_then_sinks_back() {
    let mut h = Harness::<Switch>::new();
    let (track, rest, _) = switch_thumb(&h);
    assert!(!h.animating(), "at rest it asks for no frames");
    let at = h.center("toggle");
    press(&mut h, at);
    let (_, _, lens) = switch_thumb(&h);
    assert!(lens.is_none(), "the lift starts at the plain thumb");
    assert!(h.animating());
    h.later(100);
    let half = switch_thumb(&h).2.expect("swelling");
    h.later(150);
    let held = switch_thumb(&h).2.expect("held");
    assert!(half.width() < held.width(), "it swells over its lift");
    assert!(!h.animating(), "and holds still once lifted");

    h.send_now(button(at, false));
    assert!(*h.model(), "the click toggles at once");
    let mut last = rest;
    for _ in 0..10 {
        h.later(20);
        let (_, x, lens) = switch_thumb(&h);
        assert!(x >= last, "the thumb travels to the on end, never back");
        assert!(lens.is_some(), "carried across as a lens");
        last = x;
    }
    assert!(last > track.center().x, "at the on end after its travel");
    h.later(100);
    let sinking = switch_thumb(&h).2.expect("sinking");
    assert!(sinking.width() < held.width());
    h.later(150);
    let (_, end, lens) = switch_thumb(&h);
    assert_eq!(lens, None, "a plain thumb again");
    assert_eq!(end, last);
    assert!(!h.animating(), "settled");
}

#[test]
fn with_reduce_motion_a_switch_changes_at_once() {
    let mut h = Harness::<Switch>::with_theme(reduced());
    let (_, rest, _) = switch_thumb(&h);
    let at = h.center("toggle");
    press(&mut h, at);
    assert!(switch_thumb(&h).2.is_some(), "the lens shows at once");
    assert!(!h.animating());
    h.send_now(button(at, false));
    let (_, x, lens) = switch_thumb(&h);
    assert!(x > rest && lens.is_none(), "at the on end, plain, at once");
    assert!(!h.animating());
}

// The slider.

struct Volume;

impl Application for Volume {
    type Model = f64;
    type Message = f64;

    fn init() -> (f64, Vec<Command<f64>>) {
        (50.0, Vec::new())
    }

    fn update(value: &mut f64, new: f64) -> Vec<Command<f64>> {
        *value = new;
        Vec::new()
    }

    fn view(value: &f64) -> Vec<Window<f64>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![Element::new(Slider {
                label: "Volume".into(),
                value: *value,
                min: 0.0,
                max: 100.0,
                step: None,
                style: SliderStyle::Regular,
                ticks: 0,
                enabled: true,
                on_change: Some(Box::new(|v| v)),
                on_end: None,
            })
            .key("volume")],
        }))
    }
}

/// The centre of the slider's knob: its lens's while it shows, else the plain knob's.
fn knob_x(h: &Harness<Volume>) -> f64 {
    let ops = h.ops("volume");
    if let Some(lens) = clips(&ops).first() {
        return lens.center().x;
    }
    let fills = plain_fills(&ops);
    let track = fills[0];
    fills
        .iter()
        .rev()
        .find(|r| r.height() > track.height())
        .expect("a plain knob")
        .center()
        .x
}

#[test]
fn a_held_knob_swells_into_its_lens_and_sinks_back_when_let_go() {
    let mut h = Harness::<Volume>::new();
    let at = Point::new(knob_x(&h) + h.rect("volume").min_x(), h.center("volume").y);
    press(&mut h, at);
    assert!(clips(&h.ops("volume")).is_empty(), "plain at the press");
    h.later(75);
    let fading = fades(&h.ops("volume"));
    assert!(
        fading.len() == 2 && fading.iter().all(|o| *o > 0.0 && *o < 1.0),
        "halfway, the plain knob fades as the lens comes: {fading:?}"
    );
    h.later(100);
    assert!(fades(&h.ops("volume")).is_empty(), "held: the lens alone");
    assert_eq!(clips(&h.ops("volume")).len(), 1);
    assert!(!h.animating());
    h.send_now(button(at, false));
    h.later(60);
    assert_eq!(fades(&h.ops("volume")).len(), 2, "sinking back");
    h.later(80);
    assert!(clips(&h.ops("volume")).is_empty(), "plain once let go");
    assert!(!h.animating());
}

#[test]
fn a_press_on_the_track_takes_the_value_at_once_and_the_knob_glides_there() {
    let mut h = Harness::<Volume>::new();
    let rect = h.rect("volume");
    let start = knob_x(&h);
    let at = Point::new(rect.min_x() + rect.width() * 0.9, rect.center().y);
    press(&mut h, at);
    assert!(*h.model() > 80.0, "the value is there at the press");
    assert_eq!(knob_x(&h), start, "the knob starts where it was");
    let mut last = start;
    for _ in 0..10 {
        h.later(20);
        let x = knob_x(&h);
        assert!(x >= last, "forward only");
        last = x;
    }
    assert!(last > start + rect.width() * 0.3, "and arrives");
    h.later(50);
    assert_eq!(knob_x(&h), last, "settled over its travel");
    assert!(!h.animating());
    h.send_now(button(at, false));
    h.later(200);
}

#[test]
fn with_reduce_motion_the_knob_and_its_lens_change_at_once() {
    let mut h = Harness::<Volume>::with_theme(reduced());
    let rect = h.rect("volume");
    let start = knob_x(&h);
    press(
        &mut h,
        Point::new(rect.min_x() + rect.width() * 0.9, rect.center().y),
    );
    assert!(knob_x(&h) > start + rect.width() * 0.3);
    assert_eq!(clips(&h.ops("volume")).len(), 1, "the lens at once");
    assert!(fades(&h.ops("volume")).is_empty());
    assert!(!h.animating());
}

// The segmented control.

struct View;

impl Application for View {
    type Model = Option<usize>;
    type Message = usize;

    fn init() -> (Option<usize>, Vec<Command<usize>>) {
        (Some(0), Vec::new())
    }

    fn update(view: &mut Option<usize>, new: usize) -> Vec<Command<usize>> {
        *view = Some(new);
        Vec::new()
    }

    fn view(view: &Option<usize>) -> Vec<Window<usize>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(SegmentedControl {
                label: "View".into(),
                segments: ["Icons", "List", "Columns"].map(String::from).to_vec(),
                selected: *view,
                enabled: true,
                on_select: Some(Box::new(|i| i)),
            })
            .key("view")],
        }))
    }
}

/// The segments of the control, and the share of each the chosen mark covers.
fn marks(h: &Harness<View>) -> Vec<f64> {
    let id = h.rt.find(h.window, &"view".into()).unwrap();
    let snapshot = h.snapshot();
    let children = snapshot
        .accessibility
        .iter()
        .find(|(n, _)| *n == id)
        .unwrap()
        .1
        .children
        .clone();
    let accent = support::color(
        cupertino_ui_theme::ColorRole::ControlFill,
        cupertino_ui_theme::InteractionVariant::Normal,
        true,
    );
    children
        .into_iter()
        .map(|child| {
            let rect = snapshot.nodes.iter().find(|n| n.id == child).unwrap().rect;
            h.node_ops(child)
                .iter()
                .find_map(|op| match op {
                    PaintOp::Fill {
                        shape: Shape::RoundedRect { rect: mark, .. },
                        paint: Paint::Solid(c),
                    } if *c == accent => {
                        let covered =
                            (mark.max_x().min(rect.width()) - mark.min_x().max(0.0)).max(0.0);
                        Some(covered / rect.width())
                    }
                    _ => None,
                })
                .unwrap_or(0.0)
        })
        .collect()
}

#[test]
fn a_new_choice_is_marked_at_once_at_the_release() {
    let mut h = Harness::<View>::new();
    assert_eq!(marks(&h), [1.0, 0.0, 0.0]);
    let rect = h.rect("view");
    let last = Point::new(rect.max_x() - 5.0, rect.center().y);
    press(&mut h, last);
    h.send_now(button(last, false));
    // the pointer goes away, so no segment shows it over itself
    h.send_now(pointer(PointerEvent::Leave));
    assert_eq!(*h.model(), Some(2), "chosen at the release");
    // an inline control's mark jumps: no frame shows it between the segments
    assert_eq!(marks(&h), [0.0, 0.0, 1.0], "marked in the release's frame");
    assert!(!h.animating());
    h.later(40);
    assert_eq!(marks(&h), [0.0, 0.0, 1.0]);
}

#[test]
fn with_reduce_motion_a_new_choice_is_marked_at_once() {
    let mut h = Harness::<View>::with_theme(reduced());
    let rect = h.rect("view");
    let last = Point::new(rect.max_x() - 5.0, rect.center().y);
    press(&mut h, last);
    h.send_now(button(last, false));
    // the pointer goes away, so no segment shows it over itself
    h.send_now(pointer(PointerEvent::Leave));
    assert_eq!(marks(&h), [0.0, 0.0, 1.0]);
    assert!(!h.animating());
}

// The glass button.

struct Glass;

impl Application for Glass {
    type Model = u32;
    type Message = ();

    fn init() -> (u32, Vec<Command<()>>) {
        (0, Vec::new())
    }

    fn update(count: &mut u32, (): ()) -> Vec<Command<()>> {
        *count += 1;
        Vec::new()
    }

    fn view(_: &u32) -> Vec<Window<()>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(Button {
                label: "Share".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Glass,
                enabled: true,
                busy: false,
                on_press: Some(()),
            })
            .key("share")],
        }))
    }
}

/// The alpha of the glass button's press tint, if it shows.
fn tint(h: &Harness<Glass>) -> Option<f32> {
    h.ops("share").iter().find_map(|op| match op {
        PaintOp::Fill {
            paint: Paint::Solid(c),
            ..
        } if c.a > 0.0 && c.a < 0.5 => Some(c.a),
        _ => None,
    })
}

#[test]
fn a_glass_buttons_press_tint_comes_on_quickly_and_lingers_as_it_goes() {
    let mut h = Harness::<Glass>::new();
    assert_eq!(tint(&h), None);
    let at = h.center("share");
    press(&mut h, at);
    h.later(20);
    let rising = tint(&h).expect("coming on");
    h.later(40);
    let full = tint(&h).expect("on");
    assert!(rising < full);
    assert!(!h.animating(), "on within its press time");
    h.send_now(button(at, false));
    assert_eq!(*h.model(), 1);
    h.later(150);
    let going = tint(&h).expect("still going");
    assert!(going < full);
    h.later(200);
    assert_eq!(tint(&h), None, "gone");
    assert!(!h.animating());
}
