//! The stage model: groups, the centre stage, the strip of recent groups, drops and strip hiding.

use cupertino_wm::stage::{
    ClickDesktop, DropTarget, GroupId, Grouping, Placement, Stage, StageConfig, StageEvent,
};
use cupertino_wm::{AppId, Point, Rect, WindowId};

const USABLE: Rect = Rect::new(0.0, 32.0, 1920.0, 1048.0);

fn w(n: u64) -> WindowId {
    WindowId(n)
}
fn app(n: u64) -> AppId {
    AppId(n)
}
fn frame(n: u64) -> Rect {
    Rect::new(300.0 + 20.0 * n as f64, 100.0, 800.0, 600.0)
}

fn on(cfg: StageConfig) -> Stage {
    let mut s = Stage::new(cfg, USABLE);
    s.set_enabled(true, &[], 0.0);
    s
}

/// A stage with windows 1..=n, one app each, opened in order (so the last is the centre).
fn stage_of(n: u64) -> Stage {
    let mut s = on(StageConfig::default());
    for i in 1..=n {
        s.window_opened(w(i), app(i), frame(i), i as f64);
    }
    s
}

fn created(ev: &[StageEvent]) -> Vec<GroupId> {
    ev.iter()
        .filter_map(|e| match e {
            StageEvent::GroupCreated { group } => Some(*group),
            _ => None,
        })
        .collect()
}

#[test]
fn a_disabled_stage_ignores_windows_and_disabling_forgets_every_group() {
    let mut s = Stage::new(StageConfig::default(), USABLE);
    assert!(!s.is_enabled());
    assert!(s.window_opened(w(1), app(1), frame(1), 0.0).is_empty());
    assert_eq!(s.center_group(), None);
    assert!(s.desktop_items_visible());
    assert_eq!(s.stage_area(), USABLE);
    assert!(s.strip_slots(0.0).is_empty());
    assert!(s.placements().is_empty());
    assert_eq!(s.drop_target(Point::new(500.0, 500.0)), None);
    assert!(s.click_desktop(1.0).is_empty());
    assert!(s.update(None, 1.0).is_empty());
}

#[test]
fn enabling_builds_groups_from_the_windows_most_recent_first() {
    let mut s = Stage::new(StageConfig::default(), USABLE);
    let ev = s.set_enabled(
        true,
        &[
            (w(1), app(1), frame(1)),
            (w(2), app(2), frame(2)),
            (w(3), app(3), frame(3)),
        ],
        0.0,
    );
    assert_eq!(ev.first(), Some(&StageEvent::Enabled));
    assert_eq!(created(&ev).len(), 3);
    assert_eq!(s.center_windows(), [w(1)]);
    assert_eq!(s.strip_groups().len(), 2);
    assert_eq!(s.group_windows(s.strip_groups()[0]), [w(2)]);
    assert_eq!(s.group_blend(s.center_group().unwrap(), 0.0), 1.0);
}

#[test]
fn windows_of_one_app_share_a_group_unless_grouping_is_one_at_a_time() {
    let wins = [
        (w(1), app(1), frame(1)),
        (w(2), app(1), frame(2)),
        (w(3), app(2), frame(3)),
    ];
    let mut s = Stage::new(StageConfig::default(), USABLE);
    s.set_enabled(true, &wins, 0.0);
    assert_eq!(s.group_of(w(1)), s.group_of(w(2)));
    assert_ne!(s.group_of(w(1)), s.group_of(w(3)));
    assert_eq!(s.group_apps(s.group_of(w(1)).unwrap()), [app(1)]);

    let mut s = Stage::new(
        StageConfig {
            grouping: Grouping::OneAtATime,
            ..StageConfig::default()
        },
        USABLE,
    );
    s.set_enabled(true, &wins, 0.0);
    assert_ne!(s.group_of(w(1)), s.group_of(w(2)));
    let mut s = stage_of(3);
    let ev = s.set_enabled(false, &[], 10.0);
    assert_eq!(
        ev,
        [StageEvent::Disabled, StageEvent::DesktopItemsVisible(true)]
    );
    assert!(s.center_group().is_none() && s.strip_groups().is_empty());
    assert_eq!(s.group_of(w(1)), None);
}

