//! The window overview: packing, modes, selection and keyboard navigation, the transition and the desktops bar.

use cupertino_wm::overview::{
    layout, layout_row, pack, reading_order, BarConfig, BarHit, Key, LayoutParams, Overview,
    OverviewAction, OverviewConfig, OverviewMode, OverviewWindow, TitleFade, WorkspaceBar,
};
use cupertino_wm::{AppId, Point, Rect, WindowId, WorkspaceId};

const AREA: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

fn overlap(a: &Rect, b: &Rect) -> f64 {
    let ox = a.right().min(b.right()) - a.x.max(b.x);
    let oy = a.bottom().min(b.bottom()) - a.y.max(b.y);
    if ox > 0.0 && oy > 0.0 {
        ox.min(oy)
    } else {
        0.0
    }
}

fn inside(r: &Rect, area: &Rect) -> bool {
    r.x >= area.x - 1e-6
        && r.y >= area.y - 1e-6
        && r.right() <= area.right() + 1e-6
        && r.bottom() <= area.bottom() + 1e-6
}

// ---- packing ---------------------------------------------------------------------------------

#[test]
fn a_lone_window_is_never_enlarged_is_centred_a_little_above_the_middle_and_shrinks_to_fit() {
    let p = pack(&[], AREA, &LayoutParams::default());
    assert!(p.rects.is_empty() && p.passes == 0);
    let w = Rect::new(500.0, 400.0, 600.0, 400.0);
    let r = layout(&[w], AREA, &LayoutParams::default())[0];
    assert_eq!((r.w, r.h), (600.0, 400.0));
    assert!((r.cx() - AREA.cx()).abs() < 1e-6);
    assert!(
        (r.cy() - (AREA.cy() - 20.0)).abs() < 1e-6,
        "lifted by 20 points: {r:?}"
    );
    let w = Rect::new(0.0, 0.0, 4000.0, 2000.0);
    let r = layout(&[w], AREA, &LayoutParams::default())[0];
    let work = AREA.inset(16.0);
    assert!(inside(&r, &work), "{r:?}");
    assert!((r.w / r.h - 2.0).abs() < 1e-9);
}

#[test]
fn many_windows_pack_deterministically_without_overlap_inside_the_area_keeping_their_aspect_ratios()
{
    let windows: Vec<Rect> = (0..9)
        .map(|i| {
            let i = i as f64;
            Rect::new(
                100.0 + 30.0 * i,
                80.0 + 20.0 * i,
                600.0 + 40.0 * (i % 3.0),
                400.0 + 25.0 * (i % 2.0),
            )
        })
        .collect();
    let p = LayoutParams::default();
    let out = layout(&windows, AREA, &p);
    assert_eq!(out.len(), windows.len());
    let work = AREA.inset(p.inset);
    for (i, r) in out.iter().enumerate() {
        assert!(inside(r, &work), "{i}: {r:?}");
        assert!(
            (r.w / r.h - windows[i].w / windows[i].h).abs() < 1e-9,
            "aspect of {i}"
        );
        assert!(r.w <= windows[i].w + 1e-9, "never enlarged");
        for s in &out[i + 1..] {
            assert!(overlap(r, s) <= 1e-6, "{r:?} overlaps {s:?}");
        }
    }
    let windows = [
        Rect::new(10.0, 10.0, 800.0, 600.0),
        Rect::new(50.0, 40.0, 700.0, 500.0),
        Rect::new(90.0, 70.0, 640.0, 480.0),
    ];
    let p = LayoutParams::default();
    assert_eq!(pack(&windows, AREA, &p), pack(&windows, AREA, &p));
}

#[test]
fn identical_windows_at_the_same_place_are_pushed_apart_the_later_to_the_right() {
    let w = Rect::new(400.0, 300.0, 800.0, 500.0);
    let out = layout(&[w, w], AREA, &LayoutParams::default());
    assert!(out[1].cx() > out[0].cx());
    assert!(overlap(&out[0], &out[1]) <= 1e-6);
}

