//! The quantities of an output, kept apart: the **mode** (the panel pixels the display scans out), the **logical
//! extent** (the points the desktop is laid out in), the **effective scale** (panel pixels per point) and a
//! **raster** (the pixels of a buffer drawn for a logical extent at some buffer scale). Every constructor checks its
//! inputs, so a zero extent, a scale that is not a finite positive number or a size that overflows fails with a
//! [`GeometryError`] instead of producing a degenerate output.

use std::fmt;

/// A width and a height, in whatever unit the holder says: panel pixels, points or buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

impl Extent {
    pub const fn new(width: u32, height: u32) -> Extent {
        Extent { width, height }
    }

    /// The extent with its sides swapped (a display turned by 90 or 270 degrees).
    pub const fn swapped(self) -> Extent {
        Extent {
            width: self.height,
            height: self.width,
        }
    }

    /// The number of samples, without overflow.
    pub fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

impl fmt::Display for Extent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\u{202f}×\u{202f}{}", self.width, self.height)
    }
}

/// Why a geometry could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryError {
    /// A side is zero, or the scale leaves less than one point or one pixel.
    Empty,
    /// The scale is not a finite number above zero.
    InvalidScale,
    /// A side does not fit in 32 bits.
    Overflow,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GeometryError::Empty => "the size is empty",
            GeometryError::InvalidScale => "the scale is not a finite number above zero",
            GeometryError::Overflow => "the size is too large",
        })
    }
}

impl std::error::Error for GeometryError {}

/// One side divided by `scale`, rounded to the nearest whole unit (the way an output's logical size is derived from
/// its mode); at least one unit.
fn divide(side: u32, scale: f64) -> Result<u32, GeometryError> {
    let value = (f64::from(side) / scale).round();
    if value < 1.0 {
        return Err(GeometryError::Empty);
    }
    if value > f64::from(u32::MAX) {
        return Err(GeometryError::Overflow);
    }
    Ok(value as u32)
}

fn check_scale(scale: f64) -> Result<(), GeometryError> {
    if scale.is_finite() && scale > 0.0 {
        Ok(())
    } else {
        Err(GeometryError::InvalidScale)
    }
}

/// Denominator of a buffer scale: the 120ths of the fractional-scale protocol. A whole buffer scale `n` is `n * 120`.
pub const SCALE_DENOMINATOR: u32 = 120;

/// The buffer extent for `logical` points drawn at `scale_120ths / 120` buffer pixels per point, each side rounded to
/// the nearest pixel with halves up (the rounding rule of the fractional-scale protocol). Fails on a zero side or
/// scale and when a side overflows.
pub fn raster(logical: Extent, scale_120ths: u32) -> Result<Extent, GeometryError> {
    if scale_120ths == 0 {
        return Err(GeometryError::InvalidScale);
    }
    if logical.is_empty() {
        return Err(GeometryError::Empty);
    }
    let side = |points: u32| {
        let scaled = u64::from(points) * u64::from(scale_120ths);
        u32::try_from((scaled + u64::from(SCALE_DENOMINATOR) / 2) / u64::from(SCALE_DENOMINATOR))
            .map_err(|_| GeometryError::Overflow)
    };
    Ok(Extent::new(side(logical.width)?, side(logical.height)?))
}

/// A scale as the fractional-scale protocol sends it: the nearest 120th, at least one.
pub fn scale_in_120ths(scale: f64) -> Result<u32, GeometryError> {
    check_scale(scale)?;
    let value = (scale * f64::from(SCALE_DENOMINATOR)).round();
    if value > f64::from(u32::MAX) {
        return Err(GeometryError::Overflow);
    }
    Ok((value as u32).max(1))
}

/// An output: its mode, how it is turned, its effective scale and the logical extent they give.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputGeometry {
    /// The mode in panel pixels, as the display scans it out (not turned).
    pub mode: Extent,
    /// Turned by 90 or 270 degrees: the logical extent has the mode's sides swapped.
    pub rotated: bool,
    /// Panel pixels per logical point. Drawing maps a point to `scale` pixels on both axes.
    pub scale: f64,
    /// The extent in logical points: the turned mode divided by the scale, rounded.
    pub logical: Extent,
}