#[test]
fn a_new_window_takes_the_stage_joins_its_apps_group_or_activates_a_known_window() {
    let mut s = on(StageConfig::default());
    let ev = s.window_opened(w(1), app(1), frame(1), 1.0);
    let g1 = created(&ev)[0];
    assert!(ev.contains(&StageEvent::Swap {
        incoming: g1,
        outgoing: None
    }));
    let ev = s.window_opened(w(2), app(2), frame(2), 2.0);
    let g2 = created(&ev)[0];
    assert!(ev.contains(&StageEvent::Swap {
        incoming: g2,
        outgoing: Some(g1)
    }));
    assert_eq!(s.center_group(), Some(g2));
    assert_eq!(s.strip_groups(), [g1]);
    let mut s = on(StageConfig::default());
    s.window_opened(w(1), app(1), frame(1), 1.0);
    let ev = s.window_opened(w(2), app(1), frame(2), 2.0);
    assert!(created(&ev).is_empty());
    assert!(!ev.iter().any(|e| matches!(e, StageEvent::Swap { .. })));
    assert_eq!(s.center_windows(), [w(2), w(1)], "most recent first");
    let mut s = stage_of(2);
    let ev = s.window_opened(w(1), app(1), frame(1), 5.0);
    assert!(created(&ev).is_empty());
    assert_eq!(s.group_of(w(1)), s.center_group());
}

#[test]
fn the_strip_keeps_the_most_recently_left_group_on_top() {
    let s = stage_of(4);
    let top: Vec<_> = s
        .strip_groups()
        .iter()
        .map(|g| s.group_windows(*g)[0])
        .collect();
    assert_eq!(top, [w(3), w(2), w(1)]);
}

#[test]
fn activating_a_strip_window_swaps_it_with_the_centre_and_the_centre_changes_nothing() {
    let mut s = stage_of(3);
    let old = s.center_group().unwrap();
    let ev = s.window_activated(w(1), 10.0);
    let g1 = s.group_of(w(1)).unwrap();
    assert!(ev.contains(&StageEvent::Swap {
        incoming: g1,
        outgoing: Some(old)
    }));
    assert_eq!(s.center_group(), Some(g1));
    assert!(s.strip_groups().contains(&old));
    assert!(!s.strip_groups().contains(&g1));
    let mut s = stage_of(2);
    assert!(s.window_activated(w(2), 5.0).is_empty());
    assert!(s.window_activated(w(99), 5.0).is_empty(), "unknown window");
    assert!(
        s.activate_group(GroupId(999), 5.0).is_empty(),
        "unknown group"
    );
}

#[test]
fn a_swap_animates_the_blend_between_thumbnail_and_stage() {
    let mut s = stage_of(2);
    let incoming = s.group_of(w(1)).unwrap();
    let outgoing = s.center_group().unwrap();
    s.activate_group(incoming, 10.0);
    assert_eq!(s.group_blend(incoming, 10.0), 0.0);
    let mid = s.group_blend(incoming, 10.15);
    assert!(mid > 0.0 && mid < 1.0, "{mid}");
    assert!(s.is_animating(10.15));
    assert!((s.group_blend(incoming, 12.0) - 1.0).abs() < 1e-3);
    assert!(s.group_blend(outgoing, 12.0).abs() < 1e-3);
    assert!(!s.is_animating(12.0));
}

