//! The embedded built-in font.

use cupertino_assets::{BUILTIN_FONT, BUILTIN_UI_FAMILY};

#[test]
fn the_built_in_font_is_a_variable_truetype_font_of_the_documented_family() {
    let has = |needle: &[u8]| BUILTIN_FONT.windows(needle.len()).any(|w| w == needle);
    assert_eq!(&BUILTIN_FONT[..4], &[0, 1, 0, 0], "TrueType signature");
    // the name table stores the family as ASCII (Macintosh) or UTF-16BE (Windows) strings
    let utf16: Vec<u8> = BUILTIN_UI_FAMILY
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    assert!(
        has(BUILTIN_UI_FAMILY.as_bytes()) || has(&utf16),
        "family name"
    );
    for tag in [b"fvar", b"wght", b"opsz"] {
        assert!(has(tag), "{}", String::from_utf8_lossy(tag));
    }
}