impl OutputGeometry {
    /// An output showing `mode` at `scale`. Fails when the mode is empty, the scale is not finite and positive or it
    /// leaves less than a point.
    pub fn new(mode: Extent, scale: f64, rotated: bool) -> Result<OutputGeometry, GeometryError> {
        check_scale(scale)?;
        if mode.is_empty() {
            return Err(GeometryError::Empty);
        }
        let turned = if rotated { mode.swapped() } else { mode };
        let logical = Extent::new(divide(turned.width, scale)?, divide(turned.height, scale)?);
        Ok(OutputGeometry {
            mode,
            rotated,
            scale,
            logical,
        })
    }

    /// An output showing `mode` so that it is `logical_width` points wide: the scale is the turned mode's width over
    /// it, and the height follows from that scale.
    pub fn with_logical_width(
        mode: Extent,
        logical_width: u32,
        rotated: bool,
    ) -> Result<OutputGeometry, GeometryError> {
        if logical_width == 0 {
            return Err(GeometryError::Empty);
        }
        let turned = if rotated { mode.swapped() } else { mode };
        OutputGeometry::new(
            mode,
            f64::from(turned.width) / f64::from(logical_width),
            rotated,
        )
    }

    /// The scale clients are asked to draw at, in 120ths.
    pub fn buffer_scale_120ths(&self) -> u32 {
        scale_in_120ths(self.scale).expect("the scale was checked")
    }

    /// The buffer extent a client draws the whole logical extent with at the preferred fractional scale.
    pub fn raster(&self) -> Result<Extent, GeometryError> {
        raster(self.logical, self.buffer_scale_120ths())
    }

    /// The buffer extent at the next whole scale: what a client that cannot draw fractional buffers uses.
    pub fn whole_raster(&self) -> Result<Extent, GeometryError> {
        let whole = self.buffer_scale_120ths().div_ceil(SCALE_DENOMINATOR) * SCALE_DENOMINATOR;
        raster(self.logical, whole)
    }

    /// A point of the output (relative to its top-left corner) in its turned panel pixels.
    pub fn to_pixels(&self, point: (f64, f64)) -> (f64, f64) {
        (point.0 * self.scale, point.1 * self.scale)
    }

    /// A position in the output's turned panel pixels (a pointer that reports pixels) as a point relative to the
    /// output's top-left corner: the inverse of [`OutputGeometry::to_pixels`].
    pub fn to_points(&self, pixels: (f64, f64)) -> (f64, f64) {
        (pixels.0 / self.scale, pixels.1 / self.scale)
    }

    /// The nearest point coordinate that falls on a panel pixel edge: an edge drawn there covers whole pixels.
    pub fn snap(&self, points: f64) -> f64 {
        (points * self.scale).round() / self.scale
    }

