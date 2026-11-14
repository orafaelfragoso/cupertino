//! The chrome's shapes (the title bar, its rim and the window buttons' discs and rings) drawn from the shared
//! distance function (`cupertino_raster_math::coverage`): each pixel's coverage is that of its centre's distance to
//! the outline, a stroke covers what lies inside the outline and outside the outline moved in by its width, and
//! gradients are the shared rule evaluated at each pixel's centre. A GPU program computing the same functions draws
//! the same pixels. The buttons' glyphs are paths and keep the path rasterizer.

use cupertino_geometry::{CornerStyle, Radii, Rect, RoundedRect};
use cupertino_raster_math::coverage::{coverage, rounded_distance, stroke_coverage};
use cupertino_raster_math::dither;
use cupertino_raster_math::gradient::{linear_t, Stop, Stops};
use tiny_skia::Pixmap;

/// A rounded rectangle in device pixels, each corner with its own extent, all of one exponent.
#[derive(Clone, Copy, Debug)]
pub struct Outline {
    centre: [f32; 2],
    half: [f32; 2],
    /// Top-left, top-right, bottom-right, bottom-left.
    extents: [f32; 4],
    exponent: f32,
}

impl Outline {
    /// `rect` with `radii` and `style` (logical units) at `scale` device pixels per unit; the extents shrink so
    /// adjacent corners fit, as the window's shape does.
    pub fn rounded(rect: Rect, radii: Radii, style: CornerStyle, scale: f64) -> Outline {
        let extents = RoundedRect::with_radii(rect, radii, style).extents();
        let s = scale as f32;
        Outline {
            centre: [
                ((rect.min_x() + rect.width() / 2.0) * scale) as f32,
                ((rect.min_y() + rect.height() / 2.0) * scale) as f32,
            ],
            half: [
                (rect.width() / 2.0 * scale) as f32,
                (rect.height() / 2.0 * scale) as f32,
            ],
            extents: extents.map(|e| e as f32 * s),
            exponent: style.exponent() as f32,
        }
    }

    /// A circle of `radius` around `centre`, in device pixels.
    pub fn circle(centre: [f32; 2], radius: f32) -> Outline {
        Outline {
            centre,
            half: [radius, radius],
            extents: [radius; 4],
            exponent: 2.0,
        }
    }

    /// Signed distance, in device pixels, from the centre of pixel `(x, y)` to the outline.
    pub fn distance(&self, x: u32, y: u32) -> f32 {
        let p = [
            x as f32 + 0.5 - self.centre[0],
            y as f32 + 0.5 - self.centre[1],
        ];
        let e = match (p[0] >= 0.0, p[1] >= 0.0) {
            (false, false) => self.extents[0],
            (true, false) => self.extents[1],
            (true, true) => self.extents[2],
            (false, true) => self.extents[3],
        };
        let e = e.min(self.half[0]).min(self.half[1]);
        rounded_distance(p, self.half, e, self.exponent)
    }

    /// The pixels the outline can touch inside a pixmap of `w` x `h`.
    fn span(&self, w: u32, h: u32) -> (u32, u32, u32, u32) {
        let lo = |c: f32, half: f32| (c - half - 1.0).floor().max(0.0) as u32;
        let hi = |c: f32, half: f32, n: u32| ((c + half + 1.0).ceil().max(0.0) as u32).min(n);
        (
            lo(self.centre[0], self.half[0]),
            lo(self.centre[1], self.half[1]),
            hi(self.centre[0], self.half[0], w),
            hi(self.centre[1], self.half[1], h),
        )
    }
}

/// What a shape is filled with: a straight-alpha colour, or a linear gradient (device pixels) of straight stops.
pub enum Shade {
    Solid([f32; 4]),
    Linear {
        start: [f32; 2],
        end: [f32; 2],
        stops: Stops,
    },
}

