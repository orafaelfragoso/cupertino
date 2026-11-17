//! The embedded built-in font.

use cupertino_assets::{Assets, FontSource, BUILTIN_FONT, BUILTIN_UI_FAMILY};

#[test]
fn the_built_in_font_is_a_truetype_font_file() {
    assert_eq!(&BUILTIN_FONT[..4], &[0, 1, 0, 0]);
}

#[test]
fn the_built_in_font_carries_the_family_it_is_documented_under() {
    // the name table stores the family as ASCII (Macintosh) or UTF-16BE (Windows) strings
    let ascii = BUILTIN_UI_FAMILY.as_bytes();
    let utf16: Vec<u8> = BUILTIN_UI_FAMILY
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect();
    let has = |needle: &[u8]| BUILTIN_FONT.windows(needle.len()).any(|w| w == needle);
    assert!(has(ascii) || has(&utf16));
    assert_eq!(BUILTIN_UI_FAMILY, "Inter Variable");
}

#[test]
fn the_built_in_font_is_variable_with_weight_and_optical_size_axes() {
    let has = |tag: &[u8; 4]| BUILTIN_FONT.windows(4).any(|w| w == tag);
    assert!(has(b"fvar") && has(b"wght") && has(b"opsz"));
}

#[test]
fn the_built_in_font_is_the_last_font_of_every_pack_set() {
    assert_eq!(Assets::builtin().fonts().last(), Some(&FontSource::Builtin));
}
