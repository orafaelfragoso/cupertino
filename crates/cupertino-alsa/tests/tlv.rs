//! Gain curves from TLV blocks and their gains.

use cupertino_alsa::{DbCurve, DbSegment, Error, DB_GAIN_MUTE};

fn block(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_ne_bytes()).collect()
}

fn scale(min: i32, step: u32, mute: bool) -> DbCurve {
    DbCurve::Scale { min, step, mute }
}

fn minmax(min: i32, max: i32, mute: bool) -> DbCurve {
    DbCurve::MinMax { min, max, mute }
}

fn two_part_range() -> DbCurve {
    DbCurve::Range(vec![
        DbSegment {
            raw_min: 0,
            raw_max: 1,
            curve: minmax(-4000, -1000, false),
        },
        DbSegment {
            raw_min: 2,
            raw_max: 5,
            curve: scale(-1000, 100, false),
        },
    ])
}

#[test]
fn a_block_gives_its_gain_curve_or_none_when_its_type_has_none() {
    let neg = |db: i32| db as u32;
    for (name, words, expected) in [
        (
            "scale",
            vec![1, 8, neg(-5100), 50],
            Some(scale(-5100, 50, false)),
        ),
        (
            "scale with the mute flag apart from the step",
            vec![1, 8, neg(-5100), 0x10000 | 50],
            Some(scale(-5100, 50, true)),
        ),
        (
            "minmax",
            vec![4, 8, neg(-3000), 600],
            Some(minmax(-3000, 600, false)),
        ),
        (
            "minmax with a mute step",
            vec![5, 8, neg(-3000), 600],
            Some(minmax(-3000, 600, true)),
        ),
        (
            "a range of a minmax and a scale part",
            vec![
                3,
                48,
                0,
                1,
                4,
                8,
                neg(-4000),
                neg(-1000),
                2,
                5,
                1,
                8,
                neg(-1000),
                100,
            ],
            Some(two_part_range()),
        ),
        ("linear", vec![2, 8, 0, 100], None),
        ("a container", vec![0, 0], None),
        (
            "a range with a part without a curve",
            vec![3, 24, 0, 3, 2, 8, 0, 100],
            None,
        ),
    ] {
        assert_eq!(DbCurve::parse(&block(&words)).unwrap(), expected, "{name}");
    }
}

#[test]
fn a_damaged_block_is_malformed() {
    for (name, bytes) in [
        ("not whole words", vec![1, 2, 3]),
        ("no header", block(&[1])),
        ("a length not in words", block(&[1, 6, 0, 0])),
        ("a length past the data", block(&[1, 16, 0, 0])),
        ("a scale without its second word", block(&[1, 4, 0])),
        ("an empty range", block(&[3, 0])),
    ] {
        let result = DbCurve::parse(&bytes);
        assert!(
            matches!(result, Err(Error::MalformedTlv(_))),
            "{name}: {result:?}"
        );
    }
}

#[test]
fn a_curve_gives_the_gain_of_a_raw_value_within_its_range() {
    for (name, curve, (min, max, raw), expected) in [
        (
            "scale at the lowest",
            scale(-5100, 50, false),
            (0, 100, 0),
            -5100,
        ),
        (
            "scale by its step",
            scale(-5100, 50, false),
            (0, 100, 10),
            -4600,
        ),
        (
            "scale from a raw minimum",
            scale(-5100, 50, false),
            (5, 100, 5),
            -5100,
        ),
        (
            "muting scale at the lowest",
            scale(-5100, 50, true),
            (0, 100, 0),
            DB_GAIN_MUTE,
        ),
        (
            "muting scale above it",
            scale(-5100, 50, true),
            (0, 100, 1),
            -5050,
        ),
        (
            "minmax low end",
            minmax(-3000, 600, false),
            (0, 36, 0),
            -3000,
        ),
        (
            "minmax high end",
            minmax(-3000, 600, false),
            (0, 36, 36),
            600,
        ),
        (
            "minmax between",
            minmax(-3000, 600, false),
            (0, 36, 18),
            -1200,
        ),
        (
            "minmax of one value",
            minmax(-3000, 600, false),
            (4, 4, 4),
            -3000,
        ),
        (
            "muting minmax at the lowest",
            minmax(-6000, 0, true),
            (0, 100, 0),
            DB_GAIN_MUTE,
        ),
        ("range, first part low", two_part_range(), (0, 5, 0), -4000),
        ("range, first part high", two_part_range(), (0, 5, 1), -1000),
        ("range, second part low", two_part_range(), (0, 5, 2), -1000),
        ("range, second part high", two_part_range(), (0, 5, 5), -700),
        ("range, below", two_part_range(), (0, 5, -3), -4000),
        ("range, above", two_part_range(), (0, 5, 9), -700),
    ] {
        assert_eq!(curve.db(min, max, raw), expected, "{name}");
    }
}
