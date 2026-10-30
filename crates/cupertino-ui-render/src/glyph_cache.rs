use crate::RenderError;
use crate::Tally;
use cupertino_ui_text::{FontId, GlyphFormat, GlyphId, GlyphKey, GlyphRasterizer, RasterGlyph};
use std::collections::{HashMap, HashSet};

struct Entry {
    glyph: Option<RasterGlyph>,
    used: u64,
}

/// Rasterized glyphs by their raster identity, evicted least recently used between frames. A colour glyph ignores the
/// subpixel phase and the stem darkening of its key, so once a raster shows a glyph is colour at a size, it is kept
/// under one key with both at zero.
pub(crate) struct GlyphCache {
    entries: HashMap<GlyphKey, Entry>,
    /// Font, glyph and size of the glyphs whose raster was colour.
    colour: HashSet<(FontId, GlyphId, u32)>,
    bytes: usize,
    budget: usize,
    tick: u64,
    /// Entries made since the last [`take_filled`](Self::take_filled).
    filled: Tally,
}

/// `key` without what a colour raster does not read.
fn colour_key(key: GlyphKey) -> GlyphKey {
    GlyphKey {
        subpixel_x: 0,
        dilation_q: 0,
        ..key
    }
}

impl GlyphCache {
    pub(crate) fn new(budget: usize) -> GlyphCache {
        GlyphCache {
            entries: HashMap::new(),
            colour: HashSet::new(),
            bytes: 0,
            budget,
            tick: 0,
            filled: Tally::default(),
        }
    }

    pub(crate) fn get<R: GlyphRasterizer>(
        &mut self,
        key: GlyphKey,
        rasterizer: &mut R,
    ) -> Result<Option<RasterGlyph>, RenderError> {
        self.tick += 1;
        let key = if self.colour.contains(&(key.font, key.glyph, key.size_q)) {
            colour_key(key)
        } else {
            key
        };
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.tick;
            return Ok(entry.glyph.clone());
        }
        let glyph = rasterizer.rasterize(&key).map_err(RenderError::Glyph)?;
        let key = match &glyph {
            Some(g) if g.format == GlyphFormat::ColorPremultiplied => {
                self.colour.insert((key.font, key.glyph, key.size_q));
                colour_key(key)
            }
            _ => key,
        };
        let made = glyph.as_ref().map_or(0, |g| g.pixels.len());
        self.filled.add(made as u64);
        self.bytes += made;
        self.entries.insert(
            key,
            Entry {
                glyph: glyph.clone(),
                used: self.tick,
            },
        );
        Ok(glyph)
    }

    pub(crate) fn take_filled(&mut self) -> Tally {
        std::mem::take(&mut self.filled)
    }

    pub(crate) fn retained(&self) -> Tally {
        Tally {
            count: self.entries.len() as u64,
            bytes: self.bytes as u64,
        }
    }

    /// Drops the least recently used glyphs until the cache fits its budget. Called between frames, so no glyph of the
    /// frame being painted is dropped.
    pub(crate) fn trim(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let mut order: Vec<(u64, GlyphKey)> =
            self.entries.iter().map(|(k, e)| (e.used, *k)).collect();
        order.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in order {
            if self.bytes <= self.budget {
                break;
            }
            if let Some(entry) = self.entries.remove(&key) {
                if entry
                    .glyph
                    .as_ref()
                    .is_some_and(|g| g.format == GlyphFormat::ColorPremultiplied)
                {
                    self.colour.remove(&(key.font, key.glyph, key.size_q));
                }
                self.bytes -= entry.glyph.map_or(0, |g| g.pixels.len());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_ui_text::{FontId, GlyphFormat, GlyphId, TextError};
    use std::sync::Arc;

    /// Counts its rasters; glyph 7 is a colour glyph.
    struct Counting(u32);

    impl GlyphRasterizer for Counting {
        fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
            self.0 += 1;
            Ok(Some(RasterGlyph {
                format: if key.glyph == GlyphId(7) {
                    GlyphFormat::ColorPremultiplied
                } else {
                    GlyphFormat::Coverage
                },
                width: 10,
                height: 10,
                stride: 10,
                bearing_x: 0,
                bearing_y: 0,
                pixels: Arc::from(vec![255u8; 100]),
            }))
        }
    }

    fn key(glyph: u32) -> GlyphKey {
        GlyphKey {
            font: FontId(0),
            glyph: GlyphId(glyph),
            size_q: 64,
            subpixel_x: 0,
            dilation_q: 0,
        }
    }

    #[test]
    fn trim_drops_the_least_recently_used_and_keeps_the_rest() {
        let mut cache = GlyphCache::new(200);
        let mut r = Counting(0);
        for g in 0..3 {
            cache.get(key(g), &mut r).unwrap();
        }
        cache.get(key(0), &mut r).unwrap();
        cache.trim();
        assert_eq!(cache.len(), 2);
        cache.get(key(0), &mut r).unwrap();
        assert_eq!(r.0, 3, "glyph 0 was used last and stayed cached");
        cache.get(key(1), &mut r).unwrap();
        assert_eq!(r.0, 4, "glyph 1 was evicted and rasterized again");
    }

    #[test]
    fn a_colour_glyph_is_rasterized_and_kept_once_for_every_phase_and_darkening() {
        let mut cache = GlyphCache::new(1 << 20);
        let mut r = Counting(0);
        for glyph in [7, 8] {
            for subpixel_x in 0..4 {
                for dilation_q in 0..6 {
                    let key = GlyphKey {
                        subpixel_x,
                        dilation_q,
                        ..key(glyph)
                    };
                    cache.get(key, &mut r).unwrap();
                }
            }
        }
        assert_eq!(
            r.0,
            1 + 24,
            "the colour glyph once, the outline glyph per key"
        );
        assert_eq!(cache.len(), 1 + 24);
        let larger = GlyphKey {
            size_q: 128,
            subpixel_x: 2,
            ..key(7)
        };
        cache.get(larger, &mut r).unwrap();
        assert_eq!(r.0, 26, "another size is another raster");
        let again = GlyphKey {
            subpixel_x: 0,
            dilation_q: 3,
            ..larger
        };
        cache.get(again, &mut r).unwrap();
        assert_eq!(r.0, 26, "a first raster at a phase is kept for every phase");
    }
}
