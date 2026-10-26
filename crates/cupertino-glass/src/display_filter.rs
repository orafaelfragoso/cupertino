//! Display colour filters, invert and contrast as a colour chain for a full-output pass. The
//! reference applies them after composition, per display, in this order: the filter's 3x3 matrix
//! (on linear light), then invert, channel gains, contrast and fade on the
//! encoded values (order). Gains and fade are not settable from the settings pane,
//! so the chain has no step for them.
//!
//! The matrices are the design's builders, recomputed by;
//! only the 3x3 part reaches the display (post offsets,
//! gain and reduce white point are dropped).

use cupertino_defaults::accessibility::{AccessibilitySettings, ColorFilter};
use cupertino_theme::recipe::ColorOp;

pub type Matrix = [[f64; 3]; 3];

const IDENTITY: Matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Grayscale row weights.
const GRAY: [f64; 3] = [0.3, 0.59, 0.11];

/// Scale all entries by 1 / the largest row sum when that sum is below 1 (`Normalize`).
fn normalize(m: Matrix) -> Matrix {
    let max = m.iter().map(|r| r.iter().sum::<f64>()).fold(f64::NEG_INFINITY, f64::max);
    if max < 1.0 {
        m.map(|r| r.map(|v| v / max))
    } else {
        m
    }
}

/// Row-major matrix of `filter` at `intensity` (clamped to 0 to 1 as the builders do).
pub fn filter_matrix(filter: ColorFilter, intensity: f64) -> Matrix {
    let t = intensity.clamp(0.0, 1.0);
    let m = match filter {
        ColorFilter::Protanopia => [
            [1.0 + 0.1232846863 * t, -0.123283886 * t, 1.667437058e-07 * t],
            [-0.120003647 * t, 1.0 + 0.120002868 * t, -1.623060692e-07 * t],
            [0.7325098937 * t, -0.7325051386 * t, 1.0 + 9.90726569e-07 * t],
        ],
        ColorFilter::Deuteranopia => [
            [1.0 - 0.2356889393 * t, 0.9881298247 * t, -0.7524408854 * t],
            [-0.0719766168 * t, 1.0 + 0.1307082029 * t, -0.0587315861 * t],
            [0.8301453055 * t, -0.8926484 * t, 1.0 + 0.0625030946 * t],
        ],
        ColorFilter::Tritanopia => [[1.0 + 0.1332794002 * t, 0.0, -0.1332794002 * t], [0.0, 1.0 + 0.52448918 * t, -0.52448918 * t], [0.0, 0.0, 1.0]],
        ColorFilter::Grayscale => {
            let mut m = IDENTITY;
            for row in &mut m {
                for (v, g) in row.iter_mut().zip(GRAY) {
                    *v = (1.0 - t) * *v + t * g;
                }
            }
            m
        }
        ColorFilter::Tint { hue } => {
            // HSV (hue * 359.9 degrees, 1, 1) blended towards white by 1 - intensity, on the diagonal
            let s6 = hue * 359.9 / 60.0;
            let k = s6.floor();
            let f = s6 - k;
            let c = match k as i64 {
                0 => [1.0, f, 0.0],
                1 => [1.0 - f, 1.0, 0.0],
                2 => [0.0, 1.0, f],
                3 => [0.0, 1.0 - f, 1.0],
                4 => [f, 0.0, 1.0],
                _ => [1.0, 0.0, 1.0 - f],
            };
            let d = c.map(|v| t * v + (1.0 - t));
            [[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]]
        }
    };
    normalize(m)
}

/// The colour chain of `settings`, empty when the display is drawn unchanged.
pub fn display_filter_ops(settings: &AccessibilitySettings) -> Vec<ColorOp> {
    let mut ops = Vec::new();
    if let Some((filter, intensity)) = settings.filter() {
        ops.push(ColorOp::LinearMatrix(filter_matrix(filter, intensity).map(|r| r.map(|v| v as f32))));
    }
    if settings.invert_colors {
        ops.push(ColorOp::Invert);
    }
    if settings.display_contrast > 0.0 {
        ops.push(ColorOp::Contrast(settings.display_contrast as f32));
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(got: Matrix, want: Matrix) {
        for (g, w) in got.iter().flatten().zip(want.iter().flatten()) {
            assert!((g - w).abs() < 1e-8, "{got:?} vs {want:?}");
        }
    }

    /// Expected rows printed by `cupertino_ax_filters.py` (red 0.5, green 0.5, grayscale 1, tint 0.3 0.4, tint 0.95 1).
    #[test]
    fn the_matrices_match_the_reference_builders() {
        close(
            filter_matrix(ColorFilter::Protanopia, 0.5),
            [[1.061642343, -0.061641943, 0.000000083], [-0.060001824, 1.060001434, -0.000000081], [0.366254947, -0.366252569, 1.000000495]],
        );
        close(
            filter_matrix(ColorFilter::Deuteranopia, 0.5),
            [[0.882155530, 0.494064912, -0.376220443], [-0.035988308, 1.065354101, -0.029365793], [0.415072653, -0.446324200, 1.031251547]],
        );
        close(filter_matrix(ColorFilter::Grayscale, 1.0), [GRAY; 3]);
        close(filter_matrix(ColorFilter::Tint { hue: 0.3 }, 0.4), [[0.6802, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.6]]);
        close(filter_matrix(ColorFilter::Tint { hue: 0.95 }, 1.0), [[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.301583333333]]);
    }

    #[test]
    fn the_chain_runs_matrix_then_invert_then_contrast_and_is_empty_by_default() {
        assert!(display_filter_ops(&AccessibilitySettings::default()).is_empty());
        let s = AccessibilitySettings { invert_colors: true, display_contrast: 0.4, color_filter: Some(ColorFilter::Grayscale), ..AccessibilitySettings::default() };
        let ops = display_filter_ops(&s);
        assert!(matches!(ops[0], ColorOp::LinearMatrix(m) if m == [[0.3_f32, 0.59, 0.11]; 3]));
        assert_eq!(ops[1..], [ColorOp::Invert, ColorOp::Contrast(0.4)]);
    }
}
