//! A window moves through its lifecycle as commands arrive, and the model tells the compositor what to do.

use crate::common;

use common::*;
use cupertino_wm::{
    AnimationKind, Command, DisabledControls, Effect, LifecycleState, MapParams, OutputId,
    Requester, Resting, TileKind, WindowKind, Wm,
};

#[test]
fn mapping_a_window_opens_it_and_gives_it_the_keyboard() {
    let mut wm = common::wm();
    let fx = wm.handle(
        Command::Map(MapParams::new(win(1), "org.test.a", frame(1))),
        1.0,
    );
    assert!(has(
        &fx,
        |e| matches!(e, Effect::AppRegistered { identity, .. } if identity == "org.test.a")
    ));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Focus { window: Some(w) } if *w == win(1))
    ));
    assert!(has(
        &fx,
        |e| matches!(e, Effect::StartAnimation { window, kind: AnimationKind::Opening } if *window == win(1))
    ));
    assert_eq!(wm.key_window(), Some(win(1)));
    assert_eq!(wm.window(win(1)).unwrap().geometry(), frame(1));
    assert_eq!(state(&wm, 1), LifecycleState::Opening);
    wm.tick(3.0);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);
}

#[test]
fn windows_of_one_identity_share_an_application() {
    let mut wm = common::wm();
    open(&mut wm, 1, "org.test.a", 1.0);
    open(&mut wm, 2, "org.test.a", 2.0);
    open(&mut wm, 3, "org.test.b", 3.0);
    assert_eq!(app_of(&wm, 1), app_of(&wm, 2));
    assert_ne!(app_of(&wm, 1), app_of(&wm, 3));
    assert_eq!(wm.apps().count(), 2);
    assert_eq!(wm.app_for_identity("org.test.b"), Some(app_of(&wm, 3)));
    assert_eq!(wm.app_for_identity("org.test.none"), None);
    let app = wm.app(app_of(&wm, 1)).unwrap();
    assert_eq!(
        app.windows_mru,
        [win(2), win(1)],
        "most recently used first"
    );
}

#[test]
fn the_newest_window_is_on_top_and_focusing_raises_another() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "b", 2.0);
    assert_eq!(wm.stacking_order(), [win(1), win(2)]);
    let fx = wm.handle(Command::FocusWindow { window: win(1) }, 3.0);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::Raise { window } if *window == win(1))
    ));
    assert_eq!(wm.stacking_order(), [win(2), win(1)]);
    assert_eq!(wm.key_window(), Some(win(1)));
    assert_eq!(wm.front_app(), Some(app_of(&wm, 1)));
    assert_eq!(wm.app_mru()[0], app_of(&wm, 1));
}

#[test]
fn closing_or_destroying_windows_hands_the_keyboard_to_the_next_one() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "a", 2.0);
    let fx = wm.handle(Command::Unmap { window: win(2) }, 3.0);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::StartAnimation { window, kind: AnimationKind::Close } if *window == win(2))
    ));
    assert_eq!(state(&wm, 2), LifecycleState::Closing);
    let fx = wm.tick(5.0);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WindowDestroyed { window } if *window == win(2))
    ));
    assert!(wm.window(win(2)).is_none());
    assert_eq!(wm.key_window(), Some(win(1)));
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "b", 2.0);
    wm.handle(Command::Destroy { window: win(2) }, 3.0);
    assert_eq!(wm.key_window(), None);
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let fx = wm.handle(Command::Destroy { window: win(1) }, 3.0);
    assert!(has(
        &fx,
        |e| matches!(e, Effect::WindowDestroyed { window } if *window == win(1))
    ));
    assert!(wm.window(win(1)).is_none());
    assert_eq!(wm.key_window(), None);
    assert!(wm.stacking_order().is_empty());
}

#[test]
fn commands_for_unknown_windows_do_nothing() {
    let mut wm = common::wm();
    for cmd in [
        Command::FocusWindow { window: win(9) },
        Command::Minimize {
            window: win(9),
            target: None,
        },
        Command::Restore { window: win(9) },
        Command::ToggleZoom { window: win(9) },
        Command::Tile {
            window: win(9),
            kind: TileKind::Left,
        },
        Command::Destroy { window: win(9) },
        Command::Unmap { window: win(9) },
        Command::SetTitle {
            window: win(9),
            title: "x".into(),
        },
    ] {
        assert!(wm.handle(cmd, 1.0).is_empty());
    }
    assert!(wm.windows().next().is_none());
}

