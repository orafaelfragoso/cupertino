//! Glass: the CPU model of the background pass and of the foreground colour matrix, and the uniform block the
//! shaders read (`shaders/glass.wgsl`, `shaders/gles/glass.frag`).
//!
//! A glass *container* holds up to [`MAX_SHAPES`] shapes (rounded rectangles or capsules). Its backdrop is captured
//! once, at half the output's scale, over the bounds of every shape plus the reach of the blur and the refraction, gaps
//! included. The shapes join into one signed distance field: a smooth union whose blend width is the container's
//! spacing (0 is a hard union). Every pixel of the field is then shaded in this order:
//!
//! 1. **Blur.** Four levels of the backdrop blurred with `radius · 2^i` points are laid over each other, widest first,
//!    each narrower level mixed in at its opacity; then the blur fill (the backdrop at the fill radius) is blended in
//!    darken, lighten and normal mode at their opacities.
//! 2. **Colour matrix.** Saturation about the Rec. 709 luma of the encoded colour; then on linear light the `white`
//!    gain, a per-channel limit at `clamp` and a luminance limit at `max_luma · clamp` that keeps the hue; then,
//!    encoded, the `black` lift
//!    `v + black · (1 − v)` and the fill colour over it.
//! 3. **Inner refraction.** Inside a band `height` wide along the edge, the face is read further out along the field's
//!    normal, by `−amount · REFRACTION_REACH_PER_AMOUNT · (1 − t)²` points where `t` runs from 0 at the edge to 1 at the
//!    band's inner side: what lies past the edge bends into the rim.
//! 4. **Bleed.** The widest blur level, mapped to the encoded range `[black, white]` after its own saturation, blends
//!    into the face over a band `height` wide at `amount` percent: lighten on dark glass, darken on light glass.
//! 5. **Key/fill highlight.** A thin rim band (half-width `height`, centred `offset` from the edge) adds light along
//!    the axis through `angle`: the key colour `matrix · face` at `amount` on the arc of edge facing `angle`, and the
//!    same light at `amount · (1 + color_bias)` on the arc facing the opposite way, so the rim reads lit at both ends
//!    of the axis. Each arc fades with the angle from its end of the axis ([`highlight_lobe`]: width `spread`,
//!    exponent `curvature`) to nothing towards the sides.
//! 6. **Shadow and ring shadow.** Outside the shape: a drop shadow offset by `offset`, blurred by `height` at
//!    `amount` percent of its fill's alpha, and a ring `stroke` wide, offset by `ring_offset` and blurred by
//!    `ring_blur`. Each is drawn at its opacity, which is 0 at rest.
//!
//! The foreground of text and symbols on glass is [`foreground`]: the face under the glyph through the mode's 4×5 matrix,
//! mixed over the face by the glyph's coverage.
//!
//! Colours are straight sRGB-encoded `f32` unless a function says otherwise.

use cupertino_theme::glass::{
    apply_matrix, ColorMatrix, GlassParams, BLUR_LEVELS, REFRACTION_REACH_PER_AMOUNT,
};

pub use cupertino_theme::glass::{GlassMode, GlassStyle, GLASS_BACKDROP_SCALE};

/// Most shapes one container joins.
pub const MAX_SHAPES: usize = 16;

/// Widest blend between the shapes of a container, in points. The union bulges up to a quarter of the blend width
/// past the shapes, so this bounds how far a container's glass reaches beyond them.
pub const MAX_SPACING: f32 = 64.0;

/// Rec. 709 luminance weights of linear light.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub type Rgb = [f32; 3];

/// One shape of a container: a rectangle `[x, y, w, h]` with circular corners of `radius` (clamped to half the short
/// side; a capsule has exactly that).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassShape {
    pub rect: [f32; 4],
    pub radius: f32,
}

impl GlassShape {
    pub fn new(rect: [f32; 4], radius: f32) -> Self {
        Self { rect, radius }
    }

    /// A capsule: the corners are half the short side.
    pub fn capsule(rect: [f32; 4]) -> Self {
        Self::new(rect, f32::INFINITY)
    }

    pub fn short_side(&self) -> f32 {
        self.rect[2].min(self.rect[3]).max(0.0)
    }

    /// The corner radius after fitting to the rectangle.
    pub fn fitted_radius(&self) -> f32 {
        self.radius.clamp(0.0, self.short_side() * 0.5)
    }

    /// Signed distance from `p` to the shape's outline, negative inside.
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        let [x, y, w, h] = self.rect;
        let r = self.fitted_radius();
        let q = [
            (p[0] - (x + w * 0.5)).abs() - (w * 0.5 - r),
            (p[1] - (y + h * 0.5)).abs() - (h * 0.5 - r),
        ];
        let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2)).sqrt();
        outside + q[0].max(q[1]).min(0.0) - r
    }
}

