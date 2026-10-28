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
