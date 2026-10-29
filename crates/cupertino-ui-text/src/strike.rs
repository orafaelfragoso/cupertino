//! Colour bitmap strikes (the CBLC index and CBDT data tables): the size of a glyph's image, read before anything
//! allocates for or decodes it.

use cupertino_glyph::MAX_MASK_SIDE;

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

/// The image size of `glyph` in the strike whose size record starts at `record`: `Some(None)` when none of its index
/// subtables holds the glyph, `None` when the data cannot be read.
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
        let offset = match u16_at(cblc, sub)? {
            1 => u32_at(cblc, sub + 8 + 4 * index)?,
            2 => u32_at(cblc, sub + 8)?.checked_mul(index)?,
            3 => usize::from(u16_at(cblc, sub + 8 + 2 * index)?),
            4 => (0..u32_at(cblc, sub + 8)?)
                .map(|k| sub + 12 + 4 * k)
                .find(|&pair| u16_at(cblc, pair) == Some(glyph))
                .and_then(|pair| u16_at(cblc, pair + 2))
                .map(usize::from)?,
            5 => {
                let ids = sub + 24;
                let position = (0..u32_at(cblc, sub + 20)?)
                    .position(|k| u16_at(cblc, ids + 2 * k) == Some(glyph))?;
                u32_at(cblc, sub + 8)?.checked_mul(position)?
            }
            _ => return None,
        };
        let data = u32_at(cblc, sub + 4)?.checked_add(offset)?;
        let png = match u16_at(cblc, sub + 2)? {
            1 | 2 | 5 | 6 | 7 | 8 | 9 => return Some(Some((METRICS_SIDE, METRICS_SIDE))),
            17 => data + 5 + 4,
            18 => data + 8 + 4,
            19 => data + 4,
            _ => return None,
        };
        let side = |at| u32::try_from(u32_at(cbdt, png + at)?).ok();
        return Some(Some((side(16)?, side(20)?)));
    }
    Some(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One strike of `ppem` holding glyph 7 as a PNG (image format 17) whose header claims `width` x `height`.
    fn font(ppem: u8, width: u32, height: u32) -> (Vec<u8>, Vec<u8>) {
        let mut cblc = vec![0, 3, 0, 0, 0, 0, 0, 1];
        let mut size = vec![0u8; 48];
        size[0..4].copy_from_slice(&56u32.to_be_bytes());
        size[8..12].copy_from_slice(&1u32.to_be_bytes());
        size[40..42].copy_from_slice(&7u16.to_be_bytes());
        size[42..44].copy_from_slice(&7u16.to_be_bytes());
        (size[44], size[45]) = (ppem, ppem);
        cblc.extend(size);
        cblc.extend([0, 7, 0, 7, 0, 0, 0, 8]);
        cblc.extend([0, 1, 0, 17, 0, 0, 0, 4]);
        cblc.extend([0, 0, 0, 0, 0, 0, 0, 40]);
        let mut cbdt = vec![0, 3, 0, 0];
        cbdt.extend([0; 5]);
        cbdt.extend(31u32.to_be_bytes());
        cbdt.extend(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR");
        cbdt.extend(width.to_be_bytes());
        cbdt.extend(height.to_be_bytes());
        (cblc, cbdt)
    }

    #[test]
    fn a_strike_image_is_drawn_only_while_its_header_size_fits_the_mask_cap() {
        let (cblc, cbdt) = font(109, 136, 128);
        assert!(strikes_fit(&cblc, &cbdt, 7, 24.0));
        assert!(strikes_fit(&cblc, &cbdt, 7, 2048.0));
        assert!(!strikes_fit(&cblc, &cbdt, 8, 24.0), "no strike holds it");
        let (cblc, cbdt) = font(128, 65535, 65535);
        assert!(!strikes_fit(&cblc, &cbdt, 7, 24.0));
        let (cblc, cbdt) = font(1, 136, 128);
        assert!(!strikes_fit(&cblc, &cbdt, 7, 2048.0));
        let (cblc, cbdt) = font(0, 136, 128);
        assert!(!strikes_fit(&cblc, &cbdt, 7, 24.0));
        let (cblc, cbdt) = font(109, 136, 128);
        assert!(
            !strikes_fit(&cblc, &cbdt[..cbdt.len() - 1], 7, 24.0),
            "truncated"
        );
        for field in [4, 8, 60, 68, 72] {
            let mut bogus = cblc.clone();
            bogus[field..field + 4].copy_from_slice(&u32::MAX.to_be_bytes());
            assert!(
                !strikes_fit(&bogus, &cbdt, 7, 24.0),
                "offset or count at {field}"
            );
        }
    }
}