#[test]
fn title_space_keeps_room_under_every_window() {
    let p = LayoutParams {
        title_space: 30.0,
        ..LayoutParams::default()
    };
    let windows: Vec<Rect> = (0..6)
        .map(|i| Rect::new(50.0 * i as f64, 40.0 * i as f64, 700.0, 500.0))
        .collect();
    let out = layout(&windows, AREA, &p);
    let work = AREA.inset(p.inset);
    for (i, r) in out.iter().enumerate() {
        assert!(
            r.bottom() + 30.0 <= work.bottom() + 1e-6,
            "title of {i} is inside the area"
        );
        for (j, s) in out.iter().enumerate().filter(|(j, _)| *j > i) {
            let a = Rect::new(r.x, r.y, r.w, r.h + 30.0);
            let b = Rect::new(s.x, s.y, s.w, s.h + 30.0);
            assert!(overlap(&a, &b) <= 1e-6, "{i} and {j} with titles");
        }
    }
}

#[test]
fn a_crowded_packing_that_cannot_settle_falls_back_to_a_grid() {
    let p = LayoutParams {
        max_passes: 0,
        ..LayoutParams::default()
    };
    let windows: Vec<Rect> = (0..4)
        .map(|_| Rect::new(100.0, 100.0, 800.0, 600.0))
        .collect();
    let out = layout(&windows, AREA, &p);
    for (i, r) in out.iter().enumerate() {
        for s in &out[i + 1..] {
            assert!(overlap(r, s) <= 1e-6);
        }
    }
    let scale = out[0].w / 800.0;
    assert!(
        out.iter().all(|r| (r.w / 800.0 - scale).abs() < 1e-9),
        "a common scale"
    );
}

#[test]
fn the_row_layout_is_one_centred_row_in_input_order_within_the_maximum_scale() {
    let wins = [
        Rect::new(0.0, 0.0, 400.0, 300.0),
        Rect::new(0.0, 0.0, 400.0, 300.0),
    ];
    let area = Rect::new(0.0, 800.0, 1920.0, 200.0);
    let out = layout_row(&wins, area, 16.0, 0.5);
    assert!(out[0].right() + 16.0 == out[1].x);
    let left = out[0].x - area.x;
    let right = area.right() - out[1].right();
    assert!((left - right).abs() < 1e-9, "centred");
    assert!(out[0].h <= 200.0 && out[0].w / out[0].h - 4.0 / 3.0 < 1e-9);
    assert!(layout_row(&[], area, 16.0, 0.5).is_empty());
    let wins = [Rect::new(0.0, 0.0, 400.0, 300.0)];
    let r = layout_row(&wins, Rect::new(0.0, 0.0, 1920.0, 1000.0), 16.0, 0.5)[0];
    assert_eq!((r.w, r.h), (200.0, 150.0));
}

#[test]
fn reading_order_goes_row_by_row_left_to_right() {
    let rects = [
        Rect::new(400.0, 400.0, 100.0, 100.0), // 0: lower right
        Rect::new(0.0, 0.0, 100.0, 100.0),     // 1: upper left
        Rect::new(0.0, 405.0, 100.0, 100.0),   // 2: lower left, nearly level with 0
        Rect::new(400.0, 10.0, 100.0, 100.0),  // 3: upper right, nearly level with 1
    ];
    assert_eq!(reading_order(&rects), [1, 3, 2, 0]);
    assert!(reading_order(&[]).is_empty());
}

// ---- configuration -------------------------------------------------------------------------

#[test]
fn the_title_fade_is_a_smoothstep_between_start_and_end_or_a_step_when_they_meet() {
    let f = TitleFade {
        start: 0.8,
        end: 1.0,
    };
    assert_eq!(f.opacity(0.5), 0.0);
    assert_eq!(f.opacity(0.8), 0.0);
    assert!((f.opacity(0.9) - 0.5).abs() < 1e-12);
    assert_eq!(f.opacity(1.0), 1.0);
    assert_eq!(f.opacity(1.5), 1.0);
    assert!(f.opacity(0.85) < f.opacity(0.95));
    let f = TitleFade {
        start: 0.9,
        end: 0.9,
    };
    assert_eq!(f.opacity(0.89), 0.0);
    assert_eq!(f.opacity(0.9), 1.0);
}

