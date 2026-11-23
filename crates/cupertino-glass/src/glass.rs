//! Glass: the CPU model of the background pass and of the foreground colour matrix, and the uniform block the
//! glass program reads (`shaders/glass.wgsl`, generated for every backend).
//!
//! A glass *container* holds up to [`MAX_SHAPES`] shapes (rounded rectangles or capsules). Its backdrop is captured
//! once, at half the output's scale, over the bounds of every shape plus the reach of the blur and the refraction, gaps
//! included. The shapes join into one signed distance field: a smooth union whose blend width is the container's
//! spacing (0 is a hard union). Every pixel of the field is then shaded in this order:
//!
//! 1. **Blur.** One Gaussian of the backdrop per shape, its deviation twice the radius scaled by the first level's
//!    opacity ([`GlassParams::blur_sigma`]). The container blurs the backdrop once per distinct deviation of its shapes
//!    (at most [`BLUR_LEVELS`]); a shape between two of them mixes the two ([`level_mix`]). Then the blur fill (a
//!    Gaussian of twice the fill radius) is blended in darken, lighten and normal mode at their opacities.
//! 2. **Colour matrix.** On encoded values: saturation about the Rec. 709 luma `L`; the luma compression
//!    `c − (1 − max_luma) · L · (0.75 c + 0.25 L)` (a grey `x` becomes `x − (1 − max_luma) · x²`); the levels, one shift
//!    `black · (1 − L) − (1 − white) · L` added to every channel, `L` the luma after the compression; a limit at
//!    `clamp` ([`face_tone`]). The inner refraction moves where all of this is read: inside a band `height` wide along
//!    the edge, the face is read `amount · g(t)` points along the field's outward normal, where `t` runs from 0 at the
//!    edge to 1 at the band's inner side and `g(t) = 0.9 · e^(−4.2 t) · (1 − t⁴)` ([`refraction_profile`]): a negative
//!    strength reads it further inside, so the edge magnifies what lies just inside it.
//! 3. **Edge bleed, fill and tint.** Inside a band `height` deep, the backdrop `amount · (1 − t)` points outside the
//!    pixel, blurred by its own Gaussian (the copy [`BLEED_LEVEL`]), through its own saturation and levels
//!    ([`bleed_tone`]), blends into the face at `opacity · (1 − t)²`, darken or normal ([`bleed_read`],
//!    [`bleed_into`]); none at rest. Then the face's fill and the tint over it, in normal blending ([`fill_and_tint`]).
//! 4. **Soft band and rim.** Where a surface asks for it, a soft band of the light `matrix · face` along the arcs of
//!    edge facing either end of the axis through `angle` ([`highlight_lobe`]). Then the rim ([`rim_shift`]): every
//!    channel shifted by `amount · color_bias` (darker for a negative bias), scaled by the face's brightness
//!    ([`rim_brightness`]) and by the edge's direction ([`rim_direction`], strongest on diagonals), in a band
//!    `RIM_DEPTH_PER_HEIGHT · height` deep.
//! 5. **Control outline and key and fill light.** The control outline, a black line centred on the edge, darkens the
//!    face ([`border_line`]); over it the key and fill light and their diffuse glow add white ([`specular_light`]),
//!    and where they fall the outline shows at a fifth. Neither shows in today's recipes.
//! 6. **Veil.** A black veil from the top of the shapes to their bottom ([`Container::veil`]).
//! 7. **Ring shadow.** Inside the shape, a stroke `stroke` wide along the inside of the outline, moved up by
//!    `ring_offset` and blurred with a Gaussian of `ring_blur`, darkens the face by its opacity ([`ring_shadow`]).
//! 8. **Outside.** The backdrop bent outside the edge ([`outer_read`], the sharp copy [`SHARP_LEVEL`]), under a drop
//!    shadow offset by `offset`, blurred by `height` at `amount` percent of its fill's alpha, and an outline hairline,
//!    under the edge line and the control outline. The bend, ring and drop shadow are drawn at their opacity, which is
//!    0 at rest. A pixel the shape's edge crosses shows the glass over this in the part the glass leaves uncovered.
//! 9. **Edge.** Where the container carries one ([`GlassEdge`]), a line of its width just outside each shape's own
//!    outline, left out inside the container's other shapes ([`Container::edge_alpha_at`]), so where the shapes overlap
//!    only the outline of the whole shows, as a stroke around each shape cut by the others draws it; drawn over all of
//!    the above: over what lies there and over the glass's own antialiased edge ([`edge_share`], [`over_edge`]), its
//!    band in the field's units as the glass's own coverage is. A soft edge is that band blurred by a Gaussian, in
//!    closed form ([`soft_edge_share`]), so it softens as content blurred with the same deviation does.
//!
//! Each shape of a container may carry a recipe of its own ([`Container::with_shape_mode`]): its look
//! ([`ShapeLook`]: face, fill, tint, rim, key and fill light, ring, control outline and the opacities of the bleed and
//! the bend) and its size-dependent values ([`ShapeValues`]) mix across the union with the field's weights, and where the
//! looks differ each shape's look is laid over that by the share of the pixel it covers ([`Container::field_look`]), so a
//! segment inside its group shows its own glass. The blur fill, the soft band, the shadows, the edge line and the veil
//! are the container's.
//!
//! The foreground of text and symbols on glass is [`foreground`]: the face under the glyph through the mode's 4×5 matrix,
//! mixed over the face by the glyph's coverage.
//!
//! Colours are straight sRGB-encoded `f32` unless a function says otherwise.

mod stages;
mod uniform;

pub use stages::{
    bleed_into, bleed_read, bleed_tone, border_and_light, border_line, fill_and_tint, key_and_fill,
    light_arc, outer_read, smoothstep, specular_light, ShapeLook,
};
pub use uniform::GlassUniform;

use cupertino_theme::glass::{
    apply_matrix, refraction_profile, rim_brightness, rim_direction, ColorMatrix, GlassParams,
    BLUR_LEVELS, BLUR_SIGMA_MAX, RIM_DEPTH_PER_HEIGHT,
};

/// The copy of the backdrop `Container::shade`'s sampler reads for the blur fill, after the blurred copies.
pub const FILL_LEVEL: usize = BLUR_LEVELS;
/// The copy blurred for the edge bleed ([`cupertino_theme::glass::GlassBleed::blur`]).
pub const BLEED_LEVEL: usize = BLUR_LEVELS + 1;
/// The backdrop as captured, unblurred, which the bend outside the edge reads.
pub const SHARP_LEVEL: usize = BLUR_LEVELS + 2;

/// Deviations closer than this, in points, are blurred once.
const SAME_SIGMA: f32 = 0.05;
/// How far, as a share of its own, the edge bleed's deviation may lie from that of a copy the container blurs already
/// for the bleed to read that copy instead of blurring one of its own: a faint band softened a tenth more or less looks
/// the same, and one blur pass fewer is a whole pass over the sampled area.
pub const BLEED_SHARE_TOLERANCE: f32 = 0.1;

