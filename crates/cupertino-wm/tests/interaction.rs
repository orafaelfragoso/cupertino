use crate::common;
use common::*;
use cupertino_wm::*;

fn titlebar_grab(h: &mut H, w: WindowId, x: f64, y: f64) {
    h.cmd(Command::BeginMove {
        window: w,
        pointer: Point::new(x, y),
    });
}

#[test]
fn move_follows_pointer_and_keeps_size() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 300.0, 110.0);
    assert!(h.wm.is_interacting());
    let fx = h.cmd(motion(500.0, 260.0));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Place { window, rect } if *window == w && *rect == Rect::new(300.0, 250.0, 600.0, 400.0))
    ));
    assert!(rect_eq(h.frame(w), Rect::new(300.0, 250.0, 600.0, 400.0)));
    h.cmd(Command::PointerRelease);
    assert!(!h.wm.is_interacting());
    assert_eq!(
        h.wm.window(w).unwrap().lifecycle.normal_frame().origin(),
        Point::new(300.0, 250.0)
    );
}

#[test]
fn move_keeps_titlebar_inside_usable_area() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 300.0, 110.0);
    h.cmd(motion(300.0, 5.0));
    assert!(h.frame(w).y >= USABLE.y);
    h.cmd(motion(300.0, 1070.0));
    assert!(h.frame(w).y <= USABLE.bottom() - h.wm.config().snap.titlebar_height + 1e-9);
}

#[test]
fn move_begin_focuses_and_raises() {
    let mut h = H::new();
    let a = h.map("a");
    let _b = h.map("b");
    let fx = h.cmd(Command::BeginMove {
        window: a,
        pointer: Point::new(150.0, 110.0),
    });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Raise { window } if *window == a)
    ));
    assert_eq!(h.wm.key_window(), Some(a));
}

#[test]
fn edge_dwell_shows_preview_and_release_tiles_left() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 300.0, 110.0);
    let fx = h.cmd(motion(10.0, 500.0));
    assert!(
        !has(&fx, |e| matches!(e, Effect::SnapPreview { .. })),
        "no preview before the dwell"
    );
    assert!(h.wm.snap_preview(h.now).is_none());
    h.advance(0.05);
    assert!(h.wm.snap_preview(h.now).is_none());
    let fx = h.advance(0.2);
    let half = tile_rect(TileKind::Left, USABLE, 8.0);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::SnapPreview { rect: Some(r), immediate: false, .. } if *r == half)
    ));
    let p = h.wm.snap_preview(h.now).unwrap();
    assert_eq!(p.kind, TileKind::Left);
    assert!(p.opacity > 0.0 && p.opacity <= 0.5 + 1e-9);

    let fx = h.cmd(Command::PointerRelease);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SnapPreview { rect: None, .. }
    )));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { window, size, states } if *window == w && *size == tile_rect(TileKind::Left, USABLE, 8.0).size() && states.tiled.left && states.tiled.top && states.tiled.bottom && !states.tiled.right)
    ));
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Tile(TileKind::Left),
            ..
        }
    )));
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Tiled(TileKind::Left));
    assert!(rect_eq(h.frame(w), half));
}

/// Enter the left soft zone at (10, 500), move to (10, `y`) 0.15 s later; whether the preview is
/// up 0.25 s and 0.36 s after the entry.
fn preview_after_a_move_to(y: f64) -> (bool, bool) {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 300.0, 110.0);
    h.cmd(motion(10.0, 500.0));
    h.advance(0.15);
    h.cmd(motion(10.0, y));
    h.advance(0.1);
    let early = h.wm.snap_preview(h.now).is_some();
    h.advance(0.11);
    (early, h.wm.snap_preview(h.now).is_some())
}

#[test]
fn the_dwell_tolerates_5_pt_and_starts_again_past_it() {
    assert_eq!(
        preview_after_a_move_to(505.0),
        (true, true),
        "|dy| 5.0 is still: the preview comes 0.2 s after the entry"
    );
    assert_eq!(
        preview_after_a_move_to(505.01),
        (false, true),
        "|dy| 5.01 restarts the 0.2 s dwell at the new point"
    );
}