// ---- the model ----------------------------------------------------------------------------

fn ow(id: u64, app: u64, frame: Rect) -> OverviewWindow {
    OverviewWindow::new(WindowId(id), AppId(app), &format!("Window {id}"), frame)
}

fn grid(n: u64) -> Vec<OverviewWindow> {
    (1..=n)
        .map(|i| {
            ow(
                i,
                i,
                Rect::new(
                    100.0 + 20.0 * i as f64,
                    100.0 + 20.0 * i as f64,
                    640.0,
                    400.0,
                ),
            )
        })
        .collect()
}

fn overview(n: u64) -> Overview {
    Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid(n),
    )
}

fn centre(o: &Overview, id: u64) -> Point {
    let k = o
        .shown_windows()
        .iter()
        .position(|w| w.id == WindowId(id))
        .unwrap();
    let r = o.layout_rects()[k];
    Point::new(r.cx(), r.cy())
}

#[test]
fn the_overview_opens_from_the_desktop_frames_to_the_layout_with_the_titles_fading_in() {
    let o = overview(4);
    assert!(!o.is_open());
    assert_eq!(o.mode(), OverviewMode::Overview);
    assert_eq!(o.shown_windows().len(), 4);
    assert_eq!(o.layout_rects().len(), 4);
    assert_eq!(o.progress_at(0.0), 0.0);
    assert_eq!(o.selection(), None);
    assert!(o.highlight(0.0).is_none());
    let mut o = overview(3);
    let from: Vec<Rect> = o.shown_windows().iter().map(|w| w.from).collect();
    assert_eq!(o.frames(0.0), from);
    o.toggle(true, 1.0);
    assert!(o.is_open());
    assert!(o.is_animating(1.05));
    let end = o.frames(20.0);
    for (a, b) in end.iter().zip(o.layout_rects()) {
        assert!((a.x - b.x).abs() < 1e-3 && (a.w - b.w).abs() < 1e-3);
    }
    assert!(!o.is_animating(20.0));
    let mut o = overview(2);
    assert_eq!(o.title_opacity(0.0), 0.0);
    o.toggle(true, 1.0);
    assert!((o.title_opacity(20.0) - 1.0).abs() < 1e-6);
    let placed = o.placed(20.0);
    assert!(placed.iter().all(|p| (p.title_opacity - 1.0).abs() < 1e-6));
}

#[test]
fn the_layout_stays_below_the_desktops_band() {
    let mut o = overview(3);
    let bar = BarConfig::default();
    let closed = o.layout_area();
    assert!(closed.y >= bar.closed_height - 1e-9);
    assert_eq!(closed.bottom(), AREA.bottom());
    o.set_bar_open(true, 1.0);
    let open = o.layout_area();
    assert!(
        open.y > closed.y,
        "the windows move down when the band opens"
    );
    assert!(open.y >= bar.open_height);
    o.set_bar_open(false, 2.0);
    assert_eq!(o.layout_area(), closed);
    let o = overview(5);
    let top = BarConfig::default().windows_top(false);
    for r in o.layout_rects() {
        assert!(
            r.y >= o.layout_area().y - 1e-6 && o.layout_area().y >= top - o.cfg.layout.inset - 1e-6,
            "{r:?}"
        );
    }
}

