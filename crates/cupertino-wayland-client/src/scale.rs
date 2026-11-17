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
