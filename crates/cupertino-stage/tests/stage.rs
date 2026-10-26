use cupertino_stage::*;
use cupertino_wm::geometry::{Point, Rect};
use cupertino_wm::ids::{AppId, WindowId};

const USABLE: Rect = Rect::new(0.0, 24.0, 1920.0, 1056.0);

fn w(id: u64) -> WindowId {
    WindowId(id)
}
fn app(id: u64) -> AppId {
    AppId(id)
}
fn frame(i: u64) -> Rect {
    Rect::new(300.0 + i as f64 * 10.0, 100.0, 800.0, 600.0)
}

/// windows 1..=n, each its own app, most recent first.
fn stage_with(n: u64, cfg: StageConfig) -> Stage {
    let mut s = Stage::new(cfg, USABLE);
    let wins: Vec<(WindowId, AppId, Rect)> = (1..=n).map(|i| (w(i), app(i), frame(i))).collect();
    s.set_enabled(true, &wins, 0.0);
    s
}

fn strip_windows(s: &Stage) -> Vec<Vec<u64>> {
    s.strip_groups().iter().map(|g| s.group_windows(*g).iter().map(|w| w.0).collect()).collect()
}

fn rect_eq(a: Rect, b: Rect) -> bool {
    a.to_array().iter().zip(b.to_array()).all(|(x, y)| (x - y).abs() < 1e-6)
}

fn frame_of(s: &Stage, id: u64, now: f64) -> Rect {
    s.window_frames(now).into_iter().find(|(x, _, _)| x.0 == id).unwrap().1
}

#[test]
fn enabling_builds_center_and_strip_in_mru_order() {
    let s = stage_with(4, StageConfig::default());
    assert!(s.is_enabled());
    assert_eq!(s.center_windows(), vec![w(1)]);
    assert_eq!(strip_windows(&s), vec![vec![2], vec![3], vec![4]]);
}

#[test]
fn grouping_all_windows_merges_apps_one_at_a_time_splits() {
    let wins = vec![(w(1), app(1), frame(1)), (w(2), app(2), frame(2)), (w(3), app(1), frame(3)), (w(4), app(2), frame(4))];
    let mut s = Stage::new(StageConfig::default(), USABLE);
    s.set_enabled(true, &wins, 0.0);
    assert_eq!(s.center_windows(), vec![w(1), w(3)]);
    assert_eq!(strip_windows(&s), vec![vec![2, 4]]);

    let cfg = StageConfig { grouping: Grouping::OneAtATime, ..Default::default() };
    let mut s = Stage::new(cfg, USABLE);
    s.set_enabled(true, &wins, 0.0);
    assert_eq!(s.center_windows(), vec![w(1)]);
    assert_eq!(s.strip_groups().len(), 3);
}

#[test]
fn disabling_clears_everything_and_shows_desktop() {
    let mut s = stage_with(3, StageConfig::default());
    assert!(!s.desktop_items_visible());
    let ev = s.set_enabled(false, &[], 1.0);
    assert!(ev.contains(&StageEvent::Disabled) && ev.contains(&StageEvent::DesktopItemsVisible(true)));
    assert!(s.center_group().is_none() && s.strip_groups().is_empty());
    assert!(s.desktop_items_visible());
    assert_eq!(s.stage_area(), USABLE);
    assert!(s.strip_slots(1.0).is_empty());
    assert!(s.window_opened(w(9), app(9), frame(9), 1.0).is_empty());
}

#[test]
fn rows_follow_height_within_configured_range() {
    let cfg = StageConfig::default();
    let s = Stage::new(cfg.clone(), USABLE);
    assert_eq!(s.rows(), cfg.strip_max_rows);
    for (h, expect) in [(744.0, 5usize), (600.0, 4)] {
        let s = Stage::new(cfg.clone(), Rect::new(0.0, 24.0, 1280.0, h));
        assert_eq!(s.rows(), expect, "usable height {h}");
        assert!((4..=6).contains(&s.rows()));
    }
    let tiny = Stage::new(cfg.clone(), Rect::new(0.0, 0.0, 800.0, 300.0));
    assert_eq!(tiny.rows(), cfg.strip_min_rows);
    let (rows, slot_h) = tiny.strip_metrics();
    let total = rows as f64 * slot_h + (rows as f64 - 1.0) * cfg.strip_row_gap;
    assert!(total <= 300.0 - 2.0 * cfg.strip_vertical_padding + 1e-9, "rows shrink to fit");
    let custom = Stage::new(StageConfig { strip_max_rows: 2, strip_min_rows: 1, ..cfg }, USABLE);
    assert_eq!(custom.rows(), 2);
}

