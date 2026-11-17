//! The menu bar set: space reserved, auto-hide, reveal dwell and the inactive dimming.

use cupertino_wm::menu_bar::{AutoHide, BarEvent, BarSetConfig, MenuBarSet};
use cupertino_wm::{OutputId, Point, Rect};

const A: OutputId = OutputId(1);
const B: OutputId = OutputId(2);
const RECT_A: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
const RECT_B: Rect = Rect::new(1920.0, 0.0, 1280.0, 720.0);

fn set(policy: AutoHide) -> MenuBarSet {
    let mut s = MenuBarSet::new(BarSetConfig {
        auto_hide: policy,
        ..BarSetConfig::default()
    });
    s.add_output(A, RECT_A, 0.0);
    s
}

#[test]
fn a_visible_bar_reserves_its_height_from_the_usable_area() {
    let s = set(AutoHide::Never);
    let h = s.bar_height(A).unwrap();
    assert_eq!(s.reserved_height(A), Some(h));
    assert_eq!(
        s.usable_area(A),
        Some(Rect::new(0.0, h, 1920.0, 1080.0 - h))
    );
    assert!(s.reserves_space(A));
    assert!(s.usable_area(OutputId(9)).is_none());
}

#[test]
fn a_per_output_bar_height_changes_only_that_output() {
    let mut s = set(AutoHide::Never);
    s.add_output(B, RECT_B, 0.0);
    s.set_bar_height(B, 48.0);
    assert_eq!(s.bar_height(B), Some(48.0));
    assert_eq!(s.bar_height(A), Some(BarSetConfig::default().bar_height));
    assert_eq!(s.usable_area(B).unwrap().y, 48.0);
}

#[test]
fn the_default_policy_hides_the_bar_only_while_full_screen() {
    let mut s = set(AutoHide::FullscreenOnly);
    assert!(s.reserves_space(A));
    let ev = s.set_fullscreen(A, true, 1.0);
    assert_eq!(ev, [BarEvent::UsableAreaChanged { output: A }]);
    assert!(!s.reserves_space(A));
    assert_eq!(s.reserved_height(A), Some(0.0));
    assert_eq!(s.usable_area(A), Some(RECT_A));
    assert!(
        s.set_fullscreen(A, true, 1.1).is_empty(),
        "no change, no event"
    );
    s.set_fullscreen(A, false, 2.0);
    assert!(s.reserves_space(A));
}

#[test]
fn never_ignores_full_screen_and_always_hides_without_it() {
    let mut never = set(AutoHide::Never);
    assert!(never.set_fullscreen(A, true, 1.0).is_empty());
    assert!(never.reserves_space(A));
    let always = set(AutoHide::Always);
    assert!(!always.reserves_space(A));
}

#[test]
fn changing_the_policy_reports_every_output_and_clears_a_reveal() {
    let mut s = set(AutoHide::Always);
    s.add_output(B, RECT_B, 0.0);
    s.set_revealed(A, true, 1.0);
    let ev = s.set_auto_hide(AutoHide::Never, 2.0);
    assert!(ev.contains(&BarEvent::UsableAreaChanged { output: A }));
    assert!(ev.contains(&BarEvent::UsableAreaChanged { output: B }));
    assert!(!s.is_revealed(A));
}

#[test]
fn the_slide_follows_the_hidden_target_over_time() {
    let mut s = set(AutoHide::FullscreenOnly);
    let p = s.presentation(A, 0.0).unwrap();
    assert_eq!((p.visible, p.rect.y), (1.0, 0.0));
    s.set_fullscreen(A, true, 1.0);
    let mid = s.presentation(A, 1.1).unwrap();
    assert!(mid.visible > 0.0 && mid.visible < 1.0 && mid.animating);
    assert!(mid.rect.y < 0.0, "partly above the output while hiding");
    let end = s.presentation(A, 5.0).unwrap();
    assert_eq!(end.visible, 0.0);
    assert!(!end.animating);
    assert_eq!(end.rect.y, -end.rect.h);
}

#[test]
fn dwelling_at_the_top_edge_reveals_a_hidden_bar_after_the_dwell() {
    let mut s = set(AutoHide::Always);
    let dwell = s.cfg.reveal_dwell;
    assert!(s.pointer_moved(Point::new(500.0, 0.0), 1.0).is_empty());
    assert_eq!(s.next_deadline(), Some(1.0 + dwell));
    assert!(s.tick(1.0 + dwell / 2.0).is_empty());
    assert_eq!(s.tick(1.0 + dwell), [BarEvent::Revealed { output: A }]);
    assert!(s.is_revealed(A));
    assert!(
        !s.reserves_space(A),
        "a reveal overlays, it does not take room"
    );
}

#[test]
fn leaving_the_edge_before_the_dwell_cancels_it() {
    let mut s = set(AutoHide::Always);
    s.pointer_moved(Point::new(500.0, 0.0), 1.0);
    s.pointer_moved(Point::new(500.0, 400.0), 1.05);
    assert!(s.next_deadline().is_none());
    assert!(s.tick(5.0).is_empty());
    assert!(!s.is_revealed(A));
}