#[test]
fn the_preview_frame_is_its_rectangle_outset_by_15_pt() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 300.0, 110.0);
    h.cmd(motion(10.0, 500.0));
    h.advance(0.25);
    let p = h.wm.snap_preview(h.now).unwrap();
    assert_eq!(p.frame, p.rect.inset(-15.0));
}

#[test]
fn preview_grows_out_of_the_window_with_a_spring() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 300.0, 110.0);
    h.cmd(motion(10.0, 500.0));
    h.advance(0.25);
    let t0 = h.wm.snap_preview(h.now).unwrap().rect;
    h.advance(0.05);
    let t1 = h.wm.snap_preview(h.now).unwrap().rect;
    assert!(t1.w != t0.w);
    h.advance(3.0);
    let t2 = h.wm.snap_preview(h.now).unwrap().rect;
    assert!(rect_eq(t2, tile_rect(TileKind::Left, USABLE, 8.0)));
}

#[test]
fn option_accelerator_previews_immediately_and_wider_zone() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 700.0, 310.0);
    let fx = h.cmd(motion_opt(1880.0, 500.0));
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SnapPreview {
            rect: Some(_),
            immediate: true,
            ..
        }
    )));
    assert_eq!(h.wm.snap_preview(h.now).unwrap().kind, TileKind::Right);
    // without option the same position is outside the 1 px zone
    let fx = h.cmd(motion(1880.0, 500.0));
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SnapPreview { rect: None, .. }
    )));
    assert!(h.wm.snap_preview(h.now).is_none());
}

#[test]
fn leaving_zone_cancels_preview_and_release_does_not_tile() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 300.0, 110.0);
    h.cmd(motion(0.0, 500.0));
    h.advance(0.3);
    assert!(h.wm.snap_preview(h.now).is_some());
    h.cmd(motion(400.0, 500.0));
    assert!(h.wm.snap_preview(h.now).is_none());
    let fx = h.cmd(Command::PointerRelease);
    assert!(!has(&fx, |e| matches!(e, Effect::Configure { .. })));
    assert_eq!(h.state(w), LifecycleState::Normal);
}

#[test]
fn release_before_dwell_does_not_tile() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 300.0, 110.0);
    h.cmd(motion(10.0, 500.0));
    h.advance(0.05);
    h.cmd(Command::PointerRelease);
    assert_eq!(h.state(w), LifecycleState::Normal);
}

#[test]
fn top_edge_fills_and_corners_make_quarters_and_zones_retarget() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion_opt(960.0, 0.0));
    assert_eq!(h.wm.snap_preview(h.now).unwrap().kind, TileKind::Fill);
    let fx = h.cmd(motion_opt(1919.0, 2.0));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::SnapPreview { rect: Some(r), .. } if *r == tile_rect(TileKind::TopRight, USABLE, 8.0))
    ));
    assert_eq!(h.wm.snap_preview(h.now).unwrap().kind, TileKind::TopRight);
    h.cmd(Command::PointerRelease);
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Tiled(TileKind::TopRight));
}

#[test]
fn tiled_margins_apply_to_preview_and_result() {
    let mut cfg = WmConfig::default();
    cfg.snap.tiled_margin = 8.0;
    let mut h = H::with_config(cfg);
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion_opt(0.0, 500.0));
    h.cmd(Command::PointerRelease);
    h.settle();
    assert!(rect_eq(
        h.frame(w),
        Rect::new(8.0, 32.0, 960.0 - 12.0, 1056.0 - 16.0)
    ));
}

#[test]
fn dock_zone_suppresses_hot_zone() {
    let mut cfg = WmConfig::default();
    cfg.snap.avoid_dock_hot_zones = true;
    let mut h = H::with_config(cfg);
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    h.cmd(Command::SetDockZone {
        output: OUT,
        zone: Some(Rect::new(0.0, 900.0, 100.0, 180.0)),
    });
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion_opt(0.0, 1000.0));
    assert!(h.wm.snap_preview(h.now).is_none());
}

