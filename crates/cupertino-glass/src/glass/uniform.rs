//! The uniform block of the glass composite: a container's shapes, their per-shape values and looks, and its
//! container-wide values, in target pixels.

use super::{Container, ShapeLook, MAX_SHAPES};
use cupertino_theme::glass::{RIM_DEPTH_PER_HEIGHT, RIM_OUTSIDE_GAIN};

/// Uniform block of the glass composite (`Glass` in `shaders/glass.wgsl` and the GLSL generated from it), std140
/// with 16-byte members only. Lengths are in target pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlassUniform {
    /// Where the composite draws: x, y, w, h in target pixels (y down).
    pub quad: [f32; 4],
    /// Affine map from target pixels to the capture's texture coordinates: `u = a·x + b·y + c`, and the deviation in
    /// pixels of the Gaussian the edge is softened by ([`super::GlassEdge::soft`]): `[a, b, c, soft]`.
    pub uv_x: [f32; 4],
    /// The same for `v`, and 1 when the shapes differ in look ([`Container::looks_differ`]), 0 when not.
    pub uv_y: [f32; 4],
    /// Number of shapes, blend width, antialiasing width, overall opacity.
    pub info: [f32; 4],
    /// The edge line's strength, the veil's opacity at the bottom of the shapes, and the target's width and height.
    pub levels: [f32; 4],
    /// Blur fill opacities: darken, lighten, normal, and the veil's opacity at the top of the shapes.
    pub fill: [f32; 4],
    /// Edge bleed: white, black, saturation, darken (0/1).
    pub bleed: [f32; 4],
    /// The soft band's light: angle, spread, curvature, its strength at the far end of its axis.
    pub light: [f32; 4],
    /// The soft band of light: strength, half-width, offset, and the opacity of the outline outside the shape.
    pub glow: [f32; 4],
    /// The diffuse band under it: half-width, arcs' width and strength as multiples of the soft band's, and the
    /// angular width of the arcs facing left and right that keep the edge line (0: all round).
    pub diffuse: [f32; 4],
    /// Key colour matrix rows: `[r, g, b coefficients, offset (+ alpha coefficient)]`.
    pub highlight_matrix: [[f32; 4]; 3],
    /// Drop shadow: offset x, offset y, alpha, blur radius.
    pub shadow: [f32; 4],
    /// Shadow colour (rgb) and the width of the outline outside the shape.
    pub shadow_color: [f32; 4],
    /// Each shape: x, y, w, h.
    pub shapes: [[f32; 4]; MAX_SHAPES],
    /// Each shape: how far its corners reach along the sides, refraction strength, refraction band, edge bleed opacity.
    pub shape_a: [[f32; 4]; MAX_SHAPES],
    /// Each shape: edge bleed band, largest luminance, where its blur lies among the blurred copies, the exponent of
    /// its corners' superellipse.
    pub shape_b: [[f32; 4]; MAX_SHAPES],
    /// Each shape: how far outside the edge bleed reads, and the bend outside the edge: strength, band, opacity.
    pub shape_c: [[f32; 4]; MAX_SHAPES],
    /// Each shape's face: saturation, white, black, clamp.
    pub shape_face: [[f32; 4]; MAX_SHAPES],
    /// Each shape's face fill, straight sRGB.
    pub shape_fill: [[f32; 4]; MAX_SHAPES],
    /// Each shape's tint, straight sRGB.
    pub shape_tint: [[f32; 4]; MAX_SHAPES],
    /// Each shape's rim: strength, colour bias, depth; 0. No strength for a rim outside the edge, which `rim_outside`
    /// carries.
    pub shape_rim: [[f32; 4]; MAX_SHAPES],
    /// Each shape's key and fill light: amount, half-width, band, key angle.
    pub shape_spec: [[f32; 4]; MAX_SHAPES],
    /// Each shape's fill light share, and its diffuse glow's band, half-width and strength scales.
    pub shape_spec2: [[f32; 4]; MAX_SHAPES],
    /// Each shape's ring: how far down it is moved, its stroke, its Gaussian deviation and how much it darkens the face.
    pub shape_ring: [[f32; 4]; MAX_SHAPES],
    /// Each shape's control outline: opacity, width; the share of its key and fill light that is the highlight matrix's
    /// colour; 0.
    pub shape_border: [[f32; 4]; MAX_SHAPES],
    /// The outline the container is cut to: x, y, w, h.
    pub cut: [f32; 4],
    /// How far its corners reach along the sides, the exponent of their superellipse, 1 when the container is cut and
    /// 0 when not, and 1 when the edge line keeps its strength over any backdrop (0 when it follows its brightness).
    pub cut_shape: [f32; 4],
    /// The blur's weight at the top and at the bottom of the shapes ([`Container::blur_ramp`]), the magnification
    /// about the first shape's centre ([`Container::lens`]), and the width of the edge ([`Container::edge`]).
    pub lens: [f32; 4],
    /// The edge's straight colour and opacity.
    pub edge: [f32; 4],
    /// The rim outside the edge: where its strip begins and ends outside the outline, the shift of each channel per
    /// unit of its brightness response (negative: darker; 0 for none), 0. Its arcs face the ends of the soft band's
    /// axis, `light`, as wide as its arcs.
    pub rim_outside: [f32; 4],
}