#[test]
fn slots_are_laid_out_in_the_strip_column_without_overlap() {
    let s = stage_with(8, StageConfig::default());
    assert_eq!(s.strip_groups().len(), 7);
    assert_eq!(s.visible_strip().len(), 6, "only max rows are visible");
    let slots = s.strip_slots(10.0);
    assert_eq!(slots.len(), 6);
    let cfg = &s.cfg;
    for (i, a) in slots.iter().enumerate() {
        assert_eq!(a.rect.x, USABLE.x + cfg.strip_edge_inset);
        assert_eq!(a.rect.w, cfg.strip_width);
        assert!(a.rect.y >= USABLE.y && a.rect.bottom() <= USABLE.bottom());
        for b in &slots[i + 1..] {
            assert!(!a.rect.intersects(&b.rect));
        }
    }
    assert!(slots.windows(2).all(|p| (p[1].rect.y - p[0].rect.bottom() - cfg.strip_row_gap).abs() < 1e-9));
    let top = slots[0].rect.y - USABLE.y;
    let bottom = USABLE.bottom() - slots.last().unwrap().rect.bottom();
    assert!((top - bottom).abs() < 1e-9, "centred");
    assert_eq!(slots[0].group, s.strip_groups()[0], "most recent first");
}

#[test]
fn few_groups_are_centred_and_top_alignment_is_configurable() {
    let s = stage_with(3, StageConfig::default());
    let slots = s.strip_slots(10.0);
    assert_eq!(slots.len(), 2);
    let top = slots[0].rect.y - USABLE.y;
    let bottom = USABLE.bottom() - slots[1].rect.bottom();
    assert!((top - bottom).abs() < 1e-9);
    let s = stage_with(3, StageConfig { strip_vertical_align: 0.0, ..Default::default() });
    assert_eq!(s.strip_slots(10.0)[0].rect.y, USABLE.y + s.cfg.strip_vertical_padding);
}

#[test]
fn stage_area_is_right_of_the_strip() {
    let s = stage_with(2, StageConfig::default());
    let a = s.stage_area();
    let c = &s.cfg;
    assert_eq!(a.x, USABLE.x + c.strip_edge_inset + c.strip_width + c.stage_inset_from_strip);
    assert_eq!(a.y, USABLE.y);
    assert!(a.right() <= USABLE.right() && a.bottom() == USABLE.bottom());
    assert!(a.x >= s.strip_area().right());
}

#[test]
fn clicking_a_strip_group_swaps_it_with_the_stage() {
    let mut s = stage_with(4, StageConfig::default());
    let g3 = s.strip_groups()[1];
    let old = s.center_group().unwrap();
    let ev = s.activate_group(g3, 1.0);
    assert!(ev.contains(&StageEvent::Swap { incoming: g3, outgoing: Some(old) }));
    assert_eq!(s.center_group(), Some(g3));
    assert_eq!(s.strip_groups()[0], old, "previous centre goes to the top of the strip");
    assert_eq!(strip_windows(&s), vec![vec![1], vec![2], vec![4]]);
    assert!(s.activate_group(g3, 1.1).is_empty(), "already on the stage");
    assert!(s.activate_group(GroupId(999), 1.1).is_empty());
}

#[test]
fn swap_animates_with_springs_and_ends_on_layout() {
    let mut s = stage_with(4, StageConfig::default());
    let before3 = frame_of(&s, 3, 1.0);
    let g3 = s.group_of(w(3)).unwrap();
    s.activate_group(g3, 1.0);
    assert!(s.is_animating(1.0));
    assert_eq!(s.group_blend(g3, 1.0), 0.0);
    let mid = s.group_blend(g3, 1.15);
    assert!(mid > 0.0 && mid < 1.0);
    assert!(rect_eq(frame_of(&s, 3, 1.0), before3), "the incoming window starts as its thumbnail");
    let t = 20.0;
    assert!(!s.is_animating(t));
    assert!(rect_eq(frame_of(&s, 3, t), frame(3)), "ends at its stage frame");
    let out = s.group_of(w(1)).unwrap();
    assert_eq!(s.group_blend(out, t), 0.0);
    let slot = s.strip_slots(t).into_iter().find(|x| x.group == out).unwrap();
    assert!(rect_eq(slot.rect, s.slot_rect(0)));
    let f1 = frame_of(&s, 1, t);
    assert!(f1.w < frame(1).w && slot.rect.contains(f1.center()), "outgoing window shrank into its slot");
}

