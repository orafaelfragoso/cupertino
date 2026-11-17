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
