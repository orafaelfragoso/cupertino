//! Public element constructors for shell and decoration components: rounded rectangles drawn by
//! the compositor's rounded-rectangle pass, with stable ids so damage tracking stays minimal.

use smithay::utils::{Logical, Rectangle};

use cupertino_scene::BorderFalloff;

use super::passes::RectPass;
use super::programs::{corner_uniforms, Programs};
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
    /// How the border's strength follows the direction of the edge; the default draws it the same all around.
    pub border_falloff: BorderFalloff,
    pub alpha: f32,
}

impl RoundedRectSpec {
    pub fn new(rect: Rectangle<f64, Logical>, radius: f64, fill: [f32; 4]) -> Self {
        Self {
            rect,
            radius,
            continuous: true,
            fill,
            border: None,
            border_falloff: BorderFalloff::default(),
            alpha: 1.0,
        }
    }
}

/// Room in points the element leaves around its rectangle for the anti-aliased edge.
const EDGE_MARGIN: f64 = 1.5;

/// `rect` grown by [`EDGE_MARGIN`] and then out to whole device pixels at `scale`, so that the element's pixels are the
/// output's and the shape is sampled at their centres wherever the rectangle lies.
fn padded_area(rect: Rectangle<f64, Logical>, scale: f64) -> Rectangle<f64, Logical> {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let x0 = ((rect.loc.x - EDGE_MARGIN) * scale).floor() / scale;
    let y0 = ((rect.loc.y - EDGE_MARGIN) * scale).floor() / scale;
    let x1 = ((rect.loc.x + rect.size.w + EDGE_MARGIN) * scale).ceil() / scale;
    let y1 = ((rect.loc.y + rect.size.h + EDGE_MARGIN) * scale).ceil() / scale;
    Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into())
}

/// Build the element for `spec`. `key` identifies the element across frames; `scale` is the
/// output scale (anti-aliasing width).
pub fn rounded_rect(
    programs: &Programs,
    cache: &mut ElementCache,
    scale: f64,
    key: ElementKey,
    spec: &RoundedRectSpec,
) -> ShaderRect {
    let r = spec.rect;
    let area = padded_area(r, scale);
    let (extent, exponent) = corner_uniforms(spec.radius, spec.continuous, 1.0);
    let (bw, bc) = spec.border.unwrap_or((0.0, [0.0; 4]));
    // the block of the rounded-rectangle pass: the rectangle, the shape, the fill, the border and its falloff
    let params = [
        [
            (r.loc.x - area.loc.x) as f32,
            (r.loc.y - area.loc.y) as f32,
            r.size.w as f32,
            r.size.h as f32,
        ],
        [extent, bw as f32, exponent, scale as f32],
        spec.fill,
        bc,
        [
            spec.border_falloff.sides,
            spec.border_falloff.ends,
            spec.border_falloff.floor,
            0.0,
        ],
    ];
    let mut s = Sig::new()
        .rect(area)
        .f(spec.radius)
        .f(bw)
        .f(spec.alpha as f64)
        .f(scale)
        .f(f64::from(spec.border_falloff.sides))
        .f(f64::from(spec.border_falloff.ends))
        .f(f64::from(spec.border_falloff.floor))
        .u(spec.continuous as u64);
    for v in spec.fill.iter().chain(bc.iter()) {
        s = s.f(*v as f64);
    }
    let (id, commit) = cache.slot(key, s.finish());
    ShaderRect::pass(
        &programs.passes,
        RectPass::Rounded,
        id,
        commit,
        area,
        spec.alpha,
        &params,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: f64, y: f64, w: f64, h: f64, scale: f64) -> (f64, f64, f64, f64) {
        let a = padded_area(Rectangle::new((x, y).into(), (w, h).into()), scale);
        (a.loc.x, a.loc.y, a.size.w, a.size.h)
    }

    #[test]
    fn the_element_lies_on_whole_device_pixels_around_its_rectangle() {
        // a margin of 1.5 pt is 1.5 px at 1x: out to 2 px, so the pixels are sampled at their centres
        assert_eq!(area(10.0, 10.0, 80.0, 60.0, 1.0), (8.0, 8.0, 84.0, 64.0));
        assert_eq!(area(10.0, 10.0, 80.0, 60.0, 2.0), (8.5, 8.5, 83.0, 63.0));
        let (x, y, w, h) = area(10.25, 3.0, 7.0, 5.0, 1.5);
        for v in [x, y, x + w, y + h] {
            assert!(
                ((v * 1.5) - (v * 1.5).round()).abs() < 1e-9,
                "{v} is a whole pixel"
            );
        }
        assert!(x <= 10.25 - EDGE_MARGIN && x + w >= 17.25 + EDGE_MARGIN);
    }
}
