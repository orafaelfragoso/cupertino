//! Window kinds chosen when a window maps, and where dialogs open.

use crate::common;
use common::*;
use cupertino_wm::*;

fn map_with(
    h: &mut H,
    id: u64,
    app: &str,
    size: (f64, f64),
    parent: Option<WindowId>,
    kind: WindowKind,
    modal: bool,
) -> WindowId {
    let mut p = MapParams::new(WindowId(id), app, Rect::new(0.0, 0.0, size.0, size.1));
    p.parent = parent;
    p.modal = modal;
    p.kind = kind;
    h.cmd(Command::Map(p));
    WindowId(id)
}

fn kind(h: &H, w: WindowId) -> WindowKind {
    h.wm.window(w).unwrap().lifecycle.kind()
}

#[test]
fn a_modal_child_becomes_a_sheet_that_slides_down_from_the_title_bar() {
    let mut h = H::new();
    let parent = h.map_at("a", Rect::new(200.0, 100.0, 600.0, 400.0));
    let sheet = map_with(
        &mut h,
        50,
        "a",
        (300.0, 200.0),
        Some(parent),
        window_kind(true, false),
        true,
    );
    assert_eq!(kind(&h, sheet), WindowKind::Sheet);
    let rest = Rect::new(
        350.0,
        100.0 + h.wm.config().snap.titlebar_height,
        300.0,
        200.0,
    );
    assert_eq!(h.wm.window(sheet).unwrap().geometry(), rest);
    let start = h.wm.presentation(sheet, h.now).unwrap();
    assert!(
        rect_eq(start.frame, rest.translate(0.0, -32.0)) && start.opacity == 0.0,
        "{start:?}"
    );
    h.settle();
    assert!(rect_eq(h.frame(sheet), rest));
    assert_eq!(h.wm.key_window(), Some(sheet));
    assert!(h.wm.is_blocked(parent));
}

#[test]
fn a_sheet_hangs_below_the_toolbar_row_its_parent_names() {
    let mut h = H::new();
    let mut p = MapParams::new(WindowId(7), "a", Rect::new(200.0, 100.0, 600.0, 400.0));
    p.sheet_top = Some(52.0);
    h.cmd(Command::Map(p));
    let parent = WindowId(7);
    h.settle();
    let sheet = map_with(
        &mut h,
        50,
        "a",
        (300.0, 200.0),
        Some(parent),
        window_kind(true, false),
        true,
    );
    assert_eq!(kind(&h, sheet), WindowKind::Sheet);
    assert_eq!(
        h.wm.window(sheet).unwrap().geometry(),
        Rect::new(350.0, 152.0, 300.0, 200.0),
        "below the parent's 52 pt row, not a 32 pt title bar"
    );
}

#[test]
fn without_sheets_a_modal_child_keeps_the_app_modal_transforms_and_centres_on_its_parent() {
    let mut cfg = WmConfig::default();
    cfg.focus.place_modal_children_as_sheets = false;
    let mut h = H::with_config(cfg);
    let parent = h.map_at("a", Rect::new(200.0, 100.0, 600.0, 400.0));
    let dialog = map_with(
        &mut h,
        50,
        "a",
        (300.0, 200.0),
        Some(parent),
        window_kind(true, false),
        true,
    );
    assert_eq!(kind(&h, dialog), WindowKind::AppModal);
    assert_eq!(
        h.wm.window(dialog).unwrap().geometry(),
        Rect::new(350.0, 200.0, 300.0, 200.0)
    );
    assert_eq!(
        h.wm.presentation(dialog, h.now).unwrap().scale,
        0.07,
        "pops in from small"
    );
    assert!(
        h.wm.is_blocked(parent),
        "a modal child still blocks its parent"
    );
}

#[test]
fn a_plain_child_grows_in_from_0_98_centred_on_its_parent() {
    let mut h = H::new();
    let parent = h.map_at("a", Rect::new(200.0, 100.0, 600.0, 400.0));
    let child = map_with(
        &mut h,
        50,
        "a",
        (300.0, 200.0),
        Some(parent),
        window_kind(false, false),
        false,
    );
    assert_eq!(kind(&h, child), WindowKind::Plain);
    let p = h.wm.presentation(child, h.now).unwrap();
    assert_eq!(
        (p.frame, p.opacity, p.scale),
        (Rect::new(350.0, 200.0, 300.0, 200.0), 1.0, 0.98)
    );
    h.advance(0.16);
    let p = h.wm.presentation(child, h.now).unwrap();
    assert_eq!((p.opacity, p.scale), (1.0, 1.0));
    assert!(!h.wm.is_blocked(parent));
}

