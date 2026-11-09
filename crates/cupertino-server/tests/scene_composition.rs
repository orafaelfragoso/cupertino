//! What the compositor composes into the neutral frame of an output, before any renderer lowers it.

mod common;

use common::desktop::Desktop;
use cupertino_scene::{SceneKind, SceneLayer};
use cupertino_server::render::scene::{compose_scene, LockView};

fn composed(d: &mut Desktop) -> Vec<(SceneLayer, SceneKind)> {
    let mut backend = d.state.backend.take().unwrap();
    let items = d
        .state
        .with_scene(backend.renderer(), 0, |r, mut p| {
            compose_scene(r, &mut p)
                .frame
                .items()
                .iter()
                .map(|(layer, item)| (*layer, item.id.kind))
                .collect()
        })
        .expect("render resources");
    d.state.backend = Some(backend);
    items
}

fn layers(items: &[(SceneLayer, SceneKind)]) -> Vec<SceneLayer> {
    let mut layers: Vec<SceneLayer> = items.iter().map(|(layer, _)| *layer).collect();
    layers.dedup();
    layers
}

#[test]
fn a_desktop_frame_stacks_wallpaper_windows_and_cursor_without_session_overlays() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let items = composed(&mut d);
    let layers = layers(&items);
    assert_eq!(layers.first(), Some(&SceneLayer::Cursor), "front to back");
    assert_eq!(layers.last(), Some(&SceneLayer::Wallpaper));
    assert!(layers.contains(&SceneLayer::Windows));
    assert!(!layers.contains(&SceneLayer::SessionOverlay));
    assert!(items.contains(&(SceneLayer::Windows, SceneKind::WindowContent)));
}

#[test]
fn a_locked_frame_has_no_desktop_sources_and_keeps_the_session_overlay() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let _locker = d.lock();
    d.state.session_activated();
    d.at(3050);
    let items = composed(&mut d);
    for (layer, kind) in &items {
        assert!(
            matches!(
                layer,
                SceneLayer::Wallpaper
                    | SceneLayer::LayerOverlay
                    | SceneLayer::DisplayFilter
                    | SceneLayer::Cursor
                    | SceneLayer::SessionOverlay
            ),
            "{kind:?} on {layer:?} must not show over a lock"
        );
        assert!(
            !matches!(
                kind,
                SceneKind::WindowContent | SceneKind::TitleBar | SceneKind::Rim | SceneKind::Shadow
            ),
            "{kind:?} is a desktop source"
        );
    }
    assert_eq!(
        items.first(),
        Some(&(SceneLayer::SessionOverlay, SceneKind::ActivationFade))
    );
    assert!(items.contains(&(SceneLayer::LayerOverlay, SceneKind::LockContent)));
    assert!(!items
        .iter()
        .any(|(layer, _)| *layer == SceneLayer::CompositorOverlay));
}

#[test]
fn a_locked_output_without_a_lock_surface_shows_only_black_and_the_pointer() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let mut backend = d.state.backend.take().unwrap();
    let items: Vec<_> = d
        .state
        .with_scene(backend.renderer(), 0, |r, mut p| {
            p.lock = LockView::Locked { surface: None };
            compose_scene(r, &mut p)
                .frame
                .items()
                .iter()
                .map(|(layer, item)| (*layer, item.id.kind))
                .collect()
        })
        .expect("render resources");
    d.state.backend = Some(backend);
    assert!(items.contains(&(SceneLayer::Wallpaper, SceneKind::LockBackdrop)));
    assert!(items
        .iter()
        .all(|(layer, _)| matches!(layer, SceneLayer::Wallpaper | SceneLayer::Cursor)));
}

/// What a screen capture of a locked output holds: the pixels of the entry point the capture copy uses, with and
/// without the pointer. No pixel of a window may be in them, only the lock surface and black.
#[test]
fn a_capture_of_a_locked_output_holds_only_the_lock_surface_and_black() {
    use common::{LOCK_GREEN, TOPLEVEL_BLUE};
    use cupertino_server::backend::Backend;
    use cupertino_server::render::screenshot::render_to_image;

    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let _locker = d.lock();
    d.at(3100);
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    for paint_cursor in [true, false] {
        let elements = d
            .state
            .scene_elements(&mut h.renderer, 0, paint_cursor)
            .expect("render resources");
        let image = render_to_image(
            &mut h.renderer,
            &elements,
            (1000, 700).into(),
            1.0,
            cupertino_server::backend::headless::CLEAR,
        )
        .unwrap();
        let count = |colour: [u8; 4]| {
            image
                .pixels()
                .filter(|p| p.0[0] == colour[2] && p.0[1] == colour[1] && p.0[2] == colour[0])
                .count()
        };
        assert_eq!(
            count(TOPLEVEL_BLUE),
            0,
            "no window pixel, cursor {paint_cursor}"
        );
        assert!(
            count(LOCK_GREEN) > 500_000,
            "the lock surface covers the output"
        );
    }
    d.state.backend = Some(backend);
}

