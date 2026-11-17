use crate::strike::strikes_fit;
use crate::{
    FontId, FontSource, GlyphFormat, GlyphKey, GlyphRasterizer, RasterGlyph, TextError, TextSystem,
};
use cupertino_glyph::{declarative_size, mask_bounds, rasterize, Smoothing};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::{tag_from_bytes, CacheKey, FontRef, Setting};

/// The largest size a glyph is drawn at, in [`GlyphKey::size_q`] steps (2048 pixels per em); larger keys draw at it.
/// This bounds the scale only: a colour glyph's extent is checked on its own before it is drawn.
const MAX_SIZE_Q: u32 = 2048 * 64;

struct Loaded {
    source: FontSource,
    offset: u32,
    key: CacheKey,
}

/// Bytes of glyph rasters a [`TextSystem`] keeps for all its rasterizers together. A window made again (a panel that
/// opens, a menu, a popup) draws through a renderer of its own, which finds its glyphs here instead of drawing them
/// again.
pub const SHARED_RASTER_BYTES: usize = 2 << 20;

/// What the rasters a [`TextSystem`] shares between its rasterizers hold and did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SharedRasterStats {
    pub glyphs: usize,
    pub bytes: usize,
    /// Glyphs found, and glyphs drawn because they were not there.
    pub found: u64,
    pub drawn: u64,
}

/// The glyph rasters of a [`TextSystem`], shared by every [`GlyphRaster`] on it, within [`SHARED_RASTER_BYTES`]: when
/// full, the least recently used quarter goes. A key that draws nothing is kept too.
#[derive(Default)]
pub(crate) struct SharedRasters {
    glyphs: HashMap<GlyphKey, (Option<RasterGlyph>, u64)>,
    tick: u64,
    stats: SharedRasterStats,
}

impl SharedRasters {
    fn size(glyph: &Option<RasterGlyph>) -> usize {
        // the key and its bookkeeping besides the pixels
        64 + glyph.as_ref().map_or(0, |g| g.pixels.len())
    }

    fn get(&mut self, key: &GlyphKey) -> Option<Option<RasterGlyph>> {
        self.tick += 1;
        let tick = self.tick;
        let (glyph, used) = self.glyphs.get_mut(key)?;
        *used = tick;
        self.stats.found += 1;
        Some(glyph.clone())
    }

    fn insert(&mut self, key: GlyphKey, glyph: Option<RasterGlyph>) {
        self.stats.drawn += 1;
        let size = Self::size(&glyph);
        if size > SHARED_RASTER_BYTES / 4 {
            return;
        }
        if self.stats.bytes + size > SHARED_RASTER_BYTES {
            let mut ticks: Vec<u64> = self.glyphs.values().map(|(_, t)| *t).collect();
            ticks.sort_unstable();
            let cut = ticks[ticks.len() / 4];
            self.glyphs.retain(|_, (_, t)| *t > cut);
            self.stats.bytes = self.glyphs.values().map(|(g, _)| Self::size(g)).sum();
        }
        self.tick += 1;
        if let Some((old, _)) = self.glyphs.insert(key, (glyph, self.tick)) {
            self.stats.bytes -= Self::size(&old);
        }
        self.stats.bytes += size;
        self.stats.glyphs = self.glyphs.len();
    }

    pub(crate) fn stats(&self) -> SharedRasterStats {
        SharedRasterStats {
            glyphs: self.glyphs.len(),
            ..self.stats
        }
    }
}

/// Rasterizes the glyphs of a [`TextSystem`]'s fonts: colour glyphs (emoji) as premultiplied RGBA, every other glyph
/// as 8-bit coverage with the stem darkening its key names. Glyphs are unhinted, because the hinting programs of a
/// variable font describe its default instance only.
pub struct GlyphRaster {
    system: Arc<TextSystem>,
    scale: ScaleContext,
    loaded: HashMap<FontId, Loaded>,
}

impl GlyphRaster {
    pub fn new(system: Arc<TextSystem>) -> Self {
        Self {
            system,
            scale: ScaleContext::new(),
            loaded: HashMap::new(),
        }
    }
}

