//! The compositor's dithering is the shared definition (`cupertino_raster_math::dither`): drawn on the GL path, every
//! pixel stores what the definition stores at its position, and the programs that dither (blur, material, glass,
//! shadow) build with it and stay within half a step of what they draw without it.

use cupertino_raster_math::dither;
use cupertino_raster_math::shadow::{drop_shadow, Caster, Shadow};
use cupertino_server::backend::headless;
use cupertino_server::render::passes::RectPass;
use cupertino_server::render::programs::{u4, Programs};
use cupertino_server::render::screenshot::render_to_image;
use cupertino_server::render::{MaterialGl, ShaderRect};
use image::RgbaImage;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::gles::{GlesRenderer, UniformName, UniformType};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::Color32F;
use smithay::utils::Rectangle;

/// A target of odd size, several matrix periods across, so every position of the matrix and its repeat show.
const SIZE: (i32, i32) = (37, 29);

/// A flat premultiplied colour between 8-bit steps, away from the half step where rounding could go either way.
const FLAT: [f32; 4] = [37.3 / 255.0, 120.7 / 255.0, 160.45 / 255.0, 200.6 / 255.0];

fn draw(renderer: &mut GlesRenderer, element: ShaderRect) -> RgbaImage {
    render_to_image(
        renderer,
        &[element],
        SIZE.into(),
        1.0,
        Color32F::TRANSPARENT,
    )
    .expect("render")
}

fn area() -> Rectangle<f64, smithay::utils::Logical> {
    Rectangle::new((0.0, 0.0).into(), (SIZE.0 as f64, SIZE.1 as f64).into())
}

/// A pixel program of `body` (the inside of `main`) over the shader implementation of the dither.
fn dither_program(
    renderer: &mut GlesRenderer,
    body: &str,
) -> smithay::backend::renderer::gles::GlesPixelProgram {
    let source = format!(
        "#ifdef GL_FRAGMENT_PRECISION_HIGH\nprecision highp float;\n#else\nprecision mediump float;\n#endif\n\
         varying vec2 v_coords;\nuniform float alpha;\nuniform vec2 area_pt;\nuniform vec4 value;\n{}\n\
         void main() {{\n{body}\n}}\n",
        dither::GLSL
    );
    renderer
        .compile_custom_pixel_shader(
            source,
            &[
                UniformName::new("area_pt", UniformType::_2f),
                UniformName::new("value", UniformType::_4f),
            ],
        )
        .expect("compile the dither")
}

fn flat(program: smithay::backend::renderer::gles::GlesPixelProgram) -> ShaderRect {
    ShaderRect::new(
        program,
        Id::new(),
        CommitCounter::default(),
        area(),
        1.0,
        vec![u4("value", FLAT)],
        false,
    )
}

#[test]
fn the_shader_dither_offsets_every_pixel_by_the_definition_s_matrix() {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    // the matrix entry each pixel's offset comes from, stored as a whole step so no rounding can move it
    let program = dither_program(
        &mut renderer,
        "    float m = (dither_offset(gl_FragCoord.xy) + 0.5) * 64.0 - 0.5;\n    gl_FragColor = vec4(vec3(m / 255.0), 1.0);",
    );
    let image = draw(&mut renderer, flat(program));
    let wrong: Vec<_> = image
        .enumerate_pixels()
        // the framebuffer's rows are the image's: gl_FragCoord.y counts them from the image's first row
        .filter(|(x, y, px)| u32::from(px.0[0]) != dither::matrix(*x, *y))
        .map(|(x, y, px)| (x, y, px.0[0], dither::matrix(x, y)))
        .take(4)
        .collect();
    assert!(
        wrong.is_empty(),
        "matrix entries differ from the definition: {wrong:?}"
    );
}

