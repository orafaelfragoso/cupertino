//! Gain curves from TLV blocks and their gains.

use cupertino_alsa::{DbCurve, DbSegment, Error, DB_GAIN_MUTE};

fn block(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_ne_bytes()).collect()
}

#[test]
fn a_scale_block_gives_minimum_and_step() {
    let bytes = block(&[1, 8, (-5100i32) as u32, 50]);
    assert_eq!(
        DbCurve::parse(&bytes).unwrap(),
        Some(DbCurve::Scale {
            min: -5100,
            step: 50,
            mute: false
        })
    );
}

#[test]
fn the_scale_mute_flag_is_separate_from_the_step() {
    let bytes = block(&[1, 8, (-5100i32) as u32, 0x10000 | 50]);
    assert_eq!(
        DbCurve::parse(&bytes).unwrap(),
        Some(DbCurve::Scale {
            min: -5100,
            step: 50,
            mute: true
        })
    );
}

#[test]
fn minmax_blocks_come_with_and_without_a_mute_step() {
    let plain = block(&[4, 8, (-3000i32) as u32, 600]);
    let muting = block(&[5, 8, (-3000i32) as u32, 600]);
    assert_eq!(
        DbCurve::parse(&plain).unwrap(),
        Some(DbCurve::MinMax {
            min: -3000,
            max: 600,
            mute: false
        })
    );
    assert_eq!(
        DbCurve::parse(&muting).unwrap(),
        Some(DbCurve::MinMax {
            min: -3000,
            max: 600,
            mute: true
        })
    );
}

#[test]
fn a_range_block_holds_a_curve_for_each_part() {
    let bytes = block(&[
        3,
        48,
        // raw 0..=1, minmax
        0,
        1,
        4,
        8,
        (-4000i32) as u32,
        (-1000i32) as u32,
        // raw 2..=5, scale
        2,
        5,
        1,
        8,
        (-1000i32) as u32,
        100,
    ]);
    let Some(DbCurve::Range(segments)) = DbCurve::parse(&bytes).unwrap() else {
        panic!("a range of two parts");
    };
    assert_eq!(segments.len(), 2);
    assert_eq!((segments[0].raw_min, segments[0].raw_max), (0, 1));
    assert_eq!(
        segments[1].curve,
        DbCurve::Scale {
            min: -1000,
            step: 100,
            mute: false
        }
    );
    assert_eq!((segments[1].raw_min, segments[1].raw_max), (2, 5));
}

#[test]
fn a_block_type_without_a_gain_curve_is_no_information() {
    // DB_LINEAR
    assert_eq!(DbCurve::parse(&block(&[2, 8, 0, 100])).unwrap(), None);
    // a container
    assert_eq!(DbCurve::parse(&block(&[0, 0])).unwrap(), None);
}

#[test]
fn a_range_with_a_part_without_a_curve_is_no_information() {
    let bytes = block(&[3, 24, 0, 3, 2, 8, 0, 100]);
    assert_eq!(DbCurve::parse(&bytes).unwrap(), None);
}

#[test]
fn an_empty_range_is_malformed() {
    assert!(matches!(
        DbCurve::parse(&block(&[3, 0])),
        Err(Error::MalformedTlv("empty range"))
    ));
}

#[test]
fn damaged_blocks_are_malformed() {
    // not a whole number of words
    assert!(matches!(
        DbCurve::parse(&[1, 2, 3]),
        Err(Error::MalformedTlv(_))
    ));
    // no header
    assert!(matches!(
        DbCurve::parse(&block(&[1])),
        Err(Error::MalformedTlv(_))
    ));
    // a length that is not a whole number of words
    assert!(matches!(
        DbCurve::parse(&block(&[1, 6, 0, 0])),
        Err(Error::MalformedTlv(_))
    ));
    // a length that runs past the data
    assert!(matches!(
        DbCurve::parse(&block(&[1, 16, 0, 0])),
        Err(Error::MalformedTlv(_))
    ));
    // a scale without its second word
    assert!(matches!(
        DbCurve::parse(&block(&[1, 4, 0])),
        Err(Error::MalformedTlv(_))
    ));
}

#[test]
fn a_scale_rises_by_its_step_from_the_lowest_raw_value() {
    let scale = DbCurve::Scale {
        min: -5100,
        step: 50,
        mute: false,
    };
    assert_eq!(scale.db(0, 100, 0), -5100);
    assert_eq!(scale.db(0, 100, 10), -4600);
    assert_eq!(scale.db(5, 100, 5), -5100);
}

#[test]
fn a_muting_scale_is_mute_at_its_lowest_raw_value_only() {
    let scale = DbCurve::Scale {
        min: -5100,
        step: 50,
        mute: true,
    };
    assert_eq!(scale.db(0, 100, 0), DB_GAIN_MUTE);
    assert_eq!(scale.db(0, 100, 1), -5050);
}

#[test]
fn minmax_is_linear_between_its_ends() {
    let curve = DbCurve::MinMax {
        min: -3000,
        max: 600,
        mute: false,
    };
    assert_eq!(curve.db(0, 36, 0), -3000);
    assert_eq!(curve.db(0, 36, 36), 600);
    assert_eq!(curve.db(0, 36, 18), -1200);
}

#[test]
fn minmax_of_a_single_value_range_does_not_divide_by_zero() {
    let curve = DbCurve::MinMax {
        min: -3000,
        max: 600,
        mute: false,
    };
    assert_eq!(curve.db(4, 4, 4), -3000);
}

fn two_part_range() -> DbCurve {
    DbCurve::Range(vec![
        DbSegment {
            raw_min: 0,
            raw_max: 1,
            curve: DbCurve::MinMax {
                min: -4000,
                max: -1000,
                mute: false,
            },
        },
        DbSegment {
            raw_min: 2,
            raw_max: 5,
            curve: DbCurve::Scale {
                min: -1000,
                step: 100,
                mute: false,
            },
        },
    ])
}

#[test]
fn a_range_looks_up_the_part_that_holds_the_raw_value() {
    let curve = two_part_range();
    assert_eq!(curve.db(0, 5, 0), -4000);
    assert_eq!(curve.db(0, 5, 1), -1000);
    assert_eq!(curve.db(0, 5, 2), -1000);
    assert_eq!(curve.db(0, 5, 5), -700);
}

#[test]
fn a_range_clamps_a_raw_value_outside_it_to_its_ends() {
    let curve = two_part_range();
    assert_eq!(curve.db(0, 5, -3), -4000);
    assert_eq!(curve.db(0, 5, 9), -700);
}
