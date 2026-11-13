use cupertino_overview::*;
use cupertino_wm::geometry::Point;
use cupertino_wm::ids::{AppId, WindowId, WorkspaceId};

const AREA: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);
const A: AppId = AppId(1);
const B: AppId = AppId(2);

fn win(id: u64, app: AppId, r: Rect) -> OverviewWindow {
    OverviewWindow::new(WindowId(id), app, &format!("w{id}"), r)
}

fn grid4() -> Vec<OverviewWindow> {
    vec![
        win(1, A, Rect::new(0.0, 0.0, 900.0, 500.0)),
        win(2, A, Rect::new(1000.0, 0.0, 900.0, 500.0)),
        win(3, B, Rect::new(0.0, 560.0, 900.0, 500.0)),
        win(4, B, Rect::new(1000.0, 560.0, 900.0, 500.0)),
    ]
}

fn open(o: &mut Overview) -> f64 {
    o.toggle(true, 0.0);
    10.0
}

fn rect_eq(a: Rect, b: Rect) -> bool {
    a.to_array()
        .iter()
        .zip(b.to_array())
        .all(|(x, y)| (x - y).abs() < 1e-6)
}

fn mini(id: u64, app: AppId, size: Rect) -> OverviewWindow {
    OverviewWindow::minimized(
        WindowId(id),
        app,
        &format!("m{id}"),
        size,
        Rect::new(900.0 + id as f64 * 10.0, 1030.0, 48.0, 48.0),
    )
}

#[test]
fn flick_opens_even_when_short() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    for i in 0..6 {
        o.gesture_update(i as f64 * 15.0, 0.0, i as f64 * 0.008);
    }
    assert!(o.gesture_end(0.048));
    assert!((o.transition.progress.value(5.0) - 1.0).abs() < 1e-3);
    assert!(o.is_open());
}

#[test]
fn slow_short_drag_cancels() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    for i in 0..30 {
        o.gesture_update(i as f64 * 2.0, 0.0, i as f64 * 0.016);
    }
    assert!(!o.gesture_end(0.48));
    assert!(!o.is_open());
}

#[test]
fn gesture_rubber_bands_beyond_the_ends() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    o.gesture_update(600.0, 0.0, 0.0);
    assert!((o.progress_at(0.0) - (1.0 + 1.0 * 0.15)).abs() < 1e-9);
    o.gesture_update(-300.0, 0.0, 0.1);
    assert!((o.progress_at(0.1) + 0.15).abs() < 1e-9);
}

#[test]
fn reverse_gesture_from_open() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    o.toggle(true, 0.0);
    let t = 5.0;
    for i in 0..6 {
        o.gesture_update(-(i as f64) * 20.0, 1.0, t + i as f64 * 0.008);
    }
    assert!(!o.gesture_end(t + 0.048), "fast downward flick closes");
}

#[test]
fn frames_interpolate_between_desktop_and_layout() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    let from: Vec<Rect> = o.shown_windows().iter().map(|w| w.from).collect();
    assert!(o
        .frames(0.0)
        .iter()
        .zip(&from)
        .all(|(a, b)| rect_eq(*a, *b)));
    let t = open(&mut o);
    assert!(o
        .frames(t)
        .iter()
        .zip(o.layout_rects())
        .all(|(a, b)| rect_eq(*a, *b)));
    let mid = o.frames(0.1);
    assert!(mid[0].w < from[0].w && mid[0].w > o.layout_rects()[0].w - 1e-9);
}

#[test]
fn overview_layout_avoids_the_bar_and_has_no_overlap() {
    let o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    let area = o.layout_area();
    let bar = &o.cfg.bar;
    assert_eq!(area.y, bar.windows_top(false) - o.cfg.layout.inset);
    assert_eq!(bar.windows_top(false), bar.band_height(false) + 39.0);
    for (i, r) in o.layout_rects().iter().enumerate() {
        assert!(
            r.y >= area.y - 1e-9
                && r.bottom() <= area.bottom() + 1e-9
                && r.x >= -1e-9
                && r.right() <= 1920.0 + 1e-9
        );
        for q in &o.layout_rects()[i + 1..] {
            assert!(!r.intersects(q));
        }
    }
}