#[test]
fn closing_windows_keeps_removes_or_promotes_groups() {
    let mut s = stage_of(2);
    let centre = s.center_group().unwrap();
    let ev = s.window_closed(w(2), 10.0);
    assert!(ev.contains(&StageEvent::GroupRemoved { group: centre }));
    assert!(ev.contains(&StageEvent::CenterEmptied { group: None }));
    assert_eq!(s.center_group(), None);

    let mut s = on(StageConfig {
        promote_on_empty: true,
        ..StageConfig::default()
    });
    s.window_opened(w(1), app(1), frame(1), 1.0);
    s.window_opened(w(2), app(2), frame(2), 2.0);
    let ev = s.window_closed(w(2), 10.0);
    assert!(ev.iter().any(|e| matches!(e, StageEvent::Swap { .. })));
    assert_eq!(s.center_windows(), [w(1)]);
    let mut s = on(StageConfig::default());
    s.window_opened(w(1), app(1), frame(1), 1.0);
    s.window_opened(w(2), app(1), frame(2), 2.0);
    assert!(s.window_closed(w(2), 3.0).is_empty());
    assert_eq!(s.center_windows(), [w(1)]);
    assert!(s.window_closed(w(99), 3.0).is_empty(), "unknown window");
    let mut s = stage_of(3);
    let g1 = s.group_of(w(1)).unwrap();
    let ev = s.window_closed(w(1), 10.0);
    assert_eq!(ev, [StageEvent::GroupRemoved { group: g1 }]);
    assert_eq!(s.strip_groups().len(), 1);
    assert!(s.center_group().is_some());
}

#[test]
fn clicking_the_desktop_empties_the_stage_unless_configured_to_do_nothing() {
    let mut s = stage_of(2);
    let centre = s.center_group().unwrap();
    assert!(!s.desktop_items_visible(), "hidden while the stage is on");
    let ev = s.click_desktop(10.0);
    assert!(ev.contains(&StageEvent::CenterEmptied {
        group: Some(centre)
    }));
    assert!(ev.contains(&StageEvent::DesktopItemsVisible(true)));
    assert_eq!(s.center_group(), None);
    assert_eq!(
        s.strip_groups()[0],
        centre,
        "the old centre is first in the strip"
    );
    assert!(s.desktop_items_visible());
    // The next window takes the stage and hides the desktop items again.
    let ev = s.window_opened(w(3), app(3), frame(3), 11.0);
    assert!(ev.contains(&StageEvent::DesktopItemsVisible(false)));
    let mut s = on(StageConfig {
        click_desktop: ClickDesktop::Nothing,
        ..StageConfig::default()
    });
    s.window_opened(w(1), app(1), frame(1), 1.0);
    assert!(s.click_desktop(2.0).is_empty());
    assert!(s.center_group().is_some());
}

#[test]
fn the_stage_area_sits_right_of_the_strip_and_the_strip_area_is_a_full_height_column() {
    let s = on(StageConfig::default());
    let cfg = StageConfig::default();
    let stage = s.stage_area();
    let strip = s.strip_area();
    assert_eq!(strip, Rect::new(16.0, 32.0, 170.0, 1048.0));
    assert_eq!(stage.x, strip.right() + cfg.stage_inset_from_strip);
    assert_eq!(stage.right(), USABLE.right() - 16.0);
    assert_eq!((stage.y, stage.h), (USABLE.y, USABLE.h));
    assert_eq!(cfg.stage_frame(USABLE), stage);
    let narrow = Rect::new(0.0, 0.0, 100.0, 100.0);
    let f = StageConfig::default().stage_frame(narrow);
    assert!(f.w >= 1.0 && f.h >= 1.0);
}

#[test]
fn the_strip_shows_up_to_the_maximum_rows_and_fewer_smaller_slots_on_a_short_output() {
    let s = stage_of(12);
    let (rows, height) = s.strip_metrics();
    assert!((4..=6).contains(&rows), "{rows}");
    assert!((20.0..=160.0).contains(&height), "{height}");
    assert_eq!(s.visible_strip().len(), rows);
    assert_eq!(s.rows(), rows);
    let mut s = on(StageConfig::default());
    s.set_usable(Rect::new(0.0, 0.0, 1000.0, 300.0), 0.0);
    let (rows, h) = s.strip_metrics();
    assert!(
        (4..=6).contains(&rows),
        "the row count is clamped to the minimum: {rows}"
    );
    assert!(h >= 20.0);
}

