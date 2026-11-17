//! Interactive move, resize, snapping to the screen edges and tiling by dragging.

mod common;

use common::*;
use cupertino_wm::{
    Command, Edges, Effect, LifecycleState, Modifiers, Point, Rect, SnapConfig, TileKind, Wm,
};

fn placed(fx: &[Effect], window: u64) -> Option<Rect> {
    fx.iter().rev().find_map(|e| match e {
        Effect::Place { window: w, rect } if *w == win(window) => Some(*rect),
        _ => None,
    })
}

fn motion(wm: &mut Wm, x: f64, y: f64, now: f64) -> Vec<Effect> {
    wm.handle(
        Command::PointerMotion {
            pointer: Point::new(x, y),
            modifiers: Modifiers::default(),
        },
        now,
    )
}

fn grab(wm: &mut Wm, n: u64, now: f64) -> Point {
    let f = frame(n);
    let p = Point::new(f.x + 100.0, f.y + 10.0);
    wm.handle(
        Command::BeginMove {
            window: win(n),
            pointer: p,
        },
        now,
    );
    p
}

fn one_window() -> Wm {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    wm
}

#[test]
fn a_move_keeps_the_grab_point_under_the_pointer() {
    let mut wm = one_window();
    let p = grab(&mut wm, 1, 10.0);
    assert!(wm.is_moving());
    assert_eq!(wm.interacting_window(), Some(win(1)));
    let fx = motion(&mut wm, p.x + 300.0, p.y + 200.0, 10.1);
    let f = placed(&fx, 1).expect("the window is placed");
    assert_eq!((f.w, f.h), (640.0, 480.0), "a move never resizes");
    assert!(
        (f.x - (frame(1).x + 300.0)).abs() < 40.0,
        "within the magnet's reach: {f:?}"
    );
    assert!((f.y - (frame(1).y + 200.0)).abs() < 40.0);
}

#[test]
fn the_press_box_stops_a_click_from_moving_a_tiled_window() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    wm.handle(
        Command::Tile {
            window: win(1),
            kind: TileKind::Left,
        },
        2.0,
    );
    settle(&mut wm, 2.0);
    let tiled = wm.window(win(1)).unwrap().geometry();
    let p = Point::new(tiled.x + 100.0, tiled.y + 10.0);
    wm.handle(
        Command::BeginMove {
            window: win(1),
            pointer: p,
        },
        10.0,
    );
    let fx = motion(&mut wm, p.x + 1.0, p.y + 1.0, 10.05);
    assert!(
        placed(&fx, 1).is_none(),
        "a jitter inside the press box is not a drag"
    );
    assert_eq!(
        wm.window(win(1)).unwrap().state(),
        LifecycleState::Tiled(TileKind::Left)
    );
}

#[test]
fn dragging_a_tiled_window_out_of_its_tile_restores_the_untiled_size() {
    let mut wm = one_window();
    let before = frame(1);
    wm.handle(
        Command::Tile {
            window: win(1),
            kind: TileKind::Left,
        },
        2.0,
    );
    settle(&mut wm, 2.0);
    let tiled = wm.window(win(1)).unwrap().geometry();
    assert_ne!((tiled.w, tiled.h), (before.w, before.h));
    let p = Point::new(tiled.x + 200.0, tiled.y + 10.0);
    wm.handle(
        Command::BeginMove {
            window: win(1),
            pointer: p,
        },
        10.0,
    );
    motion(&mut wm, p.x + 300.0, p.y + 300.0, 10.2);
    settle(&mut wm, 10.2);
    assert_eq!(wm.window(win(1)).unwrap().state(), LifecycleState::Normal);
    let g = wm.window(win(1)).unwrap().geometry();
    assert_eq!((g.w, g.h), (before.w, before.h));
}

#[test]
fn releasing_a_move_ends_the_interaction_and_keeps_the_frame() {
    let mut wm = one_window();
    let p = grab(&mut wm, 1, 10.0);
    motion(&mut wm, p.x + 100.0, p.y + 100.0, 10.1);
    let at = wm.window(win(1)).unwrap().geometry();
    wm.handle(Command::PointerRelease, 10.2);
    assert!(!wm.is_interacting());
    assert_eq!(wm.window(win(1)).unwrap().geometry(), at);
}

#[test]
fn cancelling_a_move_puts_the_window_back() {
    let mut wm = one_window();
    let p = grab(&mut wm, 1, 10.0);
    motion(&mut wm, p.x + 400.0, p.y + 250.0, 10.1);
    assert_ne!(wm.window(win(1)).unwrap().geometry(), frame(1));
    let fx = wm.handle(Command::CancelInteraction, 10.2);
    assert_eq!(placed(&fx, 1), Some(frame(1)));
    assert_eq!(wm.window(win(1)).unwrap().geometry(), frame(1));
    assert!(!wm.is_interacting());
}