fn three_windows_under_the_menu_bar(expand_on_hover: bool) -> Overview {
    let mut cfg = OverviewConfig::default();
    cfg.bar.expand_on_hover = expand_on_hover;
    let windows = vec![
        win(1, A, Rect::new(0.0, 24.0, 1000.0, 696.0)),
        win(2, A, Rect::new(280.0, 120.0, 800.0, 500.0)),
        win(3, B, Rect::new(460.0, 300.0, 360.0, 240.0)),
    ];
    let mut o = Overview::new(
        cfg,
        Rect::new(0.0, 24.0, 1280.0, 696.0),
        OverviewMode::Overview,
        windows,
    );
    o.set_bar_open(!expand_on_hover, -10.0);
    o.set_workspaces(&[WorkspaceId(1), WorkspaceId(2)], Some(WorkspaceId(1)));
    o
}

#[test]
fn windows_clear_a_pinned_expanded_bar_by_the_layout_gap() {
    let o = three_windows_under_the_menu_bar(false);
    let bar = o.bar.layout(0.0).rect;
    assert_eq!(bar.bottom(), 24.0 + 165.0);
    let slots = bar.bottom() + 19.0;
    let rects = o.layout_rects();
    assert_eq!(rects.len(), 3);
    for (i, r) in rects.iter().enumerate() {
        assert!(
            r.y >= slots + 20.0 - 1e-9,
            "window {i} top {} under the bar",
            r.y
        );
        assert!(!r.intersects(&bar));
        for q in &rects[i + 1..] {
            assert!(!r.intersects(q));
        }
    }
}

#[test]
fn a_bar_that_expands_on_hover_keeps_the_windows_below_its_expanded_slots() {
    let mut o = three_windows_under_the_menu_bar(true);
    assert_eq!(o.bar.layout(0.0).rect.bottom(), 24.0 + 72.0, "closed band");
    for r in o.layout_rects() {
        assert!(
            r.y >= 24.0 + 72.0 + 39.0 - 1e-9,
            "{r:?} under the closed band"
        );
    }
    let closed = o.layout_rects().to_vec();
    o.pointer_moved(Some(Point::new(640.0, 40.0)), 0.0);
    assert!(
        o.bar.is_expanded_target(),
        "the pointer at the top opens the band"
    );
    let expanded = o.bar.layout(10.0).rect.bottom();
    assert!((expanded - (24.0 + 165.0)).abs() < 0.2, "{expanded}");
    for r in o.layout_rects() {
        assert!(r.y >= expanded + 39.0 - 0.2, "{r:?} under the open band");
    }
    assert_ne!(closed, o.layout_rects(), "the windows moved down");
    o.pointer_moved(Some(Point::new(640.0, 600.0)), 10.0);
    assert_eq!(closed, o.layout_rects(), "and back up once the band closes");
}

#[test]
fn overview_hides_minimized_windows() {
    let mut w = grid4();
    w.push(mini(9, A, Rect::new(0.0, 0.0, 800.0, 600.0)));
    let o = Overview::new(OverviewConfig::default(), AREA, OverviewMode::Overview, w);
    assert_eq!(o.shown_windows().len(), 4);
    assert!(o.shown_windows().iter().all(|w| !w.minimized));
}

#[test]
fn app_overview_shows_only_the_app_with_minimized_row() {
    let mut w = grid4();
    w.push(mini(9, A, Rect::new(0.0, 0.0, 800.0, 600.0)));
    w.push(mini(10, A, Rect::new(0.0, 0.0, 600.0, 600.0)));
    w.push(mini(11, B, Rect::new(0.0, 0.0, 600.0, 600.0)));
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::AppOverview(A),
        w,
    );
    let shown: Vec<WindowId> = o.shown_windows().iter().map(|w| w.id).collect();
    assert_eq!(
        shown,
        vec![WindowId(1), WindowId(2), WindowId(9), WindowId(10)]
    );
    open(&mut o);
    let rects = o.layout_rects().to_vec();
    let cfg = &o.cfg.app_overview;
    let row_top = AREA.bottom() - AREA.h * cfg.row_height_fraction;
    assert!(rects[0].bottom() <= row_top - cfg.row_margin + 1e-6 || rects[0].bottom() <= row_top);
    assert!(rects[1].bottom() <= row_top);
    for r in &rects[2..] {
        assert!(
            r.y >= row_top - 1e-9 && r.bottom() <= AREA.bottom() + 1e-9,
            "minimized row at the bottom: {r:?}"
        );
        assert!(r.h <= AREA.h * cfg.row_height_fraction + 1e-9);
    }
    assert!(rects[2].right() <= rects[3].x, "row keeps order");
    assert!(
        (rects[2].w / 800.0 - rects[3].w / 600.0).abs() < 1e-9,
        "common scale"
    );
    assert!(rects[2].w / 800.0 <= cfg.row_max_scale + 1e-12);
    for (i, r) in rects.iter().enumerate() {
        for q in &rects[i + 1..] {
            assert!(!r.intersects(q));
        }
    }
    assert_eq!(o.layout_area(), AREA, "no workspaces bar in app overview");
}