    /// The smallest pixel-aligned rectangle (`x`, `y`, `width`, `height` in points) that covers `rect`.
    pub fn snap_outward(&self, rect: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
        let s = self.scale;
        let (x0, y0) = ((rect.0 * s).floor() / s, (rect.1 * s).floor() / s);
        let (x1, y1) = (
            ((rect.0 + rect.2) * s).ceil() / s,
            ((rect.1 + rect.3) * s).ceil() / s,
        );
        (x0, y0, x1 - x0, y1 - y0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANEL: Extent = Extent::new(3024, 1964);

    #[test]
    fn the_logical_extent_is_the_mode_over_the_scale() {
        let g = OutputGeometry::new(PANEL, 2.0, false).unwrap();
        assert_eq!(g.logical, Extent::new(1512, 982));
        assert_eq!(
            g.raster().unwrap(),
            PANEL,
            "at a whole scale the raster is the panel"
        );
        let turned = OutputGeometry::new(PANEL, 2.0, true).unwrap();
        assert_eq!(turned.logical, Extent::new(982, 1512));
        assert_eq!(turned.mode, PANEL, "the mode is not turned");
    }

    #[test]
    fn a_logical_width_keeps_its_points_and_the_panel_keeps_its_pixels() {
        for (width, height) in [
            (1024, 665),
            (1147, 745),
            (1352, 878),
            (1512, 982),
            (1800, 1169),
        ] {
            let g = OutputGeometry::with_logical_width(PANEL, width, false).unwrap();
            assert_eq!(g.logical, Extent::new(width, height));
            let (w, h) = g.to_pixels((f64::from(width), f64::from(height)));
            assert!((w - 3024.0).abs() < 1e-9, "{width}: {w}");
            assert!((h - 1964.0).abs() <= g.scale / 2.0, "{width}: {h}");
        }
    }

    #[test]
    fn rasters_round_halves_up_and_whole_rasters_use_the_next_whole_scale() {
        // 1.25: 3 points are 3.75 pixels (4), 2 points 2.5 (3)
        assert_eq!(raster(Extent::new(3, 2), 150).unwrap(), Extent::new(4, 3));
        let g = OutputGeometry::new(Extent::new(1920, 1200), 1.25, false).unwrap();
        assert_eq!(g.logical, Extent::new(1536, 960));
        assert_eq!(g.raster().unwrap(), Extent::new(1920, 1200));
        assert_eq!(g.whole_raster().unwrap(), Extent::new(3072, 1920));
    }

    #[test]
    fn invalid_and_overflowing_sizes_fail_cleanly() {
        assert_eq!(
            OutputGeometry::new(Extent::new(0, 10), 1.0, false),
            Err(GeometryError::Empty)
        );
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                OutputGeometry::new(PANEL, scale, false),
                Err(GeometryError::InvalidScale),
                "{scale}"
            );
        }
        assert_eq!(
            OutputGeometry::new(PANEL, 4000.0, false),
            Err(GeometryError::Empty)
        );
        assert_eq!(
            OutputGeometry::new(PANEL, 1e-9, false),
            Err(GeometryError::Overflow)
        );
        assert_eq!(
            OutputGeometry::with_logical_width(PANEL, 0, false),
            Err(GeometryError::Empty)
        );
        assert_eq!(
            raster(Extent::new(u32::MAX, 1), 240),
            Err(GeometryError::Overflow)
        );
        assert_eq!(
            raster(Extent::new(1, 1), 0),
            Err(GeometryError::InvalidScale)
        );
        assert_eq!(raster(Extent::new(0, 1), 120), Err(GeometryError::Empty));
        assert_eq!(scale_in_120ths(f64::MAX), Err(GeometryError::Overflow));
    }

    #[test]
    fn pointer_pixels_and_points_map_back_and_forth_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 3024.0 / 1024.0] {
            let g = OutputGeometry::new(PANEL, scale, false).unwrap();
            for pixels in [(0.0, 0.0), (1.0, 1.0), (1511.5, 981.25), (3023.0, 1963.0)] {
                let back = g.to_pixels(g.to_points(pixels));
                assert!((back.0 - pixels.0).abs() < 1e-9 && (back.1 - pixels.1).abs() < 1e-9);
            }
            // the far corner of the panel is the far corner of the logical extent, within half a point
            let corner = g.to_points((3024.0, 1964.0));
            assert!(
                (corner.0 - f64::from(g.logical.width)).abs() <= 0.5,
                "{scale}"
            );
            assert!(
                (corner.1 - f64::from(g.logical.height)).abs() <= 0.5,
                "{scale}"
            );
        }
    }

    #[test]
    fn snapped_edges_fall_on_pixel_edges() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let g = OutputGeometry::new(PANEL, scale, false).unwrap();
            for edge in [0.3, 10.25, 99.9, 512.6] {
                let pixels = g.snap(edge) * scale;
                assert!((pixels - pixels.round()).abs() < 1e-9, "{scale} {edge}");
                assert!((g.snap(edge) - edge).abs() <= 0.5 / scale + 1e-9);
            }
            let (x, y, w, h) = g.snap_outward((0.25, 0.25, 10.0, 10.0));
            assert!(x <= 0.25 && y <= 0.25 && x + w >= 10.25 && y + h >= 10.25);
            for v in [x, y, x + w, y + h] {
                assert!(((v * scale) - (v * scale).round()).abs() < 1e-9, "{scale}");
            }
        }
        // at 2, half a point is a whole pixel: (0.25, 0.25, 10, 10) covers (0, 0, 10.5, 10.5)
        let g = OutputGeometry::new(PANEL, 2.0, false).unwrap();
        assert_eq!(
            g.snap_outward((0.25, 0.25, 10.0, 10.0)),
            (0.0, 0.0, 10.5, 10.5)
        );
    }
}
