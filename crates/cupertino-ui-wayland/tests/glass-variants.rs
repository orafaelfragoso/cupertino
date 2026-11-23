//! Glass named by its system variant and tinted reaches the compositor: a window's glass region as
//! `cupertino_backdrop_v1.set_glass_variant` and `set_glass_tint` from version 23, a surface layer's look as
//! `cupertino_layer_v1.set_glass_variant` and `set_glass_tint` from version 7; an older compositor draws the region's
//! or the look's style, untinted.

use crate::wire;

use cupertino_geometry::{Rect, Size};
use cupertino_keyboard::modifiers::Profile;
use cupertino_ui_core::platform::{
    Backdrop, BlurRamp, DecorationMode, GlassModifiers, GlassRegion, GlassSize, GlassStyle,
    GlassVariant, MaterialRegion, PlatformEvent, Submitted, SurfaceLayerLook, WindowEvent,
    WindowId, WindowRequest, WindowSpec, WindowSystem,
};
use cupertino_ui_wayland::WaylandWindowSystem;
use std::time::{Duration, Instant};
use wire::{Harness, Offer};

fn spec() -> WindowSpec {
    WindowSpec {
        title: "Glass variants".into(),
        app_id: "org.cupertino.tests.GlassVariants".into(),
        size: Size::new(32.0, 32.0),
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: None,
    }
}

fn flush(system: &mut WaylandWindowSystem) {
    system
        .dispatch(Some(Duration::from_millis(20)), &mut Vec::new())
        .unwrap();
}

/// Interactive regular glass resolved at the medium size, tinted half red.
fn pressable() -> (GlassVariant, GlassModifiers) {
    (
        GlassVariant::Regular,
        GlassModifiers {
            size: GlassSize::Medium,
            interactive: true,
            ..GlassModifiers::DEFAULT
        },
    )
}

const HALF_RED: [f32; 4] = [1.0, 0.0, 0.0, 0.5];

/// One glass region of the toolbar style, named by `variant` and tinted `tint`.
fn region(variant: Option<(GlassVariant, GlassModifiers)>, tint: [f32; 4]) -> Vec<MaterialRegion> {
    vec![MaterialRegion {
        material: GlassStyle::Toolbar.material().into(),
        rect: Rect::new(0.0, 0.0, 20.0, 10.0),
        radius: 5.0,
        alpha: 1.0,
        glass: Some(GlassRegion {
            variant,
            tint,
            ..GlassRegion::of_style(GlassStyle::Toolbar, 1, 4.0)
        }),
        raised: false,
        blur_ramp: BlurRamp::EVEN,
    }]
}

fn glass_sent(server: &Harness) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter(|r| r.starts_with("SetGlass"))
        .collect()
}

#[test]
fn a_glass_regions_variant_and_tint_go_to_a_compositor_that_takes_them_and_its_style_to_an_older_one(
) {
    let (server, conn) = Harness::start(Offer {
        app_effects: Some(23),
        ..Offer::NONE
    });
    let mut system = WaylandWindowSystem::new(conn, Profile::cupertino());
    let window = system.create_window(&spec()).unwrap();
    flush(&mut system);
    system
        .request(
            window,
            WindowRequest::Materials(region(Some(pressable()), HALF_RED)),
        )
        .unwrap();
    flush(&mut system);
    let sent = server.wait_for("SetGlassVariant", 1);
    let sent: Vec<_> = sent
        .into_iter()
        .filter(|r| r.starts_with("SetGlass"))
        .collect();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(
        sent[0],
        "SetGlassTint { red: 1.0, green: 0.0, blue: 0.0, alpha: 0.5 }"
    );
    let variant = &sent[1];
    assert!(
        variant.starts_with("SetGlassVariant { variant: Value(Regular)")
            && variant.contains("Interactive")
            && variant.ends_with("size_class: Value(Medium), container: 1, spacing: 4.0 }"),
        "{variant}"
    );

    // a region of a style alone still goes as the style
    system
        .request(window, WindowRequest::Materials(region(None, [0.0; 4])))
        .unwrap();
    flush(&mut system);
    server.wait_for("SetGlass {", 1);
    assert_eq!(
        glass_sent(&server).last().unwrap(),
        "SetGlass { style: Value(Toolbar), container: 1, spacing: 4.0 }"
    );

    let (server, conn) = Harness::start(Offer {
        app_effects: Some(22),
        ..Offer::NONE
    });
    let mut system = WaylandWindowSystem::new(conn, Profile::cupertino());
    let window = system.create_window(&spec()).unwrap();
    flush(&mut system);
    system
        .request(
            window,
            WindowRequest::Materials(region(Some(pressable()), HALF_RED)),
        )
        .unwrap();
    flush(&mut system);
    server.wait_for("SetGlass {", 1);
    assert_eq!(
        glass_sent(&server),
        ["SetGlass { style: Value(Toolbar), container: 1, spacing: 4.0 }"],
        "version 22 draws the region's style, untinted"
    );
}