#[test]
fn app_overview_minimized_windows_fly_in_from_their_dock_tiles() {
    let m = mini(9, A, Rect::new(0.0, 0.0, 800.0, 600.0));
    let dock = m.from;
    let mut w = grid4();
    w.push(m);
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::AppOverview(A),
        w,
    );
    assert!(rect_eq(o.frames(0.0)[2], dock));
    let t = open(&mut o);
    assert!(rect_eq(o.frames(t)[2], o.layout_rects()[2]));
}

#[test]
fn app_overview_without_minimized_windows_uses_the_full_area() {
    let o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::AppOverview(A),
        grid4(),
    );
    assert_eq!(o.shown_windows().len(), 2);
    let r = o.layout_rects();
    assert!(r.iter().all(|r| r.bottom() <= AREA.bottom() && r.y >= 0.0));
    let both_h: f64 = r.iter().map(|r| r.h).fold(0.0, f64::max);
    assert!(both_h > 400.0);
}

#[test]
fn app_overview_with_only_minimized_windows() {
    let w = vec![mini(1, A, Rect::new(0.0, 0.0, 800.0, 600.0))];
    let o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::AppOverview(A),
        w,
    );
    assert_eq!(o.layout_rects().len(), 1);
    assert!(o.layout_rects()[0].y > AREA.h * 0.5);
}

#[test]
fn switching_mode_reflows_without_a_jump() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    let t = open(&mut o);
    let before = o.frames(t);
    o.set_mode(OverviewMode::AppOverview(A), t);
    assert_eq!(o.shown_windows().len(), 2);
    let after = o.frames(t);
    assert!(
        rect_eq(after[0], before[0]) && rect_eq(after[1], before[1]),
        "windows start from where they were"
    );
    assert!(o.is_animating(t));
    let end = o.frames(t + 5.0);
    assert!(rect_eq(end[0], o.layout_rects()[0]));
    assert!(!rect_eq(end[0], before[0]));
    o.set_mode(OverviewMode::Overview, t + 6.0);
    assert_eq!(o.shown_windows().len(), 4);
}

#[test]
fn closing_a_window_reflows_the_rest() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    let t = open(&mut o);
    let before = o.frames(t);
    let mut w = grid4();
    w.remove(0);
    o.set_windows(w, t);
    assert_eq!(o.shown_windows().len(), 3);
    let after = o.frames(t);
    assert!(
        rect_eq(after[0], before[1])
            && rect_eq(after[1], before[2])
            && rect_eq(after[2], before[3])
    );
    let settled = o.frames(t + 5.0);
    assert!(settled
        .iter()
        .zip(o.layout_rects())
        .all(|(a, b)| rect_eq(*a, *b)));
    // the windows that overlapped the closed one spread into its room, gliding there
    let moved: Vec<usize> = (0..3)
        .filter(|i| !rect_eq(settled[*i], before[i + 1]))
        .collect();
    assert!(
        !moved.is_empty(),
        "the packing changed without window 1: {settled:?}"
    );
    let mid = o.frames(t + 0.1);
    assert!(
        moved
            .iter()
            .all(|i| !rect_eq(mid[*i], before[i + 1]) && !rect_eq(mid[*i], settled[*i])),
        "windows glide to their new places"
    );
}