pub use cupertino_theme::glass::{GlassMode, GlassStyle, GLASS_BACKDROP_SCALE};

/// Most shapes one container joins.
pub const MAX_SHAPES: usize = 16;

/// Widest blend between the shapes of a container, in points. The union bulges up to a quarter of the blend width
/// past the shapes, so this bounds how far a container's glass reaches beyond them.
pub const MAX_SPACING: f32 = 64.0;

/// The line a container draws along the outside of its outline ([`Container::edge`]): its width in the container's
/// units (points; pixels in the uniform), its straight colour and opacity, and the deviation of the Gaussian it is
/// softened by in the same units (0: sharp). A width of 0 draws none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlassEdge {
    pub width: f32,
    pub color: [f32; 4],
    pub soft: f32,
}

impl GlassEdge {
    /// Whether the edge draws anything.
    pub fn shows(&self) -> bool {
        self.width > 0.0 && self.color[3] > 0.0
    }

    /// The edge softened by a Gaussian of deviation `sigma` (negative or not finite is 0, sharp), as content blurred
    /// with it softens.
    pub fn soft(self, sigma: f32) -> Self {
        Self {
            soft: if sigma.is_finite() {
                sigma.max(0.0)
            } else {
                0.0
            },
            ..self
        }
    }

    /// How far past the band the softened edge reaches: three deviations, where under a thousandth of it is left.
    pub fn soft_reach(&self) -> f32 {
        3.0 * self.soft
    }
}

/// The share of a pixel one unit wide whose centre lies `d` units outside an outline (negative inside) that the band
/// from the outline to `width` outside it covers: the stroke's coverage across a straight edge.
pub fn edge_share(d: f32, width: f32) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    ((d + 0.5).min(width) - (d - 0.5).max(0.0)).clamp(0.0, 1.0)
}

/// The share of a unit Gaussian below `x` deviations, from the error function's rational approximation (within
/// 2·10⁻⁷).
pub fn gaussian_cdf(x: f32) -> f32 {
    let z = x.abs() * std::f32::consts::FRAC_1_SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let poly = t
        * (0.254_829_6
            + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152 + t * 1.061_405_4))));
    let erf = 1.0 - poly * (-z * z).exp();
    0.5 * (1.0 + erf.copysign(x))
}

/// The integral of [`gaussian_cdf`] from minus infinity to `u`: `u·Φ(u) + φ(u)`, 0 far below and `u` far above.
fn gaussian_ramp(u: f32) -> f32 {
    const INV_SQRT_TAU: f32 = 0.398_942_3;
    u * gaussian_cdf(u) + INV_SQRT_TAU * (-0.5 * u * u).exp()
}

/// [`edge_share`] softened by a Gaussian of deviation `sigma`: the band from the outline to `width` outside it,
/// blurred by the Gaussian, over the pixel one unit wide centred `d` outside the outline. In closed form, with `R` the
/// integral of the Gaussian's share below a distance (`R(x) = σ·ψ(x/σ)`, `ψ(u) = u·Φ(u) + φ(u)`), it is
/// `R(d + ½) − R(d − ½) − R(d + ½ − width) + R(d − ½ − width)`; `R` goes to `max(x, 0)` as `sigma` goes to 0, so a
/// deviation of 0 is [`edge_share`] itself.
pub fn soft_edge_share(d: f32, width: f32, sigma: f32) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    if sigma <= 0.0 {
        return edge_share(d, width);
    }
    let r = |x: f32| sigma * gaussian_ramp(x / sigma);
    (r(d + 0.5) - r(d - 0.5) - r(d + 0.5 - width) + r(d - 0.5 - width)).clamp(0.0, 1.0)
}

/// The colour and coverage `(c, a)` of what a pixel shows with `edge` over it at `share` of the pixel (straight
/// colours): the stroke drawn over the pixel, which darkens or tints what the pixel lets through and adds its own
/// coverage.
pub fn over_edge(c: Rgb, a: f32, edge: &GlassEdge, share: f32) -> (Rgb, f32) {
    let e = (edge.color[3] * share).clamp(0.0, 1.0);
    if e <= 0.0 {
        return (c, a);
    }
    let out = a + e - a * e;
    let c = std::array::from_fn(|i| (c[i] * a * (1.0 - e) + edge.color[i] * e) / out);
    (c, out)
}

/// Rec. 709 luminance weights of linear light.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub type Rgb = [f32; 3];

/// How a glass shape's corners curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassCorner {
    /// A quarter circle of the radius.
    Circular,
    /// A superellipse quadrant over [`CONTINUOUS_EXTENT`] times the radius of each side, of exponent
    /// [`CONTINUOUS_EXPONENT`]: the curve leaves the straight edge without a kink in its curvature.
    #[default]
    Continuous,
}

/// How far a continuous corner reaches along each side, per point of its radius.
pub const CONTINUOUS_EXTENT: f32 = 1.53;
/// The exponent of a continuous corner's superellipse (2 is a circle).
pub const CONTINUOUS_EXPONENT: f32 = 3.33;

/// One shape of a container: a rectangle `[x, y, w, h]` with corners of `radius` (clamped to half the short side; a
/// capsule has exactly that), continuous unless set otherwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassShape {
    pub rect: [f32; 4],
    pub radius: f32,
    pub corner: GlassCorner,
}

impl GlassShape {
    pub fn new(rect: [f32; 4], radius: f32) -> Self {
        Self {
            rect,
            radius,
            corner: GlassCorner::Continuous,
        }
    }

    /// The shape with corners curving as `corner` says.
    pub fn with_corner(self, corner: GlassCorner) -> Self {
        Self { corner, ..self }
    }

    /// How far each corner reaches along the sides, in points, and the exponent of its superellipse. A continuous
    /// corner reaches [`CONTINUOUS_EXTENT`] times the radius; once that would pass half the short side it stays there,
    /// and its exponent eases from [`CONTINUOUS_EXPONENT`] to 2 as the radius grows to half the short side, so a
    /// capsule has round ends.
    pub fn corner_shape(&self) -> (f32, f32) {
        let r = self.fitted_radius();
        let half = self.short_side() * 0.5;
        if r <= 0.0 {
            return (0.0, 2.0);
        }
        match self.corner {
            GlassCorner::Circular => (r, 2.0),
            GlassCorner::Continuous => {
                let extent = CONTINUOUS_EXTENT * r;
                if extent <= half {
                    return (extent, CONTINUOUS_EXPONENT);
                }
                let from = half / CONTINUOUS_EXTENT;
                let t = ((r - from) / (half - from).max(1e-6)).clamp(0.0, 1.0);
                (half, CONTINUOUS_EXPONENT + (2.0 - CONTINUOUS_EXPONENT) * t)
            }
        }
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

    /// Signed distance from `p` to the shape's outline, negative inside: exact along the straight edges and for
    /// circular corners, to first order in a continuous corner (the superellipse norm's excess over its slope).
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        let [x, y, w, h] = self.rect;
        let (e, n) = self.corner_shape();
        rounded_box_sdf(
            [p[0] - (x + w * 0.5), p[1] - (y + h * 0.5)],
            [w * 0.5, h * 0.5],
            e,
            n,
        )
    }
}

