use crate::support;

use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_controls::{Slider, SliderStyle, Stack};
use cupertino_ui_core::{AccessibleValue, Action, Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use cupertino_ui_platform::PointerEvent;
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use support::{color, window, Harness, TAB};

const LEFT: u32 = 0xff51;
const RIGHT: u32 = 0xff53;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;
const PAGE_UP: u32 = 0xff55;

/// A volume slider from 0 to 100 in steps of 10, starting at 50; the model takes every change.
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
                step: Some(10.0),
                style: SliderStyle::Regular,
                ticks: 3,
                enabled: true,
                on_change: Some(Box::new(|v| v)),
                on_end: None,
            })
            .key("volume")],
        }))
    }
}

#[test]
fn the_keys_step_page_and_jump_to_the_ends() {
    let mut h = Harness::<Volume>::new();
    h.tap(TAB);
    h.tap(RIGHT);
    assert_eq!(*h.model(), 60.0);
    h.tap(LEFT);
    h.tap(LEFT);
    assert_eq!(*h.model(), 40.0);
    h.tap(PAGE_UP);
    assert_eq!(*h.model(), 50.0);
    h.tap(END);
    h.tap(RIGHT);
    assert_eq!(*h.model(), 100.0, "clamped at the maximum");
    h.tap(HOME);
    assert_eq!(*h.model(), 0.0);
}

#[test]
fn in_right_to_left_the_left_arrow_increases() {
    let mut h = Harness::<Volume>::with_direction(LayoutDirection::Rtl);
    h.tap(TAB);
    h.tap(LEFT);
    assert_eq!(*h.model(), 60.0);
}

#[test]
fn a_press_sets_the_value_where_it_lands_and_a_drag_past_the_end_stops_at_it() {
    let mut h = Harness::<Volume>::new();
    let track = h.rect("volume");
    let at = |x: f64| Point::new(x, track.center().y);
    h.pointer(PointerEvent::Motion {
        position: at(track.min_x() + 10.0),
    });
    h.button(at(track.min_x() + 10.0), true);
    assert_eq!(
        *h.model(),
        0.0,
        "the knob's half width is the minimum's end"
    );
    h.pointer(PointerEvent::Motion {
        position: at(track.max_x() + 50.0),
    });
    assert_eq!(*h.model(), 100.0);
    h.button(at(track.max_x() + 50.0), false);
}

#[test]
fn it_is_a_slider_with_its_range_and_step() {
    let h = Harness::<Volume>::new();
    let node = h.accessible("volume");
    assert_eq!(node.role, Role::Slider);
    assert_eq!(
        node.value,
        Some(AccessibleValue::Number {
            value: 50.0,
            min: 0.0,
            max: 100.0,
            step: Some(10.0),
        })
    );
    assert!(node.actions.contains(&Action::Increment) && node.actions.contains(&Action::Decrement));
}

#[test]
fn tick_marks_are_dots_under_the_track_at_each_step_position() {
    let h = Harness::<Volume>::new();
    let width = h.rect("volume").width();
    // the secondary text colour at 60 %, lighter than the text
    let text = color(ColorRole::SecondaryText, InteractionVariant::Normal, false);
    let dot = text.with_alpha(text.a * 0.6);
    let dots: Vec<Rect> = h
        .ops("volume")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, radius },
                paint: Paint::Solid(c),
            } if c == dot && radius == 1.0 => Some(rect),
            _ => None,
        })
        .collect();
    let centres: Vec<f64> = dots.iter().map(|d| d.center().x).collect();
    assert_eq!(
        centres,
        [10.0, width / 2.0, width - 10.0],
        "from the knob's run, end to end"
    );
    assert!(dots.iter().all(|d| d.size == Size::new(2.0, 2.0)));
}

#[derive(Clone, Debug, PartialEq)]
enum Heard {
    Change(f64),
    End(f64),
}

/// A brightness slider from 0 to 1 starting at 0.5 that keeps every change and end it hears, in order.
struct Brightness;

impl Application for Brightness {
    type Model = (f64, Vec<Heard>);
    type Message = Heard;

    fn init() -> (Self::Model, Vec<Command<Heard>>) {
        ((0.5, Vec::new()), Vec::new())
    }

    fn update((value, heard): &mut Self::Model, message: Heard) -> Vec<Command<Heard>> {
        if let Heard::Change(v) = message {
            *value = v;
        }
        heard.push(message);
        Vec::new()
    }

    fn view((value, _): &Self::Model) -> Vec<Window<Heard>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![Element::new(Slider {
                label: "Brightness".into(),
                value: *value,
                min: 0.0,
                max: 1.0,
                step: Some(0.25),
                style: SliderStyle::PanelLevel("symbol.sun"),
                ticks: 0,
                enabled: true,
                on_change: Some(Box::new(Heard::Change)),
                on_end: Some(Box::new(Heard::End)),
            })
            .key("brightness")],
        }))
    }
}

#[test]
fn a_drag_ends_once_when_the_pointer_lets_go_and_a_key_change_ends_at_once() {
    let mut h = Harness::<Brightness>::new();
    let track = h.rect("brightness");
    let at = |x: f64| Point::new(x, track.center().y);
    h.pointer(PointerEvent::Motion {
        position: at(track.max_x() - 1.0),
    });
    h.button(at(track.max_x() - 1.0), true);
    h.pointer(PointerEvent::Motion {
        position: at(track.min_x() + 1.0),
    });
    assert_eq!(h.model().1, [Heard::Change(1.0), Heard::Change(0.0)]);
    h.button(at(track.min_x() + 1.0), false);
    assert_eq!(h.model().1[2..], [Heard::End(0.0)]);
    h.tap(RIGHT);
    assert_eq!(h.model().1[3..], [Heard::Change(0.25), Heard::End(0.25)]);
    h.tap(LEFT);
    h.tap(LEFT);
    assert_eq!(
        h.model().1[5..],
        [Heard::Change(0.0), Heard::End(0.0)],
        "a key at the minimum moves nothing and ends nothing"
    );
}

fn has_lens(h: &Harness<Volume>) -> bool {
    h.ops("volume")
        .iter()
        .any(|op| matches!(op, PaintOp::PushClip(_)))
}

#[test]
fn the_knob_is_a_lens_only_while_dragged_and_the_lens_never_moves_the_layout() {
    let mut h = Harness::<Volume>::new();
    let rect = h.rect("volume");
    assert!(!has_lens(&h), "at rest the knob is plain");
    let at = h.center("volume");
    h.pointer(PointerEvent::Motion { position: at });
    h.button(at, true);
    assert!(has_lens(&h), "held, the knob is a lens");
    h.pointer(PointerEvent::Motion {
        position: Point::new(rect.min_x() + rect.width() * 0.8, at.y),
    });
    assert!(has_lens(&h), "dragged, it stays a lens");
    assert_eq!(*h.model(), 80.0, "the value previews during the drag");
    assert_eq!(h.rect("volume"), rect, "the lens is paint only");
    h.button(Point::new(rect.min_x() + rect.width() * 0.8, at.y), false);
    assert!(!has_lens(&h), "let go, the knob is plain again");
    assert_eq!(h.rect("volume"), rect);
}