/// What the windows layer holds for the window: the clip of its content, and whether it has a shadow, a title bar and a
/// rim.
#[derive(Debug)]
struct Framed {
    clip: cupertino_scene::Clip,
    shadow: bool,
    /// The fine outline just outside the frame.
    outline: bool,
    titlebar: bool,
    rim: bool,
}

fn framed(d: &mut Desktop) -> Framed {
    let mut backend = d.state.backend.take().unwrap();
    let items: Vec<(SceneKind, cupertino_scene::Clip)> = d
        .state
        .with_scene(backend.renderer(), 0, |r, mut p| {
            compose_scene(r, &mut p)
                .frame
                .items()
                .iter()
                .filter(|(layer, _)| *layer == SceneLayer::Windows)
                .map(|(_, item)| (item.id.kind, item.clip))
                .collect()
        })
        .expect("render resources");
    d.state.backend = Some(backend);
    let has = |kind: SceneKind| items.iter().any(|(k, _)| *k == kind);
    Framed {
        clip: items
            .iter()
            .find(|(k, _)| *k == SceneKind::WindowContent)
            .map(|(_, clip)| *clip)
            .expect("the window's content"),
        shadow: has(SceneKind::Shadow),
        outline: has(SceneKind::Outline),
        titlebar: has(SceneKind::TitleBar),
        rim: has(SceneKind::Rim),
    }
}

fn rounded(clip: &cupertino_scene::Clip) -> bool {
    matches!(clip, cupertino_scene::Clip::Rounded { radius, .. } if *radius > 0.0)
}

/// A desktop with one window of a client that asks for `mode` decorations (none: it never binds the protocol), opened
/// and settled.
fn desktop_with(
    mode: Option<
        wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode,
    >,
) -> (Desktop, cupertino_wm::WindowId) {
    let mut d = Desktop::new(1.0);
    match mode {
        None => d.window(),
        Some(mode) => {
            let client = common::Client::decorated(d.socket(), mode);
            d.clients.push(client);
            let deadline = std::time::Instant::now() + common::HANG_GUARD;
            while d.state.wm.windows().count() != 1 {
                d.pump();
                assert!(
                    std::time::Instant::now() < deadline,
                    "the client did not map"
                );
            }
        }
    }
    d.at(3000);
    let id = d.state.wm.key_window().expect("the window is key");
    (d, id)
}