#[test]
fn another_outputs_dock_zone_leaves_this_one_in_place() {
    let mut cfg = WmConfig::default();
    cfg.snap.avoid_dock_hot_zones = true;
    let mut h = H::with_config(cfg);
    let out2 = OutputId(2);
    h.cmd(Command::AddOutput {
        output: out2,
        rect: Rect::new(1920.0, 0.0, 1280.0, 800.0),
        usable: Rect::new(1920.0, 24.0, 1280.0, 776.0),
    });
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    h.cmd(Command::SetDockZone {
        output: OUT,
        zone: Some(Rect::new(0.0, 900.0, 100.0, 180.0)),
    });
    // the second output has no dock: its refresh must not clear the first output's zone
    h.cmd(Command::SetDockZone {
        output: out2,
        zone: None,
    });
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion_opt(0.0, 1000.0));
    assert!(h.wm.snap_preview(h.now).is_none());
}

#[test]
fn dragging_tiled_window_out_restores_pre_tile_size_under_pointer() {
    let mut h = H::new();
    let n = Rect::new(300.0, 200.0, 600.0, 400.0);
    let w = h.map_at("a", n);
    h.cmd(Command::Tile {
        window: w,
        kind: TileKind::Left,
    });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Tiled(TileKind::Left));
    let tile = h.frame(w);
    let tx = tile.x + 0.5 * tile.w;
    titlebar_grab(&mut h, w, tx, 40.0);
    let fx = h.cmd(motion(tx + 5.0, 45.0));
    assert!(fx.is_empty(), "still inside the 5 pt drag-start box");
    let fx = h.cmd(motion(600.0, 60.0));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { size, states, .. } if *size == Size::new(600.0, 400.0) && !states.tiled.any())
    ));
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::DragOut,
            ..
        }
    )));
    assert_eq!(h.state(w), LifecycleState::Normal);
    let f = h.wm.window(w).unwrap().lifecycle.normal_frame();
    assert_eq!(f.size(), n.size());
    assert!(f.contains(Point::new(600.0, 60.0)));
    assert!(
        (f.x - (600.0 - 0.5 * 600.0)).abs() < 1e-9,
        "proportional grab point: {f:?}"
    );
    h.cmd(Command::PointerRelease);
}

#[test]
fn zoomed_window_can_be_dragged_out_too() {
    let mut h = H::new();
    let n = Rect::new(300.0, 200.0, 600.0, 400.0);
    let w = h.map_at("a", n);
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    titlebar_grab(&mut h, w, 960.0, 40.0);
    h.cmd(motion(960.0, 100.0));
    assert_eq!(h.state(w), LifecycleState::Normal);
    assert_eq!(
        h.wm.window(w).unwrap().lifecycle.normal_frame().size(),
        n.size()
    );
}

#[test]
fn workspace_edge_dwell_switches_and_carries_window() {
    let mut cfg = WmConfig::default();
    cfg.snap.edge_threshold = 1.0;
    let mut h = H::with_config(cfg);
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion(1919.5, 600.0));
    h.advance(0.5);
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[0]));
    let fx = h.advance(0.4);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WorkspaceSwitched { to, .. } if *to == ids[1])
    ));
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[1]));
    assert_eq!(
        h.wm.window(w).unwrap().workspace,
        ids[1],
        "dragged window comes along"
    );
    assert!(
        h.wm.snap_preview(h.now).is_none(),
        "tiling preview is dropped on switch"
    );
    h.cmd(Command::PointerRelease);
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Normal);
}

#[test]
fn workspace_edge_switch_needs_neighbour_and_can_be_disabled() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion(1919.5, 600.0));
    let fx = h.advance(2.0);
    assert!(!has(&fx, |e| matches!(e, Effect::WorkspaceSwitched { .. })));

    let mut cfg = WmConfig::default();
    cfg.snap.workspace_edge_switch = false;
    let mut h = H::with_config(cfg);
    h.cmd(Command::AddWorkspace { output: OUT });
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion(1919.5, 600.0));
    let fx = h.advance(2.0);
    assert!(!has(&fx, |e| matches!(e, Effect::WorkspaceSwitched { .. })));
}