#[test]
fn hover_selects_and_highlights_with_fade_and_glide() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    let t = open(&mut o);
    let r0 = o.layout_rects()[0];
    let r1 = o.layout_rects()[1];
    assert_eq!(o.highlight(t), None);
    o.pointer_moved(Some(r0.center()), t);
    assert_eq!(o.selection(), Some(WindowId(1)));
    let h = o.highlight(t + 0.05).unwrap();
    assert!(h.opacity > 0.0 && h.opacity < 1.0);
    let h = o.highlight(t + 1.0).unwrap();
    assert!((h.opacity - 1.0).abs() < 1e-9);
    assert!(rect_eq(h.rect, r0.inset(-o.cfg.highlight.inset)));

    o.pointer_moved(Some(r1.center()), t + 1.0);
    assert_eq!(o.selection(), Some(WindowId(2)));
    let g = o.highlight(t + 1.05).unwrap();
    assert!(
        g.rect.x > h.rect.x && g.rect.x < r1.x,
        "outline glides to the new window"
    );
    assert!(rect_eq(
        o.highlight(t + 6.0).unwrap().rect,
        r1.inset(-o.cfg.highlight.inset)
    ));

    o.pointer_moved(Some(Point::new(1.0, 1070.0)), t + 6.0);
    assert_eq!(o.selection(), None);
    assert!(o.highlight(t + 7.0).is_none());
    assert_eq!(o.hit_test(r0.center(), t), Some(WindowId(1)));
    assert_eq!(o.hit_test(Point::new(1.0, 1070.0), t), None);
}

#[test]
fn click_actions() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    o.set_workspaces(&[WorkspaceId(1), WorkspaceId(2)], Some(WorkspaceId(1)));
    let t = open(&mut o);
    let r = o.layout_rects()[2];
    assert_eq!(o.click(r.center(), t), OverviewAction::Select(WindowId(3)));
    assert_eq!(o.click(Point::new(5.0, 1075.0), t), OverviewAction::Close);
    let slot = o.bar.layout(t).slots[1].name.center();
    assert_eq!(
        o.click(slot, t),
        OverviewAction::SwitchWorkspace(WorkspaceId(2))
    );
    o.pointer_moved(Some(slot), t);
    let add = o.bar.layout(t + 3.0).add.center();
    assert_eq!(o.click(add, t + 3.0), OverviewAction::AddWorkspace);
}

#[test]
fn hovering_the_bar_expands_it() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    o.set_workspaces(&[WorkspaceId(1), WorkspaceId(2)], Some(WorkspaceId(1)));
    let t = open(&mut o);
    let c = o.bar.layout(t).rect.center();
    o.pointer_moved(Some(c), t);
    assert!(o.bar.is_expanded_target());
    assert!(o.is_animating(t + 0.01));
    o.pointer_moved(Some(Point::new(960.0, 700.0)), t + 3.0);
    assert!(!o.bar.is_expanded_target());

    let mut app = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::AppOverview(A),
        grid4(),
    );
    app.set_workspaces(&[WorkspaceId(1)], Some(WorkspaceId(1)));
    let c = app.bar.layout(0.0).rect.center();
    app.pointer_moved(Some(c), 0.0);
    assert!(!app.bar.is_expanded_target(), "no bar in app overview");
    assert_eq!(app.click(c, 0.0), OverviewAction::Close);
}

#[test]
fn keyboard_directional_navigation_on_a_grid() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    let t = open(&mut o);
    assert_eq!(o.key(Key::Right, t), OverviewAction::None);
    assert_eq!(
        o.selection(),
        Some(WindowId(1)),
        "first key selects the first window"
    );
    o.key(Key::Right, t);
    assert_eq!(o.selection(), Some(WindowId(2)));
    o.key(Key::Right, t);
    assert_eq!(
        o.selection(),
        Some(WindowId(2)),
        "no wrap on directional keys"
    );
    o.key(Key::Down, t);
    assert_eq!(o.selection(), Some(WindowId(4)));
    o.key(Key::Left, t);
    assert_eq!(o.selection(), Some(WindowId(3)));
    o.key(Key::Up, t);
    assert_eq!(o.selection(), Some(WindowId(1)));
    o.key(Key::Up, t);
    assert_eq!(o.selection(), Some(WindowId(1)));
    assert_eq!(o.key(Key::Activate, t), OverviewAction::Select(WindowId(1)));
    assert_eq!(o.key(Key::Cancel, t), OverviewAction::Close);
    assert!(o.highlight(t + 1.0).is_some());
}

