use crate::support;

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_controls::{Stack, Toggle};
use cupertino_ui_core::{
    AccessibleValue, Action, Application, Checked, Command, Element, Role, Window,
};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use cupertino_ui_platform::{PointerEvent, TouchEvent};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use support::{color, point, window, Harness, RETURN, SPACE, TAB};

/// Starts disabled when `DISABLED`.
struct Switch<const DISABLED: bool>;
type App = Switch<false>;

#[derive(Default)]
struct Model {
    on: bool,
    disabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Toggled(bool),
}

impl<const DISABLED: bool> Application for Switch<DISABLED> {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (
            Model {
                on: false,
                disabled: DISABLED,
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, Message::Toggled(on): Message) -> Vec<Command<Message>> {
        model.on = on;
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(Toggle {
                label: "Wi-Fi".into(),
                on: model.on,
                enabled: !model.disabled,
                on_toggle: Some(Box::new(Message::Toggled)),
            })
            .key("toggle")],
        }))
    }
}

fn fills<A: Application>(h: &Harness<A>) -> Vec<(Rect, f64, cupertino_ui_theme::Color)> {
    h.ops("toggle")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, radius },
                paint: Paint::Solid(color),
            } => Some((rect, radius, color)),
            _ => None,
        })
        .collect()
}

/// An off track: a quiet well, black at `share` in the light appearance.
fn well(share: f32) -> cupertino_ui_theme::Color {
    cupertino_ui_theme::Color::new(0.0, 0.0, 0.0, share)
}

/// Whether the thumb is drawn as a lens, which clips what shows through it.
fn lens<A: Application>(h: &Harness<A>) -> bool {
    h.ops("toggle")
        .iter()
        .any(|op| matches!(op, PaintOp::PushClip(_)))
}

fn thumb_x(h: &Harness<App>) -> f64 {
    fills(h)[1].0.min_x()
}

#[test]
fn the_pill_lays_out_on_its_track_and_the_thumb_moves_to_the_end_when_on() {
    let mut h = Harness::<App>::new();
    assert_eq!(h.rect("toggle"), Rect::new(0.0, 0.0, 38.0, 22.0));
    let [track, thumb] = fills(&h)[..] else {
        panic!("a track and a thumb")
    };
    assert_eq!((track.0, track.1), (Rect::new(0.0, 0.0, 38.0, 22.0), 11.0));
    assert_eq!(
        (thumb.0, thumb.1),
        (Rect::new(2.0, 2.0, 28.8, 18.0), 9.0),
        "a capsule thumb, wider than it is high"
    );
    assert_eq!(track.2, well(0.095), "off, a quiet well");

    h.click(h.center("toggle"));

    assert!(h.model().on);
    assert!((thumb_x(&h) - 7.2).abs() < 1e-9);
    assert_eq!(
        fills(&h)[0].2,
        color(ColorRole::SwitchTrack, InteractionVariant::Hover, true),
        "on is the selected variant, and the pointer still hovers"
    );
}

/// The same switch at the small size of a settings form's rows.
struct SmallSwitch;

impl Application for SmallSwitch {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, Message::Toggled(on): Message) -> Vec<Command<Message>> {
        model.on = on;
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(
                Toggle {
                    label: "Wi-Fi".into(),
                    on: model.on,
                    enabled: true,
                    on_toggle: Some(Box::new(Message::Toggled)),
                }
                .small(),
            )
            .key("toggle")],
        }))
    }
}

#[test]
fn the_small_switch_has_a_smaller_track_and_a_capsule_thumb_and_toggles_as_the_toggle_does() {
    let mut h = Harness::<SmallSwitch>::new();
    assert_eq!(h.rect("toggle"), Rect::new(0.0, 0.0, 36.0, 16.0));
    let [track, thumb] = fills(&h)[..] else {
        panic!("a track and a thumb")
    };
    assert_eq!((track.0, track.1), (Rect::new(0.0, 0.0, 36.0, 16.0), 8.0));
    assert_eq!((thumb.0, thumb.1), (Rect::new(1.5, 1.5, 21.0, 13.0), 6.5));

    h.click(h.center("toggle"));
    assert!(h.model().on);
    assert_eq!(fills(&h)[1].0.min_x(), 13.5, "on, the thumb is at the end");
    h.click(h.center("toggle"));
    assert!(!h.model().on, "a second click turns it off");
}

