//! Lower-case hexadecimal for binary fields (secrets and pictures). A hex string never holds a character JSON escapes, so
//! decoding it never copies the text into the JSON parser's scratch buffer, which is not zeroed.

use zeroize::Zeroizing;

const DIGITS: &[u8; 16] = b"0123456789abcdef";

/// The hex text of `bytes`, in a buffer that is zeroed when dropped.
pub fn encode(bytes: &[u8]) -> Zeroizing<String> {
    let mut text = Zeroizing::new(String::with_capacity(bytes.len() * 2));
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// The bytes of `text`; `None` when it has an odd length or a character that is not a lower-case hex digit.
pub fn decode(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut bytes = Zeroizing::new(Vec::with_capacity(text.len() / 2));
    for pair in text.as_bytes().as_chunks::<2>().0 {
        bytes.push(digit(pair[0])? << 4 | digit(pair[1])?);
    }
    Some(bytes)
}
