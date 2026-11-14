//! The compositor's rounded rectangles are the shared coverage (`cupertino_raster_math::coverage`): drawn on the GL
//! path, every pixel's alpha is the coverage of its centre's distance to the outline, for the shapes of the coverage
//! vectors with circular corners and for the continuous corner the shell draws, at scales 1, 1.5 and 2; and the
//! passes of one frame read the context's state once and draw what they draw one by one.

use cupertino_raster_math::coverage::{coverage, rounded_distance, stroke_coverage};
use cupertino_raster_math::dither::quantize;
use cupertino_raster_math::vectors;
use cupertino_server::backend::headless;
use cupertino_server::render::programs::{corner_uniforms, Programs};
use cupertino_server::render::screenshot::render_to_image;
use cupertino_server::render::shader_rect::{ElementCache, ElementKey};
use cupertino_server::render::ui::{rounded_rect, RoundedRectSpec};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::Color32F;
use smithay::utils::Rectangle;

/// Where the shape's top-left corner lies, in points.
const AT: f64 = 3.0;

/// Draws `spec`'s shape (radius `radius`, circular or continuous, with an inside border of `border` points) at `scale`
/// and checks every pixel's alpha against the definition: within one step (a driver may round a hair either way at
/// half a step) and the definition's own value for nearly every pixel.
fn check(half: [f64; 2], radius: f64, continuous: bool, border: f64, scale: f64) {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let mut cache = ElementCache::default();
    let rect = Rectangle::new((AT, AT).into(), (half[0] * 2.0, half[1] * 2.0).into());
    let mut spec = RoundedRectSpec::new(rect, radius, [1.0, 1.0, 1.0, 1.0]);
    spec.continuous = continuous;
    if border > 0.0 {
        spec.fill = [1.0, 1.0, 1.0, 0.0];
        spec.border = Some((border, [1.0, 1.0, 1.0, 1.0]));
    }
    let element = rounded_rect(
        &programs,
        &mut cache,
        scale,
        ElementKey::new("coverage", 0),
        &spec,
    );
    let size = (
        ((half[0] * 2.0 + 2.0 * AT) * scale).ceil() as i32,
        ((half[1] * 2.0 + 2.0 * AT) * scale).ceil() as i32,
    );
    let image = render_to_image(
        &mut renderer,
        &[element],
        size.into(),
        scale,
        Color32F::TRANSPARENT,
    )
    .expect("render");
    let (extent, exponent) = corner_uniforms(radius, continuous, 1.0);
    let extent = extent.min(half[0].min(half[1]) as f32);
    let (mut exact, mut total) = (0, 0);
    for (x, y, px) in image.enumerate_pixels() {
        let p = [
            ((f64::from(x) + 0.5) / scale - AT - half[0]) as f32,
            ((f64::from(y) + 0.5) / scale - AT - half[1]) as f32,
        ];
        let d = rounded_distance(p, [half[0] as f32, half[1] as f32], extent, exponent);
        let s = scale as f32;
        let cover = if border > 0.0 {
            stroke_coverage(d, border as f32, s)
        } else {
            coverage(d, s)
        };
        let want = quantize(cover);
        let got = px.0[3];
        assert!(
            got.abs_diff(want) <= 1,
            "{half:?} r {radius} continuous {continuous} border {border} at {scale}: ({x}, {y}) is {got}, the \
             definition {want}"
        );
        exact += usize::from(got == want);
        total += 1;
    }
    assert!(
        exact * 100 >= total * 95,
        "{exact} of {total} pixels are exactly the definition's"
    );
}

#[test]
fn rounded_rectangles_are_the_shared_coverage_at_every_scale() {
    // the shapes of the coverage vectors with circular corners, at the vectors' scales
    let mut shapes: Vec<([f64; 2], f64, f64)> = vectors::rows(vectors::COVERAGE)
        .iter()
        .filter(|r| r[5] <= 2.001)
        .map(|r| ([r[2], r[3]], r[4], r[6]))
        .collect();
    shapes.dedup();
    assert!(shapes.len() >= 6, "{shapes:?}");
    for (half, extent, scale) in shapes {
        check(half, extent, false, 0.0, scale);
    }
    // the continuous corner and an inside border
    for scale in [1.0, 1.5, 2.0] {
        check([30.0, 16.0], 10.0, true, 0.0, scale);
        check([30.0, 16.0], 8.0, false, 1.0, scale);
    }
}

#[test]
fn the_passes_of_a_frame_read_the_context_s_state_once_and_draw_what_they_draw_entered_one_by_one()
{
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let mut cache = ElementCache::default();
    // four overlapping rounded rectangles with a border, each its own pass draw
    let elements: Vec<_> = (0..4u8)
        .map(|i| {
            let at = 2.0 + 5.0 * f64::from(i);
            let rect = Rectangle::new((at, at).into(), (24.0, 14.0).into());
            let mut spec = RoundedRectSpec::new(rect, 6.0, [0.2 * f32::from(i), 0.5, 0.8, 0.7]);
            spec.border = Some((1.0, [1.0, 1.0, 1.0, 1.0]));
            rounded_rect(
                &programs,
                &mut cache,
                1.5,
                ElementKey::new("frame", u64::from(i)),
                &spec,
            )
        })
        .collect();
    let draw = |renderer: &mut GlesRenderer| {
        render_to_image(
            renderer,
            &elements,
            (64, 56).into(),
            1.5,
            Color32F::new(0.1, 0.1, 0.1, 1.0),
        )
        .expect("render")
    };
    let reads = programs.passes.state_reads();
    let one_by_one = draw(&mut renderer);
    assert_eq!(
        programs.passes.state_reads() - reads,
        4,
        "outside a frame each pass reads the state"
    );
    let reads = programs.passes.state_reads();
    let in_frame = {
        let _frame = programs.passes.frame();
        draw(&mut renderer)
    };
    assert_eq!(
        programs.passes.state_reads() - reads,
        1,
        "inside one frame the first pass reads it"
    );
    assert_eq!(in_frame.as_raw(), one_by_one.as_raw());
    // and each pass reads it again once the frame is over
    let reads = programs.passes.state_reads();
    assert_eq!(draw(&mut renderer).as_raw(), one_by_one.as_raw());
    assert_eq!(programs.passes.state_reads() - reads, 4);
}
