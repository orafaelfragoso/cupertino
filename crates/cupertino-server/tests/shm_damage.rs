//! A client buffer is uploaded where the client says it changed. A new buffer committed with its whole area damaged
//! shows at once; a new buffer committed without damage says nothing changed, so the window keeps showing what it
//! showed and the compositor uploads nothing.

use crate::common;

use std::time::Instant;

use common::desktop::Desktop;
use common::Client;
use cupertino_server::backend::{headless, Backend};
use cupertino_server::render::screenshot::render_to_image;
use image::RgbaImage;

/// The pixels of `image` that are exactly `rgb`, opaque.
fn count(image: &RgbaImage, rgb: [u8; 3]) -> usize {
    image
        .pixels()
        .filter(|p| p.0 == [rgb[0], rgb[1], rgb[2], 255])
        .count()
}

/// The output as composed now.
fn shot(d: &mut Desktop) -> RgbaImage {
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let elements = d.state.build_output_elements(&mut h.renderer, 0);
    let mode = d.state.outputs[0].output.current_mode().unwrap().size;
    let image = render_to_image(
        &mut h.renderer,
        &elements,
        mode,
        d.state.outputs[0].scale,
        headless::CLEAR,
    )
    .unwrap();
    d.state.backend = Some(backend);
    image
}

/// A blue window recoloured red, with or without damage: the blue and red pixels the output shows after.
fn recoloured(damaged: bool) -> (usize, usize, usize) {
    let mut d = Desktop::new(1.0);
    let client = Client::recolouring(d.socket(), damaged);
    let deadline = Instant::now() + common::HANG_GUARD;
    while d.state.wm.windows().count() != 1 {
        d.pump();
        assert!(Instant::now() < deadline, "the test client did not map");
    }
    d.at(3000);
    let blue = [0, 0, 255];
    let before = count(&shot(&mut d), blue);
    client.unlock();
    // two full rounds of the client after the ask: the first finds it and commits, the second follows the commit
    let deadline = Instant::now() + common::HANG_GUARD;
    for _ in 0..2 {
        let barrier = client.barrier();
        while !barrier() {
            d.pump();
            assert!(Instant::now() < deadline, "the client did not recolour");
        }
    }
    d.at(3100);
    let after = shot(&mut d);
    (before, count(&after, blue), count(&after, [255, 0, 0]))
}

#[test]
fn a_new_buffer_with_its_area_damaged_shows_at_once() {
    let (before, blue, red) = recoloured(true);
    assert!(before > 50_000, "the window shows blue first ({before} px)");
    assert_eq!(blue, 0, "no blue is left");
    assert_eq!(red, before, "red shows where blue was");
}

#[test]
fn a_new_buffer_without_damage_keeps_what_the_window_showed() {
    let (before, blue, red) = recoloured(false);
    assert!(before > 50_000, "the window shows blue first ({before} px)");
    assert_eq!(blue, before, "the window still shows blue");
    assert_eq!(red, 0, "nothing of the undamaged buffer was uploaded");
}