/// A smooth union of two distances with blend width `k` (0 is a hard union), and the weight of `a` in the blend, for
/// mixing per-shape values the same way.
pub fn smooth_union(a: f32, b: f32, k: f32) -> (f32, f32) {
    if k <= 0.0 {
        return if a <= b { (a, 1.0) } else { (b, 0.0) };
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    (b + (a - b) * h - k * h * (1.0 - h), h)
}

/// The union field of `shapes` at `p` with blend width `k`, and each shape's weight in it.
pub fn union_field(shapes: &[GlassShape], k: f32, p: [f32; 2]) -> (f32, Vec<f32>) {
    let mut weights = vec![0.0; shapes.len()];
    let Some(first) = shapes.first() else {
        return (f32::INFINITY, weights);
    };
    let mut d = first.sdf(p);
    weights[0] = 1.0;
    for (i, s) in shapes.iter().enumerate().skip(1) {
        let (u, h) = smooth_union(s.sdf(p), d, k);
        for w in &mut weights[..i] {
            *w *= 1.0 - h;
        }
        weights[i] = h;
        d = u;
    }
    (d, weights)
}

/// Outward unit normal of the field at `p` (central differences one unit apart); zero where the field is flat.
pub fn field_normal(shapes: &[GlassShape], k: f32, p: [f32; 2]) -> [f32; 2] {
    let f = |dx: f32, dy: f32| union_field(shapes, k, [p[0] + dx, p[1] + dy]).0;
    let g = [f(0.5, 0.0) - f(-0.5, 0.0), f(0.0, 0.5) - f(0.0, -0.5)];
    let len = (g[0] * g[0] + g[1] * g[1]).sqrt();
    if len < 1e-6 {
        [0.0, 0.0]
    } else {
        [g[0] / len, g[1] / len]
    }
}

pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

pub fn srgb_encode(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn luma(c: Rgb) -> f32 {
    c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// Saturation `s` about the Rec. 709 luma of the encoded colour `c`: the colour moves away from (or towards) the grey of
/// its luma by `s`, on encoded values, so the weaker channels of a strong colour fall off as they look to, rather than
/// as fast as in linear light, where a small encoded value is a much smaller share of the light.
pub fn saturate_encoded(c: Rgb, s: f32) -> Rgb {
    let y = luma(c);
    c.map(|v| (y + (v - y) * s).max(0.0))
}

/// Stage 1: the blur levels laid over each other (widest first, each narrower one at its opacity).
pub fn blend_levels(levels: [Rgb; BLUR_LEVELS], opacity: [f32; BLUR_LEVELS]) -> Rgb {
    let mut c = levels[BLUR_LEVELS - 1];
    for i in (0..BLUR_LEVELS - 1).rev() {
        c = mix(c, levels[i], opacity[i].clamp(0.0, 1.0));
    }
    c
}

/// Stage 1, second part: the blur fill `fill` blended over the blurred backdrop `c`.
pub fn blur_fill(c: Rgb, fill: Rgb, p: &GlassParams) -> Rgb {
    let b = &p.blur;
    let darker: Rgb = std::array::from_fn(|i| c[i].min(fill[i]));
    let c = mix(c, darker, b.fill_darken);
    let lighter: Rgb = std::array::from_fn(|i| c[i].max(fill[i]));
    let c = mix(c, lighter, b.fill_lighten);
    mix(c, fill, b.fill_normal)
}

/// Stage 2: the face colour of the blurred backdrop `c`, with the face's `max_luma` replaced by `max_luma` (it depends
/// on the shape's size).
pub fn face_color(c: Rgb, p: &GlassParams, max_luma: f32) -> Rgb {
    let f = &p.face;
    let lin = saturate_encoded(c, f.saturation).map(srgb_decode);
    let mut lin = lin.map(|v| (v * f.white).min(f.clamp));
    let limit = max_luma * f.clamp;
    let y = luma(lin);
    if y > limit && y > 0.0 {
        lin = lin.map(|v| v * limit / y);
    }
    let enc = lin.map(|v| {
        let e = srgb_encode(v).min(1.0);
        e + f.black * (1.0 - e)
    });
    mix(enc, [f.fill[0], f.fill[1], f.fill[2]], f.fill[3])
}

/// Stage 3: where the face is read for a pixel `d` from the edge (negative inside) with outward `normal`, as an
/// offset in points. Zero outside the band and without refraction.
pub fn refraction_offset(d: f32, normal: [f32; 2], amount: f32, height: f32) -> [f32; 2] {
    if height <= 0.0 || amount == 0.0 || d > 0.0 {
        return [0.0, 0.0];
    }
    let t = (-d / height).clamp(0.0, 1.0);
    let reach = -amount * REFRACTION_REACH_PER_AMOUNT * (1.0 - t) * (1.0 - t);
    [normal[0] * reach, normal[1] * reach]
}

/// Stage 4: the bleed of the backdrop colour `wide` (the widest blur level) into the face `c`, `d` from the edge.
pub fn bleed(c: Rgb, wide: Rgb, d: f32, p: &GlassParams, amount: f32, height: f32) -> Rgb {
    let b = &p.bleed;
    if amount <= 0.0 || height <= 0.0 {
        return c;
    }
    let t = (-d / height).clamp(0.0, 1.0);
    let w = (amount / 100.0).clamp(0.0, 1.0) * (1.0 - t) * (1.0 - t);
    let tone =
        saturate_encoded(wide, b.saturation).map(|v| b.black + (b.white - b.black) * v.min(1.0));
    let blended: Rgb = std::array::from_fn(|i| {
        if b.darken {
            c[i].min(tone[i])
        } else {
            c[i].max(tone[i])
        }
    });
    mix(c, blended, w)
}

/// Angle between two directions in radians, in `[0, π]`.
fn angle_between(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    d.min(std::f32::consts::TAU - d)
}

/// How strongly an edge whose outward normal points at `normal_angle` is lit from `light`: a raised cosine of the angle
/// between them, full where they agree and falling to nothing [`LOBE_REACH`] of `spread` away, with exponent
/// `curvature`. For a light from the top and a spread of 120° the lobe ends at the sides, still lighting much of the
/// corners on its way.
pub fn highlight_lobe(normal_angle: f32, light: f32, spread: f32, curvature: f32) -> f32 {
    let reach = spread * LOBE_REACH;
    if reach <= 0.0 {
        return 0.0;
    }
    let a = angle_between(normal_angle, light);
    if a >= reach {
        return 0.0;
    }
    (0.5 + 0.5 * (a / reach * std::f32::consts::PI).cos()).powf(curvature.max(1e-3))
}

/// How far from its end of the axis a highlight arc reaches, as a share of the spread.
pub const LOBE_REACH: f32 = 0.75;

/// Stage 5: the key and fill rim lights on the face `c` at distance `d` (negative inside, points) from the edge, with
/// outward `normal` in y-down coordinates: the key colour added at the end of the axis the light comes from, and at
/// the other end the same light weakened by the colour bias.
pub fn highlight(c: Rgb, d: f32, normal: [f32; 2], p: &GlassParams) -> Rgb {
    let h = &p.highlight;
    let rim = h.amount > 0.0 && h.height > 0.0;
    let glow = h.glow_amount > 0.0 && h.glow_height > 0.0;
    if !(rim || glow) || (normal[0] == 0.0 && normal[1] == 0.0) {
        return c;
    }
    let mut band = 0.0;
    if rim {
        let x = (d - h.offset) / h.height;
        band += h.amount * (-x * x).exp();
    }
    if glow {
        let x = (d - h.glow_offset) / h.glow_height;
        band += h.glow_amount * (-x * x).exp();
    }
    // y up for the angle
    let normal_angle = (-normal[1]).atan2(normal[0]);
    let key = highlight_lobe(normal_angle, h.angle, h.spread, h.curvature);
    let fill = highlight_lobe(
        normal_angle,
        h.angle + std::f32::consts::PI,
        h.spread,
        h.curvature,
    );
    let w = band * (key + fill * (1.0 + h.color_bias).max(0.0));
    let light = matrix_rgb(&h.matrix, c);
    std::array::from_fn(|i| (c[i] + light[i] * w).min(1.0))
}

fn matrix_rgb(m: &ColorMatrix, c: Rgb) -> Rgb {
    let o = apply_matrix(m, [c[0], c[1], c[2], 1.0]);
    [o[0], o[1], o[2]].map(|v| v.clamp(0.0, 1.0))
}

/// Coverage of a Gaussian-blurred edge at signed distance `d` for blur radius `r` (a smooth step three radii wide).
fn blurred_coverage(d: f32, r: f32) -> f32 {
    if r <= 0.0 {
        return if d <= 0.0 { 1.0 } else { 0.0 };
    }
    let t = (0.5 - d / (3.0 * r)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Stage 6: the alpha of the drop shadow and of the ring shadow at a pixel outside the shape. `d_shadow` is the field
/// at the pixel minus the drop shadow's offset, `d_ring` the field minus the ring's offset.
pub fn shadow_alpha(d_shadow: f32, d_ring: f32, p: &GlassParams) -> (f32, f32) {
    let s = &p.shadow;
    let drop = s.opacity
        * (s.amount / 100.0).clamp(0.0, 1.0)
        * s.fill[3]
        * blurred_coverage(d_shadow, s.height);
    let ring = if s.ring_stroke > 0.0 {
        // the band between the outline and `stroke` inside it, blurred
        let inner = blurred_coverage(d_ring + s.ring_stroke, s.ring_blur);
        let outer = blurred_coverage(d_ring, s.ring_blur);
        s.ring_opacity * (outer - inner).max(0.0)
    } else {
        0.0
    };
    (drop, ring)
}

/// The foreground of a glyph on glass: the face `face` under it through `matrix`, mixed over the face by `coverage`
/// (the glyph's coverage times its alpha).
pub fn foreground(face: Rgb, matrix: &ColorMatrix, coverage: f32) -> Rgb {
    mix(face, matrix_rgb(matrix, face), coverage.clamp(0.0, 1.0))
}

/// Size-dependent values of one shape, as the shaders take them per shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeValues {
    pub refraction_amount: f32,
    pub refraction_height: f32,
    pub bleed_amount: f32,
    pub bleed_height: f32,
    pub max_luma: f32,
    pub narrow_opacity: f32,
}

impl ShapeValues {
    pub fn of(p: &GlassParams) -> Self {
        Self {
            refraction_amount: p.refraction.amount,
            refraction_height: p.refraction.height,
            bleed_amount: p.bleed.amount,
            bleed_height: p.bleed.height,
            max_luma: p.face.max_luma,
            narrow_opacity: p.blur.level_opacity[0],
        }
    }

    fn mix(a: Self, b: Self, t: f32) -> Self {
        let m = |x: f32, y: f32| x + (y - x) * t;
        Self {
            refraction_amount: m(a.refraction_amount, b.refraction_amount),
            refraction_height: m(a.refraction_height, b.refraction_height),
            bleed_amount: m(a.bleed_amount, b.bleed_amount),
            bleed_height: m(a.bleed_height, b.bleed_height),
            max_luma: m(a.max_luma, b.max_luma),
            narrow_opacity: m(a.narrow_opacity, b.narrow_opacity),
        }
    }
}

/// A container ready to shade: its shapes (points), blend width, the mode and each shape's recipe.
#[derive(Clone, Debug)]
pub struct Container {
    pub shapes: Vec<GlassShape>,
    pub spacing: f32,
    pub mode: GlassMode,
    /// The recipe of the container's values that do not depend on size (from its largest shape).
    pub params: GlassParams,
    pub values: Vec<ShapeValues>,
}

impl Container {
    /// The container of `shapes` (at most [`MAX_SHAPES`]) joined with blend width `spacing` (at most [`MAX_SPACING`];
    /// negative or not finite is 0).
    pub fn new(shapes: Vec<GlassShape>, spacing: f32, mode: GlassMode) -> Self {
        let mut shapes = shapes;
        shapes.truncate(MAX_SHAPES);
        let largest = shapes
            .iter()
            .map(GlassShape::short_side)
            .fold(0.0f32, f32::max);
        let values = shapes
            .iter()
            .map(|s| ShapeValues::of(&GlassParams::new(mode, s.short_side())))
            .collect();
        Self {
            shapes,
            spacing: if spacing.is_finite() {
                spacing.clamp(0.0, MAX_SPACING)
            } else {
                0.0
            },
            mode,
            params: GlassParams::new(mode, largest),
            values,
        }
    }

    /// The bounds of every shape, gaps included.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let first = self.shapes.first()?;
        let mut b = [
            first.rect[0],
            first.rect[1],
            first.rect[0] + first.rect[2],
            first.rect[1] + first.rect[3],
        ];
        for s in &self.shapes[1..] {
            b[0] = b[0].min(s.rect[0]);
            b[1] = b[1].min(s.rect[1]);
            b[2] = b[2].max(s.rect[0] + s.rect[2]);
            b[3] = b[3].max(s.rect[1] + s.rect[3]);
        }
        Some([b[0], b[1], b[2] - b[0], b[3] - b[1]])
    }

    /// What of the backdrop the container reads: its bounds grown by the reach of its blur and refraction, and by how
    /// far the smooth union bulges past the shapes (a quarter of the blend width).
    pub fn sample_area(&self) -> Option<[f32; 4]> {
        let [x, y, w, h] = self.bounds()?;
        let bulge = if self.shapes.len() > 1 {
            self.spacing * 0.25
        } else {
            0.0
        };
        let r = self.params.reach() + bulge;
        Some([x - r, y - r, w + 2.0 * r, h + 2.0 * r])
    }

    /// The field and the shape values blended like the field at `p`.
    pub fn field(&self, p: [f32; 2]) -> (f32, ShapeValues) {
        let (d, weights) = union_field(&self.shapes, self.spacing, p);
        let mut v = self.values[0];
        let mut total = weights[0];
        for (i, w) in weights.iter().enumerate().skip(1) {
            if total + w > 0.0 {
                v = ShapeValues::mix(v, self.values[i], w / (total + w));
            }
            total += w;
        }
        (d, v)
    }

    /// Shade the pixel at `p` (points): `sample(level, q)` reads blur level `level` (0..4, and 4 for the blur fill) of
    /// the backdrop at `q`. Returns the colour and its coverage (glass inside, shadow outside).
    pub fn shade(&self, p: [f32; 2], sample: &dyn Fn(usize, [f32; 2]) -> Rgb) -> (Rgb, f32) {
        let params = &self.params;
        let (d, v) = self.field(p);
        let normal = field_normal(&self.shapes, self.spacing, p);
        if d > 0.5 {
            let s = &params.shadow;
            let d_shadow = self.field([p[0] - s.offset[0], p[1] - s.offset[1]]).0;
            let d_ring = self.field([p[0], p[1] - s.ring_offset]).0;
            let (drop, ring) = shadow_alpha(d_shadow, d_ring, params);
            let a = (drop + ring).min(1.0);
            return ([s.fill[0], s.fill[1], s.fill[2]], a);
        }
        let mut opacity = params.blur.level_opacity;
        opacity[0] = v.narrow_opacity;
        opacity[1] = v.narrow_opacity;
        let face_at = |q: [f32; 2]| {
            let levels: [Rgb; BLUR_LEVELS] = std::array::from_fn(|i| sample(i, q));
            let c = blur_fill(
                blend_levels(levels, opacity),
                sample(BLUR_LEVELS, q),
                params,
            );
            face_color(c, params, v.max_luma)
        };
        let off = refraction_offset(d, normal, v.refraction_amount, v.refraction_height);
        let mut c = face_at([p[0] + off[0], p[1] + off[1]]);
        c = bleed(
            c,
            sample(BLUR_LEVELS - 1, p),
            d,
            params,
            v.bleed_amount,
            v.bleed_height,
        );
        c = highlight(c, d, normal, params);
        (c, (0.5 - d).clamp(0.0, 1.0))
    }
}

/// Uniform block of the glass composite (`Glass` in `shaders/glass.wgsl` and `shaders/gles/glass.frag`), std140
/// with 16-byte members only. Lengths are in target pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlassUniform {
    /// Where the composite draws: x, y, w, h in target pixels (y down).
    pub quad: [f32; 4],
    /// Affine map from target pixels to the capture's texture coordinates: `u = a·x + b·y + c` (`[a, b, c, 0]`).
    pub uv_x: [f32; 4],
    /// The same for `v`.
    pub uv_y: [f32; 4],
    /// Number of shapes, blend width, antialiasing width, overall opacity.
    pub info: [f32; 4],
    /// Opacity of blur levels 2 and 3 (levels 0 and 1 are per shape), and the target's width and height.
    pub levels: [f32; 4],
    /// Blur fill opacities: darken, lighten, normal, 0.
    pub fill: [f32; 4],
    /// Face: saturation, white, black, clamp.
    pub face: [f32; 4],
    /// Face fill, straight sRGB.
    pub face_fill: [f32; 4],
    /// Bleed: white, black, saturation, darken (0/1).
    pub bleed: [f32; 4],
    /// Highlight: amount, angle, spread, half-width.
    pub highlight: [f32; 4],
    /// Highlight: offset, colour bias, curvature, 0.
    pub highlight2: [f32; 4],
    /// The soft band of light: strength, half-width, offset, 0.
    pub glow: [f32; 4],
    /// Key colour matrix rows: `[r, g, b coefficients, offset (+ alpha coefficient)]`.
    pub highlight_matrix: [[f32; 4]; 3],
    /// Drop shadow: offset x, offset y, alpha, blur radius.
    pub shadow: [f32; 4],
    /// Shadow colour (rgb) and the ring's alpha.
    pub shadow_color: [f32; 4],
    /// Ring: offset, stroke, blur, 0.
    pub ring: [f32; 4],
    /// Each shape: x, y, w, h.
    pub shapes: [[f32; 4]; MAX_SHAPES],
    /// Each shape: corner radius, refraction offset at the edge, refraction band, bleed fraction.
    pub shape_a: [[f32; 4]; MAX_SHAPES],
    /// Each shape: bleed band, largest luminance, opacity of levels 0 and 1, 0.
    pub shape_b: [[f32; 4]; MAX_SHAPES],
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
        u.uv_x = [uv[0][0], uv[0][1], uv[0][2], 0.0];
        u.uv_y = [uv[1][0], uv[1][1], uv[1][2], 0.0];
        u.info = [
            container.shapes.len() as f32,
            container.spacing * scale,
            1.0,
            opacity.clamp(0.0, 1.0),
        ];
        u.levels = [
            p.blur.level_opacity[2],
            p.blur.level_opacity[3],
            target[0],
            target[1],
        ];
        u.fill = [
            p.blur.fill_darken,
            p.blur.fill_lighten,
            p.blur.fill_normal,
            0.0,
        ];
        u.face = [p.face.saturation, p.face.white, p.face.black, p.face.clamp];
        u.face_fill = p.face.fill;
        u.bleed = [
            p.bleed.white,
            p.bleed.black,
            p.bleed.saturation,
            if p.bleed.darken { 1.0 } else { 0.0 },
        ];
        let h = &p.highlight;
        u.highlight = [h.amount, h.angle, h.spread, h.height * scale];
        u.highlight2 = [h.offset * scale, h.color_bias, h.curvature, 0.0];
        u.glow = [
            h.glow_amount,
            h.glow_height * scale,
            h.glow_offset * scale,
            0.0,
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
        u.shadow_color = [s.fill[0], s.fill[1], s.fill[2], s.ring_opacity];
        u.ring = [
            s.ring_offset * scale,
            s.ring_stroke * scale,
            s.ring_blur * scale,
            0.0,
        ];
        for (i, (shape, v)) in container.shapes.iter().zip(&container.values).enumerate() {
            u.shapes[i] = shape.rect.map(|x| x * scale);
            u.shape_a[i] = [
                shape.fitted_radius() * scale,
                -v.refraction_amount * REFRACTION_REACH_PER_AMOUNT * scale,
                v.refraction_height * scale,
                (v.bleed_amount / 100.0).clamp(0.0, 1.0),
            ];
            u.shape_b[i] = [v.bleed_height * scale, v.max_luma, v.narrow_opacity, 0.0];
        }
        u
    }
}

impl Container {
    /// Blur radius in points of each level of the backdrop, and whether any shape uses it: through the levels' blend,
    /// or the widest level through the bleed.
    pub fn levels(&self) -> [(f32, bool); BLUR_LEVELS] {
        let radii = self.params.level_radii();
        let mut used = [false; BLUR_LEVELS];
        for v in &self.values {
            let mut p = self.params;
            p.blur.level_opacity[0] = v.narrow_opacity;
            p.blur.level_opacity[1] = v.narrow_opacity;
            for (u, w) in used.iter_mut().zip(p.level_weights()) {
                *u |= w > cupertino_theme::glass::LEVEL_WEIGHT_EPSILON;
            }
            used[BLUR_LEVELS - 1] |= v.bleed_amount > 0.0 && v.bleed_height > 0.0;
        }
        std::array::from_fn(|i| (radii[i], used[i]))
    }

    /// Blur radius in points of the blur fill, if it shows.
    pub fn fill_radius(&self) -> Option<f32> {
        let b = &self.params.blur;
        (b.fill_darken + b.fill_lighten + b.fill_normal > 0.0 && b.fill_radius > 0.0)
            .then_some(b.fill_radius)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_theme::glass::foreground_matrix;

    fn params(dark: bool) -> GlassParams {
        GlassParams::new(
            GlassMode {
                dark,
                ..GlassMode::default()
            },
            100.0,
        )
    }

    fn close(a: f32, b: f32, e: f32) -> bool {
        (a - b).abs() <= e
    }

    #[test]
    fn shape_distance_is_exact_for_rounded_rectangles() {
        let s = GlassShape::new([0.0, 0.0, 100.0, 40.0], 10.0);
        assert!(
            close(s.sdf([50.0, 20.0]), -20.0, 1e-5),
            "centre: half the short side inside"
        );
        assert!(close(s.sdf([110.0, 20.0]), 10.0, 1e-5));
        // the corner arc: 10 past the corner's centre along the diagonal
        let c = 10.0 - 10.0 / 2f32.sqrt();
        assert!(close(s.sdf([c, c]), 0.0, 1e-4));
        let capsule = GlassShape::capsule([0.0, 0.0, 100.0, 40.0]);
        assert_eq!(capsule.fitted_radius(), 20.0);
    }

    #[test]
    fn a_union_without_spacing_is_hard_and_spacing_bridges_the_gap() {
        // two 150 × 100 shapes 5 apart, as in a container of two buttons: a point in the gap
        let a = GlassShape::new([0.0, 0.0, 150.0, 100.0], 18.0);
        let b = GlassShape::new([155.0, 0.0, 150.0, 100.0], 18.0);
        let gap = [152.5, 50.0];
        let (hard, _) = union_field(&[a, b], 0.0, gap);
        assert!(
            close(hard, 2.5, 1e-4),
            "hard union: the gap stays open ({hard})"
        );
        let (smooth, w) = union_field(&[a, b], 20.0, gap);
        assert!(
            smooth < 0.0,
            "spacing 20 joins the shapes across a 5 point gap ({smooth})"
        );
        assert!(close(w[0], 0.5, 1e-4) && close(w[1], 0.5, 1e-4));
        // far from the gap the field is the nearer shape's
        let (far, w) = union_field(&[a, b], 20.0, [10.0, 50.0]);
        assert!(close(far, -10.0, 1e-4) && w[0] == 1.0);
    }

    #[test]
    fn level_blend_puts_the_narrow_levels_over_the_wide_ones() {
        let levels = [[1.0; 3], [0.5; 3], [0.25; 3], [0.0; 3]];
        let c = blend_levels(levels, params(true).blur.level_opacity);
        // 0.55 of level 0, 0.2475 of level 1, 0.2025 of level 2
        assert!(close(c[0], 0.55 + 0.2475 * 0.5 + 0.2025 * 0.25, 1e-5));
        assert_eq!(blend_levels(levels, [1.0, 1.0, 0.0, 0.0])[0], 1.0);
    }

    #[test]
    fn blur_fill_drops_thin_bright_detail_on_dark_glass_and_dark_detail_on_light_glass() {
        // a bright line whose wider blur is darker
        let (line, around) = ([0.9; 3], [0.3; 3]);
        let dark = blur_fill(line, around, &params(true));
        assert!(dark[0] < 0.4, "darken 0.9 then normal 0.5: {dark:?}");
        let light = blur_fill(around, line, &params(false));
        assert!(light[0] > 0.8, "lighten 0.9 then normal 0.5: {light:?}");
        // flat backdrops pass unchanged
        assert_eq!(blur_fill([0.6; 3], [0.6; 3], &params(true)), [0.6; 3]);
    }

    #[test]
    fn dark_face_limits_bright_backdrops_and_keeps_saturated_ones() {
        let p = params(true);
        // a near-white backdrop is held at the luminance limit 0.375 · 1.3083 (≈ 186 of 255), then lifted by 0.08
        let white = face_color([0.953; 3], &p, p.face.max_luma);
        let held = srgb_encode(0.375 * 1.308_310_9);
        let expect = held + 0.08 * (1.0 - held);
        assert!(close(white[0], expect, 2e-3), "{white:?} vs {expect}");
        // a saturated blue under the limit keeps its hue and grows more saturated
        let blue = face_color([0.12, 0.27, 0.93], &p, p.face.max_luma);
        assert!(blue[2] > 0.93 && blue[0] < 0.2, "{blue:?}");
        // black is lifted
        let black = face_color([0.0; 3], &p, p.face.max_luma);
        assert!(close(black[0], 0.08, 1e-4));
    }

    #[test]
    fn saturation_works_on_encoded_values_about_their_luma() {
        // greys and saturation 1 pass unchanged, and the luma of the encoded colour is kept
        assert_eq!(saturate_encoded([0.4; 3], 1.3), [0.4; 3]);
        let c = [0.157, 0.094, 0.655];
        assert_eq!(saturate_encoded(c, 1.0), c);
        let s = saturate_encoded(c, 1.3);
        assert!(close(luma(s), luma(c), 1e-6), "{s:?}");
        // each channel moves away from the luma by the factor
        let y = luma(c);
        for i in 0..3 {
            assert!(close(s[i] - y, (c[i] - y) * 1.3, 1e-6), "{s:?}");
        }
        // a channel pushed below black stops there
        assert_eq!(saturate_encoded([0.0, 0.0, 1.0], 3.0)[0], 0.0);
    }

    #[test]
    fn dark_face_keeps_a_milky_floor_over_a_strong_blue() {
        // a deep blue wallpaper behind a control tile (encoded 40, 24, 167 of 255)
        let p = GlassParams::new(
            GlassMode {
                dark: true,
                ..GlassMode::default()
            },
            64.0,
        );
        let face = face_color(
            [40.0 / 255.0, 24.0 / 255.0, 167.0 / 255.0],
            &p,
            p.face.max_luma,
        )
        .map(|v| v * 255.0);
        // about (61, 41, 220): brighter and bluer than the wallpaper, and the weak green stays well above the lift
        // alone (0.08, 20 of 255), which is where saturating linear light would leave it
        assert!(close(face[0], 61.0, 2.0), "{face:?}");
        assert!(close(face[1], 41.0, 2.0), "{face:?}");
        assert!(close(face[2], 220.0, 2.0), "{face:?}");
        // a strong red keeps green and blue near what the backdrop had (54 and 59 of 255), not lifted black
        let red = face_color(
            [227.0 / 255.0, 54.0 / 255.0, 59.0 / 255.0],
            &p,
            p.face.max_luma,
        );
        assert!(red[1] > 0.18 && red[2] > 0.2, "{red:?}");
    }

    #[test]
    fn the_large_panel_compresses_white_and_black_and_keeps_strong_colours() {
        let face = |dark: bool, style: GlassStyle, c: [f32; 3]| {
            let p = GlassParams::new(
                GlassMode {
                    dark,
                    style,
                    ..GlassMode::default()
                },
                400.0,
            );
            face_color(c.map(|v| v / 255.0), &p, p.face.max_luma).map(|v| v * 255.0)
        };
        let large = |c| face(true, GlassStyle::LargePanel, c);
        // dark: a white page behind comes out a mid grey (about 125) and black a dark grey (about 53); regular glass
        // of the same size keeps white much lighter (about 184) and black darker (about 36)
        let white = large([245.0; 3]);
        assert!(
            close(white[0], 125.0, 3.0) && close(white[0], white[2], 0.5),
            "{white:?}"
        );
        let black = large([15.0; 3]);
        assert!(close(black[0], 53.0, 3.0), "{black:?}");
        let regular_white = face(true, GlassStyle::Regular, [245.0; 3]);
        assert!(regular_white[0] > 175.0, "{regular_white:?}");
        let regular_black = face(true, GlassStyle::Regular, [15.0; 3]);
        assert!(regular_black[0] < 40.0, "{regular_black:?}");
        // a saturated red band keeps its colour and much of its brightness: about (205, 69, 73)
        let red = large([229.0, 55.0, 60.0]);
        assert!(close(red[0], 205.0, 4.0), "{red:?}");
        assert!(
            close(red[1], 69.0, 4.0) && close(red[2], 73.0, 4.0),
            "{red:?}"
        );
        assert!(
            red[0] > white[0] + 60.0,
            "brighter than the grey of white: {red:?}"
        );
        // light: the same order, gentler; the panel stays light but dimmer than regular light glass over black
        let light_black = face(false, GlassStyle::LargePanel, [15.0; 3]);
        let regular_light_black = face(false, GlassStyle::Regular, [15.0; 3]);
        assert!(light_black[0] > regular_light_black[0], "{light_black:?}");
        // a white page (or the bright centre of a picture) stays a light patch near the panel's tint, about 228,
        // rather than the glare regular light glass gives it
        let light_white = face(false, GlassStyle::LargePanel, [245.0; 3]);
        assert!(
            light_white[0] > 222.0 && light_white[0] < 234.0,
            "{light_white:?}"
        );
        let regular_light_white = face(false, GlassStyle::Regular, [245.0; 3]);
        assert!(
            regular_light_white[0] > light_white[0] + 10.0,
            "{regular_light_white:?}"
        );
    }

    /// The blurred backdrop `x` points along a backdrop that is `source(t)` at `t` points, through `p`: each blur level
    /// and the blur fill as a Gaussian of its radius, laid over each other as the shader does.
    fn blurred(p: &GlassParams, source: &impl Fn(f32) -> Rgb, x: f32) -> Rgb {
        let gauss = |sigma: f32| -> Rgb {
            let (step, mut sum, mut acc) = (0.125f32, 0.0f32, [0.0f32; 3]);
            let reach = (4.0 * sigma / step).ceil() as i32;
            for i in -reach..=reach {
                let t = i as f32 * step;
                let w = (-0.5 * (t / sigma).powi(2)).exp();
                let c = source(x - t);
                sum += w;
                for k in 0..3 {
                    acc[k] += w * c[k];
                }
            }
            acc.map(|v| v / sum)
        };
        let levels = p.level_radii().map(gauss);
        let c = blend_levels(levels, p.blur.level_opacity);
        blur_fill(c, gauss(p.blur.fill_radius), p)
    }

    /// The face's luminance (0–255) at `x` of [`blurred`], toned as the shader does.
    fn through(p: &GlassParams, source: &impl Fn(f32) -> Rgb, x: f32) -> f32 {
        luma(face_color(blurred(p, source, x), p, p.face.max_luma)) * 255.0
    }

    /// The large panel blurs twice as wide as regular glass of its size: a step from white to black behind it is
    /// blurred over about twice the distance (10 % to 90 % of the way), and a hairline bright stripe over black, like the
    /// lines of a busy picture, comes through much fainter, so fine detail melts into the colour around it instead of
    /// rippling through.
    #[test]
    fn the_large_panel_blurs_twice_as_wide_as_regular_glass() {
        for dark in [true, false] {
            let p = |style| {
                GlassParams::new(
                    GlassMode {
                        dark,
                        style,
                        ..GlassMode::default()
                    },
                    400.0,
                )
            };
            let (regular, large) = (p(GlassStyle::Regular), p(GlassStyle::LargePanel));
            let step = |t: f32| if t < 0.0 { [0.96; 3] } else { [0.06; 3] };
            // the distance over which the blurred step goes from 10 % to 90 % of its way, in points
            let width = |g: &GlassParams| {
                let xs: Vec<f32> = (-400..=400).map(|i| i as f32 * 0.25).collect();
                let ys: Vec<f32> = xs.iter().map(|&x| luma(blurred(g, &step, x))).collect();
                let (hi, lo) = (ys[0], ys[ys.len() - 1]);
                let at = |f: f32| {
                    let level = hi - (hi - lo) * f;
                    xs[ys.iter().position(|&y| y <= level).unwrap()]
                };
                at(0.9) - at(0.1)
            };
            let (w_regular, w_large) = (width(&regular), width(&large));
            assert!(
                w_large > 1.6 * w_regular && w_large < 2.4 * w_regular,
                "dark {dark}: {w_regular} pt and {w_large} pt"
            );
            // a bright stripe 6 points wide over black: how far its peak stands above the black around it
            let stripe = |t: f32| if t.abs() < 3.0 { [0.96; 3] } else { [0.06; 3] };
            let ripple = |g: &GlassParams| through(g, &stripe, 0.0) - through(g, &stripe, 200.0);
            let (r_regular, r_large) = (ripple(&regular), ripple(&large));
            assert!(
                r_large < 0.6 * r_regular,
                "dark {dark}: ripple {r_regular} and {r_large}"
            );
        }
    }

    #[test]
    fn light_face_stays_light_and_takes_its_white_fill() {
        let p = params(false);
        let white = face_color([0.953; 3], &p, p.face.max_luma);
        assert!(white[0] > 0.95, "{white:?}");
        let black = face_color([0.0; 3], &p, p.face.max_luma);
        // lift 0.4, then white at 0.2 over it
        assert!(close(black[0], 0.4 * 0.8 + 0.2, 1e-4), "{black:?}");
    }

    #[test]
    fn refraction_pulls_from_past_the_edge_and_fades_inward() {
        let n = [1.0, 0.0];
        let at_edge = refraction_offset(0.0, n, -50.0, 20.0);
        assert!(close(at_edge[0], 50.0 * REFRACTION_REACH_PER_AMOUNT, 1e-5) && at_edge[1] == 0.0);
        let mid = refraction_offset(-10.0, n, -50.0, 20.0);
        assert!(close(mid[0], at_edge[0] * 0.25, 1e-5));
        assert_eq!(
            refraction_offset(-25.0, n, -50.0, 20.0),
            [0.0, 0.0],
            "past the band"
        );
        assert_eq!(
            refraction_offset(-1.0, n, -50.0, 0.0),
            [0.0, 0.0],
            "no band, no bend"
        );
        assert_eq!(refraction_offset(-1.0, n, 0.0, 20.0), [0.0, 0.0]);
    }

    #[test]
    fn bleed_lightens_dark_glass_edges_with_the_backdrop_colour() {
        let p = params(true);
        let red = [0.9, 0.1, 0.1];
        let face = [0.1; 3];
        let edge = bleed(face, red, 0.0, &p, 35.0, 35.0);
        assert!(edge[0] > face[0] && edge[1] >= face[1], "{edge:?}");
        let inside = bleed(face, red, -40.0, &p, 35.0, 35.0);
        assert_eq!(inside, face, "past the band");
        let light = params(false);
        let darker = bleed([0.98; 3], [0.0, 0.0, 0.5], 0.0, &light, 35.0, 35.0);
        assert!(darker[0] < 0.98, "light glass darkens: {darker:?}");
    }

    #[test]
    fn highlight_lights_the_top_and_the_bottom_edge_and_not_the_sides() {
        let p = params(true);
        let face = [0.2, 0.15, 0.6];
        let top = highlight(face, -0.5, [0.0, -1.0], &p);
        let bottom = highlight(face, -0.5, [0.0, 1.0], &p);
        let side = highlight(face, -0.5, [1.0, 0.0], &p);
        for i in 0..3 {
            assert!(
                top[i] > face[i],
                "the key light lifts every channel: {top:?}"
            );
            assert!(
                bottom[i] > face[i],
                "the fill end of the axis is lit too: {bottom:?}"
            );
        }
        assert!(
            bottom[0] < top[0],
            "a little less than the key: {bottom:?} {top:?}"
        );
        assert!(
            bottom[0] - face[0] > 0.6 * (top[0] - face[0]),
            "but nearly as bright: {bottom:?} {top:?}"
        );
        assert_eq!(side, face, "nothing at the sides");
        // around a corner the light fades gradually rather than ending early
        let corner = highlight(
            face,
            -0.5,
            [
                std::f32::consts::FRAC_1_SQRT_2,
                -std::f32::consts::FRAC_1_SQRT_2,
            ],
            &p,
        );
        let (lift, full) = (corner[0] - face[0], top[0] - face[0]);
        assert!(lift > 0.4 * full && lift < full, "{lift} of {full} at 45°");
        let deep = highlight(face, -6.0, [0.0, -1.0], &p);
        assert!(close(deep[0], face[0], 1e-3), "the rim is thin");
        assert_eq!(highlight_lobe(0.0, 0.0, 1.0, 0.75), 1.0);
        let spread = p.highlight.spread;
        assert_eq!(highlight_lobe(spread * LOBE_REACH, 0.0, spread, 0.75), 0.0);
        let mut lobe = [0.0f32; 6];
        for (i, l) in lobe.iter_mut().enumerate() {
            *l = highlight_lobe(i as f32 * 0.25, 0.0, spread, 0.75);
        }
        assert!(
            lobe.windows(2).all(|w| w[1] < w[0]),
            "falls steadily: {lobe:?}"
        );
    }

    #[test]
    fn shadows_are_invisible_at_rest_and_follow_their_values_when_shown() {
        let mut p = params(true);
        assert_eq!(shadow_alpha(0.0, 0.0, &p), (0.0, 0.0));
        p.shadow.opacity = 1.0;
        p.shadow.amount = 50.0;
        p.shadow.height = 10.0;
        p.shadow.ring_opacity = 1.0;
        let (near, ring) = shadow_alpha(0.0, 0.0, &p);
        assert!(close(near, 0.5 * 0.3 * 0.5, 1e-4), "{near}");
        assert!(ring > 0.0);
        let (far, _) = shadow_alpha(40.0, 40.0, &p);
        assert_eq!(far, 0.0);
    }

    #[test]
    fn foreground_follows_the_face_under_the_glyph() {
        let dark = foreground([0.3, 0.2, 0.6], &foreground_matrix(true), 1.0);
        assert_eq!(dark, [1.0, 1.0, 1.0], "lifted to white");
        let light = foreground([0.9, 0.8, 0.95], &foreground_matrix(false), 1.0);
        assert_eq!(light, [0.0; 3]);
        let half = foreground([0.4; 3], &foreground_matrix(true), 0.5);
        assert!(close(half[0], 0.7, 1e-5));
    }

    #[test]
    fn container_reads_its_bounds_plus_the_reach() {
        let c = Container::new(
            vec![
                GlassShape::new([10.0, 10.0, 150.0, 100.0], 18.0),
                GlassShape::new([165.0, 10.0, 150.0, 100.0], 18.0),
            ],
            20.0,
            GlassMode::default(),
        );
        assert_eq!(c.bounds(), Some([10.0, 10.0, 305.0, 100.0]));
        // the reach, and the bulge of the union: a quarter of the spacing
        let r = c.params.reach() + 5.0;
        assert_eq!(
            c.sample_area(),
            Some([10.0 - r, 10.0 - r, 305.0 + 2.0 * r, 100.0 + 2.0 * r])
        );
    }

    #[test]
    fn shading_a_flat_backdrop_is_flat_inside_and_bent_at_the_edge() {
        let c = Container::new(
            vec![GlassShape::new([0.0, 0.0, 100.0, 100.0], 20.0)],
            0.0,
            GlassMode {
                dark: true,
                ..GlassMode::default()
            },
        );
        let grey = |_: usize, _: [f32; 2]| [0.2f32; 3];
        let (centre, cov) = c.shade([50.0, 50.0], &grey);
        assert_eq!(cov, 1.0);
        let expect = face_color([0.2; 3], &c.params, c.values[0].max_luma);
        assert!(close(centre[0], expect[0], 1e-5));
        // a stripe just outside the left edge shows up inside the rim through refraction
        let stripe = |_: usize, q: [f32; 2]| {
            if q[0] < 0.0 {
                [1.0, 0.0, 0.0]
            } else {
                [0.2; 3]
            }
        };
        let (rim, _) = c.shade([2.0, 50.0], &stripe);
        assert!(
            rim[0] > rim[1] + 0.1,
            "the stripe bends into the rim: {rim:?}"
        );
        let (inner, _) = c.shade([30.0, 50.0], &stripe);
        assert!(close(inner[0], expect[0], 1e-4), "{inner:?}");
    }

    #[test]
    fn uniform_layout_and_scaling() {
        assert_eq!(
            std::mem::size_of::<GlassUniform>(),
            (17 + 3 * MAX_SHAPES) * 16
        );
        let c = Container::new(
            vec![GlassShape::capsule([10.0, 20.0, 100.0, 40.0])],
            8.0,
            GlassMode::default(),
        );
        let u = GlassUniform::new(
            &c,
            2.0,
            [[0.5, 0.0, 0.0], [0.0, 0.5, 0.0]],
            [0.0; 4],
            [100.0; 2],
            1.0,
        );
        assert_eq!(u.shapes[0], [20.0, 40.0, 200.0, 80.0]);
        assert_eq!(
            u.shape_a[0][0], 40.0,
            "a capsule's radius is half the short side"
        );
        assert_eq!(u.info[..2], [1.0, 16.0]);
        // regular, short side 40: refraction −20 → 20 · 0.25 points at the edge, band 10 points
        assert_eq!(u.shape_a[0][1], 20.0 * REFRACTION_REACH_PER_AMOUNT * 2.0);
        assert_eq!(u.shape_a[0][2], 20.0);
        assert_eq!(u.highlight_matrix[0][3], 0.1471);
        let levels = c.levels();
        // level 3 has no weight in the blend, but the bleed reads it
        assert_eq!(levels.map(|l| l.1), [true, true, true, true]);
        assert_eq!(c.fill_radius(), Some(8.0));
    }

    #[test]
    fn the_widest_level_is_made_only_when_the_blend_or_the_bleed_reads_it() {
        let shape = vec![GlassShape::new([0.0, 0.0, 150.0, 100.0], 18.0)];
        let mode = |style, reduce_transparency| GlassMode {
            dark: true,
            style,
            reduce_transparency,
            ..GlassMode::default()
        };
        // regular glass bleeds the widest level into its edge though its weight in the blend is 0
        let regular = Container::new(shape.clone(), 0.0, mode(GlassStyle::Regular, false));
        assert_eq!(regular.params.level_weights()[3], 0.0);
        assert!(regular.values[0].bleed_amount > 0.0);
        assert!(regular.levels()[3].1, "the bleed reads level 3");
        // clear glass has no bleed and only its narrowest level shows
        let clear = Container::new(shape, 0.0, mode(GlassStyle::Clear, false));
        assert_eq!(clear.levels().map(|l| l.1), [true, false, false, false]);
    }

    #[test]
    fn hostile_spacing_is_bounded_and_the_sampled_area_covers_the_bulge() {
        let shapes = vec![
            GlassShape::new([0.0, 0.0, 100.0, 40.0], 12.0),
            GlassShape::new([104.0, 0.0, 100.0, 40.0], 12.0),
        ];
        let mode = GlassMode::default();
        for bad in [f32::NAN, f32::NEG_INFINITY, -3.0] {
            assert_eq!(Container::new(shapes.clone(), bad, mode).spacing, 0.0);
        }
        let huge = Container::new(shapes.clone(), 8.0e6, mode);
        assert_eq!(huge.spacing, MAX_SPACING);
        let r = huge.params.reach();
        let [x, _, w, _] = huge.sample_area().unwrap();
        assert_eq!(
            (x, w),
            (
                -r - MAX_SPACING / 4.0,
                204.0 + 2.0 * (r + MAX_SPACING / 4.0)
            )
        );
        // the field never reaches past the shapes by more than the bulge the area allows for
        let above_gap = [102.0, -MAX_SPACING / 4.0 - 0.01];
        assert!(huge.field(above_gap).0 > 0.0);
        let alone = Container::new(shapes[..1].to_vec(), 20.0, mode);
        assert_eq!(alone.sample_area().unwrap()[0], -alone.params.reach());
    }

    /// The WCAG contrast ratio of two opaque sRGB colours.
    fn contrast(a: Rgb, b: Rgb) -> f32 {
        let y = |c: Rgb| {
            0.2126 * srgb_decode(c[0]) + 0.7152 * srgb_decode(c[1]) + 0.0722 * srgb_decode(c[2])
        };
        let (hi, lo) = (y(a).max(y(b)), y(a).min(y(b)));
        (hi + 0.05) / (lo + 0.05)
    }

    /// A light-appearance panel of regular glass (Control Center's modules and menus) keeps body text, black at 85 %,
    /// at 4.5:1 or more over the darkest backdrops, at every panel size and with each accessibility recipe.
    #[test]
    fn light_glass_keeps_body_text_readable_over_a_dark_backdrop() {
        const BODY: f32 = 0.85;
        for (rt, ic) in [(false, false), (true, false), (false, true), (true, true)] {
            let mode = GlassMode {
                dark: false,
                reduce_transparency: rt,
                increase_contrast: ic,
                ..GlassMode::default()
            };
            for side in [44.0, 64.0, 120.0, 320.0] {
                let c = Container::new(
                    vec![GlassShape::new([0.0, 0.0, 2.0 * side, side], 16.0)],
                    0.0,
                    mode,
                );
                for backdrop in [[0.0; 3], [0.1, 0.1, 0.12], [0.2, 0.05, 0.3]] {
                    let (face, cov) = c.shade([side, side / 2.0], &|_, _| backdrop);
                    assert_eq!(cov, 1.0);
                    let text = face.map(|v| v * (1.0 - BODY));
                    let ratio = contrast(text, face);
                    assert!(
                        ratio >= 4.5,
                        "rt {rt} ic {ic}, {side} pt over {backdrop:?}: body text {ratio:.2}:1 on {face:?}"
                    );
                }
            }
        }
    }
}