#[test]
fn slots_are_stacked_with_the_row_gap_and_centred_vertically() {
    let s = stage_of(4);
    let (_, h) = s.strip_metrics();
    let n = s.visible_strip().len();
    assert_eq!(n, 3);
    let (a, b, c) = (s.slot_rect(0), s.slot_rect(1), s.slot_rect(2));
    assert_eq!(b.y - a.bottom(), 32.0);
    assert_eq!(c.y - b.bottom(), 32.0);
    assert_eq!((a.x, a.w, a.h), (16.0, 170.0, h));
    let top_gap = a.y - USABLE.y;
    let bottom_gap = USABLE.bottom() - c.bottom();
    assert!(
        (top_gap - bottom_gap).abs() < 1e-9,
        "centred: {top_gap} vs {bottom_gap}"
    );
}

#[test]
fn strip_slots_carry_app_icons_for_up_to_three_apps() {
    let mut s = on(StageConfig::default());
    for i in 1..=4 {
        s.window_opened(w(i), app(1), frame(i), i as f64);
    }
    s.window_opened(w(5), app(2), frame(5), 5.0);
    s.window_opened(w(6), app(3), frame(6), 6.0);
    s.window_opened(w(7), app(9), frame(7), 7.0);
    // Merge three strip groups' windows into one by hand: one-at-a-time grouping off, so use drops.
    let slots = s.strip_slots(100.0);
    assert_eq!(slots.len(), 3);
    for slot in &slots {
        assert_eq!(slot.apps.len(), 1);
        assert_eq!(slot.icons.len(), 1);
        let icon = slot.icons[0];
        assert_eq!(icon.w, 40.0);
        assert_eq!(icon.bottom(), slot.rect.bottom() - 10.0);
        assert_eq!(icon.x, slot.rect.x - 10.0);
    }
}

#[test]
fn merging_groups_onto_the_stage_collects_their_apps() {
    let mut s = on(StageConfig::default());
    s.window_opened(w(1), app(1), frame(1), 1.0);
    s.window_opened(w(2), app(2), frame(2), 2.0);
    s.window_opened(w(3), app(3), frame(3), 3.0);
    let g1 = s.group_of(w(1)).unwrap();
    let g2 = s.group_of(w(2)).unwrap();
    // Dragging both strip groups onto the stage merges them into the centre group.
    s.add_to_stage(g1, 4.0);
    s.add_to_stage(g2, 4.0);
    let centre = s.center_group().unwrap();
    assert_eq!(s.group_apps(centre).len(), 3);
    assert_eq!(s.center_windows().len(), 3);
}

#[test]
fn adding_a_group_to_the_stage_merges_it_into_the_centre_or_makes_it_the_centre() {
    let mut s = stage_of(2);
    let g1 = s.group_of(w(1)).unwrap();
    let centre = s.center_group().unwrap();
    let ev = s.add_to_stage(g1, 10.0);
    assert!(ev.contains(&StageEvent::WindowMoved {
        window: w(1),
        from: g1,
        to: centre
    }));
    assert!(ev.contains(&StageEvent::GroupRemoved { group: g1 }));
    assert_eq!(
        s.center_windows(),
        [w(1), w(2)],
        "the merged windows come first"
    );
    assert!(s.strip_groups().is_empty());
    assert_eq!(s.group_of(w(1)), Some(centre));
    let mut s = stage_of(2);
    let centre = s.center_group().unwrap();
    assert!(s.add_to_stage(centre, 5.0).is_empty());
    assert!(s.add_to_stage(GroupId(999), 5.0).is_empty());
    let mut s = stage_of(2);
    s.empty_stage(5.0);
    let g = s.strip_groups()[0];
    let ev = s.add_to_stage(g, 6.0);
    assert!(matches!(
        ev.first(),
        Some(StageEvent::Swap { outgoing: None, .. })
    ));
    assert!(ev.contains(&StageEvent::DesktopItemsVisible(false)));
    assert_eq!(s.center_group(), Some(g));
}

