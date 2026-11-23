//! A surface layer's look reaches the compositor as the layer's requests: its glass with the look's lens, within the
//! protocol's limits, and its blur ramp, each sent again only when it changes.

use crate::wire;

use cupertino_geometry::{Rect, Size};
use cupertino_keyboard::modifiers::Profile;
use cupertino_ui_core::platform::{
    BlurRamp, DecorationMode, GlassStyle, LayerList, PlatformEvent, RaisedChange, Submission,
    Submitted, SubmittedLayer, SurfaceLayerLook, WindowEvent, WindowId, WindowSpec, WindowSystem,
    MAX_LAYER_LENS,
};
use cupertino_ui_render::{CpuConfig, CpuRenderer, DamageRegion};
use cupertino_ui_text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_ui_wayland::WaylandWindowSystem;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wire::{Harness, Offer};

/// A frame of a window 32 points square with one surface layer of `look`.
fn frame(look: SurfaceLayerLook) -> Submission {
    let text = Arc::new(TextSystem::new(FontIndex::new(), ""));
    let renderer = || {
        CpuRenderer::new(
            CpuConfig {
                max_pixels: 4096,
                image_cache_bytes: 0,
                glyph_cache_bytes: 0,
                font_smoothing: false,
            },
            GlyphRaster::new(text.clone()),
        )
    };
    Submission {
        renderer: Arc::new(Mutex::new(Box::new(renderer()))),
        list: Default::default(),
        resources: Default::default(),
        scale: 1.0,
        width: 32,
        height: 32,
        damage: DamageRegion::Full,
        raised: RaisedChange::Kept,
        surface_layers: Some(vec![SubmittedLayer {
            key: 1,
            rect: Rect::new(2.0, 2.0, 8.0, 8.0),
            look,
            layer: Some(LayerList {
                renderer: Arc::new(Mutex::new(Box::new(renderer()))),
                list: Default::default(),
                width: 8,
                height: 8,
                damage: DamageRegion::Full,
            }),
        }]),
    }
}

/// Submits `look`'s frame and waits until it is shown; the glass and blur ramp requests the server received since.
fn show(
    server: &Harness,
    system: &mut WaylandWindowSystem,
    window: WindowId,
    look: SurfaceLayerLook,
) -> Vec<String> {
    let before = server.requests().len();
    assert_eq!(
        system.submit(window, &frame(look)).unwrap(),
        Submitted::Taken
    );
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
        .filter(|r| r.starts_with("SetGlass {") || r.starts_with("SetBlurRamp"))
        .cloned()
        .collect()
}

#[test]
fn a_layers_lens_and_blur_ramp_go_out_with_its_glass_and_again_only_when_they_change() {
    let (server, conn) = Harness::start(Offer {
        layers: Some(6),
        releases: true,
        frames: true,
        ..Offer::NONE
    });
    let mut system = WaylandWindowSystem::new(conn, Profile::cupertino());
    let window = system
        .create_window(&WindowSpec {
            app_id: "org.cupertino.tests.LayerLook".into(),
            title: "Window".into(),
            size: Size::new(32.0, 32.0),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        })
        .unwrap();
    let lens = SurfaceLayerLook {
        glass: Some(GlassStyle::Track),
        lens: 1.5,
        radius: 4.0,
        ..SurfaceLayerLook::default()
    };
    // the first frame waits for the configure, its layer made at rest without glass
    let bare = frame(SurfaceLayerLook::default());
    assert_eq!(system.submit(window, &bare).unwrap(), Submitted::NotReady);
    system.connection().roundtrip().unwrap();
    system.dispatch(Some(Duration::ZERO), &mut vec![]).unwrap();

    let sent = show(&server, &mut system, window, lens);
    let [glass] = sent.as_slice() else {
        panic!("one glass request, no ramp while it blurs evenly: {sent:?}")
    };
    assert!(glass.ends_with("lens: 1.5 }"), "{glass}");
    assert!(!glass.contains("4294967295"), "the glass is drawn: {glass}");
    assert!(
        show(&server, &mut system, window, lens).is_empty(),
        "an unchanged look sends nothing"
    );

    // a lens past the compositor's limits is sent at the nearest of them
    for (asked, wire) in [
        (0.5, 1.0),
        (MAX_LAYER_LENS + 2.0, MAX_LAYER_LENS),
        (f64::NAN, 1.0),
    ] {
        let sent = show(
            &server,
            &mut system,
            window,
            SurfaceLayerLook {
                lens: asked,
                ..lens
            },
        );
        let [glass] = sent.as_slice() else {
            panic!("lens {asked}: {sent:?}")
        };
        assert!(
            glass.ends_with(&format!("lens: {wire:?} }}")),
            "lens {asked}: {glass}"
        );
    }

    let pocket = SurfaceLayerLook {
        glass: Some(GlassStyle::Regular),
        blur_ramp: BlurRamp {
            top: 1.0,
            bottom: 0.0,
        },
        ..SurfaceLayerLook::default()
    };
    let sent = show(&server, &mut system, window, pocket);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[0].ends_with("lens: 1.0 }"), "{}", sent[0]);
    assert_eq!(sent[1], "SetBlurRamp { top: 1.0, bottom: 0.0 }");

    // weights outside 0 to 1 are sent at the nearest of them
    let sent = show(
        &server,
        &mut system,
        window,
        SurfaceLayerLook {
            blur_ramp: BlurRamp {
                top: 2.0,
                bottom: -1.0,
            },
            ..pocket
        },
    );
    assert_eq!(sent, ["SetBlurRamp { top: 1.0, bottom: 0.0 }"]);
}
