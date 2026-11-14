//! A rotated or flipped output shows the scene of its logical space turned: composing and drawing it must not depend
//! on the transform except by the damage tracker's turn of the whole frame. For each of the eight transforms the
//! scene is composed twice, for an output of mode `W x H` with the transform (through `configure_output`, as an output
//! configuration applies it, so the logical space is the mode turned) and for the upright output of that logical size;
//! the first frame must be the second turned. The output is not square, so a width/height mix-up in composition or
//! lowering cannot go unseen.
//!
//! The comparison allows 2/255 per channel, the goldens' bound. Exact equality was tried and does not hold: a
//! handful of pixels in 540 000 differ by one step in one channel, where the wallpaper gradient is rounded
//! differently when the rasteriser samples the turned frame. Nothing may differ by more than that, or be anywhere else.
//!
//! The dither is off here: its pattern is laid on the framebuffer's pixels, so a turned output carries it turned
//! against the scene, by up to a step per dithered stage. Its own tests (`dither`) pin it.

use crate::common;

use common::desktop::Desktop;
use cupertino_server::backend::{headless, Backend};
use cupertino_server::render::screenshot::render_to_image_transformed;
use image::{imageops, RgbaImage};
use smithay::output::Mode;
use smithay::utils::Transform;

const MODE: (i32, i32) = (900, 600);

/// Turn the dither off for this test's process (each test runs in its own), before any program is built.
fn without_dither() {
    std::env::set_var("CUPERTINO_DITHER", "0");
}

/// The mode (framebuffer) size of an output whose logical space is `logical` once turned by `transform`.
fn mode_for(logical: (i32, i32), transform: Transform) -> (i32, i32) {
    match transform {
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 => {
            (logical.1, logical.0)
        }
        _ => logical,
    }
}

/// The desktop with two windows on an output of framebuffer `mode`, turned by `transform`.
fn desktop(mode: (i32, i32), transform: Transform, scale: f64) -> Desktop {
    let display = headless::HeadlessDisplay {
        name: "headless-1".into(),
        size: mode,
        physical_mm: (0, 0),
    };
    let mut d = Desktop::with_displays(std::slice::from_ref(&display), scale);
    let id = d.state.outputs[0].id;
    d.state.configure_output(
        id,
        Mode {
            size: mode.into(),
            refresh: 60_000,
        },
        scale,
        transform,
        (0, 0),
    );
    d.window();
    d.window();
    d.at(3000);
    d.at(3016);
    d
}

/// The frame of the desktop with two windows on an output of framebuffer `mode`, turned by `transform`.
fn frame(mode: (i32, i32), transform: Transform, scale: f64) -> RgbaImage {
    let mut d = desktop(mode, transform, scale);
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let elements = d.state.build_output_elements(&mut h.renderer, 0);
    render_to_image_transformed(
        &mut h.renderer,
        &elements,
        mode.into(),
        scale,
        transform,
        headless::CLEAR,
    )
    .unwrap()
}

/// The upright frame turned the way `transform` turns the output: a flip first, then the counter-clockwise turn.
fn turned(upright: &RgbaImage, transform: Transform) -> RgbaImage {
    let flipped = |i: &RgbaImage| imageops::flip_horizontal(i);
    match transform {
        Transform::Normal => upright.clone(),
        Transform::_90 => imageops::rotate270(upright),
        Transform::_180 => imageops::rotate180(upright),
        Transform::_270 => imageops::rotate90(upright),
        Transform::Flipped => flipped(upright),
        Transform::Flipped90 => imageops::rotate270(&flipped(upright)),
        Transform::Flipped180 => imageops::rotate180(&flipped(upright)),
        Transform::Flipped270 => imageops::rotate90(&flipped(upright)),
    }
}

/// The goldens' tolerance, see the module documentation.
const CHANNEL_TOLERANCE: u8 = 2;

fn beyond_tolerance(expected: &RgbaImage, actual: &RgbaImage) -> Vec<(u32, u32, [u8; 4], [u8; 4])> {
    assert_eq!(expected.dimensions(), actual.dimensions(), "frame size");
    expected
        .enumerate_pixels()
        .zip(actual.pixels())
        .filter(|((_, _, a), b)| {
            a.0.iter()
                .zip(b.0)
                .any(|(a, b)| a.abs_diff(b) > CHANNEL_TOLERANCE)
        })
        .map(|((x, y, a), b)| (x, y, a.0, b.0))
        .take(6)
        .collect()
}

#[test]
fn every_transform_of_an_output_shows_the_upright_frame_turned() {
    without_dither();
    let transforms = [
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];
    for scale in [1.0, 1.5] {
        // the logical space is MODE.0 x MODE.1 for the transforms that keep the mode's shape and swapped otherwise
        for transform in transforms {
            let logical = match transform {
                Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 => {
                    (MODE.1, MODE.0)
                }
                _ => MODE,
            };
            let upright = frame(logical, Transform::Normal, scale);
            let expected = turned(&upright, transform);
            let actual = frame(mode_for(logical, transform), transform, scale);
            // the comparison must be able to fail: the turned frame is not the upright one
            assert!(
                upright.dimensions() != expected.dimensions()
                    || !beyond_tolerance(&upright, &expected).is_empty(),
                "{transform:?} at {scale}x: the scene looks the same turned, the test would pass trivially"
            );
            let beyond = beyond_tolerance(&expected, &actual);
            assert!(
                beyond.is_empty(),
                "{transform:?} at {scale}x: pixels beyond the tolerance {beyond:?}"
            );
        }
    }
}

/// The screenshot of that desktop: what screenshots and screen captures hand out.
fn screenshot(mode: (i32, i32), transform: Transform, scale: f64) -> RgbaImage {
    let mut d = desktop(mode, transform, scale);
    let mut backend = d.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut h) = backend
    else {
        panic!("headless backend required")
    };
    let image = d
        .state
        .draw_output_image(&mut h.renderer, 0, true, false)
        .unwrap();
    d.state.backend = Some(backend);
    image
}

#[test]
fn a_screenshot_of_a_turned_output_is_what_the_upright_output_of_its_logical_space_shows() {
    without_dither();
    let transforms = [
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];
    for scale in [1.0, 1.5] {
        for transform in transforms {
            let logical = match transform {
                Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 => {
                    (MODE.1, MODE.0)
                }
                _ => MODE,
            };
            let upright = screenshot(logical, Transform::Normal, scale);
            let shot = screenshot(mode_for(logical, transform), transform, scale);
            let beyond = beyond_tolerance(&upright, &shot);
            assert!(
                beyond.is_empty(),
                "{transform:?} at {scale}x: the screenshot differs from the upright output's {beyond:?}"
            );
        }
    }
}