impl GlassUniform {
    /// The uniform of `container` (output-local points) drawn at `scale` pixels per point: `uv` maps target pixels to
    /// the capture's texture coordinates (`[[a, b, c], [d, e, f]]`), `quad` is where it draws in a `target` that size, and
    /// `opacity` fades it.
    pub fn new(
        container: &Container,
        scale: f32,
        uv: [[f32; 3]; 2],
        quad: [f32; 4],
        target: [f32; 2],
        opacity: f32,
    ) -> Self {
        let p = &container.params;
        let mut u: Self = bytemuck::Zeroable::zeroed();
        u.quad = quad;
        let soft = if container.edge.shows() {
            container.edge.soft * scale
        } else {
            0.0
        };
        u.uv_x = [uv[0][0], uv[0][1], uv[0][2], soft];
        let layered = if container.looks_differ() { 1.0 } else { 0.0 };
        u.uv_y = [uv[1][0], uv[1][1], uv[1][2], layered];
        u.info = [
            container.shapes.len() as f32,
            container.spacing * scale,
            1.0,
            opacity.clamp(0.0, 1.0),
        ];
        u.levels = [p.shadow.edge_line, p.veil[1], target[0], target[1]];
        u.fill = [
            p.blur.fill_darken,
            p.blur.fill_lighten,
            p.blur.fill_normal,
            p.veil[0],
        ];
        u.bleed = [
            p.bleed.white,
            p.bleed.black,
            p.bleed.saturation,
            if p.bleed.darken { 1.0 } else { 0.0 },
        ];
        let h = &p.highlight;
        u.light = [h.angle, h.spread, h.curvature, h.glow_fill];
        u.glow = [
            h.glow_amount,
            h.glow_height * scale,
            h.glow_offset * scale,
            p.shadow.outline_opacity,
        ];
        u.diffuse = [
            h.diffuse.height_scale,
            h.diffuse.spread_scale,
            h.diffuse.amount_scale,
            p.shadow.edge_line_sides,
        ];
        for r in 0..3 {
            let m = &h.matrix[r * 5..r * 5 + 5];
            u.highlight_matrix[r] = [m[0], m[1], m[2], m[3] + m[4]];
        }
        let s = &p.shadow;
        u.shadow = [
            s.offset[0] * scale,
            s.offset[1] * scale,
            s.opacity * (s.amount / 100.0).clamp(0.0, 1.0) * s.fill[3],
            s.height * scale,
        ];
        u.shadow_color = [s.fill[0], s.fill[1], s.fill[2], s.outline_width * scale];
        for (i, (shape, v)) in container.shapes.iter().zip(&container.values).enumerate() {
            u.shapes[i] = shape.rect.map(|x| x * scale);
            let (extent, exponent) = shape.corner_shape();
            let look = ShapeLook::of(container.recipe(i));
            u.shape_a[i] = [
                extent * scale,
                v.refraction_amount * scale,
                v.refraction_height * scale,
                look.bleed_opacity.clamp(0.0, 1.0),
            ];
            u.shape_b[i] = [v.bleed_height * scale, v.max_luma, v.blur_level, exponent];
            u.shape_c[i] = [
                v.bleed_amount * scale,
                v.outer_amount * scale,
                v.outer_height * scale,
                look.outer_opacity.clamp(0.0, 1.0),
            ];
            u.shape_face[i] = look.face;
            u.shape_fill[i] = look.fill;
            u.shape_tint[i] = look.tint;
            u.shape_rim[i] = [
                // a rim outside the edge draws nothing inside it
                if p.highlight.outside {
                    0.0
                } else {
                    look.rim[0]
                },
                look.rim[1],
                RIM_DEPTH_PER_HEIGHT * look.rim[2] * scale,
                0.0,
            ];
            let sp = look.specular;
            u.shape_spec[i] = [sp[0], sp[1], sp[2] * scale, sp[3]];
            u.shape_spec2[i] = [sp[4], sp[5], sp[6], sp[7]];
            let r = look.ring;
            u.shape_ring[i] = [r[0] * scale, r[1] * scale, r[2] * scale, r[3]];
            u.shape_border[i] = [look.border[0], look.border[1] * scale, sp[8], 0.0];
        }
        if let Some(cut) = container.cut {
            u.cut = cut.rect.map(|x| x * scale);
            u.cut_shape = [cut.extent * scale, cut.exponent, 1.0, 0.0];
        }
        u.cut_shape[3] = if p.shadow.edge_line_even { 1.0 } else { 0.0 };
        u.lens = [
            container.blur_ramp[0].clamp(0.0, 1.0),
            container.blur_ramp[1].clamp(0.0, 1.0),
            container.lens.max(1.0),
            if container.edge.shows() {
                container.edge.width * scale
            } else {
                0.0
            },
        ];
        u.edge = container.edge.color;
        let h = &p.highlight;
        if h.outside && h.height > 0.0 {
            let (lo, hi) = (-h.offset - 0.5 * h.height, -h.offset + 0.5 * h.height);
            let shift = RIM_OUTSIDE_GAIN * h.amount * h.color_bias;
            u.rim_outside = [lo * scale, hi * scale, shift, 0.0];
        }
        u
    }

