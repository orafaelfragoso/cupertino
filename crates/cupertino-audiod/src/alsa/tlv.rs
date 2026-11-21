//! dB information of a control, from its TLV block (`include/uapi/sound/tlv.h`).
//!
//! A TLV block is a sequence of 32-bit words: `type`, `length` in bytes, then `length` bytes of data. Gains are
//! in 1/100 dB. Only the types that describe a gain curve are modelled: `DB_SCALE`, `DB_MINMAX`,
//! `DB_MINMAX_MUTE` and `DB_RANGE` (a list of ranges, each with one of the others inside). Every other type
//! (containers, `DB_LINEAR`, channel maps) is "no dB information", and the control then maps on its raw range.

use super::error::{Error, Result};

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