/// Submits a frame whose layer has `look`, waits until it is shown and returns the glass requests sent since.
fn show(
    server: &Harness,
    system: &mut WaylandWindowSystem,
    window: WindowId,
    look: SurfaceLayerLook,
) -> Vec<String> {
    let before = server.requests().len();
    let frame = crate::layer_look::frame(look);
    assert_eq!(system.submit(window, &frame).unwrap(), Submitted::Taken);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut events = Vec::new();
        system
            .dispatch(
                Some(deadline.saturating_duration_since(Instant::now())),
                &mut events,
            )
            .unwrap();
        if events.iter().any(|e| {
            matches!(e, PlatformEvent::Window { window: w, event: WindowEvent::Shown(_) } if *w == window)
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "the frame was not shown");
    }
    system.connection().roundtrip().unwrap();
    server.requests()[before..]
        .iter()
        .filter(|r| r.starts_with("SetGlass"))
        .cloned()
        .collect()
}

/// A window system on a compositor of layers `version`, a window and its first frame, its layer at rest without glass.
fn layered(version: u32) -> (Harness, WaylandWindowSystem, WindowId) {
    let (server, conn) = Harness::start(Offer {
        layers: Some(version),
        releases: true,
        frames: true,
        ..Offer::NONE
    });
    let mut system = WaylandWindowSystem::new(conn, Profile::cupertino());
    let window = system.create_window(&spec()).unwrap();
    let bare = crate::layer_look::frame(SurfaceLayerLook::default());
    assert_eq!(system.submit(window, &bare).unwrap(), Submitted::NotReady);
    system.connection().roundtrip().unwrap();
    system.dispatch(Some(Duration::ZERO), &mut vec![]).unwrap();
    (server, system, window)
}

#[test]
fn a_layers_variant_and_tint_go_out_from_version_7_and_its_style_before() {
    let look = SurfaceLayerLook {
        glass: Some(GlassStyle::Track),
        variant: Some(pressable()),
        tint: HALF_RED,
        radius: 4.0,
        ..SurfaceLayerLook::default()
    };
    let (server, mut system, window) = layered(7);
    let sent = show(&server, &mut system, window, look);
    // regular glass is 0, interactive is bit 4 and the medium size class 2
    assert_eq!(
        sent,
        [
            "SetGlassTint { red: 1.0, green: 0.0, blue: 0.0, alpha: 0.5 }",
            "SetGlassVariant { variant: 0, modifiers: 4, size_class: 2, lens: 1.0 }",
        ]
    );
    // the same look sends nothing again
    assert!(show(&server, &mut system, window, look).is_empty());

    let (server, mut system, window) = layered(6);
    let sent = show(&server, &mut system, window, look);
    // the track is wire style 7
    assert_eq!(
        sent,
        ["SetGlass { style: 7, lens: 1.0 }"],
        "version 6 draws the look's style, untinted"
    );
}