#[test]
fn workspace_edge_dwell_resets_when_pointer_leaves_edge() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion(1919.5, 600.0));
    h.advance(0.6);
    h.cmd(motion(1500.0, 600.0));
    h.advance(0.5);
    h.cmd(motion(1919.5, 600.0));
    let fx = h.advance(0.6);
    assert!(!has(&fx, |e| matches!(e, Effect::WorkspaceSwitched { .. })));
}

#[test]
fn resize_from_each_edge_with_min_max_clamps() {
    let mut h = H::new();
    let g = Rect::new(400.0, 300.0, 600.0, 400.0);
    let w = h.map_at("a", g);
    h.cmd(Command::SetSizeHints {
        window: w,
        min: Some(Size::new(300.0, 200.0)),
        max: Some(Size::new(800.0, 500.0)),
        content: None,
    });

    let right_bottom = Edges {
        right: true,
        bottom: true,
        ..Edges::NONE
    };
    let fx = h.cmd(Command::BeginResize {
        window: w,
        edges: right_bottom,
        pointer: Point::new(1000.0, 700.0),
    });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { states, .. } if states.resizing)
    ));
    h.cmd(motion(1100.0, 750.0));
    assert_eq!(h.frame(w), Rect::new(400.0, 300.0, 700.0, 450.0));
    let fx = h.cmd(motion(1500.0, 1000.0));
    assert_eq!(
        h.frame(w),
        Rect::new(400.0, 300.0, 800.0, 500.0),
        "max clamp"
    );
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { size, states, .. } if *size == Size::new(800.0, 500.0) && states.resizing)
    ));
    h.cmd(motion(0.0, 0.0));
    assert_eq!(
        h.frame(w),
        Rect::new(400.0, 300.0, 300.0, 200.0),
        "min clamp"
    );
    let fx = h.cmd(Command::PointerRelease);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { states, .. } if !states.resizing)
    ));
    assert!(!h.wm.is_interacting());

    let left_top = Edges {
        left: true,
        top: true,
        ..Edges::NONE
    };
    h.cmd(Command::BeginResize {
        window: w,
        edges: left_top,
        pointer: Point::new(400.0, 300.0),
    });
    h.cmd(motion(350.0, 270.0));
    let f = h.frame(w);
    assert_eq!((f.w, f.h), (350.0, 230.0));
    assert_eq!(
        (f.right(), f.bottom()),
        (700.0, 500.0),
        "opposite edges stay fixed"
    );
    h.cmd(motion(2000.0, 2000.0));
    let f = h.frame(w);
    assert_eq!((f.w, f.h), (300.0, 200.0));
    assert_eq!((f.right(), f.bottom()), (700.0, 500.0));
}

#[test]
fn resizing_a_tiled_window_makes_it_normal_again() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(300.0, 200.0, 600.0, 400.0));
    h.cmd(Command::Tile {
        window: w,
        kind: TileKind::Right,
    });
    h.settle();
    h.cmd(Command::BeginResize {
        window: w,
        edges: Edges {
            left: true,
            ..Edges::NONE
        },
        pointer: Point::new(960.0, 500.0),
    });
    assert_eq!(h.state(w), LifecycleState::Normal);
    h.cmd(motion(900.0, 500.0));
    assert_eq!(h.frame(w).w, 600.0 + 60.0);
}

/// Window "a" at (100, 100, 600, 400) next to "b" at (1000, 100, 400, 400), grabbed at `grab`
/// and dragged right at 280 pt/s for 1 s: its right edge stops 19 pt before b's facing edge.
fn approach(grab: Point) -> (H, WindowId) {
    let mut h = H::new();
    let a = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    h.map_at("b", Rect::new(1000.0, 100.0, 400.0, 400.0));
    titlebar_grab(&mut h, a, grab.x, grab.y);
    h.now += 1.0;
    h.cmd(motion(grab.x + 280.0, grab.y));
    assert!(rect_eq(h.frame(a), Rect::new(380.0, 100.0, 600.0, 400.0)));
    (h, a)
}