#[test]
fn outgoing_group_starts_from_the_stage_and_shrinks_into_the_strip() {
    let mut s = stage_with(3, StageConfig::default());
    let g2 = s.group_of(w(2)).unwrap();
    s.activate_group(g2, 1.0);
    assert!(rect_eq(frame_of(&s, 1, 1.0), frame(1)), "no jump at the start of the swap");
    let mid = frame_of(&s, 1, 1.15);
    assert!(mid.w < frame(1).w && mid.w > frame_of(&s, 1, 20.0).w);
}

#[test]
fn interrupting_a_swap_keeps_frames_continuous() {
    let mut s = stage_with(4, StageConfig::default());
    let (g2, g3) = (s.group_of(w(2)).unwrap(), s.group_of(w(3)).unwrap());
    s.activate_group(g2, 1.0);
    let t = 1.12;
    let before: Vec<Rect> = (1..=4).map(|i| frame_of(&s, i, t)).collect();
    s.activate_group(g3, t);
    for (i, b) in before.iter().enumerate() {
        let after = frame_of(&s, i as u64 + 1, t);
        assert!(rect_eq(after, *b), "window {} jumped: {b:?} -> {after:?}", i + 1);
    }
    assert_eq!(s.center_group(), Some(g3));
    assert!(rect_eq(frame_of(&s, 3, 30.0), frame(3)));
}

#[test]
fn other_strip_items_glide_to_their_new_slots() {
    let mut s = stage_with(4, StageConfig::default());
    let g4 = s.group_of(w(4)).unwrap();
    let g3 = s.group_of(w(3)).unwrap();
    let rect_before = s.strip_slots(1.0).into_iter().find(|x| x.group == g4).unwrap().rect;
    s.activate_group(g3, 1.0);
    let at_start = s.strip_slots(1.0).into_iter().find(|x| x.group == g4).unwrap().rect;
    assert!(rect_eq(at_start, rect_before));
    let end = s.strip_slots(30.0).into_iter().find(|x| x.group == g4).unwrap().rect;
    assert!(rect_eq(end, s.slot_rect(2)));
}

#[test]
fn new_window_of_another_app_takes_the_stage() {
    let mut s = stage_with(2, StageConfig::default());
    let old = s.center_group().unwrap();
    let ev = s.window_opened(w(10), app(10), frame(10), 1.0);
    assert!(matches!(ev[0], StageEvent::GroupCreated { .. }));
    assert!(matches!(ev[1], StageEvent::Swap { outgoing: Some(o), .. } if o == old));
    assert_eq!(s.center_windows(), vec![w(10)]);
    assert_eq!(s.strip_groups()[0], old);
    assert_eq!(s.group_blend(s.center_group().unwrap(), 0.0), 0.0);
    assert!(s.group_blend(s.center_group().unwrap(), 20.0) > 0.999);
}

#[test]
fn new_window_of_the_stage_app_joins_it_or_activates_its_strip_group() {
    let mut s = stage_with(3, StageConfig::default());
    let c = s.center_group().unwrap();
    let ev = s.window_opened(w(11), app(1), frame(11), 1.0);
    assert!(ev.is_empty(), "joins the centre group silently");
    assert_eq!(s.group_of(w(11)), Some(c));
    assert_eq!(s.center_windows(), vec![w(11), w(1)]);

    let g2 = s.group_of(w(2)).unwrap();
    let ev = s.window_opened(w(12), app(2), frame(12), 2.0);
    assert!(ev.iter().any(|e| matches!(e, StageEvent::Swap { incoming, .. } if *incoming == g2)));
    assert_eq!(s.group_windows(g2), vec![w(12), w(2)]);
    assert_eq!(s.center_group(), Some(g2));
}

#[test]
fn one_at_a_time_opens_a_new_group_even_for_the_same_app() {
    let cfg = StageConfig { grouping: Grouping::OneAtATime, ..Default::default() };
    let mut s = stage_with(2, cfg);
    s.window_opened(w(20), app(1), frame(20), 1.0);
    assert_eq!(s.center_windows(), vec![w(20)]);
    assert_eq!(strip_windows(&s)[0], vec![1]);
}

#[test]
fn window_activation_through_focus_changes() {
    let mut s = stage_with(3, StageConfig::default());
    assert!(s.window_activated(w(1), 1.0).is_empty());
    let ev = s.window_activated(w(3), 1.0);
    assert!(matches!(ev[0], StageEvent::Swap { .. }));
    assert_eq!(s.center_windows(), vec![w(3)]);
    assert!(s.window_activated(w(99), 1.0).is_empty());
    let wins = vec![(w(1), app(1), frame(1)), (w(2), app(1), frame(2))];
    let mut s = Stage::new(StageConfig::default(), USABLE);
    s.set_enabled(true, &wins, 0.0);
    s.window_activated(w(2), 1.0);
    assert_eq!(s.center_windows(), vec![w(2), w(1)], "activated window moves to the front of its group");
}