#[test]
fn minimized_windows_are_only_in_the_app_overview() {
    let windows = vec![
        ow(1, 1, Rect::new(100.0, 100.0, 600.0, 400.0)),
        OverviewWindow::minimized(
            WindowId(2),
            AppId(1),
            "Minimized",
            Rect::new(100.0, 100.0, 600.0, 400.0),
            Rect::new(900.0, 1040.0, 48.0, 48.0),
        ),
        ow(3, 2, Rect::new(300.0, 300.0, 600.0, 400.0)),
    ];
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        windows,
    );
    assert_eq!(
        o.shown_windows().iter().map(|w| w.id).collect::<Vec<_>>(),
        [WindowId(1), WindowId(3)]
    );
    o.set_mode(OverviewMode::AppOverview(AppId(1)), 1.0);
    assert_eq!(
        o.shown_windows().iter().map(|w| w.id).collect::<Vec<_>>(),
        [WindowId(1), WindowId(2)]
    );
    let rects = o.layout_rects();
    assert!(
        rects[1].y > rects[0].bottom(),
        "the minimized window sits in a lower row"
    );
    assert!(rects[1].w <= 300.0 + 1e-9, "the row scale is limited");
    o.set_mode(OverviewMode::Overview, 2.0);
    assert_eq!(o.shown_windows().len(), 2);
}

#[test]
fn setting_the_same_mode_does_not_relayout() {
    let mut o = overview(3);
    let rev = o.layout_revision();
    o.set_mode(OverviewMode::Overview, 1.0);
    assert_eq!(o.layout_revision(), rev);
    o.set_mode(OverviewMode::AppOverview(AppId(1)), 1.0);
    assert!(o.layout_revision() > rev);
}

#[test]
fn replacing_the_windows_reflows_with_a_glide_from_where_they_were() {
    let mut o = overview(2);
    o.toggle(true, 0.0);
    let before = o.frames(4.0);
    o.set_windows(grid(4), 5.0);
    assert_eq!(o.shown_windows().len(), 4);
    let early = o.frames(5.0);
    assert_eq!(early.len(), 4);
    assert!(
        (early[0].x - before[0].x).abs() < 1e-6 && (early[0].w - before[0].w).abs() < 1e-6,
        "starts where it was"
    );
    assert!(o.is_animating(5.05));
    let late = o.frames(20.0);
    assert!(
        (late[0].w - o.layout_rects()[0].w).abs() < 1e-3,
        "and ends in the new layout"
    );
}

#[test]
fn a_selection_on_a_window_that_disappears_is_dropped() {
    let mut o = overview(3);
    o.toggle(true, 0.0);
    let c = centre(&o, 2);
    o.pointer_moved(Some(c), 10.0);
    assert_eq!(o.selection(), Some(WindowId(2)));
    o.set_windows(grid(1), 11.0);
    assert_eq!(o.selection(), None);
}

#[test]
fn hovering_selects_the_window_under_the_pointer_and_shows_a_highlight() {
    let mut o = overview(3);
    o.toggle(true, 0.0);
    let c = centre(&o, 3);
    assert_eq!(o.hit_test(c, 10.0), Some(WindowId(3)));
    o.pointer_moved(Some(c), 10.0);
    assert_eq!(o.selection(), Some(WindowId(3)));
    let h = o.highlight(11.0).expect("a highlight");
    assert_eq!(h.window, WindowId(3));
    assert!((h.opacity - 1.0).abs() < 1e-6);
    assert!(h.rect.contains(c));
    assert!(
        o.placed(11.0)
            .iter()
            .find(|p| p.id == WindowId(3))
            .unwrap()
            .selected
    );
    o.pointer_moved(Some(Point::new(1.0, 1070.0)), 12.0);
    assert_eq!(o.selection(), None);
    assert!(o.highlight(20.0).is_none(), "faded out");
    o.pointer_moved(None, 21.0);
    assert_eq!(o.selection(), None);
}