#[test]
fn a_title_bar_cannot_be_dragged_above_the_usable_area() {
    let mut wm = one_window();
    let p = grab(&mut wm, 1, 10.0);
    motion(&mut wm, p.x, -500.0, 10.1);
    let g = wm.window(win(1)).unwrap().geometry();
    assert!(g.y >= USABLE.y, "{g:?}");
}

#[test]
fn the_title_bar_may_leave_the_area_when_the_setting_is_off() {
    let mut wm = one_window();
    wm.config_mut().snap.keep_titlebar_visible = false;
    wm.config_mut().magnet.enabled = false;
    let p = grab(&mut wm, 1, 10.0);
    motion(&mut wm, p.x, -500.0, 10.1);
    assert!(wm.window(win(1)).unwrap().geometry().y < USABLE.y);
}

#[test]
fn begin_move_ignores_missing_minimized_and_full_screen_windows() {
    let mut wm = one_window();
    wm.handle(
        Command::BeginMove {
            window: win(77),
            pointer: Point::new(1.0, 1.0),
        },
        5.0,
    );
    assert!(!wm.is_interacting());
    wm.handle(
        Command::Minimize {
            window: win(1),
            target: None,
        },
        6.0,
    );
    settle(&mut wm, 6.0);
    wm.handle(
        Command::BeginMove {
            window: win(1),
            pointer: Point::new(200.0, 140.0),
        },
        20.0,
    );
    assert!(!wm.is_interacting());
}

#[test]
fn a_new_begin_cancels_the_interaction_in_progress() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "b", 2.0);
    let p = grab(&mut wm, 1, 10.0);
    motion(&mut wm, p.x + 200.0, p.y + 200.0, 10.1);
    grab(&mut wm, 2, 11.0);
    assert_eq!(wm.interacting_window(), Some(win(2)));
    assert_eq!(
        wm.window(win(1)).unwrap().geometry(),
        frame(1),
        "the first move was undone"
    );
}

#[test]
fn beginning_an_interaction_focuses_the_window() {
    let mut wm = wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "b", 2.0);
    grab(&mut wm, 1, 10.0);
    assert_eq!(wm.key_window(), Some(win(1)));
}

#[test]
fn resizing_the_right_and_bottom_edges_grows_the_window_from_its_origin() {
    let mut wm = one_window();
    wm.config_mut().magnet.enabled = false;
    let f = frame(1);
    let start = Point::new(f.right(), f.bottom());
    let edges = Edges {
        right: true,
        bottom: true,
        ..Edges::NONE
    };
    let fx = wm.handle(
        Command::BeginResize {
            window: win(1),
            edges,
            pointer: start,
        },
        10.0,
    );
    assert!(
        has(&fx, |e| matches!(e, Effect::Configure { .. })),
        "the client is told it is resizing"
    );
    let fx = motion(&mut wm, start.x + 100.0, start.y + 60.0, 10.1);
    let g = placed(&fx, 1).expect("placed");
    assert_eq!((g.x, g.y), (f.x, f.y));
    assert_eq!((g.w, g.h), (f.w + 100.0, f.h + 60.0));
}

#[test]
fn resizing_the_left_edge_moves_the_origin_and_keeps_the_right_edge_fixed() {
    let mut wm = one_window();
    wm.config_mut().magnet.enabled = false;
    let f = frame(1);
    let start = Point::new(f.x, f.y + 100.0);
    let edges = Edges {
        left: true,
        ..Edges::NONE
    };
    wm.handle(
        Command::BeginResize {
            window: win(1),
            edges,
            pointer: start,
        },
        10.0,
    );
    let fx = motion(&mut wm, start.x - 80.0, start.y, 10.1);
    let g = placed(&fx, 1).unwrap();
    assert_eq!(g.right(), f.right());
    assert_eq!(g.w, f.w + 80.0);
    assert_eq!(g.h, f.h);
}

#[test]
fn a_resize_never_goes_below_the_minimum_size() {
    let mut wm = wm();
    let mut p = cupertino_wm::MapParams::new(win(1), "a", frame(1));
    p.min_size = Some(cupertino_wm::Size::new(300.0, 200.0));
    wm.handle(Command::Map(p), 1.0);
    wm.tick(3.0);
    wm.config_mut().magnet.enabled = false;
    let f = frame(1);
    let start = Point::new(f.right(), f.bottom());
    let edges = Edges {
        right: true,
        bottom: true,
        ..Edges::NONE
    };
    wm.handle(
        Command::BeginResize {
            window: win(1),
            edges,
            pointer: start,
        },
        10.0,
    );
    motion(&mut wm, start.x - 2000.0, start.y - 2000.0, 10.1);
    let g = wm.window(win(1)).unwrap().geometry();
    assert_eq!((g.w, g.h), (300.0, 200.0));
}