#[test]
fn opening_a_known_window_activates_it() {
    let mut s = stage_with(3, StageConfig::default());
    let ev = s.window_opened(w(3), app(3), frame(3), 1.0);
    assert!(matches!(ev[0], StageEvent::Swap { .. }));
    assert_eq!(s.center_windows(), vec![w(3)]);
}

#[test]
fn closing_the_last_center_window_empties_the_stage() {
    let mut s = stage_with(3, StageConfig::default());
    let c = s.center_group().unwrap();
    let ev = s.window_closed(w(1), 1.0);
    assert!(ev.contains(&StageEvent::GroupRemoved { group: c }));
    assert!(ev.contains(&StageEvent::CenterEmptied { group: None }));
    assert_eq!(s.center_group(), None);
    assert_eq!(strip_windows(&s), vec![vec![2], vec![3]]);
    assert!(s.window_closed(w(1), 1.0).is_empty());
}

#[test]
fn promote_on_empty_brings_the_next_group_to_the_stage() {
    let cfg = StageConfig { promote_on_empty: true, ..Default::default() };
    let mut s = stage_with(3, cfg);
    let ev = s.window_closed(w(1), 1.0);
    assert!(ev.iter().any(|e| matches!(e, StageEvent::Swap { outgoing: None, .. })));
    assert_eq!(s.center_windows(), vec![w(2)]);
    assert_eq!(strip_windows(&s), vec![vec![3]]);
}

#[test]
fn closing_a_strip_window_removes_group_and_slots_close_up() {
    let mut s = stage_with(4, StageConfig::default());
    let g3 = s.group_of(w(3)).unwrap();
    let g4 = s.group_of(w(4)).unwrap();
    let before = s.strip_slots(1.0).into_iter().find(|x| x.group == g4).unwrap().rect;
    let ev = s.window_closed(w(3), 1.0);
    assert_eq!(ev, vec![StageEvent::GroupRemoved { group: g3 }]);
    assert_eq!(strip_windows(&s), vec![vec![2], vec![4]]);
    let start = s.strip_slots(1.0).into_iter().find(|x| x.group == g4).unwrap().rect;
    assert!(rect_eq(start, before));
    assert!(rect_eq(s.strip_slots(30.0).into_iter().find(|x| x.group == g4).unwrap().rect, s.slot_rect(1)));
}

#[test]
fn closing_one_of_several_group_windows_keeps_the_group() {
    let wins = vec![(w(1), app(1), frame(1)), (w(2), app(1), frame(2))];
    let mut s = Stage::new(StageConfig::default(), USABLE);
    s.set_enabled(true, &wins, 0.0);
    assert!(s.window_closed(w(1), 1.0).is_empty());
    assert_eq!(s.center_windows(), vec![w(2)]);
}

#[test]
fn overflow_group_becomes_visible_when_a_row_frees_up() {
    let mut s = stage_with(8, StageConfig::default());
    let hidden = *s.strip_groups().last().unwrap();
    assert!(!s.visible_strip().contains(&hidden));
    assert!(s.strip_slots(1.0).iter().all(|x| x.group != hidden));
    s.window_closed(w(2), 1.0);
    assert!(s.visible_strip().contains(&hidden));
    let slot = s.strip_slots(1.0).into_iter().find(|x| x.group == hidden).unwrap();
    assert!(slot.rect.y >= USABLE.y && slot.rect.bottom() <= USABLE.bottom());
}

#[test]
fn empty_stage_and_click_desktop() {
    let mut s = stage_with(3, StageConfig::default());
    assert!(!s.desktop_items_visible());
    let c = s.center_group().unwrap();
    let ev = s.click_desktop(1.0);
    assert!(ev.contains(&StageEvent::CenterEmptied { group: Some(c) }));
    assert!(ev.contains(&StageEvent::DesktopItemsVisible(true)));
    assert_eq!(s.center_group(), None);
    assert_eq!(s.strip_groups()[0], c);
    assert!(s.desktop_items_visible());
    assert!(s.click_desktop(1.1).is_empty(), "nothing left to do");

    let ev = s.activate_group(c, 2.0);
    assert!(ev.contains(&StageEvent::DesktopItemsVisible(false)));
    assert!(!s.desktop_items_visible());
    assert_eq!(s.center_group(), Some(c));
}

