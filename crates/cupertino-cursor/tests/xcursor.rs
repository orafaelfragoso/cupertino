//! The XCursor file format: what the writer produces the parser reads back, and the parser refuses damaged files.

use cupertino_cursor::xcursor::{self, ParseError, XCursorImage};

fn image(nominal_size: u32, side: u32, delay_ms: u32) -> XCursorImage {
    XCursorImage {
        nominal_size,
        width: side,
        height: side,
        xhot: side / 2,
        yhot: 1,
        delay_ms,
        pixels: (0..side * side).map(|i| 0xff00_0000 | i).collect(),
    }
}

#[test]
fn written_images_parse_back_in_order() {
    let images = vec![image(24, 4, 0), image(48, 8, 0), image(48, 8, 80)];
    let bytes = xcursor::write(&images).unwrap();
    assert_eq!(xcursor::parse(&bytes).unwrap(), images);
}

#[test]
fn damaged_files_are_refused() {
    let good = xcursor::write(&[image(24, 4, 0)]).unwrap();
    let mut wrong_magic = good.clone();
    wrong_magic[0] = b'Y';
    let mut hotspot_outside = good.clone();
    // The chunk starts after the 16-byte header and one 12-byte table entry; xhot is 24 bytes into it.
    hotspot_outside[28 + 24..28 + 28].copy_from_slice(&4u32.to_le_bytes());
    let cases: [(&str, Vec<u8>, ParseError); 4] = [
        ("empty", Vec::new(), ParseError::TooShort),
        ("not a cursor file", wrong_magic, ParseError::BadMagic),
        (
            "truncated pixels",
            good[..good.len() - 4].to_vec(),
            ParseError::TooShort,
        ),
        (
            "hotspot outside the image",
            hotspot_outside,
            ParseError::BadChunk("hotspot outside the image"),
        ),
    ];
    for (what, bytes, expected) in cases {
        assert_eq!(xcursor::parse(&bytes), Err(expected), "{what}");
    }
}
