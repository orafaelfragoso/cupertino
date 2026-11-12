use crate::support;

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_controls::{Progress, Stack};
use cupertino_ui_core::{AccessibleValue, Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::Timestamp;
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant};
use support::{color, theme, window, Harness};

struct Bar<const QUARTER: bool>;

impl<const QUARTER: bool> Application for Bar<QUARTER> {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![Element::new(Progress {
                label: "Copying".into(),
                value: QUARTER.then_some(0.25),
            })
            .key("bar")],
        }))
    }
}

/// The rounded rects the bar fills: the track, then what is done or moving.
fn fills<A: Application>(h: &Harness<A>) -> Vec<(Rect, Color)> {
    h.ops("bar")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, .. },
                paint: Paint::Solid(c),
            } => Some((rect, c)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_value_fills_that_share_of_the_track_from_the_leading_edge() {
    let h = Harness::<Bar<true>>::new();
    let fills = fills(&h);
    assert_eq!(fills[1].0, Rect::new(0.0, 0.0, 50.0, 6.0));
    assert_eq!(
        fills[1].1,
        color(ColorRole::ControlFill, InteractionVariant::Normal, true)
    );
    let node = h.accessible("bar");
    assert_eq!(node.role, Role::ProgressIndicator);
    assert_eq!(
        node.value,
        Some(AccessibleValue::Number {
            value: 0.25,
            min: 0.0,
            max: 1.0,
            step: None
        })
    );
    assert!(!node.states.focusable && !node.states.busy);
}

#[test]
fn without_a_value_a_segment_sweeps_on_every_frame() {
    let mut h = Harness::<Bar<false>>::animated();
    assert!(
        h.snapshot().pending.animating,
        "it asks for frames from its first paint"
    );
    let before = fills(&h)[1].0.min_x();
    h.clock().advance_to(Timestamp(300_000_000)).unwrap();
    h.settle();
    assert!(fills(&h)[1].0.min_x() > before);
    assert!(h.accessible("bar").states.busy);
}

#[test]
fn with_reduce_motion_a_busy_bar_is_still_and_asks_for_no_frames() {
    let mut reduced = theme();
    reduced.motion.reduce = true;
    let h = Harness::<Bar<false>>::with_theme(reduced);
    assert!(!h.snapshot().pending.animating);
    let still = color(
        ColorRole::SecondarySelectedControlFill,
        InteractionVariant::Normal,
        false,
    );
    assert_eq!(fills(&h)[1], (Rect::new(0.0, 0.0, 200.0, 6.0), still));
}