impl Shade {
    /// A linear gradient of `stops` (offset, straight colour); `None` when the shared rule refuses them.
    pub fn linear(start: [f32; 2], end: [f32; 2], stops: &[(f32, [f32; 4])]) -> Option<Shade> {
        let stops: Vec<Stop> = stops
            .iter()
            .map(|&(offset, color)| Stop {
                offset,
                color: color.map(|v| v.clamp(0.0, 1.0)),
            })
            .collect();
        Some(Shade::Linear {
            start,
            end,
            stops: Stops::new(&stops).ok()?,
        })
    }

    /// The premultiplied colour at pixel `(x, y)`, and whether it is a gradient's (whose colour is dithered).
    fn at(&self, x: u32, y: u32) -> ([f32; 4], bool) {
        match self {
            Shade::Solid(c) => {
                let c = c.map(|v| v.clamp(0.0, 1.0));
                ([c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]], false)
            }
            Shade::Linear { start, end, stops } => {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                (stops.at(linear_t(p, *start, *end)), true)
            }
        }
    }
}

/// Lays `shade` over `pm` through the coverage of `outline`, or of the stroke of `stroke` device pixels inside it.
pub fn fill(pm: &mut Pixmap, outline: &Outline, shade: &Shade, stroke: Option<f32>) {
    let (w, h) = (pm.width(), pm.height());
    let (x0, y0, x1, y1) = outline.span(w, h);
    let data = pm.data_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            let d = outline.distance(x, y);
            let cov = match stroke {
                Some(width) => stroke_coverage(d, width, 1.0),
                None => coverage(d, 1.0),
            };
            if cov <= 0.0 {
                continue;
            }
            let (c, gradient) = shade.at(x, y);
            let i = (y * w + x) as usize * 4;
            let px = &mut data[i..i + 4];
            let keep = 1.0 - c[3] * cov;
            let alpha = dither::quantize(c[3] * cov + f32::from(px[3]) / 255.0 * keep);
            for k in 0..3 {
                let v = c[k] * cov + f32::from(px[k]) / 255.0 * keep;
                let stored = if gradient && dither::BY_DEFAULT {
                    dither::store(v, x, y)
                } else {
                    dither::quantize(v)
                };
                px[k] = stored.min(alpha);
            }
            px[3] = alpha;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_raster_math::vectors;

    #[test]
    fn coverage_matches_the_shared_vectors() {
        for row in vectors::rows(vectors::COVERAGE) {
            let [px, py, hx, hy, extent, exponent, pixels, distance, _] =
                row[..].try_into().expect("nine columns");
            let s = pixels as f32;
            // the vector's point at the centre of pixel (200, 200), its units `pixels` device pixels each
            let outline = Outline {
                centre: [200.5 - px as f32 * s, 200.5 - py as f32 * s],
                half: [hx as f32 * s, hy as f32 * s],
                extents: [extent as f32 * s; 4],
                exponent: exponent as f32,
            };
            let got = outline.distance(200, 200);
            let want = distance as f32 * s;
            assert!(
                (got - want).abs() < 2e-3 * want.abs().max(1.0),
                "{row:?}: {got} device pixels, the definition {want}"
            );
        }
    }

    #[test]
    fn a_title_bar_has_round_top_corners_and_square_bottom_ones() {
        let outline = Outline::rounded(
            Rect::new(0.0, 0.0, 100.0, 28.0),
            Radii::top(12.0),
            CornerStyle::Circular,
            2.0,
        );
        let mut pm = Pixmap::new(200, 56).unwrap();
        fill(&mut pm, &outline, &Shade::Solid([1.0, 1.0, 1.0, 1.0]), None);
        let alpha = |x: u32, y: u32| pm.data()[((y * 200 + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(0, 55), 255);
        assert_eq!(alpha(199, 55), 255);
        assert_eq!(alpha(100, 0), 255);
        // the stroke of one pixel inside the outline is a ring one pixel wide
        let mut ring = Pixmap::new(200, 56).unwrap();
        fill(&mut ring, &outline, &Shade::Solid([1.0; 4]), Some(1.0));
        let a = |x: u32, y: u32| ring.data()[((y * 200 + x) * 4 + 3) as usize];
        assert_eq!((a(100, 0), a(100, 1), a(100, 30)), (255, 0, 0));
    }
}