#[test]
fn a_window_backgrounds_glass_variant_goes_to_its_backdrop_and_a_fade_sends_only_the_alpha() {
    let (server, conn) = Harness::start(Offer {
        app_effects: Some(23),
        ..Offer::NONE
    });
    let mut system = WaylandWindowSystem::new(conn, Profile::cupertino());
    let window = system.create_window(&spec()).unwrap();
    flush(&mut system);
    let backdrop = |alpha| Backdrop {
        material: GlassStyle::Sidebar.material().into(),
        radius: 16.0,
        alpha,
        glass: Some(GlassRegion {
            variant: Some((GlassVariant::AbuttedSidebar, GlassModifiers::DEFAULT)),
            ..GlassRegion::of_style(GlassStyle::Sidebar, 0, 0.0)
        }),
    };
    system
        .request(window, WindowRequest::Backdrop(Some(backdrop(1.0))))
        .unwrap();
    flush(&mut system);
    server.wait_for("SetGlassVariant", 1);
    let sent = glass_sent(&server);
    assert!(
        sent.last()
            .unwrap()
            .starts_with("SetGlassVariant { variant: Value(AbuttedSidebar)"),
        "{sent:?}"
    );
    let before = server.requests().len();
    system
        .request(window, WindowRequest::Backdrop(Some(backdrop(0.5))))
        .unwrap();
    flush(&mut system);
    server.wait_for("SetAlpha", 2);
    let after: Vec<_> = server.requests().split_off(before);
    assert!(
        !after
            .iter()
            .any(|r| r.starts_with("SetGlass") || r.starts_with("GetBackdrop")),
        "{after:?}"
    );
}

#[test]
fn a_sheets_glass_region_goes_with_the_document_modal_bit() {
    let (server, conn) = Harness::start(Offer {
        app_effects: Some(23),
        ..Offer::NONE
    });
    let mut system = WaylandWindowSystem::new(conn, Profile::cupertino());
    let window = system.create_window(&spec()).unwrap();
    flush(&mut system);
    let mut regions = region(
        Some((GlassVariant::GlassWindow, GlassModifiers::DEFAULT)),
        [0.0; 4],
    );
    if let Some(g) = regions[0].glass.as_mut() {
        g.document_modal = true;
    }
    system
        .request(window, WindowRequest::Materials(regions))
        .unwrap();
    flush(&mut system);
    server.wait_for("SetGlassVariant", 1);
    let sent = glass_sent(&server);
    let variant = sent.last().unwrap();
    assert!(
        variant.starts_with("SetGlassVariant { variant: Value(GlassWindow)")
            && variant.contains("DocumentModal"),
        "{variant}"
    );
}

#[test]
fn a_glass_regions_scale_goes_on_its_own_as_it_changes() {
    let (server, conn) = Harness::start(Offer {
        app_effects: Some(23),
        ..Offer::NONE
    });
    let mut system = WaylandWindowSystem::new(conn, Profile::cupertino());
    let window = system.create_window(&spec()).unwrap();
    flush(&mut system);
    let scaled = |scale: f64| {
        let mut regions = region(Some(pressable()), HALF_RED);
        if let Some(g) = regions[0].glass.as_mut() {
            g.scale = scale;
        }
        regions
    };
    system
        .request(window, WindowRequest::Materials(scaled(1.0)))
        .unwrap();
    flush(&mut system);
    server.wait_for("SetGlassVariant", 1);
    assert!(
        !glass_sent(&server)
            .iter()
            .any(|r| r.starts_with("SetGlassScale")),
        "a region at its size sends no scale"
    );
    let before = glass_sent(&server).len();
    for scale in [1.02, 1.04] {
        system
            .request(window, WindowRequest::Materials(scaled(scale)))
            .unwrap();
        flush(&mut system);
    }
    server.wait_for("SetGlassScale", 2);
    let after = glass_sent(&server).split_off(before);
    assert_eq!(after.len(), 2, "only the scale goes again: {after:?}");
    assert!(
        after.iter().all(|r| r.starts_with("SetGlassScale")),
        "{after:?}"
    );
}