#[test]
fn click_desktop_can_be_disabled_and_desktop_hiding_is_optional() {
    let cfg = StageConfig { click_desktop: ClickDesktop::Nothing, ..Default::default() };
    let mut s = stage_with(2, cfg);
    assert!(s.click_desktop(1.0).is_empty());
    assert!(s.center_group().is_some());
    let cfg = StageConfig { hide_desktop_items: false, ..Default::default() };
    assert!(stage_with(2, cfg).desktop_items_visible());
}

#[test]
fn dragging_a_strip_group_onto_the_stage_adds_it() {
    let mut s = stage_with(4, StageConfig::default());
    let area = s.stage_area();
    assert_eq!(s.drop_target(area.center()), Some(DropTarget::Stage));
    assert_eq!(s.drop_target(s.slot_rect(0).center()), Some(DropTarget::Strip));
    assert_eq!(s.drop_target(Point::new(area.x - 5.0, area.y + 2.0)), None);
    let g2 = s.group_of(w(2)).unwrap();
    let c = s.center_group().unwrap();
    assert!(s.drop_group(g2, s.slot_rect(2).center(), 1.0).is_empty(), "dropping on the strip does nothing");
    let ev = s.drop_group(g2, area.center(), 1.0);
    assert!(ev.contains(&StageEvent::WindowMoved { window: w(2), from: g2, to: c }));
    assert!(ev.contains(&StageEvent::GroupRemoved { group: g2 }));
    assert_eq!(s.center_group(), Some(c), "the centre group keeps its identity");
    assert_eq!(s.center_windows(), vec![w(2), w(1)]);
    assert_eq!(strip_windows(&s), vec![vec![3], vec![4]]);
    assert_eq!(s.group_of(w(2)), Some(c));
}

#[test]
fn adding_to_an_empty_stage_makes_the_group_the_centre() {
    let mut s = stage_with(3, StageConfig::default());
    s.empty_stage(1.0);
    let g = s.strip_groups()[1];
    s.add_to_stage(g, 2.0);
    assert_eq!(s.center_group(), Some(g));
    assert!(s.add_to_stage(g, 3.0).is_empty());
}

#[test]
fn dragging_a_stage_window_onto_the_strip_splits_it_out() {
    let wins = vec![(w(1), app(1), frame(1)), (w(2), app(1), frame(2)), (w(3), app(3), frame(3))];
    let mut s = Stage::new(StageConfig::default(), USABLE);
    s.set_enabled(true, &wins, 0.0);
    let strip_pt = s.slot_rect(0).center();
    assert!(s.drop_window(w(1), s.stage_area().center(), 1.0).is_empty());
    let ev = s.drop_window(w(1), strip_pt, 1.0);
    assert!(matches!(ev[0], StageEvent::GroupCreated { .. }));
    assert!(matches!(ev[1], StageEvent::WindowMoved { window, .. } if window == w(1)));
    assert_eq!(s.center_windows(), vec![w(2)]);
    assert_eq!(strip_windows(&s)[0], vec![1]);
    let first = frame_of(&s, 1, 1.0);
    assert!(rect_eq(first, frame_of(&s, 1, 1.0)));

    let ev = s.drop_window(w(2), strip_pt, 2.0);
    assert!(ev.contains(&StageEvent::CenterEmptied { group: Some(s.strip_groups()[0]) }));
    assert_eq!(s.center_group(), None);
    assert!(!s.desktop_items_visible(), "dragging the last window away does not reveal the desktop");
}

#[test]
fn strip_hides_when_a_window_overlaps_and_peeks() {
    let mut s = stage_with(3, StageConfig { strip_peek_width: 12.0, ..Default::default() });
    assert!(!s.center_overlaps_strip());
    assert!(s.update(None, 0.0).is_empty());
    s.set_window_frame(w(1), Rect::new(50.0, 100.0, 800.0, 600.0));
    assert!(s.center_overlaps_strip());
    assert!(s.update(None, 1.0).is_empty(), "dwell before hiding");
    assert!(s.update(None, 1.1).is_empty());
    let ev = s.update(None, 1.0 + s.cfg.hide_delay + 0.01);
    assert_eq!(ev, vec![StageEvent::StripHidden]);
    assert!(s.is_strip_hidden_target());
    assert_eq!(s.strip_hidden_fraction(1.4), s.strip_hidden_fraction(1.4));
    let t = 20.0;
    assert!((s.strip_hidden_fraction(t) - 1.0).abs() < 1e-3);
    let slot = &s.strip_slots(t)[0];
    assert!((slot.rect.right() - (USABLE.x + s.cfg.strip_peek_width)).abs() < 1e-2, "peek width stays visible: {:?}", slot.rect);
    assert!(s.hit_test_strip(Point::new(USABLE.x + 2.0, slot.rect.cy()), t).is_some(), "peek is clickable");
    assert!(s.hit_test_strip(Point::new(USABLE.x + s.cfg.strip_peek_width + 40.0, slot.rect.cy()), t).is_none());
}