#[test]
fn slow_drag_snaps_one_point_short_of_the_next_window() {
    let (mut h, a) = approach(Point::new(300.0, 110.0));
    h.now += 0.25;
    let fx = h.cmd(motion(610.0, 110.0));
    let snapped = Rect::new(399.0, 100.0, 600.0, 400.0);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Place { window, rect } if *window == a && *rect == snapped)
    ));
    assert!(
        rect_eq(h.frame(a), snapped),
        "the pointer is inside the window: no catch-up"
    );
    h.cmd(Command::PointerRelease);
    assert_eq!(h.wm.window(a).unwrap().lifecycle.normal_frame(), snapped);
}

#[test]
fn fast_drag_and_alt_drag_do_not_snap() {
    let (mut h, a) = approach(Point::new(300.0, 110.0));
    h.now += 0.02;
    h.cmd(motion(610.0, 110.0));
    assert!(
        rect_eq(h.frame(a), Rect::new(410.0, 100.0, 600.0, 400.0)),
        "1500 pt/s"
    );
    let (mut h, a) = approach(Point::new(300.0, 110.0));
    h.now += 0.25;
    h.cmd(motion_opt(610.0, 110.0));
    assert!(
        rect_eq(h.frame(a), Rect::new(410.0, 100.0, 600.0, 400.0)),
        "Alt held"
    );
}

#[test]
fn window_catches_up_when_the_jump_leaves_the_pointer_outside() {
    // grabbed 5 pt from its right edge, the pointer ends up beside the window it was dragged from
    let (mut h, a) = approach(Point::new(695.0, 110.0));
    h.now += 0.25;
    let t = h.now;
    h.cmd(motion(975.0 + 30.0, 110.0));
    let snapped = Rect::new(399.0, 100.0, 600.0, 400.0);
    assert_eq!(h.wm.window(a).unwrap().lifecycle.normal_frame(), snapped);
    assert!(rect_eq(h.frame(a), Rect::new(380.0, 100.0, 600.0, 400.0)));
    let x = |h: &H, at: f64| h.wm.presentation(a, at).unwrap().frame.x;
    assert!(
        (x(&h, t + 0.075) - (399.0 - 19.0 * (1.0 - h.wm.config().magnet.catch_up_curve.eval(0.5))))
            .abs()
            < 1e-9
    );
    assert_eq!(x(&h, t + 0.15), 399.0);
    assert!(h.wm.wants_tick(t + 0.1) && !h.wm.wants_tick(t + 0.15));
}

#[test]
fn resized_edge_snaps_to_the_next_window() {
    let mut h = H::new();
    let a = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    h.map_at("b", Rect::new(1000.0, 100.0, 400.0, 400.0));
    h.cmd(Command::BeginResize {
        window: a,
        edges: Edges {
            right: true,
            ..Edges::NONE
        },
        pointer: Point::new(700.0, 300.0),
    });
    h.now += 1.0;
    h.cmd(motion(980.0, 300.0));
    assert_eq!(h.frame(a), Rect::new(100.0, 100.0, 880.0, 400.0));
    h.now += 0.25;
    h.cmd(motion(1010.0, 300.0));
    assert_eq!(h.frame(a), Rect::new(100.0, 100.0, 899.0, 400.0));
}

#[test]
fn children_follow_parent_while_moving() {
    let mut h = H::new();
    let parent = h.map_at("a", Rect::new(200.0, 100.0, 600.0, 400.0));
    let mut p = MapParams::new(WindowId(90), "a", Rect::new(0.0, 0.0, 300.0, 200.0));
    p.parent = Some(parent);
    p.modal = true;
    h.cmd(Command::Map(p));
    h.settle();
    let child = WindowId(90);
    let c0 = h.wm.window(child).unwrap().geometry();
    titlebar_grab(&mut h, parent, 300.0, 110.0);
    h.cmd(motion(400.0, 160.0));
    let c1 = h.wm.window(child).unwrap().geometry();
    assert_eq!((c1.x - c0.x, c1.y - c0.y), (100.0, 50.0));
}

