//! Buffer scale of a surface and where it comes from.

use std::num::NonZeroU32;

/// Output scale in 120ths, the unit of `wp_fractional_scale_v1` (protocol text). An integer `wl_output` scale `n` is
/// `n * 120`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Scale(NonZeroU32);

impl Scale {
    const DENOMINATOR: u32 = 120;

    pub const ONE: Scale = Scale(NonZeroU32::new(Self::DENOMINATOR).unwrap());

    /// Scale of a `wl_output.scale` or `preferred_buffer_scale` factor; `None` for zero or less.
    pub fn from_integer(factor: i32) -> Option<Scale> {
        u32::try_from(factor)
            .ok()
            .and_then(|f| f.checked_mul(Self::DENOMINATOR))
            .and_then(NonZeroU32::new)
            .map(Scale)
    }

    /// Scale of a `preferred_scale` value; `None` for zero.
    pub fn from_120ths(value: u32) -> Option<Scale> {
        NonZeroU32::new(value).map(Scale)
    }

    /// Device pixels per logical point.
    pub fn factor(self) -> f64 {
        f64::from(self.0.get()) / f64::from(Self::DENOMINATOR)
    }

    /// The factor when it is a whole number, as `wl_surface.set_buffer_scale` needs it.
    pub fn integer(self) -> Option<i32> {
        self.0
            .get()
            .is_multiple_of(Self::DENOMINATOR)
            .then(|| (self.0.get() / Self::DENOMINATOR) as i32)
    }

    /// The smallest whole factor at or above this scale.
    pub fn ceil(self) -> Scale {
        Scale(
            NonZeroU32::new(self.0.get().div_ceil(Self::DENOMINATOR) * Self::DENOMINATOR)
                .expect("a non-zero scale rounds up to a non-zero scale"),
        )
    }

    /// Buffer extent for a logical extent, rounded to the nearest pixel with halves up (the rule of
    /// `wp_fractional_scale_v1`); `None` when it overflows.
    pub fn to_physical(self, logical: u32) -> Option<u32> {
        let scaled = u64::from(logical) * u64::from(self.0.get());
        u32::try_from((scaled + u64::from(Self::DENOMINATOR) / 2) / u64::from(Self::DENOMINATOR))
            .ok()
    }
}

/// What the compositor said about a surface's scale. The most specific source wins: the fractional scale (with
/// viewporter), then `wl_surface.preferred_buffer_scale` (version 6), then the largest scale of the outputs the
/// surface entered, then one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScaleSources {
    pub fractional: Option<Scale>,
    pub preferred: Option<Scale>,
    pub entered: Vec<Scale>,
}

impl ScaleSources {
    pub(crate) fn resolve(&self, fractional_supported: bool) -> Scale {
        let integer = || {
            self.preferred
                .or_else(|| self.entered.iter().copied().max())
                .unwrap_or(Scale::ONE)
        };
        match (fractional_supported, self.fractional) {
            (true, Some(s)) => s,
            // without a viewport a fractional buffer cannot be shown, so it is drawn at the next whole factor
            _ => integer().ceil(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u32) -> Scale {
        Scale::from_120ths(n).unwrap()
    }

    #[test]
    fn integer_scales_are_multiples_of_120() {
        assert_eq!(Scale::from_integer(2), Some(s(240)));
        assert_eq!(s(240).integer(), Some(2));
        assert_eq!(Scale::from_integer(0), None);
        assert_eq!(Scale::from_integer(-1), None);
        assert_eq!(s(150).integer(), None);
        assert_eq!(s(150).ceil(), s(240));
    }

    #[test]
    fn physical_size_rounds_to_nearest_with_halves_up() {
        // 1.25 * 3 = 3.75 -> 4; 1.25 * 2 = 2.5 -> 3; 1.25 * 1 = 1.25 -> 1
        let r = |n| s(150).to_physical(n).unwrap();
        assert_eq!((r(3), r(2), r(1)), (4, 3, 1));
        assert_eq!(s(240).to_physical(u32::MAX), None);
    }

    #[test]
    fn fractional_beats_preferred_beats_the_largest_entered_output() {
        let mut sources = ScaleSources {
            entered: vec![s(120), s(240)],
            ..ScaleSources::default()
        };
        assert_eq!(sources.resolve(true), s(240));
        sources.preferred = Some(s(360));
        assert_eq!(sources.resolve(true), s(360));
        sources.fractional = Some(s(180));
        assert_eq!(sources.resolve(true), s(180));
        assert_eq!(ScaleSources::default().resolve(true), Scale::ONE);
    }

    #[test]
    fn a_fractional_scale_without_the_viewporter_is_ignored() {
        let sources = ScaleSources {
            fractional: Some(s(180)),
            entered: vec![s(120)],
            ..ScaleSources::default()
        };
        assert_eq!(sources.resolve(false), s(120));
    }
}