#[test]
fn hover_at_the_edge_reveals_the_hidden_strip_and_leaving_hides_again() {
    let mut s = stage_with(3, StageConfig::default());
    s.set_window_frame(w(1), Rect::new(50.0, 100.0, 800.0, 600.0));
    s.update(None, 0.0);
    s.update(None, 1.0);
    assert!(s.is_strip_hidden_target());
    let edge = Point::new(1.0, 500.0);
    assert!(s.update(Some(edge), 5.0).is_empty());
    let ev = s.update(Some(edge), 5.0 + s.cfg.reveal_delay + 0.01);
    assert_eq!(ev, vec![StageEvent::StripShown]);
    assert!(s.strip_hidden_fraction(30.0) < 1e-3);
    let over_strip = s.slot_rect(0).center();
    s.update(Some(over_strip), 30.0);
    s.update(Some(over_strip), 31.0);
    assert!(!s.is_strip_hidden_target(), "stays while the pointer is over it");
    let away = Point::new(1000.0, 500.0);
    s.update(Some(away), 32.0);
    let ev = s.update(Some(away), 32.0 + s.cfg.hide_delay + 0.01);
    assert_eq!(ev, vec![StageEvent::StripHidden]);
}

#[test]
fn strip_reappears_when_the_overlap_ends_and_auto_hide_works() {
    let mut s = stage_with(3, StageConfig::default());
    s.set_window_frame(w(1), Rect::new(50.0, 100.0, 800.0, 600.0));
    s.update(None, 0.0);
    s.update(None, 1.0);
    assert!(s.is_strip_hidden_target());
    s.set_window_frame(w(1), frame(1));
    assert_eq!(s.update(None, 2.0), vec![StageEvent::StripShown]);

    let cfg = StageConfig { auto_hide: true, ..Default::default() };
    let mut s = stage_with(3, cfg);
    s.update(None, 0.0);
    assert_eq!(s.update(None, 1.0), vec![StageEvent::StripHidden]);
}

#[test]
fn hiding_is_interruptible() {
    let mut s = stage_with(3, StageConfig::default());
    s.set_window_frame(w(1), Rect::new(50.0, 100.0, 800.0, 600.0));
    s.update(None, 0.0);
    s.update(None, 1.0);
    let mid = s.strip_hidden_fraction(1.1);
    assert!(mid > 0.0 && mid < 1.0);
    s.update(Some(Point::new(1.0, 300.0)), 1.1);
    s.update(Some(Point::new(1.0, 300.0)), 1.1 + s.cfg.reveal_delay + 0.01);
    assert!((s.strip_hidden_fraction(1.1 + s.cfg.reveal_delay + 0.01)) <= mid + 1e-6 || s.is_animating(1.3));
    assert!(s.strip_hidden_fraction(30.0) < 1e-3);
}

#[test]
fn icons_overlay_the_slot_bottom_left() {
    let wins = [(w(1), app(1), frame(1)), (w(2), app(2), frame(2))];
    let cfg = StageConfig { grouping: Grouping::OneAtATime, ..Default::default() };
    let mut s = Stage::new(cfg, USABLE);
    s.set_enabled(true, &wins, 0.0);
    let slot = &s.strip_slots(5.0)[0];
    assert_eq!(slot.icons.len(), 1);
    let c = &s.cfg;
    assert_eq!(slot.icons[0], Rect::new(slot.rect.x + c.icon_left_offset, slot.rect.bottom() - c.icon_bottom_offset - c.icon_size, c.icon_size, c.icon_size));

    let mut s = stage_with(1, StageConfig::default());
    s.window_opened(w(10), app(10), frame(10), 0.0);
    s.add_to_stage(s.group_of(w(1)).unwrap(), 0.0);
    s.window_opened(w(11), app(11), frame(11), 0.0);
    let g = s.strip_groups()[0];
    assert_eq!(s.group_apps(g), vec![app(1), app(10)]);
    let slot = s.strip_slots(10.0).into_iter().find(|x| x.group == g).unwrap();
    assert_eq!(slot.icons.len(), 2);
    assert!((slot.icons[1].x - slot.icons[0].x - s.cfg.icon_two_spacing).abs() < 1e-9);

    s.activate_group(g, 1.0);
    s.window_opened(w(12), app(12), frame(12), 1.0);
    s.add_to_stage(s.group_of(w(11)).unwrap(), 2.0);
    s.add_to_stage(s.strip_groups().iter().copied().find(|x| s.group_apps(*x).contains(&app(10))).unwrap(), 2.0);
    let center = s.center_group().unwrap();
    s.window_opened(w(13), app(13), frame(13), 3.0);
    let slot = s.strip_slots(30.0).into_iter().find(|x| x.group == center).unwrap();
    assert_eq!(slot.apps.len(), 4);
    assert_eq!(slot.icons.len(), 3, "at most three icons are drawn");
    assert!((slot.icons[2].x - slot.icons[1].x - s.cfg.icon_three_spacing).abs() < 1e-9);
}