/// Signed distance from `p` (relative to the centre) to a box of half size `half` whose corners reach `e` along each
/// side with a superellipse of exponent `n` (2 is a circle).
pub fn rounded_box_sdf(p: [f32; 2], half: [f32; 2], e: f32, n: f32) -> f32 {
    let e = e.clamp(0.0, half[0].min(half[1]));
    let q = [p[0].abs() - (half[0] - e), p[1].abs() - (half[1] - e)];
    if q[0] <= 0.0 || q[1] <= 0.0 || e <= 0.0 {
        let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2)).sqrt();
        return outside + q[0].max(q[1]).min(0.0) - e;
    }
    if n <= 2.0 {
        return (q[0] * q[0] + q[1] * q[1]).sqrt() - e;
    }
    let u = [(q[0] / e).max(1e-6), (q[1] / e).max(1e-6)];
    let un = [u[0].powf(n), u[1].powf(n)];
    let g = (un[0] + un[1]).powf(1.0 / n);
    // the norm's gradient: u^(n − 1) / g^(n − 1) on each axis
    let gn = g.powf(n - 1.0);
    let slope = ((un[0] / u[0] / gn).powi(2) + (un[1] / u[1] / gn).powi(2)).sqrt();
    // never nearer than the corner's square, which holds the curve
    ((g - 1.0) * e / slope.max(1e-6)).max(q[0].max(q[1]) - e)
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
    let g = field_gradient(shapes, k, p);
    let len = (g[0] * g[0] + g[1] * g[1]).sqrt();
    if len < 1e-6 {
        [0.0, 0.0]
    } else {
        [g[0] / len, g[1] / len]
    }
}

/// The central difference of the union's field over one unit at `p`: about a unit long near a real edge, and short
/// along the seam where two joined shapes meet, where the fields of both sides cancel.
pub fn field_gradient(shapes: &[GlassShape], k: f32, p: [f32; 2]) -> [f32; 2] {
    let f = |dx: f32, dy: f32| union_field(shapes, k, [p[0] + dx, p[1] + dy]).0;
    [f(0.5, 0.0) - f(-0.5, 0.0), f(0.0, 0.5) - f(0.0, -0.5)]
}

/// The shortest field slope [`edge_distance`] divides by.
pub const MIN_FIELD_SLOPE: f32 = 0.05;