#[test]
fn keyboard_next_previous_cycle_in_reading_order() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    let t = open(&mut o);
    let mut seen = vec![];
    for _ in 0..5 {
        o.key(Key::Next, t);
        seen.push(o.selection().unwrap().0);
    }
    assert_eq!(seen, vec![1, 2, 3, 4, 1]);
    o.key(Key::Previous, t);
    assert_eq!(o.selection(), Some(WindowId(4)));

    let mut p = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    p.key(Key::Previous, 0.0);
    assert_eq!(p.selection(), Some(WindowId(4)));
}

#[test]
fn keyboard_selection_wins_over_stationary_pointer_until_it_moves() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    let t = open(&mut o);
    let r0 = o.layout_rects()[0].center();
    o.pointer_moved(Some(r0), t);
    assert_eq!(o.selection(), Some(WindowId(1)));
    o.key(Key::Right, t);
    assert_eq!(o.selection(), Some(WindowId(2)));
    o.pointer_moved(Some(r0), t + 0.1);
    assert_eq!(o.selection(), Some(WindowId(2)), "pointer did not move");
    o.pointer_moved(Some(Point::new(r0.x + 5.0, r0.y)), t + 0.2);
    assert_eq!(o.selection(), Some(WindowId(1)));
}

#[test]
fn keyboard_navigation_in_app_overview_reaches_minimized_row() {
    let mut w = grid4();
    w.push(mini(9, A, Rect::new(0.0, 0.0, 800.0, 600.0)));
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::AppOverview(A),
        w,
    );
    o.key(Key::Next, 0.0);
    assert_eq!(o.selection(), Some(WindowId(1)));
    o.key(Key::Down, 0.0);
    assert_eq!(o.selection(), Some(WindowId(9)));
    o.key(Key::Up, 0.0);
    assert!(matches!(
        o.selection(),
        Some(WindowId(1)) | Some(WindowId(2))
    ));
    // the minimized row ends the reading order: Next reaches it from either open window
    let presses = (1..=3).find(|_| {
        o.key(Key::Next, 0.0);
        o.selection() == Some(WindowId(9))
    });
    assert!(matches!(presses, Some(1) | Some(2)), "{presses:?}");
}

#[test]
fn keyboard_on_empty_overview_is_inert() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        vec![],
    );
    assert_eq!(o.key(Key::Next, 0.0), OverviewAction::None);
    assert_eq!(o.key(Key::Activate, 0.0), OverviewAction::None);
    assert_eq!(o.key(Key::Cancel, 0.0), OverviewAction::Close);
    assert!(o.frames(0.0).is_empty());
}

#[test]
fn removed_window_drops_selection() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    o.key(Key::Next, 0.0);
    assert_eq!(o.selection(), Some(WindowId(1)));
    let mut w = grid4();
    w.remove(0);
    o.set_windows(w, 0.0);
    assert_eq!(o.selection(), None);
    assert!(o.highlight(5.0).is_none());
}

#[test]
fn title_fade_schedule() {
    let f = TitleFade::default();
    assert_eq!(f.opacity(0.0), 0.0);
    assert_eq!(f.opacity(0.8), 0.0);
    assert!((f.opacity(0.9) - 0.5).abs() < 1e-12);
    assert_eq!(f.opacity(1.0), 1.0);
    assert_eq!(f.opacity(1.1), 1.0);
    let mut prev = 0.0;
    for k in 0..=100 {
        let v = f.opacity(k as f64 / 100.0);
        assert!(v >= prev);
        prev = v;
    }
    let custom = TitleFade {
        start: 0.5,
        end: 0.5,
    };
    assert_eq!(custom.opacity(0.4), 0.0);
    assert_eq!(custom.opacity(0.5), 1.0);
}

