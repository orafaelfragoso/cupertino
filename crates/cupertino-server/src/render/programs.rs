//! The compositor's own programs of one renderer: the rectangle passes generated from WGSL and drawn by a borrowed
//! device ([`super::passes`]), and the texture programs compiled through the renderer's texture shader interface
//! (GLSL ES 1.00, which that interface takes, with the external-sampler variant it makes), with the helpers that
//! translate geometry into their uniforms.

use std::rc::Rc;

use cupertino_geometry::CornerStyle;
use smithay::backend::renderer::gles::{
    GlesError, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType, UniformValue,
};

use super::passes::Passes;

const SDF: &str = include_str!("../../shaders/compositor/sdf.glsl");
const CLIP: &str = include_str!("../../shaders/compositor/clip.frag");

/// Whether the compositor dithers its blur, material, glass and shadow outputs by pixel position
/// ([`cupertino_raster_math::dither`]): [`dither::BY_DEFAULT`](cupertino_raster_math::dither::BY_DEFAULT), on;
/// `CUPERTINO_DITHER=0` turns it off and `CUPERTINO_DITHER=1` on, to compare the two.
pub fn dithering() -> bool {
    match std::env::var_os("CUPERTINO_DITHER") {
        Some(v) if v == "0" => false,
        Some(v) if v == "1" => true,
        _ => cupertino_raster_math::dither::BY_DEFAULT,
    }
}

/// Compiled programs, shared by every output of one renderer.
#[derive(Clone)]
pub struct Programs {
    /// The shadow, rounded-rectangle and wallpaper passes.
    pub passes: Rc<Passes>,
    pub clip: GlesTexProgram,
    /// The clip program for a texture drawn smaller than its texels, each pixel the average of those it covers
    /// ([`super::clip::minify_uniforms`]).
    pub clip_minify: GlesTexProgram,
    /// The reduced copies of windows drawn well below their size.
    pub reduced: super::reduce::Reductions,
    /// The clip program drawing a vibrant layer's coverage ([`cupertino_scene::source::VibrantBlend`]).
    pub vibrant: GlesTexProgram,
}

impl Programs {
    pub fn compile(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        Self::compile_with(renderer, dithering())
    }

    /// [`Programs::compile`] with the shadow dithered or not.
    pub fn compile_with(renderer: &mut GlesRenderer, dither: bool) -> Result<Self, GlesError> {
        let f = |name: &'static str| UniformName::new(name, UniformType::_2f);
        let v4 = |name: &'static str| UniformName::new(name, UniformType::_4f);
        // a refused program is logged once, by name: the passes' error carries the driver's log, the renderer logs its
        // own log of a texture program just before
        let named = |program: &'static str| {
            move |e: GlesError| {
                tracing::error!(program, error = %e, "a compositor program failed to compile or link");
                e
            }
        };
        let passes = Passes::new(renderer, dither).map_err(|e| {
            tracing::error!(program = "shadow, rrect, wallpaper", error = %e, "a compositor program failed to compile or link");
            GlesError::ProgramLinkError
        })?;
        let clip_src = format!("#version 100\n{}", CLIP.replace("//_SDF_", SDF));
        let clip = renderer
            .compile_custom_texture_shader(
                clip_src,
                &[
                    UniformName::new("inv_proj", UniformType::Matrix3x3),
                    f("viewport"),
                    v4("clip_rect"),
                    v4("clip_shape"),
                ],
            )
            .map_err(named("clip"))?;
        let minify_src = format!(
            "#version 100\n#define MINIFY\n{}",
            CLIP.replace("//_SDF_", SDF)
        );
        let clip_minify = renderer
            .compile_custom_texture_shader(
                minify_src,
                &[
                    UniformName::new("inv_proj", UniformType::Matrix3x3),
                    f("viewport"),
                    v4("clip_rect"),
                    v4("clip_shape"),
                    f("footprint"),
                    f("texels"),
                ],
            )
            .map_err(named("clip, minifying"))?;
        let vibrant = renderer
            .compile_custom_texture_shader(
                format!(
                    "#version 100\n#define VIBRANT\n{}",
                    CLIP.replace("//_SDF_", SDF)
                ),
                &[
                    UniformName::new("inv_proj", UniformType::Matrix3x3),
                    f("viewport"),
                    v4("clip_rect"),
                    v4("clip_shape"),
                    v4("vibrant"),
                ],
            )
            .map_err(named("clip, vibrant"))?;
        Ok(Self {
            passes,
            clip,
            clip_minify,
            reduced: super::reduce::Reductions::default(),
            vibrant,
        })
    }
}

