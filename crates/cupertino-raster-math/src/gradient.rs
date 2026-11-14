//! Gradients, evaluated once per pixel at its centre.
//!
//! A gradient has two to [`MAX_STOPS`] stops of straight-alpha colour in encoded sRGB at increasing offsets. The
//! parameter `t` of a pixel is its projection onto the line from start to end (linear) or its distance from the
//! centre over the radius (radial); before the first stop and after the last the end colours hold. Between two stops
//! the straight colour is interpolated linearly in encoded sRGB, then premultiplied.

/// Most stops a gradient may have.
pub const MAX_STOPS: usize = 8;

/// A colour stop: offset (0 to 1) and straight RGBA in encoded sRGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub offset: f32,
    pub color: [f32; 4],
}

/// Why stops make no gradient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopsError {
    /// Fewer than two stops.
    TooFew,
    /// More than [`MAX_STOPS`].
    TooMany,
    /// Offsets decrease, or one lies outside 0 to 1.
    Unordered,
}

/// Stops checked once, so every renderer refuses the same gradients.
#[derive(Clone, Debug, PartialEq)]
pub struct Stops(Vec<Stop>);

impl Stops {
    pub fn new(stops: &[Stop]) -> Result<Self, StopsError> {
        if stops.len() < 2 {
            return Err(StopsError::TooFew);
        }
        if stops.len() > MAX_STOPS {
            return Err(StopsError::TooMany);
        }
        let in_range = stops.iter().all(|s| (0.0..=1.0).contains(&s.offset));
        if !in_range || stops.windows(2).any(|w| w[1].offset < w[0].offset) {
            return Err(StopsError::Unordered);
        }
        Ok(Self(stops.to_vec()))
    }

    pub fn as_slice(&self) -> &[Stop] {
        &self.0
    }

    /// The premultiplied colour at `t`.
    pub fn at(&self, t: f32) -> [f32; 4] {
        let s = &self.0;
        let t = if t.is_nan() { 0.0 } else { t };
        let straight = if t <= s[0].offset {
            s[0].color
        } else if t >= s[s.len() - 1].offset {
            s[s.len() - 1].color
        } else {
            let i = s
                .windows(2)
                .position(|w| t < w[1].offset)
                .unwrap_or(s.len() - 2);
            let (a, b) = (s[i], s[i + 1]);
            let span = b.offset - a.offset;
            let k = if span > 0.0 {
                (t - a.offset) / span
            } else {
                1.0
            };
            std::array::from_fn(|c| a.color[c] + (b.color[c] - a.color[c]) * k)
        };
        let alpha = straight[3];
        [
            straight[0] * alpha,
            straight[1] * alpha,
            straight[2] * alpha,
            alpha,
        ]
    }
}

/// `t` of the point `p` on the linear gradient from `start` to `end`.
pub fn linear_t(p: [f32; 2], start: [f32; 2], end: [f32; 2]) -> f32 {
    let d = [end[0] - start[0], end[1] - start[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    if len2 <= 0.0 {
        return 0.0;
    }
    ((p[0] - start[0]) * d[0] + (p[1] - start[1]) * d[1]) / len2
}

/// `t` of the point `p` on the radial gradient of `centre` and `radius`.
pub fn radial_t(p: [f32; 2], centre: [f32; 2], radius: f32) -> f32 {
    if radius <= 0.0 {
        return 1.0;
    }
    let d = [p[0] - centre[0], p[1] - centre[1]];
    (d[0] * d[0] + d[1] * d[1]).sqrt() / radius
}

/// The centre of pixel `(x, y)`, where a gradient is evaluated.
pub fn pixel_centre(x: u32, y: u32) -> [f32; 2] {
    [x as f32 + 0.5, y as f32 + 0.5]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(offset: f32, color: [f32; 4]) -> Stop {
        Stop { offset, color }
    }

    #[test]
    fn stops_are_two_to_eight_in_order() {
        let s = stop(0.0, [0.0; 4]);
        assert_eq!(Stops::new(&[s]), Err(StopsError::TooFew));
        assert_eq!(Stops::new(&[s; 9]), Err(StopsError::TooMany));
        assert!(Stops::new(&[s; 8]).is_ok());
        assert_eq!(
            Stops::new(&[stop(0.5, [0.0; 4]), stop(0.2, [0.0; 4])]),
            Err(StopsError::Unordered)
        );
    }

    #[test]
    fn straight_colour_is_interpolated_then_premultiplied() {
        let g = Stops::new(&[
            stop(0.0, [1.0, 0.0, 0.0, 1.0]),
            stop(1.0, [0.0, 0.0, 1.0, 0.0]),
        ])
        .unwrap();
        // halfway: straight (0.5, 0, 0.5) at alpha 0.5, premultiplied
        assert_eq!(g.at(0.5), [0.25, 0.0, 0.25, 0.5]);
        assert_eq!(g.at(-1.0), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(g.at(2.0), [0.0, 0.0, 0.0, 0.0]);
        // two stops at one offset make a hard step
        let step = Stops::new(&[
            stop(0.0, [0.0, 0.0, 0.0, 1.0]),
            stop(0.5, [0.0, 0.0, 0.0, 1.0]),
            stop(0.5, [1.0, 1.0, 1.0, 1.0]),
            stop(1.0, [1.0, 1.0, 1.0, 1.0]),
        ])
        .unwrap();
        assert_eq!(step.at(0.49)[0], 0.0);
        assert_eq!(step.at(0.5)[0], 1.0);
    }

    #[test]
    fn linear_and_radial_parameters() {
        assert_eq!(linear_t([5.0, 3.0], [0.0, 0.0], [10.0, 0.0]), 0.5);
        assert_eq!(linear_t([5.0, 3.0], [1.0, 1.0], [1.0, 1.0]), 0.0);
        assert_eq!(radial_t([3.0, 4.0], [0.0, 0.0], 10.0), 0.5);
        assert_eq!(pixel_centre(2, 0), [2.5, 0.5]);
    }
}
