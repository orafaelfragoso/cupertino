//! Colour bitmap strikes (the CBLC index and CBDT data tables): the size of a glyph's image, read before anything
//! allocates for or decodes it.

use cupertino_raster_math::glyph::MAX_MASK_SIDE;

/// The largest side of an uncompressed strike image: its metrics store width and height as bytes.
const METRICS_SIDE: u32 = 255;

fn u8_at(data: &[u8], at: usize) -> Option<u8> {
    data.get(at).copied()
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<usize> {
    let value = u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?);
    usize::try_from(value).ok()
}

/// Whether every strike in `cblc` / `cbdt` that holds `glyph` gives it an image no wider or taller than
/// [`MAX_MASK_SIDE`] once scaled from the strike's size to `size` pixels per em. False when no strike holds the glyph,
/// or a strike's data cannot be read.
pub(crate) fn strikes_fit(cblc: &[u8], cbdt: &[u8], glyph: u16, size: f32) -> bool {
    let fit = || -> Option<bool> {
        let mut held = false;
        for i in 0..u32_at(cblc, 4)? {
            let record = 8 + i.checked_mul(48)?;
            if glyph < u16_at(cblc, record + 40)? || glyph > u16_at(cblc, record + 42)? {
                continue;
            }
            let Some((width, height)) = image_size(cblc, cbdt, record, glyph)? else {
                continue;
            };
            held = true;
            let ppem = u8_at(cblc, record + 44)?.min(u8_at(cblc, record + 45)?);
            let scale = size / f32::from(ppem);
            let max = MAX_MASK_SIDE as f32;
            if ppem == 0 || width as f32 * scale > max || height as f32 * scale > max {
                return Some(false);
            }
        }
        Some(held)
    };
    fit().unwrap_or(false)
}

/// The largest image size of `glyph` in the strike whose size record starts at `record`: `Some(None)` when none of its
/// index subtables holds the glyph, `None` when the data cannot be read.
fn image_size(cblc: &[u8], cbdt: &[u8], record: usize, glyph: u16) -> Option<Option<(u32, u32)>> {
    let array = u32_at(cblc, record)?;
    for j in 0..u32_at(cblc, record + 8)? {
        let entry = array + j.checked_mul(8)?;
        let first = u16_at(cblc, entry)?;
        if glyph < first || glyph > u16_at(cblc, entry + 2)? {
            continue;
        }
        let sub = array + u32_at(cblc, entry + 4)?;
        let index = usize::from(glyph - first);
        // a lookup may land on any pair of a format 4 subtable that names the glyph, so every one counts
        let offsets = match u16_at(cblc, sub)? {
            1 => vec![u32_at(cblc, sub + 8 + 4 * index)?],
            2 => vec![u32_at(cblc, sub + 8)?.checked_mul(index)?],
            3 => vec![usize::from(u16_at(cblc, sub + 8 + 2 * index)?)],
            4 => {
                let mut offsets = Vec::new();
                for k in 0..=u32_at(cblc, sub + 8)? {
                    let pair = sub + 12 + 4 * k;
                    if u16_at(cblc, pair)? == glyph {
                        offsets.push(usize::from(u16_at(cblc, pair + 2)?));
                    }
                }
                offsets
            }
            _ => return None,
        };
        let image = u32_at(cblc, sub + 4)?;
        let format = u16_at(cblc, sub + 2)?;
        let mut largest: Option<(u32, u32)> = None;
        for offset in offsets {
            let data = image.checked_add(offset)?;
            let png = match format {
                1 | 2 | 5 | 6 | 7 | 8 | 9 => return Some(Some((METRICS_SIDE, METRICS_SIDE))),
                17 => data + 5 + 4,
                18 => data + 8 + 4,
                19 => data + 4,
                _ => return None,
            };
            let side = |at| u32::try_from(u32_at(cbdt, png + at)?).ok();
            let (width, height) = (side(16)?, side(20)?);
            largest = Some(largest.map_or((width, height), |(w, h)| (w.max(width), h.max(height))));
        }
        return Some(largest);
    }
    Some(None)
}
