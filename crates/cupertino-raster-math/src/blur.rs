//! The blur: a Gaussian of deviation σ made as a pyramid of 2× box halvings and a separable Gaussian on the last
//! level.
//!
//! * The plan ([`plan_blur`]): the capture at the backdrop scale by repeated halvings, then one more halving while the
//!   remaining variance is above 64 texels² and the level is larger than 16 texels; each halving takes 1/16 texel² of
//!   variance at the capture and 1/4 of a level texel² after it. What is left is the Gaussian of the last level.
//! * The kernel ([`gaussian_weights`]): one-sided weights to `ceil(3σ)`, at most [`MAX_BLUR_RADIUS`], normalised so
//!   the whole kernel sums to one; below σ = 0.3 a single tap.
//! * Edges ([`Edge`]): a backdrop reads past its edge as the edge texel (the screen goes on); a layer's own effects
//!   read past the layer as transparent.
//! * Alpha: every pass works on premultiplied channels.
//! * Precision ([`Precision`]): intermediate levels are 16-bit floats where the device renders to them, 8-bit
//!   otherwise; [`Image`] computes in `f32`.
//!
//! `σ_texels = σ_pt · display_scale · backdrop_scale`.

/// Largest one-sided kernel radius supported by the shaders (36 weights in 9 vec4).
pub const MAX_BLUR_RADIUS: usize = 35;

/// List of downsample steps and the residual Gaussian.
#[derive(Clone, Debug)]
pub struct BlurPlan {
    /// Sizes after each 2× downsample (the first ones are the capture at the backdrop scale).
    pub levels: Vec<(u32, u32)>,
    /// Remaining σ in texels of the last level.
    pub sigma: f32,
    /// One-sided weights (`w[0]` centre).
    pub weights: Vec<f32>,
}

impl BlurPlan {
    /// Size of the processed backdrop for a source of `(w, h)`.
    pub fn final_size(&self, w: u32, h: u32) -> (u32, u32) {
        self.levels.last().copied().unwrap_or((w, h))
    }
}

pub fn plan_blur(
    w: u32,
    h: u32,
    backdrop_scale: f32,
    sigma_pt: f32,
    display_scale: f32,
) -> BlurPlan {
    let mut levels = vec![];
    let (mut cw, mut ch) = (w, h);
    // capture at the backdrop scale via repeated 2× box (the scale is 0.5 in every archive)
    let mut s = 1.0f32;
    while s > backdrop_scale + 1e-3 {
        cw = (cw / 2).max(1);
        ch = (ch / 2).max(1);
        levels.push((cw, ch));
        s *= 0.5;
    }
    let mut var =
        (sigma_pt * display_scale * backdrop_scale).powi(2) - 0.0625 * (levels.len() as f32);
    while var > 64.0 && cw.min(ch) > 16 {
        cw /= 2;
        ch /= 2;
        levels.push((cw, ch));
        var = (var - 0.25) / 4.0;
    }
    let sigma = var.max(0.0).sqrt();
    let weights = gaussian_weights(sigma);
    BlurPlan {
        levels,
        sigma,
        weights,
    }
}

/// One-sided Gaussian weights normalised so that `w[0] + 2·Σw[1..] = 1`; radius `ceil(3σ)` clamped
/// to [`MAX_BLUR_RADIUS`].
pub fn gaussian_weights(sigma: f32) -> Vec<f32> {
    if sigma < 0.3 {
        return vec![1.0];
    }
    let r = ((3.0 * sigma).ceil() as usize).min(MAX_BLUR_RADIUS);
    let mut w: Vec<f32> = (0..=r)
        .map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum = w[0] + 2.0 * w[1..].iter().sum::<f32>();
    w.iter_mut().for_each(|x| *x /= sum);
    w
}

/// What a blur reads past the edge of its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// The nearest edge texel: a backdrop, whose picture goes on past what was captured.
    Clamp,
    /// Transparent black: the blur or shadow of a layer's own content.
    Transparent,
}

/// What the levels between the passes are stored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    /// 16-bit floats per channel, where the device can render to them.
    Half,
    /// 8 bits per channel, rounded to nearest after each pass.
    Byte,
}

impl Precision {
    /// `v` as the level stores it.
    pub fn store(self, v: f32) -> f32 {
        match self {
            Precision::Half => half_round(v),
            Precision::Byte => (v.clamp(0.0, 1.0) * 255.0).round() / 255.0,
        }
    }
}

/// `v` rounded to the nearest 16-bit float (ties to even), back as `f32`; finite inputs within the half range only.
fn half_round(v: f32) -> f32 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let e = v.abs().log2().floor().max(-14.0);
    let step = (e - 10.0).exp2();
    (v / step).round_ties_even() * step
}

/// The deviation, in points, of the blur a toolkit `radius` (points) names: half the radius.
pub fn sigma_for_radius(radius: f32) -> f32 {
    radius * 0.5
}

/// A premultiplied RGBA image of `f32` channels, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 4]>,
}

impl Image {
    pub fn new(width: usize, height: usize, pixels: Vec<[f32; 4]>) -> Self {
        assert_eq!(pixels.len(), width * height, "{width} x {height} pixels");
        Self {
            width,
            height,
            pixels,
        }
    }

    /// The pixel at `(x, y)`, or what the edge rule reads there.
    pub fn read(&self, x: i64, y: i64, edge: Edge) -> [f32; 4] {
        let (w, h) = (self.width as i64, self.height as i64);
        if (0..w).contains(&x) && (0..h).contains(&y) {
            return self.pixels[(y * w + x) as usize];
        }
        match edge {
            Edge::Clamp => self.pixels[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize],
            Edge::Transparent => [0.0; 4],
        }
    }

