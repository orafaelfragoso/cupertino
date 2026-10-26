//! Public element constructors for shell and decoration components: rounded rectangles drawn by
//! the compositor's shader, with stable ids so damage tracking stays minimal.

use smithay::utils::{Logical, Rectangle};

use super::programs::{corner_uniforms, u4, Programs};
use super::shader_rect::{ElementCache, ElementKey, ShaderRect, Sig};

/// Description of a rounded rectangle in output-local logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundedRectSpec {
    pub rect: Rectangle<f64, Logical>,
    pub radius: f64,
    /// Continuous (superellipse) corners instead of circular ones.
    pub continuous: bool,
    /// Straight RGBA.
    pub fill: [f32; 4],
    /// Border width (points) and straight RGBA colour.
    pub border: Option<(f64, [f32; 4])>,
    pub alpha: f32,
}

impl RoundedRectSpec {
    pub fn new(rect: Rectangle<f64, Logical>, radius: f64, fill: [f32; 4]) -> Self {
        Self { rect, radius, continuous: true, fill, border: None, alpha: 1.0 }
    }
}

/// Build the element for `spec`. `key` identifies the element across frames; `scale` is the
/// output scale (anti-aliasing width).
pub fn rounded_rect(programs: &Programs, cache: &mut ElementCache, scale: f64, key: ElementKey, spec: &RoundedRectSpec) -> ShaderRect {
    let margin = 1.5;
    let r = spec.rect;
    let area = Rectangle::new((r.loc.x - margin, r.loc.y - margin).into(), (r.size.w + 2.0 * margin, r.size.h + 2.0 * margin).into());
    let (extent, exponent) = corner_uniforms(spec.radius, spec.continuous, 1.0);
    let (bw, bc) = spec.border.unwrap_or((0.0, [0.0; 4]));
    let uniforms = vec![
        u4("rect", [margin as f32, margin as f32, r.size.w as f32, r.size.h as f32]),
        u4("shape", [extent, bw as f32, exponent, scale as f32]),
        u4("fill", spec.fill),
        u4("border", bc),
    ];
    let mut s = Sig::new().rect(area).f(spec.radius).f(bw).f(spec.alpha as f64).f(scale).u(spec.continuous as u64);
    for v in spec.fill.iter().chain(bc.iter()) {
        s = s.f(*v as f64);
    }
    let (id, commit) = cache.slot(key, s.finish());
    ShaderRect::new(programs.rrect.clone(), id, commit, area, spec.alpha, uniforms, false)
}
