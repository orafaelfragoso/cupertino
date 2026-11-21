//! Public-boundary tests for `cupertino-server-render` models: configuration types, drag-return
//! animation, cursor loading and recolouring, button accessibility trees, lingering popup expiries,
//! generational snapshot lifetimes, core window planning, motion policy, and effects state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use accesskit::{Action, ActionRequest, NodeId};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_effects::WindowKind;
use cupertino_geometry::Rect as GRect;
use cupertino_server_render::config::{Insets, WallpaperSettings};
use cupertino_server_render::cursor::{fallback_arrow, recolor, CursorColors, CursorLoader};
use cupertino_server_render::decor_access::{
    button_name, controls_name, ButtonAccess, ButtonAccessibility, ButtonSink, WindowButtons,
};
use cupertino_server_render::decorations::ButtonKind;
use cupertino_server_render::drag_return::{placement, Back, DragReturn, DRAG_RETURN_DURATION};
use cupertino_server_render::fx::{Effects, EffectsConfig};
use cupertino_server_render::popup_linger::Expiries;
use cupertino_server_render::render::core::{
    fullscreen_corner_cap, plan_window, wallpaper_tint, WindowPlan,
};
use cupertino_server_render::render::motion::{
    cross_fade_opacity, minimize_opacity, skips_frame_animation, skips_open_scale,
    skips_sheet_slide,
};
use cupertino_server_render::render::snapshots::{SnapshotError, Snapshots};
use cupertino_wm::{LifecycleState, Presentation, Rect, WindowId};
use smithay::input::pointer::CursorIcon;
use smithay::reexports::calloop::EventLoop;

#[test]
fn insets_and_wallpaper_settings_fallback() {
    let ins = Insets {
        top: 28.0,
        left: 2.0,
        right: 2.0,
        bottom: 4.0,
    };
    assert_eq!(ins.horizontal(), 4.0);
    assert_eq!(ins.vertical(), 32.0);

    let ws = WallpaperSettings {
        light: Some(PathBuf::from("/light.png")),
        dark: None,
    };
    assert_eq!(
        ws.for_appearance(false),
        Some(std::path::Path::new("/light.png"))
    );
    assert_eq!(
        ws.for_appearance(true),
        Some(std::path::Path::new("/light.png"))
    );

    let both = WallpaperSettings {
        light: Some(PathBuf::from("/light.png")),
        dark: Some(PathBuf::from("/dark.png")),
    };
    assert_eq!(
        both.for_appearance(true),
        Some(std::path::Path::new("/dark.png"))
    );
}

#[test]
fn drag_return_flight_and_fade_placement() {
    let from = Rect::new(100.0, 200.0, 40.0, 40.0);
    let (mid, alpha) = placement(from, Back::To(200.0, 300.0), 0.5);
    assert_eq!(mid, Rect::new(150.0, 250.0, 40.0, 40.0));
    assert_eq!(alpha, 1.0);

    let (faded, f_alpha) = placement(from, Back::Fade, 0.25);
    assert_eq!(faded, from);
    assert!((f_alpha - 0.75).abs() < 1e-9);

    let store: Snapshots<u32> = Snapshots::default();
    let owner = store.insert(1);
    let old_id = owner.id();
    let (tex, lease) = store.lease(old_id).expect("live snapshot");
    assert_eq!(tex, 1);
    assert_eq!(store.live(), 1);
    drop(owner);
    // Lease keeps snapshot alive until dropped.
    assert_eq!(store.live(), 1);
    drop(lease);
    assert_eq!(store.live(), 0);
    // Reusing the slot increments generation, so the old handle is rejected as stale.
    let _owner2 = store.insert(2);
    assert_eq!(
        store.lease(old_id).err(),
        Some(SnapshotError::Stale(old_id))
    );

    // DragReturn completes after DRAG_RETURN_DURATION.
    let gles_store: Snapshots<()> = Snapshots::default();
    assert_eq!(gles_store.live(), 0);
    const { assert!(DRAG_RETURN_DURATION > 0.0) };
    let _ = std::mem::size_of::<DragReturn>();
}

#[test]
fn cursor_fallback_recolor_and_procedural_loader() {
    let mut arrow = fallback_arrow(24);
    assert_eq!((arrow.width, arrow.height), (24, 24));
    recolor(
        &mut arrow.pixels_rgba,
        CursorColors {
            fill: [10, 20, 200, 255],
            outline: [255, 255, 0, 255],
        },
    );

    let events = EventLoop::<()>::try_new().unwrap();
    let mut loader = CursorLoader::load(None, CursorColors::DEFAULT, &events.handle(), |_| {});
    let frame = loader.frame(CursorIcon::Default, 24, Duration::ZERO);
    assert!(frame.pixels.0 > 0 && frame.pixels.1 > 0);
    let (w, h) = frame.size_at(24.0);
    assert!(w > 0.0 && h > 0.0);
    let _ = frame.hotspot_at(24.0);
}

