//! The mapping between the raw values of a volume control and a level from 0 to 1.
//!
//! The rule is that of alsa-utils' `volume_mapping.c` (ISC licence), written afresh. With dB information:
//! a control whose range is at most 24 dB is linear in dB; a wider one maps `10^((dB - max) / 60 dB)`, rescaled
//! so that its lowest step is 0 (the position then follows perceived loudness). Without dB information the
//! mapping is linear in the raw value.

use crate::error::{Error, Result};
use crate::tlv::{DbCurve, DB_GAIN_MUTE};

/// Widest range, in 1/100 dB, that maps linearly in dB (`MAX_LINEAR_DB_SCALE` is 24 dB).
const LINEAR_DB_LIMIT: i32 = 24 * 100;
/// The gain, in 1/100 dB, that changes the perceived level by a factor of 10 in the logarithmic mapping.
const DB_PER_DECADE: f64 = 6000.0;

/// The raw range of a volume control and, when the driver provides it, its gain curve.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeRange {
    raw_min: i64,
    raw_max: i64,
    db: Option<DbCurve>,
}

impl VolumeRange {
    /// A control spanning `raw_min..=raw_max`; fails when the range holds a single value.
    pub fn new(raw_min: i64, raw_max: i64, db: Option<DbCurve>) -> Result<Self> {
        if raw_min >= raw_max {
            return Err(Error::EmptyRange { min: raw_min, max: raw_max });
        }
        Ok(Self { raw_min, raw_max, db })
    }

    /// The lowest raw value.
    pub fn raw_min(&self) -> i64 {
        self.raw_min
    }

    /// The highest raw value.
    pub fn raw_max(&self) -> i64 {
        self.raw_max
    }

    /// Gain of `raw` in 1/100 dB; `None` without dB information.
    fn db_at(&self, raw: i64) -> Option<i32> {
        self.db.as_ref().map(|curve| curve.db(self.raw_min, self.raw_max, raw))
    }

    /// Gains of the lowest and highest raw value; `None` without dB information or when the curve is flat.
    fn db_span(&self) -> Option<(i32, i32)> {
        let (min, max) = (self.db_at(self.raw_min)?, self.db_at(self.raw_max)?);
        (min < max).then_some((min, max))
    }

    /// Position of the logarithmic mapping at the lowest step, which is subtracted so that step is 0.
    fn min_norm(min: i32, max: i32) -> f64 {
        if min == DB_GAIN_MUTE {
            0.0
        } else {
            10f64.powf(f64::from(min - max) / DB_PER_DECADE)
        }
    }

    /// The level, 0 to 1, of the raw value `raw` (clamped into the range).
    pub fn level(&self, raw: i64) -> f64 {
        let raw = raw.clamp(self.raw_min, self.raw_max);
        let level = match (self.db_span(), self.db_at(raw)) {
            (Some((min, max)), Some(db)) if max - min <= LINEAR_DB_LIMIT => f64::from(db - min) / f64::from(max - min),
            (Some((min, max)), Some(db)) => {
                let min_norm = Self::min_norm(min, max);
                (10f64.powf(f64::from(db - max) / DB_PER_DECADE) - min_norm) / (1.0 - min_norm)
            }
            _ => (raw - self.raw_min) as f64 / (self.raw_max - self.raw_min) as f64,
        };
        level.clamp(0.0, 1.0)
    }

    /// The raw value whose level is nearest to `level` (0 to 1; panics on NaN).
    pub fn raw(&self, level: f64) -> i64 {
        assert!(!level.is_nan(), "a volume level is a number");
        let level = level.clamp(0.0, 1.0);
        let Some((min, max)) = self.db_span() else {
            return self.raw_min + (level * (self.raw_max - self.raw_min) as f64).round() as i64;
        };
        if level == 0.0 {
            return self.raw_min;
        }
        if level == 1.0 {
            return self.raw_max;
        }
        let target = if max - min <= LINEAR_DB_LIMIT {
            f64::from(min) + level * f64::from(max - min)
        } else {
            let min_norm = Self::min_norm(min, max);
            DB_PER_DECADE * (level * (1.0 - min_norm) + min_norm).log10() + f64::from(max)
        };
        self.nearest_raw(target)
    }