#[test]
fn the_shader_dither_stores_what_the_definition_stores_but_for_the_driver_s_rounding_at_half_a_step(
) {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let program = dither_program(
        &mut renderer,
        "    gl_FragColor = vec4(dither_rgb(value.rgb, gl_FragCoord.xy), dither_alpha(value.a, gl_FragCoord.xy));",
    );
    let image = draw(&mut renderer, flat(program));
    let (mut exact, mut total) = (0, 0);
    for (x, y, px) in image.enumerate_pixels() {
        for (c, v) in FLAT.iter().enumerate() {
            let want = dither::store(*v, x, y);
            assert!(
                px.0[c].abs_diff(want) <= 1,
                "({x}, {y}) channel {c}: {} for {want}",
                px.0[c]
            );
            exact += usize::from(px.0[c] == want);
            total += 1;
        }
    }
    // a driver may round a value a hair from half a step either way; nothing else may differ
    assert!(
        exact * 100 >= total * 85,
        "{exact} of {total} channels are the definition's"
    );
    // dithered, not flat: a value between two steps stores both
    let stored: std::collections::BTreeSet<u8> = image.pixels().map(|p| p.0[0]).collect();
    assert_eq!(
        stored.into_iter().collect::<Vec<_>>(),
        [37, 38],
        "red spreads over its two steps"
    );
}

#[test]
fn the_programs_that_dither_build_with_it_and_the_shadow_stays_within_half_a_step() {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    MaterialGl::new_with(&mut renderer, true).expect("blur, material and glass build dithered");
    let plain = Programs::compile_with(&mut renderer, false).expect("plain programs");
    let dithered = Programs::compile_with(&mut renderer, true).expect("dithered programs");
    // a soft shadow under a box in the middle of the target, its own area not cut out
    let caster = Caster {
        half: [10.0, 7.0],
        radius: 4.0,
        extent: 4.0,
        exponent: 2.0,
    };
    let shadow = Shadow {
        sigma: 5.0,
        offset: [0.0, 2.0],
        density: 0.8,
    };
    let centre = [SIZE.0 as f32 * 0.5, SIZE.1 as f32 * 0.5];
    // the shadow pass's block: the caster, radius, sigma, density and offset, then no cut-out, the exponent, pixels
    // per point and the extent
    let params = [
        [
            centre[0] - caster.half[0],
            centre[1] - caster.half[1],
            caster.half[0] * 2.0,
            caster.half[1] * 2.0,
        ],
        [
            caster.radius,
            shadow.sigma,
            shadow.density,
            shadow.offset[1],
        ],
        [0.0, caster.exponent, 1.0, caster.extent],
    ];
    let element = |p: &Programs| {
        ShaderRect::pass(
            &p.passes,
            RectPass::Shadow,
            Id::new(),
            CommitCounter::default(),
            area(),
            1.0,
            &params,
            false,
        )
    };
    let off = draw(&mut renderer, element(&plain));
    let on = draw(&mut renderer, element(&dithered));
    let (mut changed, mut exact, mut total) = (0, 0, 0);
    for ((x, y, a), b) in on.enumerate_pixels().zip(off.pixels()) {
        let (a, b) = (a.0[3], b.0[3]);
        assert!(a.abs_diff(b) <= 1, "({x}, {y}): {a} dithered, {b} plain");
        changed += usize::from(a != b);
        // the definition's shadow, dithered at the pixel's position
        let p = [x as f32 + 0.5 - centre[0], y as f32 + 0.5 - centre[1]];
        let want = dither::store(drop_shadow(p, &caster, &shadow, false, 1.0), x, y);
        assert!(
            a.abs_diff(want) <= 1,
            "({x}, {y}): {a}, the definition {want}"
        );
        exact += usize::from(a == want);
        total += 1;
    }
    assert!(
        changed > total / 10,
        "dithering moved {changed} of {total} pixels"
    );
    assert!(
        exact * 100 >= total * 85,
        "{exact} of {total} pixels are exactly the definition's"
    );
}