#[test]
fn thumbnails_fit_the_slot_and_keep_relative_layout() {
    let wins = vec![(w(1), app(1), frame(1)), (w(2), app(2), Rect::new(100.0, 100.0, 400.0, 300.0)), (w(3), app(2), Rect::new(600.0, 200.0, 400.0, 300.0))];
    let mut s = Stage::new(StageConfig::default(), USABLE);
    s.set_enabled(true, &wins, 0.0);
    let slot = s.strip_slots(5.0)[0].rect;
    let inner = slot.inset(s.cfg.thumbnail_padding);
    let f2 = frame_of(&s, 2, 5.0);
    let f3 = frame_of(&s, 3, 5.0);
    for f in [f2, f3] {
        assert!(f.x >= inner.x - 1e-9 && f.right() <= inner.right() + 1e-9 && f.y >= inner.y - 1e-9 && f.bottom() <= inner.bottom() + 1e-9, "{f:?} in {inner:?}");
    }
    let scale = f2.w / 400.0;
    assert!((f2.h / 300.0 - scale).abs() < 1e-9 && (f3.w / 400.0 - scale).abs() < 1e-9, "aspect and common scale");
    assert!(((f3.x - f2.x) / 500.0 - scale).abs() < 1e-9 && ((f3.y - f2.y) / 100.0 - scale).abs() < 1e-9);
    assert!(scale < 1.0);
    assert!(rect_eq(frame_of(&s, 1, 5.0), frame(1)), "centre window at its stage frame");
}

#[test]
fn hit_testing_strip_slots() {
    let s = stage_with(4, StageConfig::default());
    for (i, g) in s.strip_groups().iter().enumerate() {
        assert_eq!(s.hit_test_strip(s.slot_rect(i).center(), 10.0), Some(*g));
    }
    assert_eq!(s.hit_test_strip(s.stage_area().center(), 10.0), None);
}

#[test]
fn fit_group_scales_down_and_centres() {
    let wins = vec![(w(1), app(1), Rect::new(0.0, 0.0, 3000.0, 1500.0)), (w(2), app(1), Rect::new(3100.0, 0.0, 400.0, 300.0))];
    let mut s = Stage::new(StageConfig::default(), USABLE);
    s.set_enabled(true, &wins, 0.0);
    let g = s.center_group().unwrap();
    let fitted = s.fit_group(g);
    let area = s.stage_area();
    let union = fitted.iter().map(|(_, r)| *r).reduce(|a, b| a.union(&b)).unwrap();
    assert!(union.x >= area.x - 1e-9 && union.right() <= area.right() + 1e-9 && union.bottom() <= area.bottom() + 1e-9);
    assert!(((union.cx() - area.cx()).abs() < 1e-6) && ((union.cy() - area.cy()).abs() < 1e-6));
    let s1 = fitted.iter().find(|(w, _)| w.0 == 1).unwrap().1.w / 3000.0;
    let s2 = fitted.iter().find(|(w, _)| w.0 == 2).unwrap().1.w / 400.0;
    assert!((s1 - s2).abs() < 1e-9 && s1 < 1.0);

    let wins = vec![(w(1), app(1), Rect::new(0.0, 0.0, 400.0, 300.0))];
    let mut s = Stage::new(StageConfig::default(), USABLE);
    s.set_enabled(true, &wins, 0.0);
    let g = s.center_group().unwrap();
    let f = s.fit_group(g)[0].1;
    assert_eq!((f.w, f.h), (400.0, 300.0), "never enlarged");
    assert!((f.cx() - s.stage_area().cx()).abs() < 1e-9);
}

#[test]
fn usable_area_change_relayouts_slots() {
    let mut s = stage_with(4, StageConfig::default());
    s.set_usable(Rect::new(0.0, 24.0, 1280.0, 600.0), 1.0);
    assert_eq!(s.rows(), 4);
    let t = 30.0;
    for (i, slot) in s.strip_slots(t).iter().enumerate() {
        assert!(rect_eq(slot.rect, s.slot_rect(i)));
        assert!(slot.rect.bottom() <= 624.0);
    }
}