#[test]
fn a_revealed_bar_hides_after_the_pointer_leaves_it_for_the_hide_delay() {
    let mut s = set(AutoHide::Always);
    s.set_revealed(A, true, 1.0);
    let delay = s.cfg.hide_delay;
    s.pointer_moved(Point::new(500.0, 500.0), 2.0);
    assert!(s.tick(2.0 + delay / 2.0).is_empty());
    assert_eq!(s.tick(2.0 + delay), [BarEvent::Hidden { output: A }]);
    assert!(!s.is_revealed(A));
}

#[test]
fn staying_over_the_bar_keeps_it_revealed() {
    let mut s = set(AutoHide::Always);
    s.set_revealed(A, true, 1.0);
    s.pointer_moved(Point::new(500.0, 500.0), 2.0);
    s.pointer_moved(Point::new(500.0, 10.0), 2.1);
    assert!(s.tick(10.0).is_empty());
    assert!(s.is_revealed(A));
}

#[test]
fn an_open_menu_keeps_the_bar_shown_and_closing_it_starts_the_hide_timer() {
    let mut s = set(AutoHide::Always);
    s.set_revealed(A, true, 1.0);
    s.set_menu_open(A, true, 2.0);
    s.pointer_moved(Point::new(500.0, 500.0), 2.1);
    assert!(s.tick(20.0).is_empty());
    assert_eq!(s.presentation(A, 30.0).unwrap().visible, 1.0);
    s.set_menu_open(A, false, 30.0);
    assert_eq!(
        s.tick(30.0 + s.cfg.hide_delay),
        [BarEvent::Hidden { output: A }]
    );
}

#[test]
fn setting_the_revealed_state_reports_only_real_changes() {
    let mut s = set(AutoHide::Always);
    assert_eq!(
        s.set_revealed(A, true, 1.0),
        [BarEvent::Revealed { output: A }]
    );
    assert!(s.set_revealed(A, true, 1.1).is_empty());
    assert_eq!(
        s.set_revealed(A, false, 1.2),
        [BarEvent::Hidden { output: A }]
    );
    assert!(s.set_revealed(OutputId(9), true, 1.3).is_empty());
}

#[test]
fn only_the_outer_top_edge_of_a_stacked_output_reveals() {
    let mut s = set(AutoHide::Always);
    // B sits directly above A: A's top edge is interior and never counts.
    s.add_output(B, Rect::new(0.0, -720.0, 1920.0, 720.0), 0.0);
    s.pointer_moved(Point::new(500.0, 0.0), 1.0);
    assert!(s
        .tick(10.0)
        .iter()
        .all(|e| *e != BarEvent::Revealed { output: A }));
    s.pointer_moved(Point::new(500.0, -720.0), 11.0);
    assert!(s.tick(20.0).contains(&BarEvent::Revealed { output: B }));
}

#[test]
fn the_inactive_output_is_dimmed() {
    let mut s = set(AutoHide::Never);
    s.add_output(B, RECT_B, 0.0);
    s.set_active_output(Some(A), 1.0);
    assert_eq!(s.active_output(), Some(A));
    assert_eq!(s.presentation(A, 5.0).unwrap().inactive, 0.0);
    assert_eq!(s.presentation(B, 5.0).unwrap().inactive, 1.0);
    s.set_active_output(Some(B), 6.0);
    assert_eq!(s.presentation(A, 10.0).unwrap().inactive, 1.0);
    assert_eq!(s.presentation(B, 10.0).unwrap().inactive, 0.0);
    let mid = s.presentation(A, 6.1).unwrap();
    assert!(mid.inactive > 0.0 && mid.inactive < 1.0, "the dim animates");
}

#[test]
fn removing_an_output_forgets_it() {
    let mut s = set(AutoHide::Never);
    s.add_output(B, RECT_B, 0.0);
    assert_eq!(s.outputs(), [A, B]);
    s.remove_output(B, 1.0);
    s.tick(5.0);
    assert_eq!(s.outputs(), [A]);
    assert!(s.presentation(B, 5.0).is_none());
}

#[test]
fn the_full_screen_title_bar_slides_in_with_the_bar() {
    let mut s = set(AutoHide::FullscreenOnly);
    assert!(
        s.fullscreen_titlebar(A, 28.0, 0.0).is_none(),
        "not full screen"
    );
    s.set_fullscreen(A, true, 1.0);
    assert!(s.fullscreen_titlebar(A, 28.0, 10.0).is_none(), "hidden");
    s.set_revealed(A, true, 11.0);
    let t = s.fullscreen_titlebar(A, 28.0, 20.0).unwrap();
    let bar = s.bar_height(A).unwrap();
    assert_eq!((t.y, t.h, t.w), (bar, 28.0, 1920.0));
    let mut never = set(AutoHide::Never);
    never.set_fullscreen(A, true, 1.0);
    assert!(
        never.fullscreen_titlebar(A, 28.0, 20.0).is_none(),
        "the bar does not auto-hide"
    );
}