#[test]
fn modal_panels_without_a_parent_centre_high_and_cascade() {
    let mut h = H::new();
    let first = map_with(
        &mut h,
        50,
        "a",
        (260.0, 156.0),
        None,
        window_kind(true, false),
        true,
    );
    // usable 1920 x 1056 at y 24: x = (1920 - 260) / 2, y = 24 + (1056 - 156) / 4
    assert_eq!(
        h.wm.window(first).unwrap().geometry(),
        Rect::new(830.0, 249.0, 260.0, 156.0)
    );
    let second = map_with(
        &mut h,
        51,
        "a",
        (260.0, 156.0),
        None,
        window_kind(true, false),
        true,
    );
    assert_eq!(
        h.wm.window(second).unwrap().geometry(),
        Rect::new(896.0, 315.0, 260.0, 156.0)
    );
    let other_app = map_with(
        &mut h,
        52,
        "b",
        (260.0, 156.0),
        None,
        window_kind(true, false),
        true,
    );
    assert_eq!(
        h.wm.window(other_app).unwrap().geometry(),
        Rect::new(830.0, 249.0, 260.0, 156.0),
        "another app's panel does not cascade"
    );
}

#[test]
fn a_utility_window_opens_at_once_and_fades_out() {
    let mut h = H::new();
    let w = map_with(
        &mut h,
        50,
        "a",
        (300.0, 200.0),
        None,
        window_kind(false, true),
        false,
    );
    h.settle();
    assert_eq!(kind(&h, w), WindowKind::Utility);
    h.cmd(Command::Unmap { window: w });
    h.advance(0.075);
    let p = h.wm.presentation(w, h.now).unwrap();
    assert!((p.opacity - 0.5).abs() < 0.05, "{p:?}");
}

#[test]
fn a_utility_panel_floats_above_normal_windows_and_a_modal_panel_above_both() {
    let mut h = H::new();
    let normal = h.map_at("a", Rect::new(0.0, 100.0, 300.0, 200.0));
    let utility = map_with(
        &mut h,
        50,
        "a",
        (300.0, 200.0),
        None,
        window_kind(false, true),
        false,
    );
    let modal = map_with(
        &mut h,
        51,
        "b",
        (300.0, 200.0),
        None,
        window_kind(true, false),
        true,
    );
    assert_eq!(h.wm.window(utility).unwrap().level.value(), 3);
    assert_eq!(h.wm.window(modal).unwrap().level.value(), 8);
    h.cmd(Command::FocusWindow { window: normal });
    assert_eq!(
        h.wm.stacking_order(),
        &[normal, utility, modal],
        "raising a normal window keeps it below the panels"
    );
    let later = h.map_at("c", Rect::new(0.0, 100.0, 300.0, 200.0));
    assert_eq!(h.wm.stacking_order(), &[normal, later, utility, modal]);
    let sheet = map_with(
        &mut h,
        52,
        "c",
        (100.0, 100.0),
        Some(later),
        window_kind(true, false),
        true,
    );
    assert_eq!(
        h.wm.window(sheet).unwrap().level,
        WindowLevel::Normal,
        "a sheet takes its parent's level"
    );
    assert_eq!(
        h.wm.stacking_order(),
        &[normal, later, sheet, utility, modal]
    );
}

#[test]
fn a_sheet_dims_its_parent_as_far_as_it_is_shown() {
    let mut h = H::new();
    let parent = h.map_at("a", Rect::new(200.0, 100.0, 600.0, 400.0));
    assert_eq!(h.wm.sheet_dimming(parent, h.now), 0.0);
    let sheet = map_with(
        &mut h,
        50,
        "a",
        (300.0, 200.0),
        Some(parent),
        window_kind(true, false),
        true,
    );
    assert_eq!(h.wm.sheet_dimming(parent, h.now), 0.0, "the slide starts");
    h.advance(0.13);
    assert!(
        (h.wm.sheet_dimming(parent, h.now) - 0.5).abs() < 0.02,
        "linear progress over 0.26 s"
    );
    h.settle();
    assert_eq!(h.wm.sheet_dimming(parent, h.now), 1.0);
    h.cmd(Command::Unmap { window: sheet });
    h.advance(0.065);
    assert!(
        (h.wm.sheet_dimming(parent, h.now) - 0.75).abs() < 0.02,
        "1 - p while it closes"
    );
    h.settle();
    assert_eq!(h.wm.sheet_dimming(parent, h.now), 0.0);
}
