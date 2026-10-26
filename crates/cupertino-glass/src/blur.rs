//! Blur plan shared by the GPU renderer and the CPU reference.
//!
//! The σ → texel mapping
//! `σ_texels = radius · display_scale · backdrop_scale` is, and the pyramid thresholds
//! below are an independent design.

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
    fn no_blur_without_sigma() {
        let p = plan_blur(100, 100, 1.0, 0.0, 1.0);
        assert!(p.levels.is_empty());
        assert_eq!(p.weights, vec![1.0]);
        assert_eq!(p.final_size(100, 100), (100, 100));
    }
}