#[test]
fn cancel_restores_start_frame() {
    let mut h = H::new();
    let g = Rect::new(100.0, 100.0, 600.0, 400.0);
    let w = h.map_at("a", g);
    titlebar_grab(&mut h, w, 300.0, 110.0);
    h.cmd(motion(600.0, 400.0));
    h.cmd(motion(0.0, 500.0));
    h.advance(0.3);
    let fx = h.cmd(Command::CancelInteraction);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SnapPreview { rect: None, .. }
    )));
    assert!(rect_eq(h.frame(w), g));
    assert!(!h.wm.is_interacting());

    h.cmd(Command::BeginResize {
        window: w,
        edges: Edges {
            right: true,
            ..Edges::NONE
        },
        pointer: Point::new(700.0, 300.0),
    });
    h.cmd(motion(800.0, 300.0));
    h.cmd(Command::CancelInteraction);
    assert!(rect_eq(h.frame(w), g));
}

#[test]
fn interaction_ends_when_window_closes_or_minimizes() {
    let mut h = H::new();
    let w = h.map("a");
    titlebar_grab(&mut h, w, 300.0, 110.0);
    h.cmd(Command::Minimize {
        window: w,
        target: None,
    });
    assert!(!h.wm.is_interacting());
    let w2 = h.map("b");
    titlebar_grab(&mut h, w2, 300.0, 110.0);
    h.cmd(Command::Destroy { window: w2 });
    assert!(!h.wm.is_interacting());
    assert!(h.cmd(motion(10.0, 10.0)).is_empty());
}

#[test]
fn fullscreen_and_minimized_windows_cannot_be_moved() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    h.settle();
    titlebar_grab(&mut h, w, 100.0, 10.0);
    assert!(!h.wm.is_interacting());
}

#[test]
fn move_onto_other_output_changes_output_and_workspace() {
    let mut h = H::new();
    let out2 = OutputId(2);
    h.cmd(Command::AddOutput {
        output: out2,
        rect: Rect::new(1920.0, 0.0, 1280.0, 800.0),
        usable: Rect::new(1920.0, 24.0, 1280.0, 776.0),
    });
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion(2500.0, 310.0));
    assert_eq!(h.wm.window(w).unwrap().output, out2);
    assert_eq!(
        h.wm.window(w).unwrap().workspace,
        h.wm.current_workspace(out2).unwrap()
    );
    h.cmd(Command::PointerRelease);
    assert!(h.wm.visible_windows(out2).contains(&w));
    assert!(!h.wm.visible_windows(OUT).contains(&w));
}

#[test]
fn hard_edge_previews_without_dwell_and_soft_zone_waits() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(100.0, 100.0, 600.0, 400.0));
    titlebar_grab(&mut h, w, 300.0, 110.0);
    h.cmd(motion(0.0, 500.0));
    h.advance(0.01);
    assert_eq!(h.wm.snap_preview(h.now).unwrap().kind, TileKind::Left);
    h.cmd(motion(400.0, 500.0));
    h.cmd(motion(15.0, 500.0));
    h.advance(0.1);
    assert!(h.wm.snap_preview(h.now).is_none());
    h.advance(0.15);
    assert_eq!(h.wm.snap_preview(h.now).unwrap().kind, TileKind::Left);
}

#[test]
fn disabled_tiling_animations_are_instant() {
    let mut cfg = WmConfig::default();
    cfg.animation.disable_tiling = true;
    let mut h = H::with_config(cfg);
    let w = h.map_at("a", Rect::new(300.0, 200.0, 600.0, 400.0));
    h.cmd(Command::Tile {
        window: w,
        kind: TileKind::Left,
    });
    h.advance(0.02);
    assert_eq!(h.state(w), LifecycleState::Tiled(TileKind::Left));
}