#[test]
fn clicks_select_a_window_close_on_the_background_and_switch_or_add_workspaces_in_the_bar() {
    let mut o = overview(2);
    o.toggle(true, 0.0);
    let c = centre(&o, 1);
    assert_eq!(o.click(c, 10.0), OverviewAction::Select(WindowId(1)));
    assert_eq!(
        o.click(Point::new(1.0, 1075.0), 10.0),
        OverviewAction::Close
    );
    let mut o = overview(2);
    o.set_workspaces(&[WorkspaceId(7), WorkspaceId(8)], Some(WorkspaceId(7)));
    let l = o.bar.layout(0.0);
    let name = l.slots[1].name;
    assert_eq!(
        o.click(Point::new(name.cx(), name.cy()), 1.0),
        OverviewAction::SwitchWorkspace(WorkspaceId(8))
    );
    assert_eq!(
        o.click(Point::new(l.add.cx(), l.add.cy()), 1.0),
        OverviewAction::AddWorkspace
    );
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::AppOverview(AppId(1)),
        grid(3),
    );
    o.set_workspaces(&[WorkspaceId(7)], Some(WorkspaceId(7)));
    let add = o.bar.layout(0.0).add;
    let a = o.click(Point::new(add.cx(), add.cy()), 1.0);
    assert_ne!(a, OverviewAction::AddWorkspace);
    assert_eq!(o.layout_area(), AREA);
}

#[test]
fn cancel_closes_and_activate_needs_a_selection() {
    let mut o = overview(2);
    assert_eq!(o.key(Key::Cancel, 0.0), OverviewAction::Close);
    assert_eq!(o.key(Key::Activate, 0.0), OverviewAction::None);
    o.key(Key::Next, 0.0);
    let sel = o.selection().unwrap();
    assert_eq!(o.key(Key::Activate, 0.1), OverviewAction::Select(sel));
}

#[test]
fn next_and_previous_walk_the_reading_order_and_wrap_from_either_end() {
    let mut o = overview(4);
    o.toggle(true, 0.0);
    o.key(Key::Next, 10.0);
    let first = o.selection().unwrap();
    let rects = o.layout_rects().to_vec();
    let order = reading_order(&rects);
    let ids: Vec<WindowId> = order.iter().map(|k| o.shown_windows()[*k].id).collect();
    assert_eq!(first, ids[0], "the first press selects the first window");
    for want in &ids[1..] {
        o.key(Key::Next, 10.0);
        assert_eq!(o.selection(), Some(*want));
    }
    o.key(Key::Next, 10.0);
    assert_eq!(o.selection(), Some(ids[0]), "wraps to the start");
    o.key(Key::Previous, 10.0);
    assert_eq!(
        o.selection(),
        Some(*ids.last().unwrap()),
        "and back to the end"
    );
    let mut o = overview(3);
    o.key(Key::Previous, 0.0);
    let rects = o.layout_rects().to_vec();
    let last = *reading_order(&rects).last().unwrap();
    assert_eq!(o.selection(), Some(o.shown_windows()[last].id));
}

#[test]
fn arrow_keys_move_to_a_window_in_that_direction_when_there_is_one() {
    let rects = overview(6).layout_rects().to_vec();
    let order = reading_order(&rects);
    for (key, goes) in [
        (
            Key::Right,
            (|dx: f64, _: f64| dx > 0.0) as fn(f64, f64) -> bool,
        ),
        (Key::Left, |dx, _| dx < 0.0),
        (Key::Down, |_, dy| dy > 0.0),
        (Key::Up, |_, dy| dy < 0.0),
    ] {
        // Start from each window in turn and require that any move goes the way of the key.
        for from in 0..rects.len() {
            let mut o = overview(6);
            for _ in 0..=order.iter().position(|k| *k == from).unwrap() {
                o.key(Key::Next, 0.0);
            }
            let start = o.selection().unwrap();
            assert_eq!(o.shown_windows()[from].id, start);
            o.key(key, 0.0);
            let end = o.selection().unwrap();
            if end != start {
                let to = o.shown_windows().iter().position(|w| w.id == end).unwrap();
                let (dx, dy) = (
                    rects[to].cx() - rects[from].cx(),
                    rects[to].cy() - rects[from].cy(),
                );
                assert!(goes(dx, dy), "{key:?} from {from} to {to}: ({dx}, {dy})");
            }
        }
    }
    let rects = overview(6).layout_rects().to_vec();
    let mut moved_right = false;
    let mut moved_down = false;
    for from in 0..rects.len() {
        for (key, flag) in [(Key::Right, &mut moved_right), (Key::Down, &mut moved_down)] {
            let mut o = overview(6);
            let order = reading_order(&rects);
            for _ in 0..=order.iter().position(|k| *k == from).unwrap() {
                o.key(Key::Next, 0.0);
            }
            let start = o.selection();
            o.key(key, 0.0);
            *flag |= o.selection() != start;
        }
    }
    assert!(moved_right && moved_down);
}

