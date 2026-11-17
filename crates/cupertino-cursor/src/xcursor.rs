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
        if img.width == 0
            || img.height == 0
            || img.width > MAX_DIMENSION
            || img.height > MAX_DIMENSION
        {
            return Err(format!(
                "image size {}x{} is out of range",
                img.width, img.height
            ));
        }
        if img.pixels.len() != (img.width * img.height) as usize {
            return Err(format!(
                "{} pixels for {}x{}",
                img.pixels.len(),
                img.width,
                img.height
            ));
        }
        if img.xhot >= img.width || img.yhot >= img.height {
            return Err(format!(
                "hotspot ({}, {}) outside {}x{}",
                img.xhot, img.yhot, img.width, img.height
            ));
        }
        put(&mut out, IMAGE_TYPE);
        put(&mut out, img.nominal_size);
        put(&mut out, pos);
        pos += IMAGE_HEADER_SIZE + 4 * img.width * img.height;
    }
    for img in images {
        for v in [
            IMAGE_HEADER_SIZE,
            IMAGE_TYPE,
            img.nominal_size,
            IMAGE_VERSION,
            img.width,
            img.height,
            img.xhot,
            img.yhot,
            img.delay_ms,
        ] {
            put(&mut out, v);
        }
        for p in &img.pixels {
            put(&mut out, *p);
        }
    }
    Ok(out)
}

fn rd(b: &[u8], at: usize) -> Result<u32, ParseError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(ParseError::TooShort)
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
        let (chunk_header, chunk_type, chunk_subtype) =
            (rd(bytes, pos)?, rd(bytes, pos + 4)?, rd(bytes, pos + 8)?);
        if chunk_type != ty || chunk_subtype != subtype {
            return Err(ParseError::BadChunk(
                "type or nominal size differs from the table of contents",
            ));
        }
        if chunk_header < IMAGE_HEADER_SIZE {
            return Err(ParseError::BadChunk("header size"));
        }
        let (width, height, xhot, yhot, delay) = (
            rd(bytes, pos + 16)?,
            rd(bytes, pos + 20)?,
            rd(bytes, pos + 24)?,
            rd(bytes, pos + 28)?,
            rd(bytes, pos + 32)?,
        );
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
        let pixels = (0..count)
            .map(|k| rd(bytes, start + 4 * k))
            .collect::<Result<Vec<_>, _>>()?;
        images.push(XCursorImage {
            nominal_size: subtype,
            width,
            height,
            xhot,
            yhot,
            delay_ms: delay,
            pixels,
        });
    }
    Ok(images)
}