#[test]
fn animation_is_idle_after_settling() {
    let mut s = stage_with(3, StageConfig::default());
    assert!(!s.wants_update(0.0));
    s.activate_group(s.strip_groups()[0], 1.0);
    assert!(s.wants_update(1.0));
    assert!(!s.wants_update(30.0));
}

#[test]
fn verified_layout_defaults() {
    let c = StageConfig::default();
    assert_eq!((c.strip_width, c.strip_row_height, c.strip_row_gap), (170.0, 170.0, 32.0));
    assert_eq!((c.strip_edge_inset, c.stage_inset_from_strip), (16.0, 16.0));
    assert_eq!((c.strip_min_rows, c.strip_max_rows), (4, 6));
    assert_eq!((c.strip_max_item_height, c.strip_preferred_min_item_height, c.strip_absolute_min_item_height), (160.0, 96.0, 20.0));
    assert_eq!((c.icon_size, c.icon_left_offset, c.icon_bottom_offset, c.icon_two_spacing, c.icon_three_spacing), (40.0, -10.0, 10.0, -4.0, -22.0));
    assert_eq!((c.strip_peek_width, c.strip_corner_radius, c.click_session_timeout), (0.0, 8.0, 1.25));
    assert!(!c.auto_hide && c.hide_desktop_items);
    assert_eq!(c.reveal_delay, 0.15);
}

#[test]
fn stage_frame_inset_matches_the_verified_computed_default() {
    let s = stage_with(2, StageConfig::default());
    assert!((s.stage_area().x - USABLE.x - 202.0).abs() < 1e-9);
}

#[test]
fn slot_height_is_capped_by_the_maximum_item_height() {
    let s = Stage::new(StageConfig { strip_max_rows: 4, strip_min_rows: 4, ..Default::default() }, USABLE);
    let (rows, h) = s.strip_metrics();
    assert_eq!(rows, 4);
    assert_eq!(h, 160.0);
}

fn placement_of(s: &Stage, id: u64) -> Placement {
    s.placements().into_iter().find(|(x, _)| x.0 == id).unwrap().1
}

#[test]
fn placements_are_the_resting_frames_of_the_swap_from_its_start() {
    let mut s = stage_with(3, StageConfig::default());
    assert_eq!(placement_of(&s, 1), Placement::Stage);
    let g3 = s.group_of(w(3)).unwrap();
    s.activate_group(g3, 1.0);
    // asked at the start of the swap, the placements are already where the animation ends
    let settled = 30.0;
    assert_eq!(placement_of(&s, 3), Placement::Stage);
    for id in [1, 2] {
        let Placement::Strip(rect) = placement_of(&s, id) else { panic!("window {id} is not in the strip") };
        assert!(rect_eq(rect, frame_of(&s, id, settled)), "window {id}: {rect:?} vs {:?}", frame_of(&s, id, settled));
    }
    let Placement::Strip(top) = placement_of(&s, 1) else { unreachable!() };
    assert!(s.slot_rect(0).contains(top.center()), "the previous centre takes the top slot");
}

#[test]
fn groups_beyond_the_rows_are_hidden_and_disabled_has_no_placements() {
    let cfg = StageConfig::default();
    let rows = cfg.strip_max_rows as u64;
    // one centre group and one more strip group than there are rows
    let mut s = stage_with(rows + 2, cfg);
    let placements = s.placements();
    assert_eq!(placements.len() as u64, rows + 2);
    assert_eq!(placement_of(&s, rows + 2), Placement::Hidden, "the oldest group has no slot");
    assert!(matches!(placement_of(&s, rows + 1), Placement::Strip(_)));
    s.set_enabled(false, &[], 1.0);
    assert!(s.placements().is_empty());
}

#[test]
fn a_group_of_two_windows_keeps_its_layout_in_one_slot() {
    let wins = vec![(w(1), app(1), frame(1)), (w(2), app(2), Rect::new(100.0, 100.0, 400.0, 300.0)), (w(3), app(2), Rect::new(600.0, 100.0, 400.0, 300.0))];
    let mut s = Stage::new(StageConfig::default(), USABLE);
    s.set_enabled(true, &wins, 0.0);
    let (Placement::Strip(a), Placement::Strip(b)) = (placement_of(&s, 2), placement_of(&s, 3)) else { panic!("both windows of app 2 are in the strip") };
    assert!((a.w - b.w).abs() < 1e-9 && a.right() < b.x, "same scale, side by side: {a:?} {b:?}");
    let slot = s.slot_rect(0);
    assert!(slot.contains(a.center()) && slot.contains(b.center()));
}