#[test]
fn a_resize_without_any_edge_does_nothing() {
    let mut wm = one_window();
    wm.handle(
        Command::BeginResize {
            window: win(1),
            edges: Edges::NONE,
            pointer: Point::new(10.0, 10.0),
        },
        5.0,
    );
    assert!(!wm.is_interacting());
}

#[test]
fn cancelling_a_resize_restores_the_start_frame_and_reports_the_end() {
    let mut wm = one_window();
    wm.config_mut().magnet.enabled = false;
    let f = frame(1);
    let start = Point::new(f.right(), f.bottom());
    let edges = Edges {
        right: true,
        ..Edges::NONE
    };
    wm.handle(
        Command::BeginResize {
            window: win(1),
            edges,
            pointer: start,
        },
        10.0,
    );
    motion(&mut wm, start.x + 200.0, start.y, 10.1);
    let fx = wm.handle(Command::CancelInteraction, 10.2);
    assert_eq!(placed(&fx, 1), Some(f));
    assert!(has(&fx, |e| matches!(e, Effect::Configure { .. })));
    assert!(!wm.is_interacting());
}

#[test]
fn releasing_a_resize_tells_the_client_it_is_done() {
    let mut wm = one_window();
    let f = frame(1);
    let start = Point::new(f.right(), f.bottom());
    let edges = Edges {
        right: true,
        ..Edges::NONE
    };
    wm.handle(
        Command::BeginResize {
            window: win(1),
            edges,
            pointer: start,
        },
        10.0,
    );
    let fx = wm.handle(Command::PointerRelease, 10.5);
    assert!(has(&fx, |e| matches!(e, Effect::Configure { .. })));
    assert!(!wm.is_interacting());
}

#[test]
fn pointer_motion_without_an_interaction_is_ignored() {
    let mut wm = one_window();
    assert!(motion(&mut wm, 500.0, 500.0, 5.0).is_empty());
    assert!(wm.handle(Command::PointerRelease, 5.1).is_empty());
    assert!(wm.handle(Command::CancelInteraction, 5.2).is_empty());
}

#[test]
fn a_double_click_on_an_edge_stretches_the_window_to_the_usable_edge() {
    let mut wm = one_window();
    let f = frame(1);
    let fx = wm.handle(
        Command::SnapEdgesToScreen {
            window: win(1),
            edges: Edges {
                right: true,
                ..Edges::NONE
            },
            opposite: false,
        },
        10.0,
    );
    let g = placed(&fx, 1).expect("placed");
    assert_eq!(g, Rect::new(f.x, f.y, USABLE.right() - f.x, f.h));
}

#[test]
fn the_opposite_modifier_stretches_both_sides_of_the_axis() {
    let mut wm = one_window();
    let f = frame(1);
    let fx = wm.handle(
        Command::SnapEdgesToScreen {
            window: win(1),
            edges: Edges {
                bottom: true,
                ..Edges::NONE
            },
            opposite: true,
        },
        10.0,
    );
    assert_eq!(
        placed(&fx, 1),
        Some(Rect::new(f.x, USABLE.y, f.w, USABLE.h))
    );
}

#[test]
fn stretching_to_the_edge_is_off_with_its_setting_and_for_tiled_windows() {
    let mut wm = one_window();
    wm.config_mut().snap = SnapConfig {
        snap_size_on_double_click: false,
        ..SnapConfig::default()
    };
    let right = Edges {
        right: true,
        ..Edges::NONE
    };
    let fx = wm.handle(
        Command::SnapEdgesToScreen {
            window: win(1),
            edges: right,
            opposite: false,
        },
        10.0,
    );
    assert!(fx.is_empty());

    let mut wm = one_window();
    wm.handle(
        Command::Tile {
            window: win(1),
            kind: TileKind::Left,
        },
        2.0,
    );
    settle(&mut wm, 2.0);
    let fx = wm.handle(
        Command::SnapEdgesToScreen {
            window: win(1),
            edges: right,
            opposite: false,
        },
        10.0,
    );
    assert!(fx.is_empty());
}