#[test]
fn titles_follow_transition_progress() {
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    assert_eq!(o.title_opacity(0.0), 0.0);
    o.toggle(true, 0.0);
    assert_eq!(o.title_opacity(0.05), 0.0, "still early in the transition");
    assert_eq!(o.title_opacity(5.0), 1.0);
    assert!(o.placed(5.0).iter().all(|p| p.title_opacity == 1.0));
    o.toggle(false, 5.0);
    assert!(
        o.title_opacity(5.0 + 0.2) < 1.0,
        "titles fade out again while closing"
    );
}

#[test]
fn placed_windows_report_selection_and_minimized() {
    let mut w = grid4();
    w.push(mini(9, A, Rect::new(0.0, 0.0, 800.0, 600.0)));
    let mut o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::AppOverview(A),
        w,
    );
    o.key(Key::Next, 0.0);
    let p = o.placed(0.0);
    assert_eq!(p.len(), 3);
    assert!(p[0].selected && !p[1].selected);
    assert!(p[2].minimized && !p[0].minimized);
}

#[test]
fn transition_uses_the_verified_duration_and_slow_motion_is_longer() {
    let settle = |slow: bool| {
        let mut o = Overview::new(
            OverviewConfig::default(),
            AREA,
            OverviewMode::Overview,
            grid4(),
        );
        o.set_slow_motion(slow);
        assert_eq!(o.slow_motion(), slow);
        o.toggle(true, 0.0);
        let mut t = 0.0;
        while o.transition.is_animating(t) && t < 30.0 {
            t += 0.01;
        }
        t
    };
    let (fast, slow) = (settle(false), settle(true));
    assert!(fast < 1.0, "normal transition settles quickly: {fast}");
    assert!(
        slow > 4.0 * fast,
        "slow motion is much longer: {slow} vs {fast}"
    );
}

#[test]
fn the_closed_bar_band_is_72pt() {
    let o = Overview::new(
        OverviewConfig::default(),
        AREA,
        OverviewMode::Overview,
        grid4(),
    );
    assert_eq!(o.cfg.bar.band_height(false), 72.0);
    assert_eq!(o.bar.layout(0.0).rect.h, 72.0);
}

/// Output modes in device pixels and the scales they run at; the overview works in points.
const MODES: [(f64, f64); 6] = [
    (1280.0, 800.0),
    (1366.0, 768.0),
    (1920.0, 1080.0),
    (2560.0, 1440.0),
    (2880.0, 1800.0),
    (3840.0, 2160.0),
];
const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];

/// An edge as the output draws it: on the device pixel grid.
fn snap(v: f64, scale: f64) -> f64 {
    (v * scale).round() / scale
}

/// Three windows of different sizes (a large one, a medium one, a small one) at places of a
/// desktop of `w`×`h` points, in a few arrangements.
fn three_windows(w: f64, h: f64, arrangement: usize) -> Vec<OverviewWindow> {
    let large = (w * 0.7, h * 0.75);
    let medium = (w * 0.45, h * 0.5);
    let small = (w * 0.18, h * 0.16);
    let places = [
        [(0.0, 24.0), (w * 0.3, h * 0.2), (w * 0.4, h * 0.35)],
        [(w * 0.3, 24.0), (0.0, h * 0.45), (w * 0.8, h * 0.05)],
        [(w * 0.1, h * 0.1), (w * 0.1, h * 0.1), (w * 0.1, h * 0.1)],
        [(w * 0.25, h * 0.2), (w * 0.5, h * 0.5), (w * 0.78, h * 0.8)],
    ][arrangement];
    [large, medium, small]
        .into_iter()
        .zip(places)
        .enumerate()
        .map(|(i, ((sw, sh), (x, y)))| {
            win(
                i as u64 + 1,
                A,
                Rect::new(x, y, sw.min(w - x), sh.min(h - y)),
            )
        })
        .collect()
}