/// How far inside the edge a point with field value `d` lies, from the field's `slope` there (the length of
/// [`field_gradient`]): `d` itself near a real edge, and far inside along a seam between joined shapes, whose shallow
/// field would otherwise put a rim light across the join.
pub fn edge_distance(d: f32, slope: f32) -> f32 {
    d / slope.max(MIN_FIELD_SLOPE)
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

/// The weight of each blurred copy for a shape at `position` among them: the copy itself at a whole position, the two
/// on either side mixed between.
pub fn level_weights(position: f32) -> [f32; BLUR_LEVELS] {
    let p = position.clamp(0.0, (BLUR_LEVELS - 1) as f32);
    std::array::from_fn(|i| (1.0 - (p - i as f32).abs()).max(0.0))
}

/// Stage 1: the blurred copies `levels` mixed for a shape at `position` among them ([`level_weights`]).
pub fn level_mix(levels: [Rgb; BLUR_LEVELS], position: f32) -> Rgb {
    let w = level_weights(position);
    std::array::from_fn(|c| (0..BLUR_LEVELS).map(|i| levels[i][c] * w[i]).sum())
}

/// The deviations (points, ascending) a container blurs its backdrop with for shapes blurring with `sigmas`: each
/// distinct one when there are at most [`BLUR_LEVELS`], else that many spread evenly from the narrowest to the widest.
pub fn blur_levels(sigmas: &[f32]) -> Vec<f32> {
    let mut d: Vec<f32> = sigmas.iter().map(|s| s.max(0.0)).collect();
    d.sort_by(f32::total_cmp);
    d.dedup_by(|a, b| (*a - *b).abs() < SAME_SIGMA);
    if d.len() <= BLUR_LEVELS {
        return d;
    }
    let (lo, hi) = (d[0], d[d.len() - 1]);
    (0..BLUR_LEVELS)
        .map(|i| lo + (hi - lo) * i as f32 / (BLUR_LEVELS - 1) as f32)
        .collect()
}

/// Where a shape blurring with `sigma` lies among `levels` (ascending): the index of the copy it reads, with the share
/// of the next one as its fraction.
pub fn level_position(levels: &[f32], sigma: f32) -> f32 {
    let Some(last) = levels.len().checked_sub(1) else {
        return 0.0;
    };
    for i in 0..last {
        let (a, b) = (levels[i], levels[i + 1]);
        if sigma <= b + SAME_SIGMA * 0.5 {
            let t = if b - a > 1e-6 {
                (sigma - a) / (b - a)
            } else {
                0.0
            };
            return i as f32 + t.clamp(0.0, 1.0);
        }
    }
    last as f32
}

/// Stage 1, second part: the blur fill `fill` blended over the blurred backdrop `c`.
pub fn blur_fill(c: Rgb, fill: Rgb, p: &GlassParams) -> Rgb {
    blur_fill_weighted(c, fill, p, 1.0)
}

/// [`blur_fill`] with each of its blends weighed by `w`, the share of its blur the glass keeps there
/// ([`Container::blur_weight`]).
pub fn blur_fill_weighted(c: Rgb, fill: Rgb, p: &GlassParams, w: f32) -> Rgb {
    let b = &p.blur;
    let darker: Rgb = std::array::from_fn(|i| c[i].min(fill[i]));
    let c = mix(c, darker, b.fill_darken * w);
    let lighter: Rgb = std::array::from_fn(|i| c[i].max(fill[i]));
    let c = mix(c, lighter, b.fill_lighten * w);
    mix(c, fill, b.fill_normal * w)
}

/// The luma compression of the face: `c − (1 − max_luma) · L · (0.75 c + 0.25 L)`, `L` the luma of `c`. A grey `x`
/// becomes `x − (1 − max_luma) · x²`: white comes out at `max_luma`, and below 0.5 the brightest greys come out darker
/// than slightly dimmer ones.
pub fn compress_luma(c: Rgb, max_luma: f32) -> Rgb {
    let (k, l) = (1.0 - max_luma, luma(c));
    c.map(|v| v - k * l * (0.75 * v + 0.25 * l))
}

/// The levels of the face: `black · (1 − L) − (1 − white) · L` added to every channel, `L` the luma of `c`.
pub fn face_levels(c: Rgb, black: f32, white: f32) -> Rgb {
    let l = luma(c);
    let shift = black * (1.0 - l) - (1.0 - white) * l;
    c.map(|v| v + shift)
}

/// Stage 2: the tone of the face of the blurred backdrop `c`, with the face's `max_luma` replaced by `max_luma` (it
/// depends on the shape's size): saturation, luma compression and levels on encoded values, within the face's limit.
pub fn face_tone(c: Rgb, p: &GlassParams, max_luma: f32) -> Rgb {
    let f = &p.face;
    let c = compress_luma(saturate_encoded(c, f.saturation), max_luma);
    face_levels(c, f.black, f.white).map(|v| v.clamp(0.0, f.clamp.min(1.0)))
}

/// The face colour of the blurred backdrop `c`: its tone ([`face_tone`]), then the fill over it.
pub fn face_color(c: Rgb, p: &GlassParams, max_luma: f32) -> Rgb {
    let f = &p.face;
    mix(
        face_tone(c, p, max_luma),
        [f.fill[0], f.fill[1], f.fill[2]],
        f.fill[3],
    )
}

/// Stage 2: where the face is read for a pixel `d` from the edge (negative inside) with outward `normal`, as an
/// offset in points: `amount · g(−d / height)` along the normal ([`refraction_profile`]), further inside for a negative
/// strength. Zero outside the shape, past the band and without refraction.
pub fn refraction_offset(d: f32, normal: [f32; 2], amount: f32, height: f32) -> [f32; 2] {
    if height <= 0.0 || amount == 0.0 || d > 0.0 {
        return [0.0, 0.0];
    }
    let reach = amount * refraction_profile(-d / height);
    [normal[0] * reach, normal[1] * reach]
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

/// Stage 5, first part: the soft band of light on the face `c` at distance `d` (negative inside, points) from the edge,
/// with outward `normal` in y-down coordinates: the colour `matrix · face` added along the arcs facing either end of the
/// axis through the light's angle, the far end at `glow_fill` of the near one; under it the diffuse band, as wide,
/// on arcs as wide and as strong as the soft band's times its scales. Nothing where the recipe has no band.
pub fn glow(c: Rgb, d: f32, normal: [f32; 2], p: &GlassParams) -> Rgb {
    let h = &p.highlight;
    if h.glow_amount <= 0.0 || h.glow_height <= 0.0 || (normal[0] == 0.0 && normal[1] == 0.0) {
        return c;
    }
    // y up for the angle
    let normal_angle = (-normal[1]).atan2(normal[0]);
    let ends = |spread: f32| {
        let key = highlight_lobe(normal_angle, h.angle, spread, h.curvature);
        let fill = highlight_lobe(
            normal_angle,
            h.angle + std::f32::consts::PI,
            spread,
            h.curvature,
        );
        key + fill * h.glow_fill.max(0.0)
    };
    let band = |height: f32| {
        let x = (d - h.glow_offset) / height;
        (-x * x).exp()
    };
    let mut w = h.glow_amount * band(h.glow_height) * ends(h.spread);
    let diffuse = &h.diffuse;
    if diffuse.amount_scale > 0.0 && diffuse.height_scale > 0.0 {
        w += h.glow_amount
            * diffuse.amount_scale
            * band(h.glow_height * diffuse.height_scale)
            * ends(h.spread * diffuse.spread_scale);
    }
    let light = matrix_rgb(&h.matrix, c);
    std::array::from_fn(|i| (c[i] + light[i] * w).min(1.0))
}

/// How much of the rim a pixel whose centre lies `depth` units inside the edge gets (the antialiased edge counts as on
/// it), for a rim `reach` units deep: all of it up to half a unit short of the reach, fading over the unit across it.
pub fn rim_band(depth: f32, reach: f32) -> f32 {
    (reach + 0.5 - depth.max(0.0)).clamp(0.0, 1.0)
}

/// Stage 5, second part: the shift the rim adds to every channel of the face `c` at distance `d` (negative inside,
/// points) from the edge, with outward `normal`: `amount · color_bias`, times the face's brightness response and the
/// edge's direction response, inside the rim's depth.
pub fn rim_shift(c: Rgb, d: f32, normal: [f32; 2], p: &GlassParams) -> f32 {
    let h = &p.highlight;
    if h.amount == 0.0 || h.height <= 0.0 || (normal[0] == 0.0 && normal[1] == 0.0) {
        return 0.0;
    }
    let band = rim_band(-d, RIM_DEPTH_PER_HEIGHT * h.height);
    if band <= 0.0 {
        return 0.0;
    }
    let angle = normal[1].atan2(normal[0]);
    h.amount * h.color_bias * rim_brightness(luma(c)) * rim_direction(angle) * band
}

/// Stage 5: the soft band, then the rim, on the face `c`.
pub fn highlight(c: Rgb, d: f32, normal: [f32; 2], p: &GlassParams) -> Rgb {
    let c = glow(c, d, normal, p);
    let shift = rim_shift(c, d, normal, p);
    c.map(|v| (v + shift).clamp(0.0, 1.0))
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

/// The share of a Gaussian of deviation 1 below `x` (a logistic fit of the normal distribution, within 0.01).
pub fn normal_cdf(x: f32) -> f32 {
    1.0 / (1.0 + (-1.702 * x).exp())
}

/// Stage 6: how much the ring shadow darkens the face at a pixel whose field, with the ring moved up by its offset, is
/// `d_ring` (negative inside): the stroke from the outline to `stroke` inside it, blurred with a Gaussian of
/// `ring_blur`, at the ring's opacity. The face is multiplied by one minus it.
pub fn ring_shadow(d_ring: f32, p: &GlassParams) -> f32 {
    let s = &p.shadow;
    if s.ring_opacity <= 0.0 || s.ring_stroke <= 0.0 {
        return 0.0;
    }
    // the share of the blurred stroke [-stroke, 0] at the pixel's depth
    let mask = if s.ring_blur > 0.0 {
        normal_cdf(-d_ring / s.ring_blur) - normal_cdf((-s.ring_stroke - d_ring) / s.ring_blur)
    } else if (-s.ring_stroke..=0.0).contains(&d_ring) {
        1.0
    } else {
        0.0
    };
    s.ring_opacity.min(1.0) * mask.max(0.0)
}

/// Stage 7: the alpha of the drop shadow at a pixel outside the shape, whose field minus the drop shadow's offset is
/// `d_shadow`.
pub fn shadow_alpha(d_shadow: f32, p: &GlassParams) -> f32 {
    let s = &p.shadow;
    s.opacity
        * (s.amount / 100.0).clamp(0.0, 1.0)
        * s.fill[3]
        * blurred_coverage(d_shadow, s.height)
}

/// The alpha of the outline hairline at a pixel `d` points outside the shape: full within its width, fading over the
/// last pixel.
pub fn outline_alpha(d: f32, p: &GlassParams) -> f32 {
    let s = &p.shadow;
    if s.outline_opacity <= 0.0 || s.outline_width <= 0.0 {
        return 0.0;
    }
    s.outline_opacity * (s.outline_width + 0.5 - d).clamp(0.0, 1.0)
}

/// Stage 7, the edge line: the alpha of black a pixel `d` from the edge (a unit wide band just outside it, one device
/// pixel on the GPU) takes over a backdrop of colour `backdrop`, where the outline's outward normal is `normal`: the
/// recipe's `edge_line` times the rim's response to the backdrop's luma ([`rim_brightness`]), so the line shows along
/// light content and not at all over black (unless the recipe keeps it even over any backdrop), and times the share its
/// direction keeps ([`edge_line_side_share`]).
pub fn edge_line_alpha(d: f32, backdrop: Rgb, normal: [f32; 2], p: &GlassParams) -> f32 {
    let k = p.shadow.edge_line;
    if k <= 0.0 {
        return 0.0;
    }
    let response = if p.shadow.edge_line_even {
        1.0
    } else {
        rim_brightness(luma(backdrop))
    };
    k.min(1.0)
        * response
        * (1.5 - d).clamp(0.0, 1.0)
        * edge_line_side_share(normal, p.shadow.edge_line_sides)
}

/// How much of the edge line shows where the outline's outward normal is `normal` (any length, y down), for arcs
/// `sides` radians wide facing left and right ([`GlassShadow::edge_line_sides`]): a raised cosine of the normal's angle
/// from the horizontal, full along a straight side and nothing [`LOBE_REACH`] of `sides` away from it, so a top or
/// bottom edge draws none. All of it everywhere when `sides` is 0, and where the normal is unknown.
pub fn edge_line_side_share(normal: [f32; 2], sides: f32) -> f32 {
    let len = (normal[0] * normal[0] + normal[1] * normal[1]).sqrt();
    if sides <= 0.0 || len < 1e-6 {
        return 1.0;
    }
    let from_horizontal = (normal[0].abs() / len).clamp(0.0, 1.0).acos();
    let reach = sides * LOBE_REACH;
    if from_horizontal >= reach {
        return 0.0;
    }
    0.5 + 0.5 * (from_horizontal / reach * std::f32::consts::PI).cos()
}

/// The foreground of a glyph on glass: the face `face` under it through `matrix`, mixed over the face by `coverage`
/// (the glyph's coverage times its alpha).
pub fn foreground(face: Rgb, matrix: &ColorMatrix, coverage: f32) -> Rgb {
    mix(face, matrix_rgb(matrix, face), coverage.clamp(0.0, 1.0))
}

/// Size-dependent values of one shape, as the shaders take them per shape: lengths in points.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShapeValues {
    pub refraction_amount: f32,
    pub refraction_height: f32,
    /// How far outside the edge the edge bleed reads, and the depth of its band.
    pub bleed_amount: f32,
    pub bleed_height: f32,
    /// The strength and the band of the bend outside the edge.
    pub outer_amount: f32,
    pub outer_height: f32,
    pub max_luma: f32,
    /// The deviation of the shape's blur in points.
    pub blur_sigma: f32,
    /// Where that blur lies among the container's blurred copies ([`level_position`]).
    pub blur_level: f32,
}

impl ShapeValues {
    pub fn of(p: &GlassParams) -> Self {
        Self {
            refraction_amount: p.refraction.amount,
            refraction_height: p.refraction.height,
            bleed_amount: p.bleed.amount,
            bleed_height: p.bleed.height,
            outer_amount: p.refraction.outer_amount,
            outer_height: p.refraction.outer_height,
            max_luma: p.face.max_luma,
            blur_sigma: p.blur_sigma(),
            blur_level: 0.0,
        }
    }

    fn mix(a: Self, b: Self, t: f32) -> Self {
        let m = |x: f32, y: f32| x + (y - x) * t;
        Self {
            refraction_amount: m(a.refraction_amount, b.refraction_amount),
            refraction_height: m(a.refraction_height, b.refraction_height),
            bleed_amount: m(a.bleed_amount, b.bleed_amount),
            bleed_height: m(a.bleed_height, b.bleed_height),
            outer_amount: m(a.outer_amount, b.outer_amount),
            outer_height: m(a.outer_height, b.outer_height),
            max_luma: m(a.max_luma, b.max_luma),
            blur_sigma: m(a.blur_sigma, b.blur_sigma),
            blur_level: m(a.blur_level, b.blur_level),
        }
    }
}

/// An outline a container is cut to besides its own shapes: a rectangle `[x, y, w, h]` in points whose corners reach
/// `extent` along each side with a superellipse of exponent `exponent` (2 is a circle). Nothing the container draws
/// shows outside it, its shadow and edge line included; a shape inside it is left as it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub rect: [f32; 4],
    pub extent: f32,
    pub exponent: f32,
}

impl Cut {
    /// Signed distance from `p` to the outline, negative inside.
    pub fn sdf(&self, p: [f32; 2]) -> f32 {
        let [x, y, w, h] = self.rect;
        rounded_box_sdf(
            [p[0] - (x + w * 0.5), p[1] - (y + h * 0.5)],
            [w * 0.5, h * 0.5],
            self.extent,
            self.exponent,
        )
    }

    /// The share of the pixel at `p` inside the outline, over an antialiased edge one pixel wide.
    pub fn coverage(&self, p: [f32; 2]) -> f32 {
        (0.5 - self.sdf(p)).clamp(0.0, 1.0)
    }
}

/// A container ready to shade: its shapes (points), blend width, the mode, each shape's recipe and the outline it is
/// cut to, if any.
#[derive(Clone, Debug)]
pub struct Container {
    pub shapes: Vec<GlassShape>,
    pub spacing: f32,
    pub mode: GlassMode,
    /// The recipe of the container's values that do not depend on size (from its largest shape).
    pub params: GlassParams,
    pub values: Vec<ShapeValues>,
    /// The deviations in points the backdrop is blurred with, ascending ([`blur_levels`]).
    pub sigmas: Vec<f32>,
    /// The outline the container is cut to besides its shapes ([`Cut`]).
    pub cut: Option<Cut>,
    /// How much of its blur the glass keeps at the top and at the bottom of its first shape, smoothly between: `[1, 1]`
    /// blurs evenly, a lower bottom fades the blur out towards it (a variable blur).
    pub blur_ramp: [f32; 2],
    /// How much the glass magnifies what lies behind it about its first shape's centre: 1 for none, more for a lens.
    pub lens: f32,
    /// The line along the outside of the union's outline ([`GlassEdge`]); none by default.
    pub edge: GlassEdge,
    /// Each shape's own recipe, where it has one ([`Container::with_shape_mode`]); `None` takes the container's
    /// `params`. Its look ([`ShapeLook`]) and its size-dependent values replace the container's on that shape.
    pub recipes: Vec<Option<GlassParams>>,
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
        let values: Vec<ShapeValues> = shapes
            .iter()
            .map(|s| ShapeValues::of(&GlassParams::new(mode, s.short_side())))
            .collect();
        let mut c = Self {
            recipes: vec![None; shapes.len()],
            shapes,
            spacing: if spacing.is_finite() {
                spacing.clamp(0.0, MAX_SPACING)
            } else {
                0.0
            },
            mode,
            params: GlassParams::new(mode, largest),
            values,
            sigmas: Vec::new(),
            cut: None,
            blur_ramp: [1.0, 1.0],
            lens: 1.0,
            edge: GlassEdge::default(),
        };
        c.place_levels();
        c
    }

    /// The blurred copies the shapes' blurs need ([`blur_levels`], with an unblurred one under a blur ramp), and where
    /// each shape's blur lies among them.
    fn place_levels(&mut self) {
        let mut all: Vec<f32> = self.values.iter().map(|v| v.blur_sigma).collect();
        if self.blur_ramp[0] < 1.0 || self.blur_ramp[1] < 1.0 {
            all.push(0.0);
        }
        self.sigmas = blur_levels(&all);
        for v in &mut self.values {
            v.blur_level = level_position(&self.sigmas, v.blur_sigma);
        }
    }

    /// The recipe of shape `i`: its own, or the container's.
    pub fn recipe(&self, i: usize) -> &GlassParams {
        self.recipes
            .get(i)
            .and_then(Option::as_ref)
            .unwrap_or(&self.params)
    }

    /// The container with shape `i` in the recipe `params` (for that shape's size): its look and its size-dependent
    /// values, while the blur fill, the soft band, the shadows, the edge line and the veil stay the container's. Out of
    /// range does nothing.
    pub fn with_shape_recipe(mut self, i: usize, params: GlassParams) -> Self {
        if i < self.shapes.len() {
            self.values[i] = ShapeValues::of(&params);
            self.recipes[i] = Some(params);
            self.place_levels();
        }
        self
    }

    /// The container with shape `i` in the glass of `mode` at its size ([`Container::with_shape_recipe`]): a selected
    /// segment of another style than the group it is in, joined in one container.
    pub fn with_shape_mode(self, i: usize, mode: GlassMode) -> Self {
        match self.shapes.get(i) {
            Some(s) => {
                let params = GlassParams::new(mode, s.short_side());
                self.with_shape_recipe(i, params)
            }
            None => self,
        }
    }

    /// `change` made to the container's recipe and to each shape's own.
    pub fn for_each_recipe(&mut self, mut change: impl FnMut(&mut GlassParams)) {
        change(&mut self.params);
        for p in self.recipes.iter_mut().flatten() {
            change(p);
        }
    }

    /// Each shape's look ([`ShapeLook`]), in order.
    pub fn looks(&self) -> Vec<ShapeLook> {
        (0..self.shapes.len())
            .map(|i| ShapeLook::of(self.recipe(i)))
            .collect()
    }

    /// The container with `edge` along the outside of its outline.
    pub fn with_edge(mut self, edge: GlassEdge) -> Self {
        self.edge = edge;
        self
    }

    /// The opacity of the edge over the pixel (one unit wide) centred at `p`: each shape's band from its outline to the
    /// edge's width outside it ([`soft_edge_share`]) at the edge's opacity, left out by the share of the pixel each
    /// other shape covers, the bands laid over each other. Where the shapes overlap, a shape's outline inside another
    /// draws nothing, and where they only blend (the glass's neck between two shapes close to each other) each keeps its
    /// own outline: the edge is a stroke around each shape cut by the others, not the outline of the blended union.
    pub fn edge_alpha_at(&self, p: [f32; 2]) -> f32 {
        if !self.edge.shows() {
            return 0.0;
        }
        let d: Vec<f32> = self.shapes.iter().map(|s| s.sdf(p)).collect();
        let mut keep = 1.0;
        for (i, di) in d.iter().enumerate() {
            let band = soft_edge_share(*di, self.edge.width, self.edge.soft);
            if band <= 0.0 {
                continue;
            }
            let outside: f32 = d
                .iter()
                .enumerate()
                .filter(|&(j, _)| j != i)
                .map(|(_, dj)| 1.0 - (0.5 - dj).clamp(0.0, 1.0))
                .product();
            keep *= 1.0 - self.edge.color[3] * band * outside;
        }
        (1.0 - keep).clamp(0.0, 1.0)
    }

    /// The container whose blur fades by `ramp` from its top to its bottom ([`Container::blur_ramp`]): a ramp below 1
    /// adds an unblurred copy as the first level, so where the weight reaches 0 the glass shows what lies behind it
    /// sharp.
    pub fn with_blur_ramp(mut self, ramp: [f32; 2]) -> Self {
        self.blur_ramp = ramp.map(|w| w.clamp(0.0, 1.0));
        self.place_levels();
        self
    }

    /// Where the glass reads what lies behind `q`: magnified by [`Container::lens`] about its first shape's centre.
    pub fn lensed(&self, q: [f32; 2]) -> [f32; 2] {
        match self.shapes.first() {
            Some(s) if self.lens > 1.0 => {
                let c = [s.rect[0] + 0.5 * s.rect[2], s.rect[1] + 0.5 * s.rect[3]];
                [
                    c[0] + (q[0] - c[0]) / self.lens,
                    c[1] + (q[1] - c[1]) / self.lens,
                ]
            }
            _ => q,
        }
    }

    /// The share of its blur the glass keeps at `p` ([`Container::blur_ramp`]): from the top weight at the top of its
    /// first shape to the bottom weight at its bottom, along a smoothstep.
    pub fn blur_weight(&self, p: [f32; 2]) -> f32 {
        let [top, bottom] = self.blur_ramp;
        match self.shapes.first() {
            Some(s) if top < 1.0 || bottom < 1.0 => {
                let t = ((p[1] - s.rect[1]) / s.rect[3].max(1e-3)).clamp(0.0, 1.0);
                let t = t * t * (3.0 - 2.0 * t);
                top + (bottom - top) * t
            }
            _ => 1.0,
        }
    }

    /// The container cut to `cut` as well as to its shapes.
    pub fn cut_to(mut self, cut: Cut) -> Self {
        self.cut = Some(cut);
        self
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
        let edge = if self.edge.shows() {
            self.edge.width + 2.0 + self.edge.soft_reach()
        } else {
            0.0
        };
        let bleed_sigma = self.bleed_sigma();
        let stages = (0..self.shapes.len())
            .map(|i| self.stage_reach(i, bleed_sigma))
            .fold(self.params.reach(), f32::max);
        let r = stages.max(edge) + bulge;
        Some([x - r, y - r, w + 2.0 * r, h + 2.0 * r])
    }

    /// How far past shape `i` its recipe reads the backdrop or draws, in points: its blur and shadow
    /// ([`GlassParams::reach`]), the edge bleed's reach and three deviations of the copy it reads
    /// ([`Container::bleed_sigma`]), the bend outside the edge and the outer half of the control outline, each where it
    /// shows.
    fn stage_reach(&self, i: usize, bleed_sigma: Option<f32>) -> f32 {
        let (p, v) = (self.recipe(i), &self.values[i]);
        let bleed = if self.bleeds_at(i) {
            let sigma = bleed_sigma.unwrap_or(p.bleed.blur);
            v.bleed_amount.abs() + 3.0 * sigma.max(0.0)
        } else {
            0.0
        };
        let outer = if self.bends_outside(i) {
            v.outer_height + v.outer_amount.abs()
        } else {
            0.0
        };
        let border = if p.border.opacity > 0.0 && p.border.width > 0.0 {
            0.5 * p.border.width + 1.0
        } else {
            0.0
        };
        p.reach().max(bleed).max(outer).max(border)
    }

    /// Whether shape `i` bleeds the backdrop outside it into its edge.
    fn bleeds_at(&self, i: usize) -> bool {
        let v = &self.values[i];
        self.recipe(i).bleed.opacity > 0.0 && v.bleed_height > 0.0
    }

    /// Whether shape `i` bends the backdrop outside its edge.
    fn bends_outside(&self, i: usize) -> bool {
        let v = &self.values[i];
        self.recipe(i).refraction.outer_opacity > 0.0
            && v.outer_height > 0.0
            && v.outer_amount != 0.0
    }

    /// The field, the shape values and the shapes' looks blended like the field at `p`; where the shapes differ in look
    /// ([`Container::looks_differ`]), each shape's look laid over that, back to front, by the share of the pixel it
    /// covers, so a segment inside its group shows its own glass, antialiased along its outline.
    pub fn field_look(&self, p: [f32; 2]) -> (f32, ShapeValues, ShapeLook) {
        let (d, weights) = union_field(&self.shapes, self.spacing, p);
        let mut v = self.values[0];
        let mut look = ShapeLook::of(self.recipe(0));
        let mut total = weights[0];
        for (i, w) in weights.iter().enumerate().skip(1) {
            if total + w > 0.0 {
                let t = w / (total + w);
                v = ShapeValues::mix(v, self.values[i], t);
                look = ShapeLook::mix(look, ShapeLook::of(self.recipe(i)), t);
            }
            total += w;
        }
        if self.looks_differ() {
            for (i, s) in self.shapes.iter().enumerate() {
                let cov = (0.5 - s.sdf(p)).clamp(0.0, 1.0);
                if cov > 0.0 {
                    look = ShapeLook::mix(look, ShapeLook::of(self.recipe(i)), cov);
                }
            }
        }
        (d, v, look)
    }

    /// Whether the shapes' looks differ: some shape has a recipe of its own whose look is not the container's.
    pub fn looks_differ(&self) -> bool {
        let own = ShapeLook::of(&self.params);
        self.recipes
            .iter()
            .flatten()
            .any(|r| ShapeLook::of(r) != own)
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

    /// Shade the pixel at `p` (points): `sample(level, q)` reads copy `level` of the backdrop at `q`: the blurred copies
    /// `0..BLUR_LEVELS`, the blur fill ([`FILL_LEVEL`]), the edge bleed's blur ([`BLEED_LEVEL`]) and the backdrop as
    /// captured ([`SHARP_LEVEL`]). Returns the colour and its coverage (glass inside, shadow outside), none outside the
    /// outline the container is cut to.
    pub fn shade(&self, p: [f32; 2], sample: &dyn Fn(usize, [f32; 2]) -> Rgb) -> (Rgb, f32) {
        let (c, a) = self.shade_shapes(p, sample);
        let (c, a) = if self.edge.shows() {
            over_edge(c, a, &self.edge, self.edge_alpha_at(p) / self.edge.color[3])
        } else {
            (c, a)
        };
        (c, a * self.cut.map_or(1.0, |cut| cut.coverage(p)))
    }

    /// The colour and coverage of the shapes at `p`, before the cut.
    fn shade_shapes(&self, p: [f32; 2], sample: &dyn Fn(usize, [f32; 2]) -> Rgb) -> (Rgb, f32) {
        let (d, v, look) = self.field_look(p);
        // the container's recipe with the look of the shapes at p
        let mut here = self.params;
        look.apply(&mut here);
        let params = &here;
        let gradient = field_gradient(&self.shapes, self.spacing, p);
        let slope = (gradient[0] * gradient[0] + gradient[1] * gradient[1]).sqrt();
        let normal = if slope < 1e-6 {
            [0.0, 0.0]
        } else {
            [gradient[0] / slope, gradient[1] / slope]
        };
        // what lies under the shape at p, as its blur reads it: the edge line darkens it by its brightness
        let backdrop = || {
            let levels: [Rgb; BLUR_LEVELS] = std::array::from_fn(|i| sample(i, p));
            level_mix(levels, v.blur_level)
        };
        // the backdrop bent outside the edge, under what the shape draws there
        let bent = || {
            outer_read(
                p,
                d,
                normal,
                v.outer_amount,
                v.outer_height,
                params.refraction.outer_opacity,
            )
            .map(|(q, alpha)| (sample(SHARP_LEVEL, q), alpha))
        };
        if d > 0.5 {
            return self.outside(p, d, normal, backdrop(), params, bent());
        }
        let weight = self.blur_weight(p);
        let face_at = |q: [f32; 2]| {
            let levels: [Rgb; BLUR_LEVELS] = std::array::from_fn(|i| sample(i, q));
            let c = blur_fill_weighted(
                level_mix(levels, v.blur_level * weight),
                sample(FILL_LEVEL, q),
                params,
                weight,
            );
            face_tone(c, params, v.max_luma)
        };
        let off = refraction_offset(d, normal, v.refraction_amount, v.refraction_height);
        let mut c = face_at(self.lensed([p[0] + off[0], p[1] + off[1]]));
        let read = bleed_read(
            p,
            d,
            normal,
            v.bleed_amount,
            v.bleed_height,
            params.bleed.opacity,
        );
        if let Some((at, w)) = read {
            c = bleed_into(c, bleed_tone(sample(BLEED_LEVEL, at), params), w, params);
        }
        c = fill_and_tint(c, params);
        let de = edge_distance(d, slope);
        c = highlight(c, de, normal, params);
        c = border_and_light(c, d, (-de).max(0.0), normal, params);
        c = self.veil(p, c);
        // the ring is moved up: at p it is what the shape's field is a ring offset below p
        let ring = ring_shadow(
            self.field([p[0], p[1] + params.shadow.ring_offset]).0,
            params,
        );
        c = c.map(|v| v * (1.0 - ring));
        // an edge pixel: the glass over what lies outside it in the part it leaves uncovered
        let cov = (0.5 - d).clamp(0.0, 1.0);
        let (outside, alpha) = self.outside(p, d, normal, backdrop(), params, bent());
        let under = (1.0 - cov) * alpha;
        let a = cov + under;
        if a > 0.0 {
            c = std::array::from_fn(|i| (c[i].clamp(0.0, 1.0) * cov + outside[i] * under) / a);
        }
        (c, a)
    }

    /// The black veil over the face `c` at `p`: its opacity runs from the recipe's top value at the top of the shapes'
    /// bounds to its bottom value at their bottom.
    pub fn veil(&self, p: [f32; 2], c: Rgb) -> Rgb {
        let [top, bottom] = self.params.veil;
        if top <= 0.0 && bottom <= 0.0 {
            return c;
        }
        let Some(b) = self.bounds() else { return c };
        // the bounds are x, y, width, height
        let t = ((p[1] - b[1]) / b[3].max(1e-3)).clamp(0.0, 1.0);
        let k = 1.0 - (top + (bottom - top) * t).clamp(0.0, 1.0);
        c.map(|v| v * k)
    }

    /// The alpha of what the shape draws outside itself at `p`, `d` from its edge: the drop shadow and the outline.
    pub fn outside_alpha(&self, p: [f32; 2], d: f32) -> f32 {
        let s = &self.params.shadow;
        let d_shadow = self.field([p[0] - s.offset[0], p[1] - s.offset[1]]).0;
        (shadow_alpha(d_shadow, &self.params) + outline_alpha(d, &self.params)).min(1.0)
    }

    /// What the shape draws outside itself at `p`, `d` from its edge whose outward normal is `normal`, over a
    /// `backdrop` of that colour, in the recipe `here` of the shapes there: the drop shadow and the outline in the
    /// shadow's colour, with the edge line ([`edge_line_alpha`]) and the control outline ([`border_line`]) over them in
    /// black, all over `bent`, the bent backdrop's colour and alpha where it shows ([`outer_read`]). Returns the colour
    /// and its alpha.
    pub fn outside(
        &self,
        p: [f32; 2],
        d: f32,
        normal: [f32; 2],
        backdrop: Rgb,
        here: &GlassParams,
        bent: Option<(Rgb, f32)>,
    ) -> (Rgb, f32) {
        let fill = self.params.shadow.fill;
        let shadow = self.outside_alpha(p, d);
        let mut line = edge_line_alpha(d, backdrop, normal, &self.params);
        let border = border_line(d, normal, here);
        if border > 0.0 {
            line = line + border - line * border;
        }
        let a = line + shadow * (1.0 - line);
        let (c, a) = if a <= 0.0 {
            ([fill[0], fill[1], fill[2]], 0.0)
        } else {
            let k = shadow * (1.0 - line) / a;
            ([fill[0] * k, fill[1] * k, fill[2] * k], a)
        };
        match bent {
            Some((r, o)) if o > 0.0 => {
                let out = a + o * (1.0 - a);
                (
                    std::array::from_fn(|i| (c[i] * a + r[i] * o * (1.0 - a)) / out),
                    out,
                )
            }
            _ => (c, a),
        }
    }
}

/// Where the edge bleed of a container reads its blurred backdrop ([`Container::bleed_source`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BleedSource {
    /// A blurred copy the shapes read already, of the same deviation.
    Level(usize),
    /// The blur fill, of the same deviation.
    Fill,
    /// A copy of its own, blurred with this deviation in points.
    Own(f32),
}

