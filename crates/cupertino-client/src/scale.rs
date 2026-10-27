//! Buffer scale of a surface.

use std::num::NonZeroU32;

/// Output scale in 120ths, the unit of `wp_fractional_scale_v1` (protocol text). An
/// integer `wl_output` scale `n` is `n * 120`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Scale(NonZeroU32);

impl Scale {
    const DENOMINATOR: u32 = 120;

    pub const ONE: Scale = Scale(NonZeroU32::new(Self::DENOMINATOR).unwrap());

    /// Scale of a `wl_output.scale` factor; `None` for zero.
    pub fn from_integer(factor: u32) -> Option<Scale> {
        factor
            .checked_mul(Self::DENOMINATOR)
            .and_then(NonZeroU32::new)
            .map(Scale)
    }

    /// Scale of a `preferred_scale` value; `None` for zero.
    pub fn from_120ths(value: u32) -> Option<Scale> {
        NonZeroU32::new(value).map(Scale)
    }

    /// Device pixels per logical point.
    pub fn factor(self) -> f32 {
        self.0.get() as f32 / Self::DENOMINATOR as f32
    }

    /// The factor when it is a whole number, as `wl_surface.set_buffer_scale` needs it.
    pub fn integer(self) -> Option<i32> {
        self.0
            .get()
            .is_multiple_of(Self::DENOMINATOR)
            .then(|| (self.0.get() / Self::DENOMINATOR) as i32)
    }

    /// The transport's scale, which counts in 120ths as well.
    pub(crate) fn of(scale: cupertino_wayland_client::Scale) -> Scale {
        let value = (scale.factor() * f64::from(Self::DENOMINATOR)).round() as u32;
        Scale::from_120ths(value).unwrap_or(Scale::ONE)
    }

    /// Buffer extent for a logical extent, rounded to the nearest pixel with halves up (the
    /// rule of `wp_fractional_scale_v1`).
    pub fn to_physical(self, logical: u32) -> u32 {
        let scaled = u64::from(logical) * u64::from(self.0.get());
        ((scaled + u64::from(Self::DENOMINATOR) / 2) / u64::from(Self::DENOMINATOR)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_scales_are_multiples_of_120() {
        assert_eq!(Scale::from_integer(2), Scale::from_120ths(240));
        assert_eq!(Scale::from_integer(2).unwrap().integer(), Some(2));
        assert_eq!(Scale::from_integer(0), None);
        assert_eq!(Scale::from_120ths(0), None);
    }

    #[test]
    fn fractional_scale_has_no_integer_factor() {
        let s = Scale::from_120ths(150).unwrap();
        assert_eq!(s.integer(), None);
        assert_eq!(s.factor(), 1.25);
    }

    #[test]
    fn physical_size_rounds_to_nearest_with_halves_up() {
        let s = Scale::from_120ths(150).unwrap();
        // 1.25 * 3 = 3.75 -> 4; 1.25 * 2 = 2.5 -> 3; 1.25 * 1 = 1.25 -> 1
        assert_eq!(
            (s.to_physical(3), s.to_physical(2), s.to_physical(1)),
            (4, 3, 1)
        );
        assert_eq!(Scale::ONE.to_physical(344), 344);
        assert_eq!(Scale::from_integer(2).unwrap().to_physical(344), 688);
    }
}