#[test]
fn minimizing_hides_the_window_and_restoring_brings_back_the_exact_frame() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "a", 2.0);
    let target = cupertino_wm::Rect::new(900.0, 1000.0, 60.0, 60.0);
    let fx = wm.handle(
        Command::Minimize {
            window: win(2),
            target: Some(target),
        },
        3.0,
    );
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Minimize,
            ..
        }
    )));
    assert!(wm.window(win(2)).unwrap().state().is_minimized());
    assert_eq!(
        wm.key_window(),
        Some(win(1)),
        "the keyboard moves to the next window"
    );
    settle(&mut wm, 3.0);
    assert_eq!(state(&wm, 2), LifecycleState::Minimized);
    assert!(!wm.window_on_screen(win(2)));
    assert!(!wm.focusable(win(2)));
    assert!(!wm.visible_windows(OUT).contains(&win(2)));

    let fx = wm.handle(Command::Restore { window: win(2) }, 10.0);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Restore,
            ..
        }
    )));
    settle(&mut wm, 10.0);
    assert_eq!(state(&wm, 2), LifecycleState::Normal);
    assert_eq!(wm.window(win(2)).unwrap().geometry(), frame(2));
    assert_eq!(wm.key_window(), Some(win(2)));
}

#[test]
fn minimize_or_restore_undoes_a_zoom_or_a_tile_before_it_minimizes() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    wm.handle(Command::ToggleZoom { window: win(1) }, 3.0);
    settle(&mut wm, 3.0);
    wm.handle(Command::MinimizeOrRestore { window: win(1) }, 10.0);
    settle(&mut wm, 10.0);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);

    wm.handle(
        Command::Tile {
            window: win(1),
            kind: TileKind::Left,
        },
        20.0,
    );
    settle(&mut wm, 20.0);
    wm.handle(Command::MinimizeOrRestore { window: win(1) }, 30.0);
    settle(&mut wm, 30.0);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);

    wm.handle(Command::MinimizeOrRestore { window: win(1) }, 40.0);
    settle(&mut wm, 40.0);
    assert_eq!(state(&wm, 1), LifecycleState::Minimized);
}

#[test]
fn a_window_whose_app_disabled_minimize_stays_up() {
    let mut wm = common::wm();
    let mut p = MapParams::new(win(1), "a", frame(1));
    p.disabled = DisabledControls {
        minimize: true,
        zoom: false,
    };
    wm.handle(Command::Map(p), 1.0);
    wm.tick(3.0);
    assert!(!wm.window(win(1)).unwrap().can_minimize());
    wm.handle(
        Command::Minimize {
            window: win(1),
            target: None,
        },
        4.0,
    );
    settle(&mut wm, 4.0);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);
    wm.handle(
        Command::SetDisabledControls {
            window: win(1),
            disabled: DisabledControls::default(),
        },
        5.0,
    );
    assert!(wm.window(win(1)).unwrap().can_minimize());
}

#[test]
fn zoom_fits_the_window_within_its_size_limits_and_unzoom_returns_it() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let fx = wm.handle(Command::ToggleZoom { window: win(1) }, 3.0);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Zoom,
            ..
        }
    )));
    settle(&mut wm, 3.0);
    assert_eq!(state(&wm, 1), LifecycleState::Zoomed);
    assert_eq!(wm.window(win(1)).unwrap().geometry(), USABLE);
    let fx = wm.handle(Command::ToggleZoom { window: win(1) }, 10.0);
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Unzoom,
            ..
        }
    )));
    settle(&mut wm, 10.0);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);
    assert_eq!(wm.window(win(1)).unwrap().geometry(), frame(1));
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let fixed = Some(cupertino_wm::Size::new(640.0, 480.0));
    wm.handle(
        Command::SetSizeHints {
            window: win(1),
            min: fixed,
            max: fixed,
            content: None,
        },
        2.0,
    );
    assert!(!wm.window(win(1)).unwrap().can_zoom());
    wm.handle(Command::ToggleZoom { window: win(1) }, 3.0);
    settle(&mut wm, 3.0);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    wm.handle(
        Command::SetSizeHints {
            window: win(1),
            min: None,
            max: Some(cupertino_wm::Size::new(1000.0, 700.0)),
            content: None,
        },
        2.0,
    );
    wm.handle(Command::ToggleZoom { window: win(1) }, 3.0);
    settle(&mut wm, 3.0);
    let g = wm.window(win(1)).unwrap().geometry();
    assert_eq!((g.w, g.h), (1000.0, 700.0));
    assert!(
        g.x >= USABLE.x
            && g.right() <= USABLE.right()
            && g.y >= USABLE.y
            && g.bottom() <= USABLE.bottom()
    );
}