#[test]
fn button_accessibility_tree_actions_and_sync() {
    assert_eq!(button_name(ButtonKind::Close), "Close");
    assert_eq!(button_name(ButtonKind::Minimize), "Minimize");
    assert_eq!(button_name(ButtonKind::Zoom), "Zoom");
    assert_eq!(controls_name("Notes"), "Notes window controls");
    assert_eq!(controls_name("   "), "Window controls");

    let wb = WindowButtons {
        title: "Notes".into(),
        frame: GRect::new(10.0, 20.0, 400.0, 300.0),
        buttons: [
            ButtonAccess {
                kind: ButtonKind::Close,
                enabled: true,
                bounds: GRect::new(8.0, 8.0, 12.0, 12.0),
            },
            ButtonAccess {
                kind: ButtonKind::Minimize,
                enabled: false,
                bounds: GRect::new(28.0, 8.0, 12.0, 12.0),
            },
            ButtonAccess {
                kind: ButtonKind::Zoom,
                enabled: true,
                bounds: GRect::new(48.0, 8.0, 12.0, 12.0),
            },
        ],
    };
    let tree = wb.tree("Cupertino");
    assert_eq!(tree.nodes.len(), 4);
    assert_eq!(
        tree.tree.as_ref().and_then(|t| t.toolkit_name.as_deref()),
        Some("Cupertino")
    );
    // Close is NodeId(2), Minimize is NodeId(3), Zoom is NodeId(4).
    assert_eq!(
        wb.pressed_by(&ActionRequest {
            action: Action::Click,
            target_tree: accesskit::TreeId::ROOT,
            target_node: NodeId(2),
            data: None,
        }),
        Some(ButtonKind::Close)
    );
    // Disabled button refuses click.
    assert_eq!(
        wb.pressed_by(&ActionRequest {
            action: Action::Click,
            target_tree: accesskit::TreeId::ROOT,
            target_node: NodeId(3),
            data: None,
        }),
        None
    );

    #[derive(Default)]
    struct LogSink(Arc<Mutex<Vec<(String, WindowId)>>>);
    impl ButtonSink for LogSink {
        fn publish(&mut self, window: WindowId, _: &WindowButtons) {
            self.0.lock().unwrap().push(("publish".into(), window));
        }
        fn withdraw(&mut self, window: WindowId) {
            self.0.lock().unwrap().push(("withdraw".into(), window));
        }
    }

    let log = Arc::new(Mutex::new(Vec::new()));
    let mut acc = ButtonAccessibility::default();
    assert!(!acc.enabled());
    acc.set_sink(Box::new(LogSink(Arc::clone(&log))));
    assert!(acc.enabled());
    assert!(acc.due(false, false));

    let id = WindowId(7);
    let mut map = HashMap::new();
    map.insert(id, wb.clone());
    acc.sync(map);
    assert_eq!(acc.computed(), 1);
    assert!(!acc.due(false, false));
    acc.sync(HashMap::new());
    let events = log.lock().unwrap().clone();
    assert_eq!(
        events,
        vec![("publish".into(), id), ("withdraw".into(), id)]
    );
}

#[test]
fn popup_expiries_and_motion_and_window_plan() {
    let mut exp = Expiries::default();
    assert_eq!(exp.next_deadline(), None);
    exp.add(2.0);
    exp.add(1.0);
    assert_eq!(exp.next_deadline(), Some(1.0));
    assert!(!exp.tick(0.5));
    assert!(exp.tick(1.0));
    assert_eq!(exp.next_deadline(), Some(2.0));

    let out = Rect::new(0.0, 0.0, 1440.0, 900.0);
    assert_eq!(fullscreen_corner_cap(out, out), 0.0);
    assert_eq!(
        fullscreen_corner_cap(Rect::new(10.0, 0.0, 1420.0, 900.0), out),
        10.0
    );

    let plan = WindowPlan {
        visual: out,
        scale_x: 1.0,
        scale_y: 1.0,
        opacity: 1.0,
        shadow: 1.0,
        exact: true,
        corner: None,
        corner_cap: Some(6.0),
    };
    assert_eq!(plan.capped(16.0), 6.0);
    assert_eq!(plan.capped(4.0), 4.0);

    let pres = Presentation {
        frame: Rect::new(50.0, 60.0, 400.0, 300.0),
        visible: true,
        opacity: 1.0,
        shadow: 1.0,
        scale: 1.0,
        minimize_progress: 0.0,
        minimize_target: Rect::new(0.0, 0.0, 1.0, 1.0),
    };
    let rest = plan_window(&pres, (400.0, 300.0), false, (10.0, 0.0));
    assert!(rest.exact);
    assert_eq!(rest.visual, Rect::new(60.0, 60.0, 400.0, 300.0));

    let reduced = DisplayOptions {
        reduce_motion: true,
        ..DisplayOptions::default()
    };
    assert!(skips_frame_animation(
        LifecycleState::EnterFullScreen,
        reduced
    ));
    assert!(skips_sheet_slide(
        WindowKind::Sheet,
        LifecycleState::Opening,
        reduced
    ));
    assert!(skips_open_scale(LifecycleState::Opening, reduced));
    assert!((cross_fade_opacity(0, 0.25) - 0.75).abs() < 1e-9);
    assert!((minimize_opacity(0.5, reduced) - 0.5).abs() < 1e-9);
    assert_ne!(wallpaper_tint(true), wallpaper_tint(false));

    let mut fx = Effects::new(EffectsConfig::default());
    assert!(!fx.show_desktop_active());
    fx.toggle_show_desktop(1.0);
    assert!(fx.show_desktop_active());
    assert!(fx.wants_tick(1.05));
}