/// Corner uniforms: `(extent in px, superellipse exponent)` for a nominal radius.
pub fn corner_uniforms(radius: f64, continuous: bool, px_per_pt: f64) -> (f32, f32) {
    let style = if continuous {
        CornerStyle::continuous()
    } else {
        CornerStyle::Circular
    };
    (
        (radius * style.expansion() * px_per_pt) as f32,
        style.exponent() as f32,
    )
}

pub fn u2(name: &'static str, a: f32, b: f32) -> Uniform<'static> {
    Uniform::new(name, UniformValue::_2f(a, b))
}

pub fn u4(name: &'static str, v: [f32; 4]) -> Uniform<'static> {
    Uniform::new(name, UniformValue::_4f(v[0], v[1], v[2], v[3]))
}

pub fn umat3(name: &'static str, m: [f32; 9]) -> Uniform<'static> {
    Uniform::new(
        name,
        UniformValue::Matrix3x3 {
            matrices: vec![m],
            transpose: false,
        },
    )
}

/// Inverse of a column-major 3x3 matrix; `None` when singular.
pub fn invert3(m: &[f32; 9]) -> Option<[f32; 9]> {
    let (a, b, c) = (m[0], m[3], m[6]);
    let (d, e, f) = (m[1], m[4], m[7]);
    let (g, h, i) = (m[2], m[5], m[8]);
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-12 {
        return None;
    }
    let id = 1.0 / det;
    let inv = [
        [
            (e * i - f * h) * id,
            (c * h - b * i) * id,
            (b * f - c * e) * id,
        ],
        [
            (f * g - d * i) * id,
            (a * i - c * g) * id,
            (c * d - a * f) * id,
        ],
        [
            (d * h - e * g) * id,
            (b * g - a * h) * id,
            (a * e - b * d) * id,
        ],
    ];
    Some([
        inv[0][0], inv[1][0], inv[2][0], inv[0][1], inv[1][1], inv[2][1], inv[0][2], inv[1][2],
        inv[2][2],
    ])
}

/// Apply a column-major 3x3 matrix to `(x, y, 1)`.
pub fn apply3(m: &[f32; 9], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[3] * y + m[6], m[1] * x + m[4] * y + m[7])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_roundtrips() {
        // scale + translate + flip, as produced by an output projection
        let m = [0.002, 0.0, 0.0, 0.0, -0.004, 0.0, -1.0, 1.0, 1.0];
        let inv = invert3(&m).unwrap();
        let (cx, cy) = apply3(&m, 320.0, 140.0);
        let (x, y) = apply3(&inv, cx, cy);
        assert!(
            (x - 320.0).abs() < 1e-3 && (y - 140.0).abs() < 1e-3,
            "{x} {y}"
        );
        assert!(invert3(&[0.0; 9]).is_none());
    }

    #[test]
    fn corner_uniforms_scale_with_density() {
        let (e1, n1) = corner_uniforms(12.0, true, 1.0);
        let (e2, n2) = corner_uniforms(12.0, true, 2.0);
        assert!((e2 / e1 - 2.0).abs() < 1e-5 && n1 == n2 && n1 > 2.0);
        let (e, n) = corner_uniforms(12.0, false, 1.0);
        assert_eq!((e, n), (12.0, 2.0));
    }

    #[test]
    fn dithering_is_on_by_default() {
        assert!(!cupertino_raster_math::dither::BY_DEFAULT || dithering());
    }

    #[test]
    fn the_hand_written_programs_left_are_the_texture_programs_of_the_renderer_s_interface() {
        // the clip program and its variants, GLSL ES 1.00 with the distance put in place; the rectangle passes are
        // generated ([`super::super::passes`])
        let clip = CLIP.replace("//_SDF_", SDF);
        assert!(clip.contains("float rounded_sdf(") && !clip.contains("//_SDF_"));
        assert!(clip.contains("uniform sampler2D tex;") && clip.contains("samplerExternalOES"));
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/shaders/compositor");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .collect();
        files.sort();
        assert_eq!(files, ["clip.frag", "sdf.glsl"]);
    }
}