impl Container {
    /// Where the edge bleed reads its blurred backdrop, if a shape bleeds: the widest blur of the bleeding shapes'
    /// recipes, from the copy the container blurs already whose deviation is nearest it when that lies within
    /// [`BLEED_SHARE_TOLERANCE`] of it (or within the deviation two copies share), a level's before the blur fill's at
    /// the same distance, else from a copy of its own.
    pub fn bleed_source(&self) -> Option<BleedSource> {
        let sigma = (0..self.shapes.len())
            .filter(|&i| self.bleeds_at(i))
            .map(|i| self.recipe(i).bleed.blur.clamp(0.0, BLUR_SIGMA_MAX))
            .reduce(f32::max)?;
        let tolerance = (BLEED_SHARE_TOLERANCE * sigma).max(SAME_SIGMA);
        let levels = self
            .levels()
            .into_iter()
            .enumerate()
            .filter(|&(_, (_, used))| used)
            .map(|(i, (s, _))| (BleedSource::Level(i), (s - sigma).abs()));
        let fill = self
            .fill_sigma()
            .map(|s| (BleedSource::Fill, (s - sigma).abs()));
        let nearest = levels
            .chain(fill)
            .filter(|&(_, d)| d < tolerance)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        Some(nearest.map_or(BleedSource::Own(sigma), |(source, _)| source))
    }