#[test]
fn three_windows_stay_below_the_strip_and_apart_at_every_size_and_scale() {
    for (pw, ph) in MODES {
        for scale in SCALES {
            let (w, h) = (pw / scale, ph / scale);
            // the menu bar above and a dock zone below
            let area = Rect::new(0.0, 24.0, w, h - 24.0 - 70.0);
            for expand_on_hover in [false, true] {
                for arrangement in 0..4 {
                    let mut cfg = OverviewConfig::default();
                    cfg.bar.expand_on_hover = expand_on_hover;
                    let mut o = Overview::new(
                        cfg,
                        area,
                        OverviewMode::Overview,
                        three_windows(w, h, arrangement),
                    );
                    let ids: Vec<WorkspaceId> = (1..=3).map(WorkspaceId).collect();
                    o.set_workspaces(&ids, Some(WorkspaceId(1)));
                    o.set_bar_open(true, 0.0);
                    let strip = o.bar.layout(10.0);
                    let strip_bottom = area.y + o.cfg.bar.band_height(true);
                    let case = format!(
                        "{pw}×{ph} px at {scale} ({w}×{h} pt), arrangement {arrangement}, \
                         hover {expand_on_hover}"
                    );
                    let rects: Vec<Rect> = o
                        .layout_rects()
                        .iter()
                        .map(|r| {
                            let (x, y) = (snap(r.x, scale), snap(r.y, scale));
                            Rect::new(
                                x,
                                y,
                                snap(r.right(), scale) - x,
                                snap(r.bottom(), scale) - y,
                            )
                        })
                        .collect();
                    assert_eq!(rects.len(), 3, "{case}");
                    let gap = snap(strip_bottom, scale) + o.cfg.bar.layout_gap;
                    for (i, r) in rects.iter().enumerate() {
                        assert!(r.w > 0.0 && r.h > 0.0, "{case}: window {i} is {r:?}");
                        assert!(
                            r.y >= gap - 1.0 / scale,
                            "{case}: window {i} {r:?} within 39 pt of the band"
                        );
                        for s in strip
                            .slots
                            .iter()
                            .map(|s| s.thumb)
                            .chain(std::iter::once(strip.add))
                        {
                            assert!(
                                !r.intersects(&s),
                                "{case}: window {i} {r:?} over slot {s:?}"
                            );
                        }
                        assert!(
                            r.x >= area.x
                                && r.right() <= area.right()
                                && r.bottom() <= area.bottom(),
                            "{case}: window {i} {r:?} outside {area:?}"
                        );
                        for (j, q) in rects.iter().enumerate().skip(i + 1) {
                            assert!(
                                !r.intersects(q),
                                "{case}: windows {i} {r:?} and {j} {q:?} overlap"
                            );
                        }
                    }
                    // balanced: the arrangement is centred across the area
                    let used = o
                        .layout_rects()
                        .iter()
                        .copied()
                        .reduce(|a, b| a.union(&b))
                        .unwrap();
                    assert!(
                        (used.cx() - area.cx()).abs() < 1e-6,
                        "{case}: {used:?} off centre in {area:?}"
                    );
                    // labels and hit testing follow the frames
                    for (wnd, r) in o.shown_windows().iter().zip(o.layout_rects()) {
                        assert_eq!(o.hit_test(r.center(), 10.0), Some(wnd.id), "{case}");
                    }
                }
            }
        }
    }
}

/// A fixed-seed generator for the adversarial layouts below, so a failure names a case that reruns the same.
struct Cases(u64);

impl Cases {
    /// A number in `[lo, hi)`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        lo + (hi - lo) * ((self.0 >> 11) as f64 / (1u64 << 53) as f64)
    }
}

/// The smallest output the overview is laid out on, in points: 1280×720 pixels at scale 2.
const SMALLEST_OUTPUT: (f64, f64) = (640.0, 360.0);

/// What the dock draws under every window: nothing, a window's name lies on the window.
const TITLE_BAND: f64 = 0.0;