#[test]
fn a_direction_with_nothing_beyond_or_no_windows_keeps_the_selection() {
    let mut o = overview(1);
    o.key(Key::Next, 0.0);
    let only = o.selection();
    for k in [Key::Left, Key::Right, Key::Up, Key::Down] {
        o.key(k, 0.1);
        assert_eq!(o.selection(), only);
    }
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        vec![],
    );
    assert_eq!(o.key(Key::Next, 0.0), OverviewAction::None);
    assert_eq!(o.key(Key::Left, 0.0), OverviewAction::None);
    assert_eq!(o.selection(), None);
}

#[test]
fn the_keyboard_selection_survives_a_pointer_that_has_not_moved() {
    let mut o = overview(3);
    o.toggle(true, 0.0);
    let spot = Point::new(1.0, 1070.0);
    o.pointer_moved(Some(spot), 5.0);
    o.key(Key::Next, 6.0);
    let sel = o.selection();
    assert!(sel.is_some());
    o.pointer_moved(Some(spot), 7.0);
    assert_eq!(o.selection(), sel, "no motion: the keyboard keeps control");
    o.pointer_moved(Some(Point::new(1.0, 1000.0)), 8.0);
    assert_eq!(
        o.selection(),
        None,
        "motion hands the selection back to the pointer"
    );
}

#[test]
fn the_pointer_at_the_top_opens_the_band_and_leaving_it_closes() {
    let mut o = overview(3);
    assert!(!o.bar.is_expanded_target());
    o.pointer_moved(Some(Point::new(900.0, 5.0)), 1.0);
    assert!(o.bar.is_expanded_target());
    o.pointer_moved(Some(Point::new(900.0, 600.0)), 2.0);
    assert!(!o.bar.is_expanded_target());
}

// ---- transition ----------------------------------------------------------------------------

#[test]
fn a_gesture_drives_the_progress_one_to_one_and_rubber_bands_past_the_ends() {
    let mut o = overview(2);
    let cfg = OverviewConfig::default();
    o.gesture_update(150.0, 0.0, 1.0);
    assert!((o.progress_at(1.0) - 0.5).abs() < 1e-9);
    o.gesture_update(cfg.swipe_distance * 2.0, 0.0, 1.1);
    let over = o.progress_at(1.1);
    assert!(
        (over - (1.0 + 1.0 * cfg.rubber_band)).abs() < 1e-9,
        "{over}"
    );
    o.gesture_update(-cfg.swipe_distance, 0.0, 1.2);
    assert!((o.progress_at(1.2) + cfg.rubber_band).abs() < 1e-9);
}

#[test]
fn releasing_a_gesture_decides_by_position_or_by_a_fast_flick() {
    let mut o = overview(2);
    o.gesture_update(200.0, 0.0, 1.0);
    o.gesture_update(200.0, 0.0, 2.0);
    assert!(o.gesture_end(2.0), "past half way opens");
    assert!(o.is_open());
    o.gesture_update(-200.0, 1.0, 10.0);
    o.gesture_update(-200.0, 1.0, 11.0);
    assert!(!o.gesture_end(11.0), "back below half way closes");
    assert!(!o.is_open());
    let mut o = overview(2);
    o.gesture_update(10.0, 0.0, 1.00);
    o.gesture_update(40.0, 0.0, 1.02);
    o.gesture_update(80.0, 0.0, 1.04);
    assert!(o.progress_at(1.04) < 0.5);
    assert!(o.gesture_end(1.04), "the velocity carries it over");
}

