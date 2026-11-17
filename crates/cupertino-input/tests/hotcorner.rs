//! A hot corner arms when the pointer enters it with its modifiers held and triggers once after the dwell.

use cupertino_input::{
    Corner, CornerAction, CornerBinding, CornerEvent, HotCornerConfig, HotCorners, Modifiers,
    Point, Rect,
};

const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
const NONE: Modifiers = Modifiers::NONE;
const CTRL: Modifiers = Modifiers::new(false, true, false, false);

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
            CTRL,
        )),
    );
    HotCorners::new(cfg)
}

fn armed(corner: Corner, output: usize) -> CornerEvent {
    CornerEvent::Armed { corner, output }
}

#[test]
fn entering_a_corner_arms_it_the_dwell_triggers_it_once_and_leaving_earlier_disarms() {
    let mut hc = corners();
    let events = hc.update(Point::new(1.0, 1.0), NONE, &[SCREEN], 0.0);
    assert_eq!(events, [armed(Corner::TopLeft, 0)]);
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

    let mut hc = corners();
    hc.update(Point::new(1.0, 1.0), NONE, &[SCREEN], 0.0);
    assert_eq!(
        hc.update(Point::new(200.0, 200.0), NONE, &[SCREEN], 0.05),
        [CornerEvent::Disarmed {
            corner: Corner::TopLeft,
            output: 0
        }]
    );
    assert_eq!(hc.current(), None);
    assert!(hc.tick(1.0).is_empty());
}

#[test]
fn a_bound_corner_is_a_square_of_the_configured_size() {
    for (name, point, arms) in [
        ("inside the square", Point::new(3.9, 3.9), true),
        ("past it across", Point::new(4.5, 1.0), false),
        ("past it down", Point::new(1.0, 4.5), false),
        ("a corner without a binding", Point::new(1919.0, 1.0), false),
    ] {
        let mut hc = corners();
        let events = hc.update(point, NONE, &[SCREEN], 0.0);
        assert_eq!(events.len(), usize::from(arms), "{name}");
        assert_eq!(hc.current().is_some(), arms, "{name}");
    }
}

#[test]
fn the_corner_needs_its_modifiers_allows_extra_ones_and_disarms_when_they_are_released() {
    let at = Point::new(1919.0, 1079.0);
    let mut hc = corners();
    assert!(hc.update(at, NONE, &[SCREEN], 0.0).is_empty());
    assert_eq!(
        hc.current(),
        Some((0, Corner::BottomRight)),
        "hovering, not armed"
    );
    assert_eq!(hc.next_deadline(), None);
    assert_eq!(
        hc.update(at, CTRL, &[SCREEN], 1.0),
        [armed(Corner::BottomRight, 0)]
    );
    assert_eq!(
        hc.update(at, NONE, &[SCREEN], 1.05),
        [CornerEvent::Disarmed {
            corner: Corner::BottomRight,
            output: 0
        }]
    );
    assert!(hc.tick(5.0).is_empty());

    let all = Modifiers::new(true, true, true, true);
    assert_eq!(
        corners().update(at, all, &[SCREEN], 0.0),
        [armed(Corner::BottomRight, 0)]
    );
}

#[test]
fn a_corner_hidden_by_a_neighbouring_output_is_not_exposed() {
    let left = Rect::new(0.0, 0.0, 1000.0, 1000.0);
    let right = Rect::new(1000.0, 0.0, 1000.0, 1000.0);
    let low = Rect::new(500.0, 1000.0, 1000.0, 1000.0);
    for (name, outputs, index, corner, exposed) in [
        ("outer corner", vec![left, right], 0, Corner::TopLeft, true),
        (
            "the pointer would pass to the right output",
            vec![left, right],
            0,
            Corner::TopRight,
            false,
        ),
        (
            "inner corner of the right output",
            vec![left, right],
            1,
            Corner::BottomLeft,
            false,
        ),
        (
            "outer corner of the right output",
            vec![left, right],
            1,
            Corner::BottomRight,
            true,
        ),
        ("a lone output", vec![left], 0, Corner::TopRight, true),
        (
            "staggered, outer",
            vec![left, low],
            0,
            Corner::BottomLeft,
            true,
        ),
        (
            "staggered, inner",
            vec![left, low],
            0,
            Corner::BottomRight,
            false,
        ),
    ] {
        assert_eq!(
            HotCorners::is_exposed(&outputs, index, corner),
            exposed,
            "{name}"
        );
    }
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
    assert_eq!(
        hc.update(Point::new(1999.5, 999.5), NONE, &[left, right], 0.0),
        [armed(Corner::BottomRight, 1)]
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
