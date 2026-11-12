use crate::common;
use common::*;
use cupertino_wm::*;

#[test]
fn tick_reports_state_changes() {
    let mut h = H::new();
    let id = WindowId(1);
    h.cmd(Command::Map(MapParams::new(
        id,
        "a",
        Rect::new(100.0, 100.0, 600.0, 400.0),
    )));
    let fx = h.settle();
    assert!(has(&fx, |e| matches!(
        e,
        Effect::WindowStateChanged {
            from: LifecycleState::Opening,
            to: LifecycleState::Normal,
            ..
        }
    )));
}

#[test]
fn zoom_uses_best_fit_with_size_hints() {
    let mut h = H::new();
    let w = h.map_at("a", Rect::new(500.0, 300.0, 600.0, 400.0));
    h.cmd(Command::SetSizeHints {
        window: w,
        min: None,
        max: Some(Size::new(1000.0, 700.0)),
        content: None,
    });
    let fx = h.cmd(Command::ToggleZoom { window: w });
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { size, states, .. } if *size == Size::new(1000.0, 700.0) && states.maximized)
    ));
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Zoom,
            ..
        }
    )));
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Zoomed);
    let g = h.frame(w);
    assert_eq!((g.w, g.h), (1000.0, 700.0));
    assert!(g.w < USABLE.w, "zoom is best fit, not maximize");
    let fx = h.cmd(Command::ToggleZoom { window: w });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Unzoom,
            ..
        }
    )));
    h.settle();
    assert!(rect_eq(h.frame(w), Rect::new(500.0, 300.0, 600.0, 400.0)));
}

#[test]
fn zoom_without_hints_fills_usable_area_and_respects_content_hint() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    assert!(rect_eq(h.frame(w), USABLE));
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    h.cmd(Command::SetSizeHints {
        window: w,
        min: None,
        max: None,
        content: Some(Size::new(900.0, 700.0)),
    });
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    assert_eq!((h.frame(w).w, h.frame(w).h), (900.0, 700.0));
}

#[test]
fn zoom_toggle_midway_reverses_without_jump() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::ToggleZoom { window: w });
    h.advance(0.1);
    let f = h.frame(w);
    h.cmd(Command::ToggleZoom { window: w });
    assert_eq!(h.state(w), LifecycleState::Unzooming);
    assert!(rect_eq(h.frame(w), f));
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Normal);
}

#[test]
fn zoomed_tiled_then_untile_returns_to_pre_zoom_geometry() {
    let mut h = H::new();
    let g = Rect::new(250.0, 150.0, 640.0, 420.0);
    let w = h.map_at("a", g);
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    h.cmd(Command::Tile {
        window: w,
        kind: TileKind::Right,
    });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Tiled(TileKind::Right));
    h.cmd(Command::Untile { window: w });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Normal);
    assert!(rect_eq(h.frame(w), g));
}

#[test]
fn tile_command_uses_output_usable_area() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::SetUsableArea {
        output: OUT,
        usable: Rect::new(0.0, 24.0, 1920.0, 960.0),
    });
    h.cmd(Command::Tile {
        window: w,
        kind: TileKind::BottomLeft,
    });
    h.settle();
    assert!(rect_eq(
        h.frame(w),
        tile_rect(
            TileKind::BottomLeft,
            Rect::new(0.0, 24.0, 1920.0, 960.0),
            8.0
        )
    ));
}

#[test]
fn fullscreen_zoomed_exit_returns_to_zoomed_and_dock_hides_nothing() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    h.settle();
    h.cmd(Command::ExitFullscreen { window: w });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Zoomed);
    assert!(rect_eq(h.frame(w), USABLE));
}

#[test]
fn minimize_ignored_for_fullscreen() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: w,
        output: None,
    });
    h.settle();
    let fx = h.cmd(Command::Minimize {
        window: w,
        target: None,
    });
    assert!(fx.is_empty());
    assert_eq!(h.state(w), LifecycleState::FullScreen);
}

#[test]
fn restore_of_minimized_window_on_other_workspace_switches() {
    let mut h = H::new();
    h.cmd(Command::AddWorkspace { output: OUT });
    let ids = h.ws();
    let w = h.map("a");
    h.cmd(Command::Minimize {
        window: w,
        target: None,
    });
    h.settle();
    h.cmd(Command::SwitchWorkspace {
        output: OUT,
        workspace: ids[1],
    });
    h.cmd(Command::Restore { window: w });
    assert_eq!(h.wm.current_workspace(OUT), Some(ids[0]));
    h.settle();
    assert_eq!(h.wm.key_window(), Some(w));
}