#[test]
fn a_fast_release_settles_on_a_shorter_spring() {
    let o = overview(2);
    let t = &o.transition;
    let normal = t.release_spring(0.0);
    let fast = t.release_spring(10.0);
    assert_eq!(normal, t.spring());
    assert!(fast.perceptual_duration() < normal.perceptual_duration());
    assert!((fast.perceptual_duration() / normal.perceptual_duration() - 0.6).abs() < 1e-6);
    assert_eq!(
        t.release_spring(-10.0),
        fast,
        "the speed, not the direction"
    );
}

#[test]
fn slow_motion_selects_the_slow_spring_for_transitions_that_start_afterwards() {
    let mut o = overview(2);
    let normal = o.transition_spring();
    o.set_slow_motion(true);
    assert!(o.slow_motion());
    assert!(o.transition_spring().perceptual_duration() > 5.0 * normal.perceptual_duration());
    o.toggle(true, 1.0);
    assert!(
        o.is_animating(2.0),
        "still travelling after a second in slow motion"
    );
    o.set_slow_motion(false);
    assert_eq!(o.transition_spring(), normal);
}

#[test]
fn toggling_back_retargets_from_where_the_progress_is() {
    let mut o = overview(2);
    o.toggle(true, 1.0);
    let mid = o.progress_at(1.1);
    assert!(mid > 0.0 && mid < 1.0);
    o.toggle(false, 1.1);
    assert!(!o.is_open());
    assert!((o.progress_at(1.1) - mid).abs() < 0.05, "no jump");
    assert!(o.progress_at(10.0).abs() < 1e-3);
}

// ---- the desktops bar ----------------------------------------------------------------------

fn bar(n: u64) -> WorkspaceBar {
    let mut b = WorkspaceBar::new(BarConfig::default(), AREA, AREA);
    let ids: Vec<WorkspaceId> = (1..=n).map(WorkspaceId).collect();
    b.set_workspaces(&ids, ids.first().copied());
    b
}

#[test]
fn a_closed_bar_is_the_closed_height_with_names_only() {
    let b = bar(3);
    let l = b.layout(0.0);
    assert_eq!(l.rect, Rect::new(0.0, 0.0, 1920.0, 72.0));
    assert_eq!(l.expansion, 0.0);
    assert_eq!(l.slots.len(), 3);
    for s in &l.slots {
        assert_eq!(
            (s.thumb.w, s.thumb.h),
            (0.0, 0.0),
            "pictures are shrunk to nothing"
        );
        assert_eq!(s.name.cy(), 52.0);
        assert_eq!(s.name.h, 24.0);
    }
    assert!(l.slots[0].current && !l.slots[1].current);
}

#[test]
fn an_open_bar_shows_pictures_at_the_output_aspect_with_names_below() {
    let mut b = bar(3);
    b.set_expanded(true, 0.0);
    let l = b.layout(20.0);
    assert_eq!(l.expansion, 1.0);
    assert_eq!(l.rect.h, 165.0);
    let s = l.slots[0];
    assert!((s.thumb.w / s.thumb.h - 1920.0 / 1080.0).abs() < 1e-9);
    assert_eq!((s.thumb.h, s.thumb.y), (95.0, 20.0));
    assert_eq!(s.name.cy(), 20.0 + 95.0 + 20.0);
}

#[test]
fn the_slots_are_centred_with_the_gap_between_them_and_narrow_to_fit_the_margins() {
    let b = bar(3);
    let l = b.layout(0.0);
    let (a, c) = (l.slots[0].name, l.slots[2].name);
    assert!(
        (a.x - (1920.0 - c.right())).abs() < 1e-9,
        "symmetric about the centre"
    );
    assert!((l.slots[1].name.x - a.right() - 24.0).abs() < 1e-9);
    let b = bar(30);
    let l = b.layout(0.0);
    let first = l.slots.first().unwrap().name;
    let last = l.slots.last().unwrap().name;
    assert!(first.x >= 64.0 - 1e-6 && last.right() <= 1920.0 - 64.0 + 1e-6);
    let single = bar(3).layout(0.0).slots[0].name.w;
    assert!(first.w < single);
}