/// The window's frame in each state the corner policy names: normal, zoomed, filling the usable area and full screen.
fn frames_by_state(d: &mut Desktop, id: cupertino_wm::WindowId) -> [(&'static str, Framed); 4] {
    use cupertino_wm::{Command, Requester, TileKind};
    let normal = framed(d);
    d.state.command(Command::ToggleZoom { window: id });
    d.at(6000);
    assert!(
        d.state
            .wm
            .window(id)
            .unwrap()
            .lifecycle
            .toplevel_states()
            .maximized
    );
    let zoomed = framed(d);
    d.state.command(Command::ToggleZoom { window: id });
    d.at(9000);
    d.state.command(Command::Tile {
        window: id,
        kind: TileKind::Fill,
    });
    d.at(12000);
    assert_eq!(
        d.state.wm.window(id).unwrap().state(),
        cupertino_wm::LifecycleState::Tiled(TileKind::Fill)
    );
    let filled = framed(d);
    d.state.command(Command::Fullscreen {
        window: id,
        output: None,
        requested_by: Requester::User,
    });
    d.at(15000);
    assert_eq!(
        d.state.wm.window(id).unwrap().state(),
        cupertino_wm::LifecycleState::FullScreen
    );
    let full_screen = framed(d);
    [
        ("normal", normal),
        ("zoomed", zoomed),
        ("filled", filled),
        ("full screen", full_screen),
    ]
}

#[test]
fn a_zoomed_or_filled_window_keeps_its_rounded_corners_and_shadow_and_a_full_screen_one_is_square()
{
    use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode;
    // the compositor's decorations, and a client that never said whether it draws its own
    for mode in [Some(Mode::ServerSide), None] {
        let (mut d, id) = desktop_with(mode);
        for (state, frame) in frames_by_state(&mut d, id) {
            let decorated = mode.is_some();
            if state == "full screen" {
                assert!(!rounded(&frame.clip), "{mode:?} {state}: {frame:?}");
                assert!(
                    !(frame.shadow || frame.outline || frame.titlebar || frame.rim),
                    "{mode:?} {state}: {frame:?}"
                );
            } else {
                assert!(rounded(&frame.clip), "{mode:?} {state}: {frame:?}");
                assert!(frame.shadow && frame.outline, "{mode:?} {state}: {frame:?}");
                assert_eq!(
                    (frame.titlebar, frame.rim),
                    (decorated, decorated),
                    "{mode:?} {state}"
                );
            }
        }
    }
}

#[test]
fn a_window_whose_client_draws_its_own_decorations_keeps_its_own_shape_in_every_state() {
    use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode;
    let (mut d, id) = desktop_with(Some(Mode::ClientSide));
    assert!(d.state.entry(id).unwrap().draws_own_frame());
    for (state, frame) in frames_by_state(&mut d, id) {
        assert!(!rounded(&frame.clip), "{state}: {frame:?}");
        assert!(
            !(frame.shadow || frame.outline || frame.titlebar || frame.rim),
            "{state}: {frame:?}"
        );
    }
}

/// At a fractional scale a window's frame need not lie on device pixels. The content's rounded mask covers whole device
/// pixels (its edges rounded), and the outline lies just outside those pixels, so it meets the content wherever the
/// window is, without a gap or an overlap that would make it softer or weaker.
#[test]
fn the_outline_meets_the_device_pixels_of_the_window_s_content_at_a_fractional_scale() {
    let mut fractional = 0;
    for scale in [1.25, 1.5, 1.75] {
        let mut d = Desktop::new(scale);
        d.window();
        d.at(3000);
        let mut backend = d.state.backend.take().unwrap();
        let (clip, outline) = d
            .state
            .with_scene(backend.renderer(), 0, |r, mut p| {
                let frame = compose_scene(r, &mut p).frame;
                let items = frame.items();
                let windows = || {
                    items
                        .iter()
                        .filter(|(layer, _)| *layer == SceneLayer::Windows)
                        .map(|(_, item)| item)
                };
                let clip = windows()
                    .find(|item| item.id.kind == SceneKind::WindowContent)
                    .map(|item| item.clip)
                    .expect("the window's content");
                let outline = windows()
                    .find(|item| item.id.kind == SceneKind::Outline)
                    .map(|item| item.source.clone())
                    .expect("the window's outline");
                (clip, outline)
            })
            .expect("render resources");
        d.state.backend = Some(backend);
        let cupertino_scene::Clip::Rounded { rect: mask, .. } = clip else {
            panic!("a rounded mask: {clip:?}");
        };
        let cupertino_scene::Source::RoundedRect(ring) = outline else {
            panic!("the outline is a rounded rectangle: {outline:?}");
        };
        let (width, _) = ring.border.expect("the outline is a border");
        let edges = |r: &cupertino_geometry::Rect, inset: f64| {
            [
                r.origin.x + inset,
                r.origin.y + inset,
                r.origin.x + r.size.width - inset,
                r.origin.y + r.size.height - inset,
            ]
        };
        let content = edges(&mask.0, 0.0);
        if content
            .iter()
            .any(|v| ((v * scale) - (v * scale).round()).abs() > 1e-6)
        {
            fractional += 1;
        }
        let inner = edges(&ring.rect.0, width);
        for (got, mask) in inner.iter().zip(content) {
            let pixel = (mask * scale).round();
            assert!(
                (got * scale - pixel).abs() < 1e-6,
                "at {scale}x the outline's inner edge {got} is not the mask's device pixel edge {pixel} ({mask})"
            );
        }
    }
    assert!(
        fractional > 0,
        "some scale must put the frame off the device pixels for the test to mean anything"
    );
}
