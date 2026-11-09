//! A GPU reset loses the rendering context and everything made with it; the server makes a new renderer and the user's
//! windows come back as they were, their clients still connected.

mod common;

use std::time::Instant;

use common::desktop::Desktop;
use cupertino_server::backend::headless;

fn screenshot(d: &mut Desktop, name: &str) -> image::RgbaImage {
    let path =
        std::env::temp_dir().join(format!("cupertino-test-{}-{name}.png", std::process::id()));
    d.state.screenshot_requests.push(path.clone());
    d.state.request_redraw();
    let deadline = Instant::now() + common::HANG_GUARD;
    while !path.exists() {
        d.pump();
        assert!(Instant::now() < deadline, "the screenshot was not taken");
    }
    let img = image::open(&path).unwrap().to_rgba8();
    let _ = std::fs::remove_file(path);
    img
}

#[test]
fn a_window_closing_after_a_reset_fades_out_from_a_capture_of_the_new_context() {
    let mut d = Desktop::new(1.0);
    d.state.fx.cfg.closing = true;
    d.window();
    d.at(3000);
    let id = d.state.wm.key_window().unwrap();
    d.state.refresh_snapshot(id);
    assert!(d.state.fx.snapshot_age(id, d.state.now()).is_finite());

    headless::simulate_gpu_reset(&mut d.state).unwrap();
    assert!(d.state.fx.snapshot_age(id, d.state.now()).is_infinite());

    d.clients.clear();
    let deadline = Instant::now() + common::HANG_GUARD;
    while d.state.wm.windows().count() != 0 {
        d.pump();
        assert!(Instant::now() < deadline, "the window did not close");
    }
    assert_eq!(d.state.fx.closing.len(), 1, "the window fades out");
    assert_eq!(
        d.state.render.as_ref().unwrap().snapshots.live(),
        1,
        "its ghost is a capture made with the new renderer"
    );
}

#[test]
fn a_window_survives_a_gpu_reset_and_is_drawn_again_from_its_buffer() {
    let mut d = Desktop::new(1.0);
    d.window();
    d.at(3000);
    let before = screenshot(&mut d, "before-reset");

    headless::simulate_gpu_reset(&mut d.state).unwrap();
    d.at(3100);
    let after = screenshot(&mut d, "after-reset");

    assert_eq!(d.state.wm.windows().count(), 1);
    assert!(d.state.running);
    assert!(
        before == after,
        "the desktop differs after the reset in {} pixels",
        before
            .pixels()
            .zip(after.pixels())
            .filter(|(a, b)| a != b)
            .count()
    );
}
