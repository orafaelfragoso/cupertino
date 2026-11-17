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
