//! Custom GLES shader programs of the compositor (GLSL ES 1.00, compiled through the renderer's
//! pixel / texture shader interface) and the helpers that translate geometry into their uniforms.

use cupertino_geometry::CornerStyle;
use smithay::backend::renderer::gles::{
    GlesError, GlesPixelProgram, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType,
    UniformValue,
};

const HEADER: &str = include_str!("../../shaders/compositor/header.glsl");
const SDF: &str = include_str!("../../shaders/compositor/sdf.glsl");
const SHADOW: &str = include_str!("../../shaders/compositor/shadow.frag");
const RRECT: &str = include_str!("../../shaders/compositor/rrect.frag");
const WALLPAPER: &str = include_str!("../../shaders/compositor/wallpaper.frag");
const CLIP: &str = include_str!("../../shaders/compositor/clip.frag");

fn assemble(body: &str) -> String {
    format!("{HEADER}\n{}", body.replace("//_SDF_", SDF))
}

/// Whether the compositor dithers its blur, material, glass and shadow outputs by pixel position
/// ([`cupertino_raster_math::dither`]). Off until the look freeze turns [`dither::BY_DEFAULT`](cupertino_raster_math::dither::BY_DEFAULT)
/// on; `CUPERTINO_DITHER=1` turns it on before that, to look at it or test it.
pub fn dithering() -> bool {
    cupertino_raster_math::dither::BY_DEFAULT
        || std::env::var_os("CUPERTINO_DITHER").is_some_and(|v| v == "1")
}

/// Compiled programs, shared by every output of one renderer.
#[derive(Clone)]
pub struct Programs {
    pub shadow: GlesPixelProgram,
    pub rrect: GlesPixelProgram,
    pub wallpaper: GlesPixelProgram,
    pub clip: GlesTexProgram,
}

impl Programs {
    pub fn compile(renderer: &mut GlesRenderer) -> Result<Self, GlesError> {
        Self::compile_with(renderer, dithering())
    }

    /// [`Programs::compile`] with the shadow dithered or not.
    pub fn compile_with(renderer: &mut GlesRenderer, dither: bool) -> Result<Self, GlesError> {
        let f = |name: &'static str| UniformName::new(name, UniformType::_2f);
        let v4 = |name: &'static str| UniformName::new(name, UniformType::_4f);
        let shadow = renderer.compile_custom_pixel_shader(
            cupertino_raster_math::dither::with_dither(&assemble(SHADOW), dither),
            &[f("area_pt"), v4("caster"), v4("params"), v4("flags")],
        )?;
        let rrect = renderer.compile_custom_pixel_shader(
            assemble(RRECT),
            &[
                f("area_pt"),
                v4("rect"),
                v4("shape"),
                v4("fill"),
                v4("border"),
                v4("edge"),
            ],
        )?;
        let wallpaper = renderer.compile_custom_pixel_shader(
            assemble(WALLPAPER),
            &[f("area_pt"), v4("c0"), v4("c1"), v4("c2"), v4("c3")],
        )?;
        let clip_src = format!("#version 100\n{}", CLIP.replace("//_SDF_", SDF));
        let clip = renderer.compile_custom_texture_shader(
            clip_src,
            &[
                UniformName::new("inv_proj", UniformType::Matrix3x3),
                f("viewport"),
                v4("clip_rect"),
                v4("clip_shape"),
            ],
        )?;
        Ok(Self {
            shadow,
            rrect,
            wallpaper,
            clip,
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
    fn the_shadow_takes_the_dither_only_when_dithering() {
        use cupertino_raster_math::dither::with_dither;
        let plain = with_dither(&assemble(SHADOW), false);
        assert_eq!(plain, assemble(SHADOW));
        assert!(!plain.contains("#define CUPERTINO_DITHER"));
        let on = with_dither(&assemble(SHADOW), true);
        assert!(on.contains("#define CUPERTINO_DITHER 1") && on.contains("float dither_alpha("));
        assert!(!cupertino_raster_math::dither::BY_DEFAULT || dithering());
    }

    #[test]
    fn shader_sources_are_assembled() {
        let s = assemble(SHADOW);
        assert!(s.contains("rounded_sdf") && !s.contains("//_SDF_"));
        assert!(assemble(RRECT).starts_with("#ifdef"));
    }
}
