//! dB information of a control, from its TLV block (`include/uapi/sound/tlv.h`).
//!
//! A TLV block is a sequence of 32-bit words: `type`, `length` in bytes, then `length` bytes of data. Gains are
//! in 1/100 dB. Only the types that describe a gain curve are modelled: `DB_SCALE`, `DB_MINMAX`,
//! `DB_MINMAX_MUTE` and `DB_RANGE` (a list of ranges, each with one of the others inside). Every other type
//! (containers, `DB_LINEAR`, channel maps) is "no dB information", and the control then maps on its raw range.

use crate::error::{Error, Result};

/// `SNDRV_CTL_TLVD_DB_GAIN_MUTE`: the gain of a muted step, in 1/100 dB.
pub const DB_GAIN_MUTE: i32 = -9_999_999;

const TYPE_DB_SCALE: u32 = 1;
const TYPE_DB_RANGE: u32 = 3;
const TYPE_DB_MINMAX: u32 = 4;
const TYPE_DB_MINMAX_MUTE: u32 = 5;

/// `SNDRV_CTL_TLVD_DB_SCALE_MASK` and `SNDRV_CTL_TLVD_DB_SCALE_MUTE` of the second word of `DB_SCALE`.
const SCALE_STEP_MASK: u32 = 0xffff;
const SCALE_MUTE: u32 = 0x10000;

/// The gain of every raw value of a control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbCurve {
    /// `DB_SCALE`: `min` at the lowest raw value, `step` more for each raw step.
    Scale {
        /// Gain of the lowest raw value, 1/100 dB.
        min: i32,
        /// Gain per raw step, 1/100 dB.
        step: u32,
        /// The lowest raw value is mute instead of `min`.
        mute: bool,
    },
    /// `DB_MINMAX` and `DB_MINMAX_MUTE`: gains `min` and `max` at the ends, linear in between.
    MinMax {
        /// Gain of the lowest raw value, 1/100 dB.
        min: i32,
        /// Gain of the highest raw value, 1/100 dB.
        max: i32,
        /// The lowest raw value is mute instead of `min`.
        mute: bool,
    },
    /// `DB_RANGE`: a curve for each part of the raw range.
    Range(Vec<DbSegment>),
}

/// One part of a [`DbCurve::Range`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbSegment {
    /// First raw value of the part.
    pub raw_min: i64,
    /// Last raw value of the part.
    pub raw_max: i64,
    /// The gain curve inside the part, which spans `raw_min..=raw_max` by itself.
    pub curve: DbCurve,
}

fn word(words: &[u32], index: usize) -> Result<u32> {
    words
        .get(index)
        .copied()
        .ok_or(Error::MalformedTlv("data ends inside an entry"))
}

/// Parse one `type, length, data` block from the start of `words`; returns the curve (if the type carries one) and
/// the words the block took.
fn parse_block(words: &[u32]) -> Result<(Option<DbCurve>, usize)> {
    let (kind, len) = (word(words, 0)?, word(words, 1)? as usize);
    if !len.is_multiple_of(4) {
        return Err(Error::MalformedTlv("length is not a whole number of words"));
    }
    let data = words
        .get(2..2 + len / 4)
        .ok_or(Error::MalformedTlv("length runs past the data"))?;
    let used = 2 + len / 4;
    let curve = match kind {
        TYPE_DB_SCALE => {
            let (min, step_and_mute) = (word(data, 0)?, word(data, 1)?);
            Some(DbCurve::Scale {
                min: min as i32,
                step: step_and_mute & SCALE_STEP_MASK,
                mute: step_and_mute & SCALE_MUTE != 0,
            })
        }
        TYPE_DB_MINMAX | TYPE_DB_MINMAX_MUTE => Some(DbCurve::MinMax {
            min: word(data, 0)? as i32,
            max: word(data, 1)? as i32,
            mute: kind == TYPE_DB_MINMAX_MUTE,
        }),
        TYPE_DB_RANGE => parse_range(data)?,
        _ => None,
    };
    Ok((curve, used))
}

/// The segments of a `DB_RANGE` body: `raw_min, raw_max, <nested block>` repeated. A nested block without a gain
/// curve makes the whole range unusable.
fn parse_range(mut data: &[u32]) -> Result<Option<DbCurve>> {
    let mut segments = Vec::new();
    while !data.is_empty() {
        let (raw_min, raw_max) = (word(data, 0)? as i32, word(data, 1)? as i32);
        let (curve, used) = parse_block(&data[2..])?;
        let Some(curve) = curve else { return Ok(None) };
        segments.push(DbSegment {
            raw_min: i64::from(raw_min),
            raw_max: i64::from(raw_max),
            curve,
        });
        data = &data[2 + used..];
    }
    if segments.is_empty() {
        return Err(Error::MalformedTlv("empty range"));
    }
    Ok(Some(DbCurve::Range(segments)))
}

