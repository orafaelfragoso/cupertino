//! What a frame holds is released when the frame is: window snapshots stay in the store while a closing ghost or a
//! frame composed with them needs them, and go when the last of them goes.

mod common;

use common::desktop::Desktop;
use cupertino_render_api::Renderer;
use cupertino_server::backend::Backend;
use cupertino_server::render::gles::{Gles, GlesResources, GlesTarget, RenderError};

#[test]
fn gles_refuses_another_frames_resources_without_consuming_them() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let first = d.state.compose_output(&mut h.renderer, 0, true).unwrap();
    let second = d.state.compose_output(&mut h.renderer, 0, true).unwrap();
    let (res, cache, _) = d.state.gles_parts(0).unwrap();
    let mut resources = GlesResources::new(res, cache, first.resources);
    // Exercise the production entry point; this assertion also runs with debug assertions disabled.
    let result = Gles.render(
        &second.frame,
        &mut resources,
        GlesTarget::Image {
            renderer: &mut h.renderer,
            clear: cupertino_server::backend::headless::CLEAR,
        },
    );
    assert!(matches!(result, Err(RenderError::Resources(_))));
    // Rejection must leave the original frame usable, before any cache finalization or resource take.
    assert!(Gles::lower(&mut h.renderer, &first.frame, &mut resources).is_ok());
}

fn live(d: &Desktop) -> usize {
    d.state
        .render
        .as_ref()
        .expect("render resources")
        .snapshots
        .live()
}

/// A desktop whose only window closed into a ghost.
fn ghost() -> Desktop {
    let mut d = Desktop::new(1.0);
    d.state.fx.cfg.closing = true;
    d.window();
    d.at(3000);
    d.clients.clear();
    let deadline = std::time::Instant::now() + common::HANG_GUARD;
    while d.state.wm.windows().count() != 0 {
        d.pump();
        assert!(
            std::time::Instant::now() < deadline,
            "the window did not close"
        );
    }
    d
}

#[test]
fn a_closing_ghost_keeps_its_snapshot_until_it_is_done() {
    let mut d = ghost();
    assert_eq!(d.state.fx.closing.len(), 1, "a ghost");
    assert_eq!(live(&d), 1, "its snapshot is stored");
    let later = d.state.now() + 5.0;
    d.state.fixed_time = Some(later);
    d.state.tick();
    assert!(d.state.fx.closing.is_empty(), "the ghost is done");
    assert_eq!(live(&d), 0, "its snapshot is retired with it");
}

#[test]
fn a_composed_frame_keeps_the_snapshot_of_an_ended_ghost_until_it_is_dropped() {
    let mut d = ghost();
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let composed = d
        .state
        .compose_output(&mut h.renderer, 0, true)
        .expect("render resources");
    d.state.backend = Some(backend);
    assert_eq!(live(&d), 1);
    // the ghost ends while the frame composed with it has not been drawn yet
    let later = d.state.now() + 5.0;
    d.state.fixed_time = Some(later);
    d.state.tick();
    assert!(d.state.fx.closing.is_empty());
    assert_eq!(live(&d), 1, "the frame's lease keeps the snapshot");
    drop(composed);
    assert_eq!(live(&d), 0, "it is retired once the frame is gone");
}

#[test]
fn a_stored_window_snapshot_goes_when_the_window_is_forgotten() {
    let mut d = Desktop::new(1.0);
    d.state.fx.cfg.closing = true;
    d.window();
    d.at(3000);
    let id = d.state.wm.key_window().unwrap();
    // the window's lifecycle may have stored a capture already; a refresh replaces it and never adds a second one
    d.state.refresh_snapshot(id);
    assert_eq!(live(&d), 1, "the capture is kept for a later close");
    d.state.refresh_snapshot(id);
    assert_eq!(
        live(&d),
        1,
        "a second capture of the window replaces the first"
    );
    d.state.fx.forget_snapshot(id);
    assert_eq!(live(&d), 0, "forgetting the window retires it");
}