#[test]
fn full_screen_covers_the_output_and_exiting_restores_the_frame_or_the_zoom() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let fx = wm.handle(
        Command::Fullscreen {
            window: win(1),
            output: None,
            requested_by: Requester::User,
        },
        3.0,
    );
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::EnterFullScreen,
            ..
        }
    )));
    settle(&mut wm, 3.0);
    assert_eq!(state(&wm, 1), LifecycleState::FullScreen);
    assert_eq!(wm.window(win(1)).unwrap().geometry(), SCREEN);
    assert_eq!(wm.fullscreen_front(OUT), Some(win(1)));
    assert!(
        wm.window(win(1))
            .unwrap()
            .lifecycle
            .toplevel_states()
            .fullscreen
    );

    wm.handle(Command::ExitFullscreen { window: win(1) }, 10.0);
    settle(&mut wm, 10.0);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);
    assert_eq!(wm.window(win(1)).unwrap().geometry(), frame(1));
    assert_eq!(wm.fullscreen_front(OUT), None);
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    wm.handle(Command::ToggleZoom { window: win(1) }, 3.0);
    settle(&mut wm, 3.0);
    wm.handle(
        Command::Fullscreen {
            window: win(1),
            output: None,
            requested_by: Requester::User,
        },
        10.0,
    );
    settle(&mut wm, 10.0);
    wm.handle(Command::ExitFullscreen { window: win(1) }, 20.0);
    settle(&mut wm, 20.0);
    assert_eq!(state(&wm, 1), LifecycleState::Zoomed);
    assert_eq!(
        wm.window(win(1)).unwrap().lifecycle.resting(),
        Resting::Zoomed
    );
}

#[test]
fn tiling_places_the_window_in_its_half_and_a_later_zoom_returns_to_the_frame_before_either() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let fx = wm.handle(
        Command::Tile {
            window: win(1),
            kind: TileKind::Left,
        },
        3.0,
    );
    assert!(has(&fx, |e| matches!(
        e,
        Effect::StartAnimation {
            kind: AnimationKind::Tile(TileKind::Left),
            ..
        }
    )));
    settle(&mut wm, 3.0);
    assert_eq!(state(&wm, 1), LifecycleState::Tiled(TileKind::Left));
    let margin = wm.config().snap.tiled_margin;
    assert_eq!(
        wm.window(win(1)).unwrap().geometry(),
        cupertino_wm::tile_rect(TileKind::Left, USABLE, margin)
    );
    let states = wm.window(win(1)).unwrap().lifecycle.toplevel_states();
    assert!(states.tiled.left && !states.tiled.right);
    wm.handle(Command::Untile { window: win(1) }, 10.0);
    settle(&mut wm, 10.0);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);
    assert_eq!(wm.window(win(1)).unwrap().geometry(), frame(1));
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    wm.handle(Command::ToggleZoom { window: win(1) }, 3.0);
    settle(&mut wm, 3.0);
    wm.handle(
        Command::Tile {
            window: win(1),
            kind: TileKind::Right,
        },
        10.0,
    );
    settle(&mut wm, 10.0);
    wm.handle(Command::Untile { window: win(1) }, 20.0);
    settle(&mut wm, 20.0);
    assert_eq!(
        wm.window(win(1)).unwrap().lifecycle.normal_frame(),
        frame(1)
    );
}