#[test]
fn in_right_to_left_layout_the_thumb_starts_on_the_right() {
    let mut h = Harness::<App>::with_direction(LayoutDirection::Rtl);
    let x = h.rect("toggle").min_x();
    assert!(
        (thumb_x(&h) - 7.2).abs() < 1e-9,
        "off is at the start, which is the right"
    );
    h.click(h.center("toggle"));
    assert_eq!(thumb_x(&h), 2.0);
    assert_eq!(h.rect("toggle").min_x(), x);
}

#[test]
fn a_press_shows_pressed_and_a_release_outside_does_not_toggle() {
    let mut h = Harness::<App>::new();
    let center = h.center("toggle");
    h.pointer(PointerEvent::Motion { position: center });
    h.button(center, true);
    assert!(lens(&h), "pressed, the thumb is held");
    assert_eq!(fills(&h)[0].2, well(0.095), "the track stays quiet");

    let outside = point(150.0, 80.0);
    h.pointer(PointerEvent::Motion { position: outside });
    assert!(!lens(&h), "dragged off, it no longer looks pressed");
    assert_eq!(fills(&h)[0].2, well(0.095));
    h.button(outside, false);
    assert!(!h.model().on);

    h.button(center, true);
    h.pointer(PointerEvent::Leave);
    h.button(center, false);
    assert!(!h.model().on, "leaving the surface cancels the press");
}

#[test]
fn touch_toggles_on_release_over_the_switch_only() {
    let mut h = Harness::<App>::new();
    let center = h.center("toggle");
    h.touch(TouchEvent::Down {
        id: 1,
        position: center,
    });
    h.touch(TouchEvent::Up { id: 1 });
    assert!(h.model().on);

    h.touch(TouchEvent::Down {
        id: 2,
        position: center,
    });
    h.touch(TouchEvent::Motion {
        id: 2,
        position: point(150.0, 80.0),
    });
    h.touch(TouchEvent::Up { id: 2 });
    assert!(h.model().on, "lifted away from the switch");

    h.touch(TouchEvent::Down {
        id: 3,
        position: center,
    });
    h.touch(TouchEvent::Cancel);
    h.touch(TouchEvent::Up { id: 3 });
    assert!(h.model().on, "a cancelled touch does nothing");
}

#[test]
fn space_toggles_on_release_and_return_does_not() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    assert!(h.focused("toggle"));

    h.key(SPACE, true);
    assert!(lens(&h), "held by the key");
    assert!(!h.model().on, "nothing happens until Space comes up");
    h.key(SPACE, false);
    assert!(h.model().on);

    h.tap(RETURN);
    assert!(h.model().on);
}

#[test]
fn a_keyboard_focus_draws_the_ring_outside_the_pill() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    let ring = h.ops("toggle").into_iter().find_map(|op| match op {
        PaintOp::Stroke {
            shape: Shape::RoundedRect { rect, radius },
            width,
            paint: Paint::Solid(c),
        } => Some((rect, radius, width, c)),
        _ => None,
    });
    assert_eq!(
        ring,
        Some((
            Rect::new(-3.0, -3.0, 44.0, 28.0),
            14.0,
            2.0,
            color(
                ColorRole::KeyboardFocusIndicator,
                InteractionVariant::Normal,
                false
            )
        ))
    );
}

#[test]
fn it_is_a_switch_named_by_its_label_with_a_checked_value() {
    let mut h = Harness::<App>::new();
    let node = h.accessible("toggle");
    assert_eq!(node.role, Role::Switch);
    assert_eq!(node.name.as_deref(), Some("Wi-Fi"));
    assert_eq!(node.value, Some(AccessibleValue::Toggle(Checked::False)));
    assert_eq!(node.actions, [Action::Click]);
    assert!(node.states.focusable && !node.states.disabled);

    h.click(h.center("toggle"));
    assert_eq!(
        h.accessible("toggle").value,
        Some(AccessibleValue::Toggle(Checked::True))
    );
}