#[test]
fn drop_targets_are_the_stage_the_strip_column_or_nothing() {
    let s = stage_of(2);
    assert_eq!(
        s.drop_target(Point::new(1000.0, 500.0)),
        Some(DropTarget::Stage)
    );
    assert_eq!(
        s.drop_target(Point::new(100.0, 500.0)),
        Some(DropTarget::Strip)
    );
    assert_eq!(
        s.drop_target(Point::new(2.0, 500.0)),
        Some(DropTarget::Strip),
        "the strip reaches the edge"
    );
    assert_eq!(
        s.drop_target(Point::new(1000.0, 10.0)),
        None,
        "above the usable area"
    );
    assert_eq!(
        s.drop_target(Point::new(196.0, 500.0)),
        None,
        "between strip and stage"
    );
    let mut s = stage_of(2);
    let g1 = s.group_of(w(1)).unwrap();
    assert!(s.drop_group(g1, Point::new(100.0, 500.0), 5.0).is_empty());
    assert!(!s.drop_group(g1, Point::new(1000.0, 500.0), 5.0).is_empty());
}

#[test]
fn dropping_a_centre_window_on_the_strip_splits_it_into_its_own_group() {
    let mut s = on(StageConfig::default());
    s.window_opened(w(1), app(1), frame(1), 1.0);
    s.window_opened(w(2), app(1), frame(2), 2.0);
    let centre = s.center_group().unwrap();
    let ev = s.drop_window(w(2), Point::new(100.0, 500.0), 5.0);
    let new = s.group_of(w(2)).unwrap();
    assert_ne!(new, centre);
    assert!(ev.contains(&StageEvent::GroupCreated { group: new }));
    assert!(ev.contains(&StageEvent::WindowMoved {
        window: w(2),
        from: centre,
        to: new
    }));
    assert_eq!(s.strip_groups()[0], new);
    assert_eq!(s.center_windows(), [w(1)]);
    let mut s = stage_of(1);
    let centre = s.center_group().unwrap();
    let ev = s.drop_window(w(1), Point::new(100.0, 500.0), 5.0);
    assert_eq!(
        ev,
        [StageEvent::CenterEmptied {
            group: Some(centre)
        }]
    );
    assert!(!s.desktop_items_visible());
    assert_eq!(s.strip_groups(), [centre]);
    let mut s = stage_of(2);
    assert!(
        s.drop_window(w(1), Point::new(100.0, 500.0), 5.0)
            .is_empty(),
        "a strip window"
    );
    assert!(
        s.drop_window(w(2), Point::new(1000.0, 500.0), 5.0)
            .is_empty(),
        "not over the strip"
    );
    assert!(
        s.drop_window(w(99), Point::new(100.0, 500.0), 5.0)
            .is_empty(),
        "unknown window"
    );
}

#[test]
fn placements_say_where_each_window_rests() {
    let s = stage_of(8);
    let all = s.placements();
    assert_eq!(all.len(), 8);
    assert!(all.windows(2).all(|p| p[0].0 < p[1].0), "sorted by id");
    let get = |n: u64| all.iter().find(|(id, _)| *id == w(n)).unwrap().1;
    assert_eq!(get(8), Placement::Stage);
    let rows = s.rows();
    let in_strip = all
        .iter()
        .filter(|(_, p)| matches!(p, Placement::Strip(_)))
        .count();
    let hidden = all.iter().filter(|(_, p)| *p == Placement::Hidden).count();
    assert_eq!(in_strip, rows);
    assert_eq!(hidden, 7 - rows, "groups beyond the rows are not shown");
    let Placement::Strip(r) = get(7) else {
        panic!("the newest strip group has a slot")
    };
    assert!(s
        .slot_rect(0)
        .inset(6.0)
        .contains(Point::new(r.cx(), r.cy())));
    assert!(
        r.w <= s.slot_rect(0).w && r.h <= s.slot_rect(0).h,
        "a thumbnail is never larger than its slot"
    );
}

