mod support;

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
    let dot = color(ColorRole::SecondaryText, InteractionVariant::Normal, false);
    let dots: Vec<Rect> = h
        .ops("volume")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, radius },
                paint: Paint::Solid(c),
            } if c == dot && radius == 2.0 => Some(rect),
            _ => None,
        })
        .collect();
    let centres: Vec<f64> = dots.iter().map(|d| d.center().x).collect();
    assert_eq!(
        centres,
        [10.0, width / 2.0, width - 10.0],
        "from the knob's run, end to end"
    );
    assert!(dots.iter().all(|d| d.size == Size::new(4.0, 4.0)));
}