/// Up to sixteen windows, among them tiny ones, very wide and very tall ones and windows on the same place, on
/// outputs from the smallest to 4K at scale 1 and at fractional scales. Every thumbnail keeps a size, sits at least
/// the layout gap below the expanded strip's slots, inside the area with the title space under it, and neither it
/// nor its title space meets another window or its title, also with edges snapped to device pixels; a click on the
/// middle of a thumbnail hits its window.
#[test]
fn adversarial_window_sets_keep_thumbnails_and_titles_apart_and_below_the_strip() {
    let mut rng = Cases(0x0067);
    for case in 0..3000 {
        let scale: f64 = [1.0, 1.25, 1.5, 1.75, 2.0][case % 5];
        let w = rng.range(SMALLEST_OUTPUT.0, 3840.0 / scale);
        let h = rng.range(SMALLEST_OUTPUT.1, 2160.0 / scale);
        let area = Rect::new(0.0, 24.0, w, h - 24.0 - 70.0);
        let n = 1 + (rng.range(0.0, 16.0) as usize);
        let same_place = rng.range(0.0, 1.0) < 0.1;
        let windows: Vec<OverviewWindow> = (0..n)
            .map(|i| {
                let kind = rng.range(0.0, 1.0);
                let (ww, wh) = if kind < 0.25 {
                    // tiny
                    (rng.range(1.0, 60.0), rng.range(1.0, 60.0))
                } else if kind < 0.35 {
                    // a wide, thin strip
                    (rng.range(w * 0.5, w), rng.range(1.0, 30.0))
                } else if kind < 0.45 {
                    // a tall, narrow column
                    (rng.range(1.0, 30.0), rng.range(h * 0.5, h))
                } else {
                    (rng.range(w * 0.1, w), rng.range(h * 0.1, h))
                };
                let (x, y) = if same_place {
                    (w * 0.2, h * 0.2)
                } else {
                    (
                        rng.range(-ww * 0.2, w - ww * 0.8),
                        rng.range(24.0, h - wh * 0.8),
                    )
                };
                win(i as u64 + 1, A, Rect::new(x, y, ww, wh))
            })
            .collect();
        let mut cfg = OverviewConfig::default();
        cfg.bar.expand_on_hover = case % 2 == 0;
        let mut o = Overview::new(cfg, area, OverviewMode::Overview, windows);
        let ids: Vec<WorkspaceId> = (1..=1 + case as u64 % 6).map(WorkspaceId).collect();
        o.set_workspaces(&ids, Some(WorkspaceId(1)));
        o.set_bar_open(true, 0.0);
        let strip = o.bar.layout(10.0);
        let slots: Vec<Rect> = strip
            .slots
            .iter()
            .map(|s| s.thumb)
            .chain(std::iter::once(strip.add))
            .collect();
        let strip_bottom = area.y + o.cfg.bar.band_height(true);
        let id = format!("case {case}: {w}×{h} pt at {scale}, {n} windows");
        let snapped = |r: &Rect| {
            let (x, y) = (snap(r.x, scale), snap(r.y, scale));
            Rect::new(
                x,
                y,
                snap(r.right(), scale) - x,
                snap(r.bottom(), scale) - y,
            )
        };
        let rects = o.layout_rects().to_vec();
        assert_eq!(rects.len(), n, "{id}");
        for (i, r) in rects.iter().enumerate() {
            let titled = Rect::new(r.x, r.y, r.w, r.h + TITLE_BAND);
            assert!(r.w > 0.0 && r.h > 0.0, "{id}: window {i} is {r:?}");
            assert!(
                r.y >= strip_bottom + o.cfg.bar.layout_gap - 1e-6,
                "{id}: window {i} {r:?} within the gap of the strip at {strip_bottom}"
            );
            assert!(
                snapped(r).y >= snap(strip_bottom, scale) + o.cfg.bar.layout_gap - 1.0 / scale,
                "{id}: window {i} {r:?} drawn within the gap of the strip"
            );
            assert!(
                r.x >= area.x - 1e-6
                    && r.right() <= area.right() + 1e-6
                    && titled.bottom() <= area.bottom() + 1e-6,
                "{id}: window {i} {r:?} or its title outside {area:?}"
            );
            for s in &slots {
                assert!(!r.intersects(s), "{id}: window {i} {r:?} over slot {s:?}");
            }
            for (j, q) in rects.iter().enumerate() {
                if i == j {
                    continue;
                }
                let other = Rect::new(q.x, q.y, q.w, q.h + TITLE_BAND);
                assert!(
                    !titled.intersects(&other),
                    "{id}: window {i} {r:?} or its title meets window {j} {q:?} or its title"
                );
                assert!(
                    !snapped(&titled).intersects(&snapped(&other)),
                    "{id}: windows {i} and {j} meet as drawn"
                );
            }
        }
        for (wnd, r) in o.shown_windows().iter().zip(o.layout_rects()) {
            assert_eq!(o.hit_test(r.center(), 10.0), Some(wnd.id), "{id}");
        }
    }
}
