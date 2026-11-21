//! The mapping between the raw values of a volume control and a level from 0 to 1.
//!
//! The rule is that of alsa-utils' `volume_mapping.c` (ISC licence), written afresh. With dB information:
//! a control whose range is at most 24 dB is linear in dB; a wider one maps `10^((dB - max) / 60 dB)`, rescaled
//! so that its lowest step is 0 (the position then follows perceived loudness). Without dB information the
//! mapping is linear in the raw value.

use super::error::{Error, Result};
use super::tlv::{DbCurve, DB_GAIN_MUTE};

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
            return Err(Error::EmptyRange {
                min: raw_min,
                max: raw_max,
            });
        }
        Ok(Self {
            raw_min,
            raw_max,
            db,
        })
    }

    /// Gain of `raw` in 1/100 dB; `None` without dB information.
    fn db_at(&self, raw: i64) -> Option<i32> {
        self.db
            .as_ref()
            .map(|curve| curve.db(self.raw_min, self.raw_max, raw))
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
            (Some((min, max)), Some(db)) if max - min <= LINEAR_DB_LIMIT => {
                f64::from(db - min) / f64::from(max - min)
            }
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
}