#[test]
fn a_disabled_switch_ignores_input_skips_tab_and_says_it_is_disabled() {
    let mut h = Harness::<Switch<true>>::new();

    h.click(h.center("toggle"));
    h.tap(TAB);
    h.tap(SPACE);
    assert!(!h.model().on);
    assert!(!h.focused("toggle"));
    assert_eq!(fills(&h)[0].2, well(0.05), "a fainter well");
    let node = h.accessible("toggle");
    assert!(node.states.disabled && !node.states.focusable);
    assert!(node.actions.is_empty());
}

/// The held thumb is a lens: the only part of the switch drawn through a clip.
fn has_lens(h: &Harness<App>) -> bool {
    h.ops("toggle")
        .iter()
        .any(|op| matches!(op, PaintOp::PushClip(_)))
}

#[test]
fn the_thumb_is_a_lens_only_while_it_is_held_and_the_lens_never_moves_the_layout() {
    let mut h = Harness::<App>::new();
    let rect = h.rect("toggle");
    assert!(!has_lens(&h), "at rest the thumb is plain");
    let center = h.center("toggle");
    h.pointer(PointerEvent::Motion { position: center });
    h.button(center, true);
    assert!(has_lens(&h), "held, the thumb is a lens");
    assert_eq!(h.rect("toggle"), rect, "the lens is paint only");
    let lens_reach = h
        .ops("toggle")
        .iter()
        .filter_map(|op| match op {
            PaintOp::PushClip(Shape::RoundedRect { rect, .. }) => Some(*rect),
            _ => None,
        })
        .next()
        .unwrap();
    assert!(
        lens_reach.height() > 18.0,
        "the lens is larger than the thumb it replaces"
    );
    h.button(center, false);
    assert!(h.model().on);
    assert!(!has_lens(&h), "released, the thumb is plain again");
    assert_eq!(h.rect("toggle"), rect);
}

#[test]
fn a_dragged_thumb_sets_the_state_it_lands_on_when_let_go_over_the_switch() {
    let mut h = Harness::<App>::new();
    let r = h.rect("toggle");
    let start = point(r.min_x() + 12.0, r.center().y);
    h.pointer(PointerEvent::Motion { position: start });
    h.button(start, true);
    let to = point(start.x + 5.0, start.y);
    h.pointer(PointerEvent::Motion { position: to });
    assert!(has_lens(&h), "a dragged thumb stays a lens");
    assert!(thumb_lens_moved(&h), "the lens follows the pointer");
    h.button(to, false);
    assert!(h.model().on, "the thumb landed in the on half");

    // a drag that stays in the on half changes nothing
    let start = point(r.max_x() - 10.0, r.center().y);
    h.pointer(PointerEvent::Motion { position: start });
    h.button(start, true);
    let to = point(start.x + 6.0, start.y);
    h.pointer(PointerEvent::Motion { position: to });
    h.button(to, false);
    assert!(h.model().on);

    // dragged back past the middle, it lands off
    let start = point(r.max_x() - 10.0, r.center().y);
    h.pointer(PointerEvent::Motion { position: start });
    h.button(start, true);
    let to = point(start.x - 8.0, start.y);
    h.pointer(PointerEvent::Motion { position: to });
    h.button(to, false);
    assert!(!h.model().on);
}

/// The lens's clip no longer sits at the thumb's resting place.
fn thumb_lens_moved(h: &Harness<App>) -> bool {
    h.ops("toggle").iter().any(|op| match op {
        PaintOp::PushClip(Shape::RoundedRect { rect, .. }) => {
            (rect.center().x - (2.0 + 14.4)).abs() > 1.0
        }
        _ => false,
    })
}

#[test]
fn a_drag_let_go_away_from_the_switch_is_cancelled() {
    let mut h = Harness::<App>::new();
    let r = h.rect("toggle");
    let start = point(r.min_x() + 12.0, r.center().y);
    h.pointer(PointerEvent::Motion { position: start });
    h.button(start, true);
    h.pointer(PointerEvent::Motion {
        position: point(start.x + 5.0, start.y),
    });
    let away = point(start.x + 5.0, r.max_y() + 60.0);
    h.pointer(PointerEvent::Motion { position: away });
    assert!(!has_lens(&h), "away from the switch the thumb goes back");
    h.button(away, false);
    assert!(!h.model().on);
}