    /// The uniform with the edges of its square shapes and of its cut on the nearest whole pixel: a square edge is then
    /// hard and meets what is drawn beside it on the same pixel, with no column of partial coverage between them.
    /// Rounded shapes keep their edges where they are.
    pub fn on_pixels(mut self) -> Self {
        let n = (self.info[0].max(0.0) as usize).min(MAX_SHAPES);
        for i in 0..n {
            if self.shape_a[i][0] <= 0.0 {
                self.shapes[i] = whole_pixels(self.shapes[i]);
            }
        }
        if self.cut_shape[2] > 0.0 {
            self.cut = whole_pixels(self.cut);
        }
        self
    }
}

/// How far below a half pixel an edge still counts as on it: the error of a shape placed from its surface's device
/// origin, which is no exact number of points at a fractional scale, carried in single precision.
const HALF_PIXEL_TOLERANCE: f32 = 1.0 / 1024.0;

/// `[x, y, w, h]` with each edge on the nearest whole pixel, a half towards positive infinity (within
/// [`HALF_PIXEL_TOLERANCE`]), as the content of the surface the shape belongs to rounds it in its own pixels.
fn whole_pixels([x, y, w, h]: [f32; 4]) -> [f32; 4] {
    let pixel = |v: f32| (v + 0.5 + HALF_PIXEL_TOLERANCE).floor();
    let (x0, y0) = (pixel(x), pixel(y));
    [x0, y0, pixel(x + w) - x0, pixel(y + h) - y0]
}