#[test]
fn an_axis_that_would_break_the_size_limit_keeps_its_edges() {
    let mut wm = wm();
    let mut p = cupertino_wm::MapParams::new(win(1), "a", frame(1));
    p.max_size = Some(cupertino_wm::Size::new(700.0, 0.0));
    wm.handle(Command::Map(p), 1.0);
    wm.tick(3.0);
    let fx = wm.handle(
        Command::SnapEdgesToScreen {
            window: win(1),
            edges: Edges {
                right: true,
                ..Edges::NONE
            },
            opposite: false,
        },
        10.0,
    );
    assert!(fx.is_empty(), "stretching would exceed the maximum width");
}

#[test]
fn dragging_into_the_left_edge_previews_and_then_tiles_the_window() {
    let mut wm = one_window();
    wm.config_mut().magnet.enabled = false;
    let p = grab(&mut wm, 1, 10.0);
    let mut shown = Vec::new();
    shown.extend(motion(&mut wm, p.x - 200.0, 500.0, 10.1));
    shown.extend(motion(&mut wm, 0.0, 500.0, 10.2));
    shown.extend(wm.tick(10.3));
    let preview = shown.iter().rev().find_map(|e| match e {
        Effect::SnapPreview { rect, .. } => *rect,
        _ => None,
    });
    let preview = preview.expect("a preview shows after the hot-zone dwell");
    assert_eq!(
        preview.x,
        cupertino_wm::tile_rect(TileKind::Left, USABLE, 8.0).x
    );
    assert!(wm.snap_preview(10.5).is_some());

    wm.handle(Command::PointerRelease, 10.6);
    settle(&mut wm, 10.6);
    assert_eq!(
        wm.window(win(1)).unwrap().state(),
        LifecycleState::Tiled(TileKind::Left)
    );
    assert!(wm.snap_preview(20.0).is_none());
}

#[test]
fn leaving_the_hot_zone_withdraws_the_preview_and_release_does_not_tile() {
    let mut wm = one_window();
    wm.config_mut().magnet.enabled = false;
    let p = grab(&mut wm, 1, 10.0);
    motion(&mut wm, p.x - 200.0, 500.0, 10.1);
    motion(&mut wm, 0.0, 500.0, 10.2);
    wm.tick(10.5);
    assert!(wm.snap_preview(10.6).is_some());
    let fx = motion(&mut wm, 900.0, 500.0, 10.7);
    wm.tick(10.8);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::SnapPreview { rect: None, .. }
    )));
    wm.handle(Command::PointerRelease, 11.0);
    settle(&mut wm, 11.0);
    assert_eq!(wm.window(win(1)).unwrap().state(), LifecycleState::Normal);
}

#[test]
fn the_option_accelerator_previews_without_a_dwell() {
    let mut wm = one_window();
    wm.config_mut().magnet.enabled = false;
    let p = grab(&mut wm, 1, 10.0);
    motion(&mut wm, p.x - 200.0, 500.0, 10.05);
    let fx = wm.handle(
        Command::PointerMotion {
            pointer: Point::new(60.0, 500.0),
            modifiers: Modifiers {
                option: true,
                ..Modifiers::default()
            },
        },
        10.1,
    );
    let fx2 = wm.tick(10.1);
    assert!(
        fx.iter().chain(fx2.iter()).any(|e| matches!(
            e,
            Effect::SnapPreview {
                rect: Some(_),
                immediate: true,
                ..
            }
        )),
        "{fx:?} {fx2:?}"
    );
}

#[test]
fn the_edge_dwell_switches_workspace_and_the_window_follows_the_pointer_onto_it() {
    let mut wm = one_window();
    wm.config_mut().magnet.enabled = false;
    wm.config_mut().snap.edge_drag_tiling = false;
    wm.config_mut().snap.top_edge_tiling = false;
    wm.handle(Command::AddWorkspace { output: OUT }, 5.0);
    let first = wm.workspaces(OUT)[0].id;
    let second = wm.workspaces(OUT)[1].id;
    let p = grab(&mut wm, 1, 10.0);
    motion(&mut wm, p.x + 200.0, 500.0, 10.05);
    motion(&mut wm, 1919.5, 500.0, 10.1);
    let dwell = wm.config().snap.workspace_edge_dwell;
    assert_eq!(wm.current_workspace(OUT), Some(first));
    let mut fx = wm.tick(10.1 + dwell / 2.0);
    assert!(!has(&fx, |e| matches!(e, Effect::WorkspaceSwitched { .. })));
    fx = wm.tick(10.1 + dwell + 0.05);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WorkspaceSwitched { to, .. } if *to == second)
    ));
    assert_eq!(wm.window(win(1)).unwrap().workspace, second);
}
