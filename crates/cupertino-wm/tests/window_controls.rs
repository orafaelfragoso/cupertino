//! Controls an application disabled on its own window (`Command::SetDisabledControls`): the model refuses them whoever
//! asks, and leaves every way out of them open.
mod common;
use common::*;
use cupertino_wm::*;

fn disable(h: &mut H, window: WindowId, minimize: bool, zoom: bool) {
    h.cmd(Command::SetDisabledControls {
        window,
        disabled: DisabledControls { minimize, zoom },
    });
}

fn zoomed(state: LifecycleState) -> bool {
    matches!(state, LifecycleState::Zooming | LifecycleState::Zoomed)
}

#[test]
fn a_disabled_minimize_is_refused_on_every_path_and_works_again_once_enabled() {
    let mut h = H::new();
    let w = h.map("a");
    h.settle();
    disable(&mut h, w, true, false);
    h.cmd(Command::Minimize {
        window: w,
        target: None,
    });
    h.cmd(Command::MinimizeOrRestore { window: w });
    h.settle();
    assert!(!h.state(w).is_minimized());

    disable(&mut h, w, false, false);
    h.cmd(Command::Minimize {
        window: w,
        target: None,
    });
    h.settle();
    assert!(h.state(w).is_minimized());
}

#[test]
fn a_disabled_zoom_is_refused_but_a_zoomed_window_can_still_unzoom() {
    let mut h = H::new();
    let w = h.map("a");
    h.settle();
    disable(&mut h, w, false, true);
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    assert!(!zoomed(h.state(w)));

    disable(&mut h, w, false, false);
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    assert!(zoomed(h.state(w)));
    disable(&mut h, w, false, true);
    h.cmd(Command::MinimizeOrRestore { window: w });
    h.settle();
    assert!(
        !zoomed(h.state(w)),
        "disabling zoom must not trap a zoomed window"
    );
}

#[test]
fn a_fixed_size_window_cannot_zoom_whatever_its_application_says() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::SetSizeHints {
        window: w,
        min: Some(Size::new(400.0, 300.0)),
        max: Some(Size::new(400.0, 300.0)),
        content: None,
    });
    h.settle();
    assert!(!h.wm.window(w).unwrap().can_zoom());
    h.cmd(Command::ToggleZoom { window: w });
    h.settle();
    assert!(!zoomed(h.state(w)));
}

#[test]
fn a_window_whose_minimize_is_disabled_is_still_hidden_with_its_application() {
    let mut h = H::new();
    let _other = h.map("b");
    let w = h.map("a");
    h.settle();
    disable(&mut h, w, true, true);
    h.cmd(Command::HideApp { app: h.app("a") });
    h.settle();
    assert!(!h.wm.window_on_screen(w));
}

#[test]
fn controls_disabled_before_the_window_maps_hold_from_the_start() {
    let mut h = H::new();
    let w = WindowId(1);
    h.cmd(Command::Map(MapParams {
        disabled: DisabledControls {
            minimize: true,
            zoom: false,
        },
        ..MapParams::new(w, "a", Rect::new(100.0, 100.0, 600.0, 400.0))
    }));
    h.settle();
    assert!(!h.wm.window(w).unwrap().can_minimize());
    h.cmd(Command::Minimize {
        window: w,
        target: None,
    });
    h.settle();
    assert!(!h.state(w).is_minimized());
}

#[test]
fn a_window_that_cannot_zoom_is_refused_fill_but_keeps_the_half_tiles() {
    let mut h = H::new();
    let w = h.map("a");
    h.settle();
    disable(&mut h, w, false, true);
    h.cmd(Command::Tile {
        window: w,
        kind: TileKind::Fill,
    });
    h.settle();
    assert!(!matches!(h.state(w), LifecycleState::Tiled(_)));

    h.cmd(Command::Tile {
        window: w,
        kind: TileKind::Left,
    });
    h.settle();
    assert_eq!(h.state(w), LifecycleState::Tiled(TileKind::Left));
}

#[test]
fn a_fixed_size_window_is_refused_fill() {
    let mut h = H::new();
    let w = h.map("a");
    h.cmd(Command::SetSizeHints {
        window: w,
        min: Some(Size::new(400.0, 300.0)),
        max: Some(Size::new(400.0, 300.0)),
        content: None,
    });
    h.settle();
    h.cmd(Command::Tile {
        window: w,
        kind: TileKind::Fill,
    });
    h.settle();
    assert!(!matches!(h.state(w), LifecycleState::Tiled(_)));
}
