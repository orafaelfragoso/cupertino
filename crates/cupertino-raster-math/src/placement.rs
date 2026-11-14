//! Where an image drawn one source pixel per device pixel lands: on whole pixels, so it is copied rather than
//! resampled. Its device position is taken in window pixels and moved to the nearest whole pixel, a half pixel moving
//! towards positive infinity. Rounded so, a position lands on the same window pixel whatever piece of the window a
//! renderer draws it into: moving by whole pixels (the origin of a scratch image, a target or a scroll layer's image)
//! commutes with it, where rounding halves away from zero does not (in a piece whose origin lies past the image, its
//! position is negative there and a half moves the other way).

/// `v` on the nearest whole pixel, a half towards positive infinity: `floor(v + 0.5)`.
pub fn whole_pixel(v: f32) -> f32 {
    (v + 0.5).floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_pixel_moves_towards_positive_infinity_on_either_side_of_zero() {
        assert_eq!(whole_pixel(77.5), 78.0);
        assert_eq!(whole_pixel(-17.5), -17.0);
        assert_eq!(whole_pixel(0.5), 1.0);
        assert_eq!(whole_pixel(-0.5), 0.0);
        assert_eq!(whole_pixel(12.25), 12.0);
        assert_eq!(whole_pixel(-12.75), -13.0);
    }

    #[test]
    fn moving_by_whole_pixels_before_or_after_rounding_gives_the_same_pixel() {
        for quarter in -4000..4000 {
            let v = quarter as f32 / 4.0;
            for origin in [-301, -95, -1, 0, 1, 17, 95, 2160] {
                let o = origin as f32;
                assert_eq!(whole_pixel(v - o), whole_pixel(v) - o, "{v} from {origin}");
            }
        }
    }
}