#[test]
fn closing_utility_window_stays_drawn_while_it_fades() {
    let mut h = H::new();
    let w = WindowId(50);
    let mut p = MapParams::new(w, "a", Rect::new(100.0, 100.0, 300.0, 200.0));
    p.kind = WindowKind::Utility;
    h.cmd(Command::Map(p));
    h.settle();
    h.cmd(Command::Unmap { window: w });
    assert!(h.wm.window(w).is_some());
    h.advance(0.075);
    let p = h.wm.presentation(w, h.now).unwrap();
    assert!(p.visible && (p.opacity - 0.5).abs() < 1e-9, "{p:?}");
    assert!(!h.wm.focusable(w));
    let fx = h.advance(0.075);
    assert!(has(&fx, |e| matches!(e, Effect::WindowDestroyed { .. })));
}

#[test]
fn plain_window_is_destroyed_on_the_next_tick_after_unmap() {
    let mut h = H::new();
    let w = h.map("a");
    let fx = h.cmd(Command::Unmap { window: w });
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Close,
            ..
        }
    )));
    let fx = h.wm.tick(h.now);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WindowDestroyed { window } if *window == w)
    ));
}

#[test]
fn minimized_window_closes_immediately() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::Minimize {
        window: w,
        target: None,
    });
    h.settle();
    let fx = h.cmd(Command::Unmap { window: w });
    assert!(has(&fx, |e| matches!(e, Effect::WindowDestroyed { .. })));
}

#[test]
fn wants_tick_is_false_when_idle() {
    let mut h = H::new();
    let _w = h.map("a");
    assert!(!h.wm.wants_tick(h.now));
}

#[test]
fn map_without_outputs_is_ignored() {
    let mut wm = Wm::new(WmConfig::default());
    let fx = wm.handle(
        Command::Map(MapParams::new(
            WindowId(1),
            "a",
            Rect::new(0.0, 0.0, 10.0, 10.0),
        )),
        0.0,
    );
    assert!(fx.is_empty());
    assert!(wm.window(WindowId(1)).is_none());
}

#[test]
fn output_resize_refits_fullscreen_zoomed_tiled_and_normal_windows() {
    let mut h = H::new();
    let fs = h.map("fullscreen");
    h.cmd(Command::Fullscreen {
        requested_by: Requester::Client,
        window: fs,
        output: None,
    });
    h.settle();
    let zoomed = h.map("zoomed");
    h.cmd(Command::ToggleZoom { window: zoomed });
    h.settle();
    let tiled = h.map("tiled");
    h.cmd(Command::Tile {
        window: tiled,
        kind: TileKind::Left,
    });
    h.settle();
    let normal = h.map_at("normal", Rect::new(1500.0, 900.0, 400.0, 300.0));
    assert!(rect_eq(h.frame(fs), SCREEN));

    let rect = Rect::new(0.0, 0.0, 1280.0, 720.0);
    let usable = Rect::new(0.0, 24.0, 1280.0, 696.0);
    let fx = h.cmd(Command::SetOutputRect {
        output: OUT,
        rect,
        usable,
    });
    assert!(rect_eq(h.frame(fs), rect), "full screen follows the output");
    assert!(
        rect_eq(h.frame(zoomed), usable),
        "zoom refits the usable area"
    );
    assert!(h.frame(tiled).w < 1280.0 / 2.0 + 1.0 && h.frame(tiled).right() <= usable.right());
    assert!(
        h.frame(normal).right() <= usable.right() + 1e-6
            && h.frame(normal).bottom() <= usable.bottom() + 1e-6
    );
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Configure { window, size, .. } if *window == fs && *size == Size::new(1280.0, 720.0))
    ));
    assert_eq!(h.wm.output(OUT).unwrap().rect, rect);
    // unknown outputs are ignored
    assert!(h
        .cmd(Command::SetOutputRect {
            output: OutputId(9),
            rect,
            usable
        })
        .is_empty());
}

#[test]
fn minimize_or_restore_restores_a_tiled_window_first_and_minimizes_a_normal_one() {
    let mut h = H::new();
    let g = Rect::new(250.0, 150.0, 640.0, 420.0);
    let w = h.map_at("a", g);
    h.cmd(Command::Tile {
        window: w,
        kind: TileKind::Fill,
    });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Tiled(TileKind::Fill));
    h.cmd(Command::MinimizeOrRestore { window: w });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Normal);
    assert!(rect_eq(h.frame(w), g));
    h.cmd(Command::MinimizeOrRestore { window: w });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Minimized);
}

#[test]
fn minimize_or_restore_unzooms_a_zoomed_window() {
    let mut h = H::new();
    let g = Rect::new(250.0, 150.0, 640.0, 420.0);
    let w = h.map_at("a", g);
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Zoomed);
    h.cmd(Command::MinimizeOrRestore { window: w });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Normal);
    assert!(rect_eq(h.frame(w), g));
}