fn load<'a>(
    system: &TextSystem,
    loaded: &'a mut HashMap<FontId, Loaded>,
    font: FontId,
) -> Result<&'a Loaded, TextError> {
    if let Entry::Vacant(entry) = loaded.entry(font) {
        let source = system.font_source(font)?;
        let reference = FontRef::from_index(&source.data, source.index as usize)
            .ok_or(TextError::InvalidFont("font".into()))?;
        let (offset, key) = (reference.offset, reference.key);
        entry.insert(Loaded {
            source,
            offset,
            key,
        });
    }
    Ok(&loaded[&font])
}

impl GlyphRasterizer for GlyphRaster {
    /// The glyph from the rasters the text system shares, drawn and kept there when it is not yet.
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        if let Some(glyph) = self.system.rasters().get(key) {
            return Ok(glyph);
        }
        let glyph = self.draw(key)?;
        self.system.rasters().insert(*key, glyph.clone());
        Ok(glyph)
    }
}

impl GlyphRaster {
    fn draw(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        let size = key.size_q.min(MAX_SIZE_Q) as f32 / 64.0;
        let Ok(glyph) = u16::try_from(key.glyph.0) else {
            return Ok(None);
        };
        if key.size_q == 0 {
            return Ok(None);
        }
        let Self {
            system,
            scale,
            loaded,
        } = self;
        let loaded = load(system, loaded, key.font)?;
        let font = FontRef {
            data: &loaded.source.data,
            offset: loaded.offset,
            key: loaded.key,
        };
        let mut scaler = scale
            .builder(font)
            .size(size)
            .hint(false)
            .variations(loaded.source.variations.iter().map(|variation| Setting {
                tag: tag_from_bytes(&variation.tag),
                value: variation.value,
            }))
            .build();

        let mut sources = Vec::with_capacity(2);
        if scaler.has_color_outlines()
            && scaler.scale_color_outline(glyph).is_some_and(|outline| {
                let bounds = outline.bounds();
                mask_bounds((bounds.min.x, bounds.min.y), (bounds.max.x, bounds.max.y)).is_some()
            })
        {
            sources.push(Source::ColorOutline(0));
        }
        if font.table(tag_from_bytes(b"sbix")).is_none()
            && font
                .table(tag_from_bytes(b"CBLC"))
                .zip(font.table(tag_from_bytes(b"CBDT")))
                .is_some_and(|(cblc, cbdt)| strikes_fit(cblc, cbdt, glyph, size))
        {
            sources.push(Source::ColorBitmap(StrikeWith::BestFit));
        }
        let colour = (!sources.is_empty())
            .then(|| Render::new(&sources).render(&mut scaler, glyph))
            .flatten();
        if let Some(image) = colour.filter(|image| image.content == Content::Color) {
            return Ok(Some(RasterGlyph {
                format: GlyphFormat::ColorPremultiplied,
                width: image.placement.width,
                height: image.placement.height,
                stride: image.placement.width * 4,
                bearing_x: image.placement.left,
                bearing_y: image.placement.top,
                pixels: premultiplied(&image.data),
            }));
        }

        let Some(outline) = scaler.scale_outline(glyph) else {
            return Ok(None);
        };
        let smoothing = if key.dilation_q == 0 {
            Smoothing::Off
        } else {
            Smoothing::On
        };
        let luminance = f32::from(key.dilation_q.saturating_sub(1)) / 4.0;
        let dilation = smoothing.dilation(declarative_size(size), luminance);
        let offset = (f32::from(key.subpixel_x) / 4.0, 0.0);
        Ok(rasterize(
            outline.points(),
            outline.verbs(),
            offset,
            (dilation, dilation),
        )
        .map(|mask| RasterGlyph {
            format: GlyphFormat::Coverage,
            width: mask.width,
            height: mask.height,
            stride: mask.width,
            bearing_x: mask.left,
            bearing_y: mask.top,
            pixels: mask.data.into(),
        }))
    }
}

fn premultiplied(rgba: &[u8]) -> Arc<[u8]> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let alpha = u32::from(pixel[3]);
            let scale = |channel: u8| ((u32::from(channel) * alpha + 127) / 255) as u8;
            [scale(pixel[0]), scale(pixel[1]), scale(pixel[2]), pixel[3]]
        })
        .collect()
}