    /// The raw value whose gain is nearest to `target` (1/100 dB); the gain rises with the raw value.
    fn nearest_raw(&self, target: f64) -> i64 {
        let gain = |raw: i64| f64::from(self.db_at(raw).expect("only called with dB information"));
        // first raw value whose gain reaches the target
        let (mut low, mut high) = (self.raw_min, self.raw_max);
        while low < high {
            let mid = low + (high - low) / 2;
            if gain(mid) < target {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        if low > self.raw_min && target - gain(low - 1) < gain(low) - target {
            low - 1
        } else {
            low
        }
    }

    /// The raw value one press of size `step` (a level difference) from `raw` in `direction` (the sign counts). A
    /// press always moves at least one raw step unless the end is reached; `direction` 0 stays.
    pub fn step(&self, raw: i64, direction: i32, step: f64) -> i64 {
        let current = raw.clamp(self.raw_min, self.raw_max);
        match direction.signum() {
            1 => self.raw(self.level(current) + step).max(current + 1).min(self.raw_max),
            -1 => self.raw(self.level(current) - step).min(current - 1).max(self.raw_min),
            _ => current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An HDA codec amplifier: raw 0 to 87, -97.50 dB at 0 and 0.75 dB per step (65.25 dB wide: logarithmic mapping).
    fn hda() -> VolumeRange {
        VolumeRange::new(0, 87, Some(DbCurve::Scale { min: -9750, step: 75, mute: false })).unwrap()
    }

    #[test]
    fn wide_db_range_maps_on_the_loudness_curve() {
        let range = hda();
        assert_eq!(range.level(0), 0.0);
        assert!((range.level(60) - 0.411_626).abs() < 1e-5, "{}", range.level(60));
        assert_eq!(range.level(87), 1.0);
    }

    #[test]
    fn wide_db_range_inverts_to_the_nearest_raw_value() {
        let range = hda();
        // target gain for level 0.5 is -48.26 dB = raw 65.65
        assert_eq!(range.raw(0.5), 66);
        assert_eq!((range.raw(0.0), range.raw(1.0)), (0, 87));
        assert_eq!(range.raw(range.level(23)), 23);
    }

    #[test]
    fn narrow_db_range_is_linear_in_db() {
        // -20 dB to 0 dB in 0.5 dB steps: 20 dB wide
        let range = VolumeRange::new(0, 40, Some(DbCurve::Scale { min: -2000, step: 50, mute: false })).unwrap();
        assert_eq!(range.level(10), 0.25);
        assert_eq!(range.raw(0.25), 10);
    }

    #[test]
    fn muted_lowest_step_is_level_zero_without_a_min_norm() {
        // raw 0 is mute, raw 51 is 0 dB with 1 dB per step above it: 50 dB wide plus mute
        let range = VolumeRange::new(0, 51, Some(DbCurve::Scale { min: -5100, step: 100, mute: true })).unwrap();
        assert_eq!(range.level(0), 0.0);
        // raw 1 is -50 dB: 10^(-50/60) = 0.1468
        assert!((range.level(1) - 0.146_780).abs() < 1e-5, "{}", range.level(1));
        assert_eq!(range.raw(0.0), 0);
    }

    #[test]
    fn controls_without_db_data_are_linear_in_the_raw_value() {
        let range = VolumeRange::new(0, 31, None).unwrap();
        assert!((range.level(8) - 8.0 / 31.0).abs() < 1e-12);
        assert_eq!(range.raw(0.5), 16);
        let offset = VolumeRange::new(10, 20, None).unwrap();
        assert_eq!((offset.level(15), offset.raw(0.5)), (0.5, 15));
    }

    #[test]
    fn a_press_moves_by_the_step_and_clamps_at_the_ends() {
        let range = VolumeRange::new(0, 64, None).unwrap();
        assert_eq!(range.step(32, 1, 1.0 / 16.0), 36);
        assert_eq!(range.step(32, -1, 1.0 / 16.0), 28);
        assert_eq!(range.step(62, 1, 1.0 / 16.0), 64);
        assert_eq!(range.step(64, 1, 1.0 / 16.0), 64);
        assert_eq!(range.step(2, -1, 1.0 / 16.0), 0);
        assert_eq!(range.step(0, -1, 1.0 / 16.0), 0);
    }

    #[test]
    fn a_press_moves_at_least_one_raw_step() {
        // 4 raw values: a 1/64 press is far below one raw step
        let range = VolumeRange::new(0, 3, None).unwrap();
        assert_eq!(range.step(1, 1, 1.0 / 64.0), 2);
        assert_eq!(range.step(2, -1, 1.0 / 64.0), 1);
    }

    #[test]
    fn a_range_of_one_value_is_refused() {
        assert!(matches!(VolumeRange::new(5, 5, None), Err(Error::EmptyRange { min: 5, max: 5 })));
    }
}