    /// One 2× box halving into `(w, h)`: each texel the mean of the 2 × 2 texels it covers.
    pub fn halve(&self, (w, h): (u32, u32), edge: Edge, precision: Precision) -> Image {
        let (w, h) = (w as usize, h as usize);
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let mut acc = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = self.read(2 * x + dx, 2 * y + dy, edge);
                    (0..4).for_each(|c| acc[c] += p[c]);
                }
                out.push(acc.map(|v| precision.store(v * 0.25)));
            }
        }
        Image::new(w, h, out)
    }

    /// One separable pass of the one-sided `weights` along `(dx, dy)`.
    pub fn convolve(
        &self,
        weights: &[f32],
        (dx, dy): (i64, i64),
        edge: Edge,
        precision: Precision,
    ) -> Image {
        let mut out = Vec::with_capacity(self.pixels.len());
        for y in 0..self.height as i64 {
            for x in 0..self.width as i64 {
                let centre = self.read(x, y, edge);
                let mut acc = centre.map(|v| v * weights[0]);
                for (i, w) in weights.iter().enumerate().skip(1) {
                    let i = i as i64;
                    let (a, b) = (
                        self.read(x + dx * i, y + dy * i, edge),
                        self.read(x - dx * i, y - dy * i, edge),
                    );
                    (0..4).for_each(|c| acc[c] += (a[c] + b[c]) * w);
                }
                out.push(acc.map(|v| precision.store(v)));
            }
        }
        Image::new(self.width, self.height, out)
    }

    /// The whole blur of `plan`: its halvings, then the Gaussian across and down the last level.
    pub fn blur(&self, plan: &BlurPlan, edge: Edge, precision: Precision) -> Image {
        let mut cur = self.clone();
        for &level in &plan.levels {
            cur = cur.halve(level, edge, precision);
        }
        if plan.weights.len() > 1 {
            cur = cur
                .convolve(&plan.weights, (1, 0), edge, precision)
                .convolve(&plan.weights, (0, 1), edge, precision);
        }
        cur
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_are_normalised() {
        for s in [0.5, 1.0, 4.0, 8.0, 100.0] {
            let w = gaussian_weights(s);
            let sum = w[0] + 2.0 * w[1..].iter().sum::<f32>();
            assert!((sum - 1.0).abs() < 1e-5);
            assert!(w.len() <= MAX_BLUR_RADIUS + 1);
        }
        assert_eq!(gaussian_weights(0.1), vec![1.0]);
    }

    #[test]
    fn menu_bar_plan() {
        // σ = 180 pt at scale 0.5 on a 1× display: 90 texels → pyramid until σ² ≤ 64
        let p = plan_blur(1920, 1080, 0.5, 180.0, 1.0);
        assert_eq!(p.levels[0], (960, 540));
        assert!(p.levels.len() >= 3);
        assert!(p.sigma <= 8.0 + 1e-3);
        assert!(p.weights.len() <= MAX_BLUR_RADIUS + 1);
    }

    #[test]
    fn sigma_30_pt_adds_one_pyramid_level() {
        // σ = 30 pt at scale 0.5 → 15 texels: variance 224.9 > 64 → one more 2× step
        let p = plan_blur(1920, 1080, 0.5, 30.0, 1.0);
        assert_eq!(p.levels, vec![(960, 540), (480, 270)]);
        let expect = ((15.0f32 * 15.0 - 0.0625 - 0.25) / 4.0).sqrt();
        assert!((p.sigma - expect).abs() < 1e-3, "{} vs {expect}", p.sigma);
    }

    #[test]
    fn a_flat_backdrop_stays_flat_and_a_layer_fades_at_its_edge() {
        let flat = Image::new(32, 32, vec![[0.25, 0.5, 0.75, 1.0]; 32 * 32]);
        let plan = plan_blur(32, 32, 0.5, 4.0, 1.0);
        let clamped = flat.blur(&plan, Edge::Clamp, Precision::Half);
        assert!(clamped
            .pixels
            .iter()
            .all(|p| (p[1] - 0.5).abs() < 1e-3 && (p[3] - 1.0).abs() < 1e-3));
        let faded = flat.blur(&plan, Edge::Transparent, Precision::Half);
        let corner = faded.pixels[0];
        let middle = faded.pixels[faded.width * (faded.height / 2) + faded.width / 2];
        assert!(
            corner[3] < 0.6 && (middle[3] - 1.0).abs() < 1e-2,
            "{corner:?} {middle:?}"
        );
        // premultiplied: the colour fades with the alpha, so the straight colour is kept
        assert!((corner[1] / corner[3] - 0.5).abs() < 1e-2);
    }

    #[test]
    fn levels_keep_their_precision() {
        assert_eq!(Precision::Byte.store(0.5), 128.0 / 255.0);
        assert_eq!(Precision::Byte.store(1.5), 1.0);
        assert_eq!(Precision::Half.store(1.0), 1.0);
        // 1 + 2^-11 lies halfway between two halves and rounds to the even one
        assert_eq!(Precision::Half.store(1.0 + 2f32.powi(-11)), 1.0);
        assert_eq!(
            Precision::Half.store(1.0 + 3.0 * 2f32.powi(-11)),
            1.0 + 2f32.powi(-9)
        );
        assert_eq!(sigma_for_radius(10.0), 5.0);
    }

    #[test]
    fn no_blur_without_sigma() {
        let p = plan_blur(100, 100, 1.0, 0.0, 1.0);
        assert!(p.levels.is_empty());
        assert_eq!(p.weights, vec![1.0]);
        assert_eq!(p.final_size(100, 100), (100, 100));
    }
}
