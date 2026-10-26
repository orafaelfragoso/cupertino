//! Name collation of the design file manager.
//!
//! The design compares names with the flag set {case-insensitive, non-literal, localized,
//! numeric, width-insensitive, forced-ordering}.
//! Diacritics are **not** ignored (the diacritic-insensitive flag is absent). This module builds a
//! byte-comparable [`NameKey`] with the same behaviour:
//!
//! * primary level: punctuation and symbols (by code point) < digit runs (by numeric value) <
//!   letters (accent- and case-folded, fullwidth forms mapped to ASCII);
//! * secondary level: accents; tertiary level: case (lower before upper);
//! * last level: the raw bytes (forced ordering, so that two different names never compare equal).
//!
//! The Latin fold table is approximate (a locale collator is not available without a vendor table).

use std::cmp::Ordering;

/// Fold a Latin letter with diacritics to `(base, accent rank)`. Rank 0 means no accent.
fn fold_latin(c: char) -> (char, u8) {
    const GROUPS: &[(&str, char)] = &[
        ("àáâãäåāăą", 'a'),
        ("çćĉċč", 'c'),
        ("ďđ", 'd'),
        ("èéêëēĕėęě", 'e'),
        ("ĝğġģ", 'g'),
        ("ĥħ", 'h'),
        ("ìíîïĩīĭįı", 'i'),
        ("ĵ", 'j'),
        ("ķ", 'k'),
        ("ĺļľŀł", 'l'),
        ("ñńņň", 'n'),
        ("òóôõöøōŏő", 'o'),
        ("ŕŗř", 'r'),
        ("śŝşš", 's'),
        ("ţťŧ", 't'),
        ("ùúûüũūŭůűų", 'u'),
        ("ŵ", 'w'),
        ("ýÿŷ", 'y'),
        ("źżž", 'z'),
    ];
    for (set, base) in GROUPS {
        if let Some(pos) = set.chars().position(|x| x == c) {
            return (*base, pos as u8 + 1);
        }
    }
    (c, 0)
}

fn fold_width(c: char) -> char {
    match c {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c,
    }
}

/// Byte string whose lexicographic order is the display order of names.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NameKey(Vec<u8>);

impl NameKey {
    pub fn new(name: &str) -> Self {
        let mut primary: Vec<u8> = Vec::with_capacity(name.len() * 2 + 4);
        let mut secondary: Vec<u8> = Vec::new();
        let mut tertiary: Vec<u8> = Vec::new();
        let chars: Vec<char> = name.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = fold_width(chars[i]);
            if c.is_ascii_digit() {
                let start = i;
                while i < chars.len() && fold_width(chars[i]).is_ascii_digit() {
                    i += 1;
                }
                let digits: Vec<u8> = chars[start..i]
                    .iter()
                    .map(|d| fold_width(*d) as u8)
                    .collect();
                let trimmed: &[u8] = {
                    let nz = digits
                        .iter()
                        .position(|d| *d != b'0')
                        .unwrap_or(digits.len().saturating_sub(1));
                    &digits[nz..]
                };
                primary.push(0x20);
                primary.extend_from_slice(&(trimmed.len() as u16).to_be_bytes());
                primary.extend_from_slice(trimmed);
                continue;
            }
            i += 1;
            if c.is_alphabetic() {
                let lower: Vec<char> = c.to_lowercase().collect();
                let is_upper = lower.len() != 1 || lower[0] != c;
                for l in lower {
                    let (base, accent) = fold_latin(l);
                    let reps = if l == 'ß' { 2 } else { 1 };
                    for _ in 0..reps {
                        primary.push(0x30);
                        primary.extend_from_slice(
                            &((if l == 'ß' { 's' } else { base }) as u32).to_be_bytes(),
                        );
                    }
                    secondary.push(accent);
                }
                tertiary.push(is_upper as u8);
            } else {
                primary.push(0x10);
                primary.extend_from_slice(&(c as u32).to_be_bytes());
            }
        }
        let mut key = primary;
        key.push(0);
        key.extend(secondary);
        key.push(0);
        key.extend(tertiary);
        key.push(0);
        key.extend_from_slice(name.as_bytes());
        NameKey(key)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Compare two names with the design collation.
pub fn compare_names(a: &str, b: &str) -> Ordering {
    NameKey::new(a).cmp(&NameKey::new(b))
}

/// Collation used while typing to select: case-insensitive, accents and punctuation
/// significant, digits compared as text (the digit-as-number option is off by default).
pub fn type_select_key(name: &str) -> Vec<char> {
    name.chars()
        .flat_map(|c| fold_width(c).to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<&str>) -> Vec<&str> {
        v.sort_by(|a, b| compare_names(a, b));
        v
    }

    #[test]
    fn numeric_runs_compare_by_value() {
        assert_eq!(
            sorted(vec!["file10", "file2", "file1", "file20"]),
            vec!["file1", "file2", "file10", "file20"]
        );
        assert_eq!(
            sorted(vec!["a100b", "a99b", "a100a"]),
            vec!["a99b", "a100a", "a100b"]
        );
    }

    #[test]
    fn case_insensitive_with_forced_order() {
        assert_eq!(
            sorted(vec!["banana", "Avocado", "avocado", "Cherry"]),
            vec!["avocado", "Avocado", "banana", "Cherry"]
        );
        assert_ne!(compare_names("a", "A"), Ordering::Equal);
        assert_eq!(compare_names("same", "same"), Ordering::Equal);
    }

    #[test]
    fn punctuation_sorts_before_digits_before_letters() {
        assert_eq!(
            sorted(vec!["b", "1", "_x", "-y"]),
            vec!["-y", "_x", "1", "b"]
        );
    }

    #[test]
    fn accents_are_secondary_not_ignored() {
        assert_eq!(
            sorted(vec!["eclair", "écran", "ecran", "eclat"]),
            vec!["eclair", "eclat", "ecran", "écran"]
        );
        assert_ne!(compare_names("e", "é"), Ordering::Equal);
    }

    #[test]
    fn width_insensitive() {
        assert_eq!(
            NameKey::new("ＡＢＣ１").as_bytes()[..13],
            NameKey::new("ABC1").as_bytes()[..13]
        );
    }

    #[test]
    fn leading_zeros_break_ties_only() {
        assert_eq!(compare_names("a007", "a7"), Ordering::Less);
        assert_eq!(sorted(vec!["a8", "a007", "a7"]), vec!["a007", "a7", "a8"]);
    }

    #[test]
    fn key_is_total_order_on_equal_folds() {
        let mut v = vec!["Äpfel", "apfel", "Apfel", "äpfel"];
        v.sort_by(|a, b| compare_names(a, b));
        assert_eq!(v, vec!["apfel", "Apfel", "äpfel", "Äpfel"]);
    }
}