#[test]
fn the_add_button_sits_at_the_top_right_in_both_states() {
    let mut b = bar(2);
    let closed = b.layout(0.0).add;
    assert_eq!((closed.w, closed.h), (32.0, 32.0));
    assert_eq!(closed.cx(), 1920.0 - 28.0);
    assert_eq!(closed.cy(), 35.0);
    b.set_expanded(true, 0.0);
    assert_eq!(b.layout(20.0).add, closed);
}

#[test]
fn expanding_animates_and_can_be_interrupted() {
    let mut b = bar(2);
    b.set_expanded(true, 1.0);
    assert!(b.is_animating(1.05));
    let mid = b.expansion(1.1);
    assert!(mid > 0.0 && mid < 1.0);
    b.set_expanded(false, 1.1);
    assert!(!b.is_expanded_target());
    assert!(b.expansion(10.0) < 1e-3);
    // Asking again for what it already heads for changes nothing.
    b.set_expanded(false, 11.0);
    assert!(!b.is_animating(11.0));
}

#[test]
fn hit_testing_finds_the_add_button_names_and_open_pictures() {
    let mut b = bar(2);
    let l = b.layout(0.0);
    assert_eq!(
        b.hit_test(Point::new(l.add.cx(), l.add.cy()), 0.0),
        Some(BarHit::Add)
    );
    let n = l.slots[1].name;
    assert_eq!(
        b.hit_test(Point::new(n.cx(), n.cy()), 0.0),
        Some(BarHit::Workspace(WorkspaceId(2)))
    );
    assert_eq!(b.hit_test(Point::new(960.0, 1000.0), 0.0), None);
    b.set_expanded(true, 0.0);
    let t = b.layout(20.0).slots[0].thumb;
    assert_eq!(
        b.hit_test(Point::new(t.cx(), t.cy()), 20.0),
        Some(BarHit::Workspace(WorkspaceId(1)))
    );
}

#[test]
fn the_pointer_over_the_band_expands_it_when_enabled_and_marks_the_hovered_slot() {
    let mut b = bar(2);
    let n = b.layout(0.0).slots[0].name;
    b.pointer_moved(Some(Point::new(n.cx(), n.cy())), 1.0);
    assert!(b.is_expanded_target());
    let at = b.layout(30.0);
    let p = at.slots[0].thumb;
    b.pointer_moved(Some(Point::new(p.cx(), p.cy())), 30.0);
    assert_eq!(b.hovered(), Some(BarHit::Workspace(WorkspaceId(1))));
    assert!(b.layout(30.0).slots[0].hovered);
    b.pointer_moved(Some(Point::new(960.0, 900.0)), 31.0);
    assert!(!b.is_expanded_target());
    assert_eq!(b.hovered(), None);
    b.pointer_moved(None, 32.0);
    assert_eq!(b.hovered(), None);
    let mut b = WorkspaceBar::new(
        BarConfig {
            expand_on_hover: false,
            ..BarConfig::default()
        },
        AREA,
        AREA,
    );
    b.set_workspaces(&[WorkspaceId(1)], Some(WorkspaceId(1)));
    b.pointer_moved(Some(Point::new(900.0, 10.0)), 1.0);
    assert!(!b.is_expanded_target());
    let mut b = bar(2);
    let n = b.layout(0.0).slots[1].name;
    b.pointer_moved(Some(Point::new(n.cx(), n.cy())), 1.0);
    assert_eq!(b.hovered(), Some(BarHit::Workspace(WorkspaceId(2))));
    b.set_workspaces(&[WorkspaceId(1)], Some(WorkspaceId(1)));
    assert_eq!(b.hovered(), None);
}