    /// The deviation in points of the copy the edge bleed reads ([`Container::bleed_source`]), if a shape bleeds.
    pub fn bleed_sigma(&self) -> Option<f32> {
        Some(match self.bleed_source()? {
            BleedSource::Level(i) => self.levels()[i].0,
            BleedSource::Fill => self.fill_sigma().unwrap_or(0.0),
            BleedSource::Own(sigma) => sigma,
        })
    }

    /// Whether a shape bends the backdrop outside its edge, which reads the backdrop as captured ([`SHARP_LEVEL`]).
    pub fn reads_sharp(&self) -> bool {
        (0..self.shapes.len()).any(|i| self.bends_outside(i))
    }
    /// The deviation in points of each blurred copy of the backdrop, and whether a shape reads it.
    pub fn levels(&self) -> [(f32, bool); BLUR_LEVELS] {
        let mut used = [false; BLUR_LEVELS];
        // a blur ramp reads every level from the sharp one up to the shape's
        let ramp = self.blur_ramp[0] < 1.0 || self.blur_ramp[1] < 1.0;
        for v in &self.values {
            for (i, (u, w)) in used.iter_mut().zip(level_weights(v.blur_level)).enumerate() {
                *u |= w > 1e-3 || (ramp && i as f32 <= v.blur_level.ceil());
            }
        }
        let last = self.sigmas.last().copied().unwrap_or(0.0);
        std::array::from_fn(|i| (self.sigmas.get(i).copied().unwrap_or(last), used[i]))
    }

    /// The deviation in points of the blur fill, if it shows.
    pub fn fill_sigma(&self) -> Option<f32> {
        let sigma = self.params.fill_sigma();
        (self.params.fill_shows() && sigma > 0.0).then_some(sigma)
    }
}