#[test]
fn a_thumbnail_keeps_the_aspect_ratio_and_is_never_enlarged() {
    let mut s = on(StageConfig::default());
    s.window_opened(w(1), app(1), Rect::new(0.0, 0.0, 100.0, 50.0), 1.0);
    s.window_opened(w(2), app(2), frame(2), 2.0);
    let all = s.placements();
    let Placement::Strip(r) = all.iter().find(|(id, _)| *id == w(1)).unwrap().1 else {
        panic!("in the strip")
    };
    assert_eq!((r.w, r.h), (100.0, 50.0), "a small window is not enlarged");
    s.set_window_frame(w(1), Rect::new(0.0, 0.0, 1600.0, 800.0));
    let all = s.placements();
    let Placement::Strip(r) = all.iter().find(|(id, _)| *id == w(1)).unwrap().1 else {
        panic!()
    };
    assert!((r.w / r.h - 2.0).abs() < 1e-9);
    assert!(r.w <= 170.0 - 12.0 + 1e-9);
}

#[test]
fn window_frames_blend_between_the_thumbnail_and_the_stage_frame() {
    let mut s = stage_of(2);
    let at_rest = s.window_frames(100.0);
    let f2 = at_rest.iter().find(|(id, _, _)| *id == w(2)).unwrap();
    assert_eq!((f2.1, f2.2), (frame(2), 1.0));
    let f1 = at_rest.iter().find(|(id, _, _)| *id == w(1)).unwrap();
    assert_eq!(f1.2, 0.0);
    assert!(f1.1.w < frame(1).w);
    s.activate_group(s.group_of(w(1)).unwrap(), 200.0);
    let mid = s.window_frames(200.15);
    let m1 = mid.iter().find(|(id, _, _)| *id == w(1)).unwrap();
    assert!(m1.2 > 0.0 && m1.2 < 1.0);
    assert!(m1.1.w > f1.1.w && m1.1.w < frame(1).w);
}

#[test]
fn fitting_a_group_shrinks_it_to_the_stage_and_centres_it_but_never_enlarges() {
    let mut s = on(StageConfig::default());
    s.window_opened(w(1), app(1), Rect::new(0.0, 0.0, 400.0, 300.0), 1.0);
    let g = s.center_group().unwrap();
    let fit = s.fit_group(g);
    let area = s.stage_area();
    assert_eq!(fit.len(), 1);
    assert_eq!(fit[0].1.w, 400.0);
    assert!((fit[0].1.cx() - area.cx()).abs() < 1e-9 && (fit[0].1.cy() - area.cy()).abs() < 1e-9);

    s.set_window_frame(w(1), Rect::new(0.0, 0.0, 4000.0, 1000.0));
    let r = s.fit_group(g)[0].1;
    assert!(r.w <= area.w + 1e-9 && r.h <= area.h + 1e-9);
    assert!((r.w / r.h - 4.0).abs() < 1e-9, "the aspect ratio is kept");
    assert!(s.fit_group(GroupId(999)).is_empty());
}

#[test]
fn the_strip_hides_when_a_centre_window_overlaps_it_or_auto_hide_is_on() {
    let mut s = stage_of(2);
    assert!(!s.center_overlaps_strip());
    assert!(s.update(Some(Point::new(900.0, 500.0)), 5.0).is_empty());
    assert!(!s.is_strip_hidden_target());

    s.set_window_frame(w(2), Rect::new(100.0, 100.0, 800.0, 600.0));
    assert!(s.center_overlaps_strip());
    assert!(
        s.update(Some(Point::new(900.0, 500.0)), 10.0).is_empty(),
        "the hide delay starts"
    );
    assert!(s.update(Some(Point::new(900.0, 500.0)), 10.1).is_empty());
    let ev = s.update(Some(Point::new(900.0, 500.0)), 10.31);
    assert_eq!(ev, [StageEvent::StripHidden]);
    assert!(s.is_strip_hidden_target());
    assert!(s.strip_shift(20.0) < 0.0);
    assert_eq!(s.strip_hidden_fraction(20.0), 1.0);
}

