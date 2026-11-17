//! A hot corner arms when the pointer enters it with its modifiers held and triggers once after the dwell.

use cupertino_input::{
    Corner, CornerAction, CornerBinding, CornerEvent, HotCornerConfig, HotCorners, Modifiers,
    Point, Rect,
};

const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
const NONE: Modifiers = Modifiers::NONE;

fn corners() -> HotCorners {
    let mut cfg = HotCornerConfig::default();
    cfg.set(
        Corner::TopLeft,
        Some(CornerBinding::new(CornerAction::Overview)),
    );
    cfg.set(
        Corner::BottomRight,
        Some(CornerBinding::with_modifiers(
            CornerAction::Exec("lock".into()),
            Modifiers::new(false, true, false, false),
        )),
    );
    HotCorners::new(cfg)
}

#[test]
fn entering_a_corner_arms_it_and_the_dwell_triggers_it() {
    let mut hc = corners();
    let events = hc.update(Point::new(1.0, 1.0), NONE, &[SCREEN], 0.0);
    assert_eq!(
        events,
        [CornerEvent::Armed {
            corner: Corner::TopLeft,
            output: 0
        }]
    );
    assert_eq!(hc.current(), Some((0, Corner::TopLeft)));
    assert_eq!(hc.next_deadline(), Some(0.1));
    assert!(hc.tick(0.05).is_empty());
    assert_eq!(
        hc.tick(0.1),
        [CornerEvent::Triggered {
            corner: Corner::TopLeft,
            output: 0,
            action: CornerAction::Overview
        }]
    );
    assert!(hc.tick(5.0).is_empty(), "a corner fires once per visit");
    assert_eq!(hc.next_deadline(), None);
}

#[test]
fn leaving_before_the_dwell_disarms_without_triggering() {
    let mut hc = corners();
    hc.update(Point::new(1.0, 1.0), NONE, &[SCREEN], 0.0);
    let events = hc.update(Point::new(200.0, 200.0), NONE, &[SCREEN], 0.05);
    assert_eq!(
        events,
        [CornerEvent::Disarmed {
            corner: Corner::TopLeft,
            output: 0
        }]
    );
    assert_eq!(hc.current(), None);
    assert!(hc.tick(1.0).is_empty());
}

#[test]
fn a_corner_without_a_binding_is_not_a_hot_corner() {
    let mut hc = corners();
    assert!(hc
        .update(Point::new(1919.0, 1.0), NONE, &[SCREEN], 0.0)
        .is_empty());
    assert_eq!(hc.current(), None);
}

#[test]
fn the_corner_needs_its_modifiers_and_disarms_when_they_are_released() {
    let mut hc = corners();
    let ctrl = Modifiers::new(false, true, false, false);
    let at = Point::new(1919.0, 1079.0);
    assert!(hc.update(at, NONE, &[SCREEN], 0.0).is_empty());
    assert_eq!(
        hc.current(),
        Some((0, Corner::BottomRight)),
        "hovering, not armed"
    );
    assert_eq!(hc.next_deadline(), None);
    assert_eq!(
        hc.update(at, ctrl, &[SCREEN], 1.0),
        [CornerEvent::Armed {
            corner: Corner::BottomRight,
            output: 0
        }]
    );
    assert_eq!(
        hc.update(at, NONE, &[SCREEN], 1.05),
        [CornerEvent::Disarmed {
            corner: Corner::BottomRight,
            output: 0
        }]
    );
    assert!(hc.tick(5.0).is_empty());
}

#[test]
fn extra_modifiers_do_not_prevent_a_corner() {
    let mut hc = corners();
    let all = Modifiers::new(true, true, true, true);
    let events = hc.update(Point::new(1919.0, 1079.0), all, &[SCREEN], 0.0);
    assert_eq!(events.len(), 1);
}

#[test]
fn the_corner_is_a_square_of_the_configured_size() {
    let mut hc = corners();
    assert!(hc.update(Point::new(3.9, 3.9), NONE, &[SCREEN], 0.0).len() == 1);
    hc.update(Point::new(500.0, 500.0), NONE, &[SCREEN], 0.1);
    assert!(hc
        .update(Point::new(4.5, 1.0), NONE, &[SCREEN], 0.2)
        .is_empty());
    assert!(hc
        .update(Point::new(1.0, 4.5), NONE, &[SCREEN], 0.3)
        .is_empty());
}

#[test]
fn a_corner_hidden_by_a_neighbouring_output_is_not_exposed() {
    let left = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    let right = Rect::new(1000.0, 0.0, 1000.0, 1000.0);
    let outputs = [left, right];
    assert!(HotCorners::is_exposed(&outputs, 0, Corner::TopLeft));
    assert!(
        !HotCorners::is_exposed(&outputs, 0, Corner::TopRight),
        "the pointer would pass to the right output"
    );
    assert!(!HotCorners::is_exposed(&outputs, 1, Corner::BottomLeft));
    assert!(HotCorners::is_exposed(&outputs, 1, Corner::BottomRight));
    assert!(HotCorners::is_exposed(&[left], 0, Corner::TopRight));
}

#[test]
fn a_staggered_layout_exposes_the_outer_corner_only() {
    let top = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    let low = Rect::new(500.0, 1000.0, 1000.0, 1000.0);
    assert!(HotCorners::is_exposed(&[top, low], 0, Corner::BottomLeft));
    assert!(!HotCorners::is_exposed(&[top, low], 0, Corner::BottomRight));
}

#[test]
fn the_output_index_names_where_the_corner_was_hit() {
    let left = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    let right = Rect::new(1000.0, 0.0, 1000.0, 1000.0);
    let mut hc = corners();
    hc.cfg.set(
        Corner::BottomRight,
        Some(CornerBinding::new(CornerAction::ShowDesktop)),
    );
    let events = hc.update(Point::new(1999.5, 999.5), NONE, &[left, right], 0.0);
    assert_eq!(
        events,
        [CornerEvent::Armed {
            corner: Corner::BottomRight,
            output: 1
        }]
    );
}

#[test]
fn configuring_a_corner_replaces_or_clears_its_binding() {
    let mut cfg = HotCornerConfig::default();
    assert!(Corner::ALL.iter().all(|c| cfg.binding(*c).is_none()));
    cfg.set(
        Corner::TopRight,
        Some(CornerBinding::new(CornerAction::AppGrid)),
    );
    assert_eq!(
        cfg.binding(Corner::TopRight).unwrap().action,
        CornerAction::AppGrid
    );
    cfg.set(Corner::TopRight, None);
    assert!(cfg.binding(Corner::TopRight).is_none());
}
