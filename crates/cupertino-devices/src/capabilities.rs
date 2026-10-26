use std::ops::Range;

use crate::error::ParseError;

/// A kernel capability bitmask as printed in sysfs (`capabilities/*`, `properties`).
///
/// The text is hexadecimal words separated by spaces, highest word first. Each word is an
/// `unsigned long` of the kernel, assumed to be 64 bits (every 64-bit target this crate supports).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityMask {
    /// Lowest word first, so bit `n` lives in `words[n / 64]`.
    words: Vec<u64>,
}

impl CapabilityMask {
    /// Parses the sysfs text of a mask. A word that is not hexadecimal is an error.
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let words = text
            .split_whitespace()
            .rev()
            .map(|word| {
                u64::from_str_radix(word, 16).map_err(|_| ParseError::InvalidNumber {
                    key: "capability word",
                    value: word.to_owned(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if words.is_empty() {
            return Err(ParseError::EmptyMask);
        }
        Ok(Self { words })
    }

    /// Builds a mask with exactly `bits` set.
    #[cfg(test)]
    pub(crate) fn from_bits(bits: &[u32]) -> Self {
        let mut words = vec![0u64; 12];
        for &bit in bits {
            words[bit as usize / 64] |= 1 << (bit % 64);
        }
        Self { words }
    }

    /// Whether `bit` is set. Bits beyond the printed words are clear.
    pub fn test(&self, bit: u32) -> bool {
        self.words
            .get(bit as usize / 64)
            .is_some_and(|word| (word >> (bit % 64)) & 1 == 1)
    }

    /// Whether any bit in `range` is set.
    pub fn any_in(&self, range: Range<u32>) -> bool {
        range.into_iter().any(|bit| self.test(bit))
    }

    /// Number of set bits in `range`.
    pub fn count_in(&self, range: Range<u32>) -> usize {
        range.into_iter().filter(|&bit| self.test(bit)).count()
    }

    /// The lowest 32 bits (the first block of keys, `ESC` to `D`).
    pub fn low_word32(&self) -> u32 {
        // Truncation intended: only the low half of word 0.
        self.words[0] as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highest_word_is_printed_first() {
        let mask = CapabilityMask::parse("1 8000000000000000").unwrap();
        assert!(mask.test(63));
        assert!(mask.test(64));
        assert!(!mask.test(0));
        assert!(!mask.test(65));
    }

    #[test]
    fn bits_past_the_printed_words_are_clear() {
        let mask = CapabilityMask::parse("120013").unwrap();
        assert!(mask.test(0) && mask.test(1) && mask.test(4) && mask.test(17) && mask.test(20));
        assert!(!mask.test(2));
        assert!(!mask.test(700));
    }

    #[test]
    fn range_queries() {
        let mask = CapabilityMask::parse("1f0000 0 0 0 0").unwrap();
        assert_eq!(mask.count_in(0x110..0x120), 5);
        assert!(mask.any_in(0x100..0x120));
        assert!(!mask.any_in(0..0x100));
    }

    #[test]
    fn bad_hex_word_is_an_error() {
        assert_eq!(
            CapabilityMask::parse("12 zz"),
            Err(ParseError::InvalidNumber { key: "capability word", value: "zz".to_owned() })
        );
    }

    #[test]
    fn empty_text_is_an_error() {
        assert_eq!(CapabilityMask::parse("\n"), Err(ParseError::EmptyMask));
    }
}
