//! XCursor file format: writer and parser.
//!
//! Layout (all fields little-endian `u32`): file header (`"Xcur"`, header size 16, version
//! `0x10000`, number of table-of-contents entries), one TOC entry per image (type, subtype =
//! nominal size, absolute position of the chunk) and one chunk per image (chunk header size 36,
//! type `0xfffd0002`, nominal size, version 1, width, height, hotspot x, hotspot y, delay in ms,
//! then `width × height` premultiplied ARGB pixels). Images of the same nominal size that follow
//! each other form an animation.

pub const MAGIC: &[u8; 4] = b"Xcur";
pub const FILE_HEADER_SIZE: u32 = 16;
pub const FILE_VERSION: u32 = 0x1_0000;
pub const IMAGE_TYPE: u32 = 0xfffd_0002;
pub const IMAGE_HEADER_SIZE: u32 = 36;
pub const IMAGE_VERSION: u32 = 1;
/// Largest width/height accepted (the X cursor library limit).
pub const MAX_DIMENSION: u32 = 0x7fff;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XCursorImage {
    /// Nominal size (the `subtype` of the chunk).
    pub nominal_size: u32,
    pub width: u32,
    pub height: u32,
    pub xhot: u32,
    pub yhot: u32,
    /// Frame delay in milliseconds (0 for static cursors).
    pub delay_ms: u32,
    /// Premultiplied `0xAARRGGBB` pixels, row-major.
    pub pixels: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    TooShort,
    BadMagic,
    BadHeader(&'static str),
    BadChunk(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::TooShort => write!(f, "file is truncated"),
            ParseError::BadMagic => write!(f, "not an XCursor file"),
            ParseError::BadHeader(m) => write!(f, "bad file header: {m}"),
            ParseError::BadChunk(m) => write!(f, "bad image chunk: {m}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Serialise `images` (in the order given) into an XCursor file.
pub fn write(images: &[XCursorImage]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let put = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    out.extend_from_slice(MAGIC);
    put(&mut out, FILE_HEADER_SIZE);
    put(&mut out, FILE_VERSION);
    put(&mut out, images.len() as u32);
    let mut pos = FILE_HEADER_SIZE + 12 * images.len() as u32;
    for img in images {
        if img.width == 0 || img.height == 0 || img.width > MAX_DIMENSION || img.height > MAX_DIMENSION {
            return Err(format!("image size {}x{} is out of range", img.width, img.height));
        }
        if img.pixels.len() != (img.width * img.height) as usize {
            return Err(format!("{} pixels for {}x{}", img.pixels.len(), img.width, img.height));
        }
        if img.xhot >= img.width || img.yhot >= img.height {
            return Err(format!("hotspot ({}, {}) outside {}x{}", img.xhot, img.yhot, img.width, img.height));
        }
        put(&mut out, IMAGE_TYPE);
        put(&mut out, img.nominal_size);
        put(&mut out, pos);
        pos += IMAGE_HEADER_SIZE + 4 * img.width * img.height;
    }
    for img in images {
        for v in [IMAGE_HEADER_SIZE, IMAGE_TYPE, img.nominal_size, IMAGE_VERSION, img.width, img.height, img.xhot, img.yhot, img.delay_ms] {
            put(&mut out, v);
        }
        for p in &img.pixels {
            put(&mut out, *p);
        }
    }
    Ok(out)
}

fn rd(b: &[u8], at: usize) -> Result<u32, ParseError> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or(ParseError::TooShort)
}

/// Parse an XCursor file into its images in table-of-contents order. Entries that are not
/// images (comments) are skipped.
pub fn parse(bytes: &[u8]) -> Result<Vec<XCursorImage>, ParseError> {
    if bytes.len() < 16 {
        return Err(ParseError::TooShort);
    }
    if &bytes[..4] != MAGIC {
        return Err(ParseError::BadMagic);
    }
    let header = rd(bytes, 4)?;
    if header < FILE_HEADER_SIZE {
        return Err(ParseError::BadHeader("header size"));
    }
    let ntoc = rd(bytes, 12)? as usize;
    if ntoc > 65_536 {
        return Err(ParseError::BadHeader("too many entries"));
    }
    let mut images = Vec::new();
    for i in 0..ntoc {
        let e = header as usize + 12 * i;
        let (ty, subtype, pos) = (rd(bytes, e)?, rd(bytes, e + 4)?, rd(bytes, e + 8)? as usize);
        if ty != IMAGE_TYPE {
            continue;
        }
        let (chunk_header, chunk_type, chunk_subtype) = (rd(bytes, pos)?, rd(bytes, pos + 4)?, rd(bytes, pos + 8)?);
        if chunk_type != ty || chunk_subtype != subtype {
            return Err(ParseError::BadChunk("type or nominal size differs from the table of contents"));
        }
        if chunk_header < IMAGE_HEADER_SIZE {
            return Err(ParseError::BadChunk("header size"));
        }
        let (width, height, xhot, yhot, delay) = (rd(bytes, pos + 16)?, rd(bytes, pos + 20)?, rd(bytes, pos + 24)?, rd(bytes, pos + 28)?, rd(bytes, pos + 32)?);
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(ParseError::BadChunk("dimensions"));
        }
        if xhot >= width || yhot >= height {
            return Err(ParseError::BadChunk("hotspot outside the image"));
        }
        let start = pos + chunk_header as usize;
        let count = (width * height) as usize;
        if bytes.len() < start + 4 * count {
            return Err(ParseError::TooShort);
        }
        let pixels = (0..count).map(|k| rd(bytes, start + 4 * k)).collect::<Result<Vec<_>, _>>()?;
        images.push(XCursorImage { nominal_size: subtype, width, height, xhot, yhot, delay_ms: delay, pixels });
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(size: u32, w: u32, h: u32, hot: (u32, u32), delay: u32, seed: u32) -> XCursorImage {
        XCursorImage { nominal_size: size, width: w, height: h, xhot: hot.0, yhot: hot.1, delay_ms: delay, pixels: (0..w * h).map(|i| 0xff00_0000 | (i.wrapping_mul(2654435761).wrapping_add(seed) & 0x00ff_ffff)).collect() }
    }

    #[test]
    fn round_trip_static_multi_size() {
        let images = vec![img(24, 24, 24, (3, 4), 0, 1), img(32, 32, 32, (4, 5), 0, 2), img(48, 40, 50, (6, 7), 0, 3)];
        let bytes = write(&images).unwrap();
        assert_eq!(parse(&bytes).unwrap(), images);
    }

    #[test]
    fn round_trip_animation_keeps_frame_order_and_delay() {
        let frames: Vec<XCursorImage> = (0..5).map(|i| img(32, 16, 16, (8, 8), 33 + i, i)).collect();
        let parsed = parse(&write(&frames).unwrap()).unwrap();
        assert_eq!(parsed.len(), 5);
        assert!(parsed.iter().enumerate().all(|(i, f)| f.delay_ms == 33 + i as u32 && f.pixels == frames[i].pixels));
    }

    #[test]
    fn header_and_toc_layout_follow_the_format() {
        let bytes = write(&[img(24, 2, 3, (1, 2), 0, 0), img(32, 4, 4, (0, 0), 0, 0)]).unwrap();
        assert_eq!(&bytes[..4], b"Xcur");
        assert_eq!(rd(&bytes, 4).unwrap(), 16);
        assert_eq!(rd(&bytes, 8).unwrap(), 0x10000);
        assert_eq!(rd(&bytes, 12).unwrap(), 2);
        assert_eq!(rd(&bytes, 16).unwrap(), 0xfffd0002);
        assert_eq!(rd(&bytes, 20).unwrap(), 24);
        let first = rd(&bytes, 24).unwrap();
        assert_eq!(first, 16 + 2 * 12);
        let second = rd(&bytes, 36).unwrap();
        assert_eq!(second, first + 36 + 2 * 3 * 4);
        assert_eq!(rd(&bytes, first as usize).unwrap(), 36);
        assert_eq!(rd(&bytes, first as usize + 16).unwrap(), 2);
        assert_eq!(rd(&bytes, first as usize + 20).unwrap(), 3);
        assert_eq!(bytes.len() as u32, second + 36 + 4 * 16);
    }

    #[test]
    fn pixel_bytes_are_little_endian_argb() {
        let mut i = img(24, 1, 1, (0, 0), 0, 0);
        i.pixels = vec![0x80_40_20_10];
        let bytes = write(&[i]).unwrap();
        let tail = &bytes[bytes.len() - 4..];
        assert_eq!(tail, &[0x10, 0x20, 0x40, 0x80], "B, G, R, A in memory");
    }

    #[test]
    fn writer_validates_input() {
        assert!(write(&[img(24, 0, 4, (0, 0), 0, 0)]).is_err());
        let mut bad = img(24, 4, 4, (0, 0), 0, 0);
        bad.pixels.pop();
        assert!(write(&[bad]).is_err());
        assert!(write(&[img(24, 4, 4, (4, 0), 0, 0)]).unwrap_err().contains("hotspot"));
        assert!(write(&[img(24, MAX_DIMENSION + 1, 1, (0, 0), 0, 0)]).is_err());
        assert!(write(&[]).is_ok());
    }

    #[test]
    fn parser_rejects_corrupt_files() {
        assert_eq!(parse(b"nope"), Err(ParseError::TooShort));
        assert_eq!(parse(&[0u8; 32]), Err(ParseError::BadMagic));
        let good = write(&[img(24, 4, 4, (1, 1), 0, 0)]).unwrap();
        assert_eq!(parse(&good[..good.len() - 1]), Err(ParseError::TooShort));
        assert_eq!(parse(&good[..20]), Err(ParseError::TooShort));
        let mut bad_sub = good.clone();
        bad_sub[20] = 99;
        assert!(matches!(parse(&bad_sub), Err(ParseError::BadChunk(_))));
        let mut bad_hot = good.clone();
        let pos = rd(&good, 24).unwrap() as usize;
        bad_hot[pos + 24] = 9;
        assert!(matches!(parse(&bad_hot), Err(ParseError::BadChunk(_))));
        let mut huge = good.clone();
        huge[12] = 0xff;
        huge[13] = 0xff;
        huge[14] = 0xff;
        assert!(matches!(parse(&huge), Err(ParseError::BadHeader(_))));
    }

    #[test]
    fn non_image_toc_entries_are_skipped() {
        let mut bytes = write(&[img(24, 2, 2, (0, 0), 0, 0)]).unwrap();
        bytes[16] = 0x01;
        bytes[17] = 0x00;
        bytes[18] = 0xfe;
        bytes[19] = 0xff;
        assert_eq!(parse(&bytes).unwrap(), vec![]);
    }
}