#[test]
fn dwelling_at_the_left_edge_brings_a_hidden_strip_back_unless_the_pointer_leaves() {
    let mut s = stage_of(2);
    s.set_window_frame(w(2), Rect::new(100.0, 100.0, 800.0, 600.0));
    s.update(Some(Point::new(900.0, 500.0)), 10.0);
    s.update(Some(Point::new(900.0, 500.0)), 10.4);
    assert!(s.is_strip_hidden_target());
    assert!(s.update(Some(Point::new(1.0, 500.0)), 20.0).is_empty());
    assert!(s.wants_update(20.0), "a timer is running");
    assert!(s.update(Some(Point::new(1.0, 500.0)), 20.1).is_empty());
    let ev = s.update(Some(Point::new(1.0, 500.0)), 20.16);
    assert_eq!(ev, [StageEvent::StripShown]);
    assert!(!s.is_strip_hidden_target());
    let mut s = stage_of(2);
    s.set_window_frame(w(2), Rect::new(100.0, 100.0, 800.0, 600.0));
    s.update(Some(Point::new(900.0, 500.0)), 10.0);
    s.update(Some(Point::new(900.0, 500.0)), 10.4);
    s.update(Some(Point::new(1.0, 500.0)), 20.0);
    s.update(Some(Point::new(900.0, 500.0)), 20.1);
    assert!(s.update(Some(Point::new(900.0, 500.0)), 21.0).is_empty());
    assert!(s.is_strip_hidden_target());
    let mut s = on(StageConfig {
        auto_hide: true,
        ..StageConfig::default()
    });
    s.window_opened(w(1), app(1), frame(1), 1.0);
    s.window_opened(w(2), app(2), frame(2), 2.0);
    s.update(Some(Point::new(900.0, 500.0)), 5.0);
    let ev = s.update(Some(Point::new(900.0, 500.0)), 5.4);
    assert_eq!(ev, [StageEvent::StripHidden]);
    // The resting placement of the strip windows is shifted out to the peek width.
    let Placement::Strip(r) = s.placements().iter().find(|(id, _)| *id == w(1)).unwrap().1 else {
        panic!("strip")
    };
    assert!(
        r.right() <= 0.0 + 1e-9,
        "fully off the left edge with no peek: {r:?}"
    );
}

#[test]
fn hit_testing_finds_the_strip_group_under_the_pointer() {
    let s = stage_of(3);
    let slot = s.slot_rect(0);
    let p = Point::new(slot.cx(), slot.cy());
    assert_eq!(s.hit_test_strip(p, 100.0), Some(s.strip_groups()[0]));
    assert_eq!(s.hit_test_strip(Point::new(1000.0, 500.0), 100.0), None);
    let gap = s.slot_rect(0).bottom() + 10.0;
    assert_eq!(
        s.hit_test_strip(Point::new(slot.cx(), gap), 100.0),
        None,
        "the gap between slots"
    );
}

#[test]
fn changing_the_usable_area_relayouts_the_strip_with_an_animation() {
    let mut s = stage_of(3);
    let before = s.slot_rect(0);
    s.set_usable(Rect::new(0.0, 32.0, 1920.0, 700.0), 50.0);
    let after = s.slot_rect(0);
    assert_ne!(before, after);
    assert!(s.is_animating(50.1));
    let slots = s.strip_slots(60.0);
    assert!((slots[0].rect.y - after.y).abs() < 1e-3);
}