#[test]
fn double_clicking_a_resize_edge_moves_it_to_the_usable_edge() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(400.0, 300.0, 600.0, 400.0));
    let fx = h.cmd(Command::SnapEdgesToScreen {
        window: w,
        edges: Edges {
            bottom: true,
            ..Edges::NONE
        },
        opposite: false,
    });
    let snapped = Rect::new(400.0, 300.0, 600.0, USABLE.bottom() - 300.0);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { size, .. } if *size == snapped.size())
    ));
    assert_eq!(h.wm.window(w).unwrap().geometry(), snapped);
    assert!(!rect_eq(h.frame(w), snapped), "the frame animates there");
    h.settle();
    assert!(rect_eq(h.frame(w), snapped));
    // a corner with Alt: both axes take the whole usable area
    h.cmd(Command::SnapEdgesToScreen {
        window: w,
        edges: Edges {
            left: true,
            top: true,
            ..Edges::NONE
        },
        opposite: true,
    });
    assert_eq!(h.wm.window(w).unwrap().geometry(), USABLE);
}

#[test]
fn double_click_edge_snap_keeps_size_limits_and_skips_full_screen() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(400.0, 300.0, 600.0, 400.0));
    h.cmd(Command::SetSizeHints {
        window: w,
        min: None,
        max: Some(Size::new(1000.0, 500.0)),
        content: None,
    });
    assert!(
        h.cmd(Command::SnapEdgesToScreen {
            window: w,
            edges: Edges {
                bottom: true,
                ..Edges::NONE
            },
            opposite: false
        })
        .is_empty(),
        "too tall for the maximum"
    );
    h.cmd(Command::SnapEdgesToScreen {
        window: w,
        edges: Edges {
            left: true,
            bottom: true,
            ..Edges::NONE
        },
        opposite: false,
    });
    assert_eq!(
        h.wm.window(w).unwrap().geometry(),
        Rect::new(0.0, 300.0, 1000.0, 400.0),
        "the width fits, the height does not"
    );
    h.settle();
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    h.settle();
    assert!(h
        .cmd(Command::SnapEdgesToScreen {
            window: w,
            edges: Edges {
                right: true,
                ..Edges::NONE
            },
            opposite: false
        })
        .is_empty());
}

#[test]
fn size_snapping_while_resizing_is_off_by_default_and_waits_0_4_s() {
    let start = Rect::new(400.0, 300.0, 600.0, 400.0);
    let bottom = Edges {
        bottom: true,
        ..Edges::NONE
    };
    let mut h = H::new();
    let w = h.map_at("a", start);
    h.cmd(Command::BeginResize {
        window: w,
        edges: bottom,
        pointer: Point::new(700.0, 700.0),
    });
    h.cmd(motion(700.0, 1040.0));
    assert!(!h.wm.wants_tick(h.now), "off by default: nothing waits");

    let mut cfg = WmConfig::default();
    cfg.snap.snap_size_to_screen_edges = true;
    let mut h = H::with_config(cfg);
    let w = h.map_at("a", start);
    h.cmd(Command::BeginResize {
        window: w,
        edges: bottom,
        pointer: Point::new(700.0, 700.0),
    });
    h.cmd(motion(700.0, 1040.0));
    h.advance(0.3);
    h.cmd(motion(700.0, 1041.0));
    h.advance(0.3);
    assert_eq!(
        h.wm.window(w).unwrap().geometry().h,
        741.0,
        "moving restarted the delay"
    );
    h.advance(0.15);
    assert_eq!(
        h.wm.window(w).unwrap().geometry(),
        Rect::new(400.0, USABLE.y, 600.0, USABLE.h),
        "the bottom edge rested near the screen edge: full height"
    );
    h.cmd(motion(700.0, 1031.0));
    assert_eq!(
        h.wm.window(w).unwrap().geometry().bottom(),
        USABLE.bottom() - 10.0,
        "the resize goes on from the snapped frame"
    );
}

#[test]
fn a_window_that_cannot_zoom_is_not_offered_the_fill_zone_but_keeps_the_corners() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    h.cmd(Command::SetDisabledControls {
        window: w,
        disabled: DisabledControls {
            minimize: false,
            zoom: true,
        },
    });
    titlebar_grab(&mut h, w, 700.0, 310.0);
    h.cmd(motion_opt(960.0, 0.0));
    assert!(h.wm.snap_preview(h.now).is_none(), "no fill preview");
    h.cmd(motion_opt(1919.0, 2.0));
    assert_eq!(h.wm.snap_preview(h.now).unwrap().kind, TileKind::TopRight);
}