impl DbCurve {
    /// The gain curve of a TLV block as `SNDRV_CTL_IOCTL_TLV_READ` returns it (after the 8-byte header); `None`
    /// when the block has a type that carries no gain curve.
    pub fn parse(bytes: &[u8]) -> Result<Option<DbCurve>> {
        let (words, rest) = bytes.as_chunks::<4>();
        if !rest.is_empty() {
            return Err(Error::MalformedTlv("size is not a whole number of words"));
        }
        let words: Vec<u32> = words.iter().map(|w| u32::from_ne_bytes(*w)).collect();
        Ok(parse_block(&words)?.0)
    }

    /// Gain of the raw value `raw` of a control spanning `raw_min..=raw_max`, in 1/100 dB; [`DB_GAIN_MUTE`] for a
    /// mute step.
    pub fn db(&self, raw_min: i64, raw_max: i64, raw: i64) -> i32 {
        match self {
            DbCurve::Scale { min, step, mute } => {
                if *mute && raw == raw_min {
                    DB_GAIN_MUTE
                } else {
                    (i64::from(*min) + i64::from(*step) * (raw - raw_min)) as i32
                }
            }
            DbCurve::MinMax { min, max, mute } => {
                if *mute && raw == raw_min {
                    DB_GAIN_MUTE
                } else {
                    let span = (raw_max - raw_min).max(1);
                    let gain = f64::from(*min)
                        + f64::from(max - min) * (raw - raw_min) as f64 / span as f64;
                    gain.round() as i32
                }
            }
            DbCurve::Range(segments) => {
                let segment = segments
                    .iter()
                    .find(|s| (s.raw_min..=s.raw_max).contains(&raw))
                    .unwrap_or_else(|| {
                        if raw < raw_min {
                            &segments[0]
                        } else {
                            &segments[segments.len() - 1]
                        }
                    });
                segment.curve.db(
                    segment.raw_min,
                    segment.raw_max,
                    raw.clamp(segment.raw_min, segment.raw_max),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_ne_bytes()).collect()
    }

    /// `DB_SCALE` of an HDA codec amplifier: -97.50 dB at raw 0, 0.75 dB per step.
    fn hda_scale() -> Vec<u8> {
        bytes(&[1, 8, (-9750i32) as u32, 75])
    }

    #[test]
    fn db_scale_gains_follow_the_step() {
        let curve = DbCurve::parse(&hda_scale()).unwrap().unwrap();
        assert_eq!(
            curve,
            DbCurve::Scale {
                min: -9750,
                step: 75,
                mute: false
            }
        );
        assert_eq!(curve.db(0, 87, 0), -9750);
        assert_eq!(curve.db(0, 87, 87), -3225);
    }

    #[test]
    fn db_scale_mute_flag_mutes_the_lowest_raw_value() {
        let curve = DbCurve::parse(&bytes(&[1, 8, (-5100i32) as u32, 0x10000 | 100]))
            .unwrap()
            .unwrap();
        assert_eq!(curve.db(0, 51, 0), DB_GAIN_MUTE);
        assert_eq!(curve.db(0, 51, 1), -5000);
        assert_eq!(curve.db(0, 51, 51), 0);
    }

    #[test]
    fn db_minmax_interpolates_between_the_ends() {
        let curve = DbCurve::parse(&bytes(&[4, 8, (-4000i32) as u32, 2000]))
            .unwrap()
            .unwrap();
        assert_eq!(curve.db(0, 100, 0), -4000);
        assert_eq!(curve.db(0, 100, 50), -1000);
        assert_eq!(curve.db(0, 100, 100), 2000);
        let muted = DbCurve::parse(&bytes(&[5, 8, (-4000i32) as u32, 2000]))
            .unwrap()
            .unwrap();
        assert_eq!(muted.db(0, 100, 0), DB_GAIN_MUTE);
        assert_eq!(muted.db(0, 100, 100), 2000);
    }

    #[test]
    fn db_range_picks_the_segment_of_the_raw_value() {
        // raw 0..=1: scale from -6000 with 0 step (flat); raw 2..=4: scale from -3000 with 500 per step
        let block = bytes(&[
            3,
            48,
            0,
            1,
            1,
            8,
            (-6000i32) as u32,
            0,
            2,
            4,
            1,
            8,
            (-3000i32) as u32,
            500,
        ]);
        let curve = DbCurve::parse(&block).unwrap().unwrap();
        assert_eq!(curve.db(0, 4, 1), -6000);
        assert_eq!(curve.db(0, 4, 2), -3000);
        assert_eq!(curve.db(0, 4, 4), -2000);
    }

    #[test]
    fn types_without_a_gain_curve_give_none() {
        // DB_LINEAR
        assert_eq!(DbCurve::parse(&bytes(&[2, 8, 0, 100])).unwrap(), None);
        // a container
        assert_eq!(DbCurve::parse(&bytes(&[0, 0])).unwrap(), None);
    }

    #[test]
    fn truncated_blocks_are_errors() {
        assert!(matches!(
            DbCurve::parse(&bytes(&[1, 8, 5])),
            Err(Error::MalformedTlv(_))
        ));
        assert!(matches!(
            DbCurve::parse(&[0, 0, 0]),
            Err(Error::MalformedTlv(_))
        ));
        assert!(matches!(
            DbCurve::parse(&bytes(&[1, 6, 5, 6])),
            Err(Error::MalformedTlv(_))
        ));
    }
}