#[test]
fn titles_and_parents_are_kept_on_the_window() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    open(&mut wm, 2, "a", 2.0);
    wm.handle(
        Command::SetTitle {
            window: win(1),
            title: "Notes".into(),
        },
        3.0,
    );
    assert_eq!(wm.window(win(1)).unwrap().title, "Notes");
    wm.handle(
        Command::SetParent {
            window: win(2),
            parent: Some(win(1)),
        },
        3.0,
    );
    assert_eq!(wm.window(win(2)).unwrap().parent, Some(win(1)));
    wm.handle(
        Command::SetParent {
            window: win(2),
            parent: None,
        },
        3.0,
    );
    assert_eq!(wm.window(win(2)).unwrap().parent, None);
}

#[test]
fn a_close_request_is_passed_to_the_client_not_acted_on() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let fx = wm.handle(Command::RequestClose { window: win(1) }, 3.0);
    assert_eq!(fx, [Effect::RequestClose { window: win(1) }]);
    assert_eq!(state(&wm, 1), LifecycleState::Normal);
}

#[test]
fn a_sheet_hangs_from_its_parent_dims_it_and_blocks_its_input() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let mut sheet = MapParams::new(win(2), "a", cupertino_wm::Rect::new(0.0, 0.0, 400.0, 200.0));
    sheet.parent = Some(win(1));
    sheet.modal = true;
    sheet.kind = WindowKind::Sheet;
    wm.handle(Command::Map(sheet), 3.0);
    let parent = wm.window(win(1)).unwrap().geometry();
    let g = wm.window(win(2)).unwrap().geometry();
    assert_eq!(g.w, 400.0);
    assert_eq!(g.x, parent.x + ((parent.w - 400.0) / 2.0).round());
    assert!(g.y >= parent.y);
    wm.tick(3.3);
    assert!(wm.sheet_dimming(win(1), 4.0) > 0.0);
    assert_eq!(wm.sheet_dimming(win(2), 4.0), 0.0);
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    let mut dialog = MapParams::new(
        win(2),
        "a",
        cupertino_wm::Rect::new(300.0, 300.0, 300.0, 150.0),
    );
    dialog.parent = Some(win(1));
    dialog.modal = true;
    wm.handle(Command::Map(dialog), 3.0);
    wm.tick(5.0);
    wm.handle(Command::FocusWindow { window: win(1) }, 6.0);
    assert_eq!(
        wm.key_window(),
        Some(win(2)),
        "the modal child keeps the keyboard"
    );
}

/// A monitor that comes back (as a new output) is given its windows before the menu bar registers again; the bar's area arrives later.
const BACK: OutputId = OutputId(2);

fn readd_output_with_full_usable(wm: &mut Wm) {
    wm.handle(Command::RemoveOutput { output: OUT }, 10.0);
    wm.handle(
        Command::AddOutput {
            output: BACK,
            rect: SCREEN,
            usable: SCREEN,
        },
        11.0,
    );
    settle(wm, 11.0);
    wm.handle(
        Command::SetUsableArea {
            output: BACK,
            usable: USABLE,
        },
        20.0,
    );
    settle(wm, 20.0);
}

#[test]
fn a_usable_area_that_arrives_after_the_output_refits_a_zoomed_window() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    wm.handle(Command::ToggleZoom { window: win(1) }, 3.0);
    settle(&mut wm, 3.0);
    readd_output_with_full_usable(&mut wm);
    assert_eq!(state(&wm, 1), LifecycleState::Zoomed);
    assert_eq!(wm.window(win(1)).unwrap().geometry(), USABLE);
}

#[test]
fn a_usable_area_that_arrives_after_the_output_refits_a_tiled_window() {
    let mut wm = common::wm();
    open(&mut wm, 1, "a", 1.0);
    wm.handle(
        Command::Tile {
            window: win(1),
            kind: TileKind::Top,
        },
        3.0,
    );
    settle(&mut wm, 3.0);
    readd_output_with_full_usable(&mut wm);
    assert_eq!(state(&wm, 1), LifecycleState::Tiled(TileKind::Top));
    assert!(wm.window(win(1)).unwrap().geometry().y >= USABLE.y);
}
