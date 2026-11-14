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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontIndex, FontStyle, FontWeight, GlyphId};

    const INTER: &[u8] = include_bytes!("../../cupertino-assets/fonts/InterVariable-subset.ttf");

    fn raster() -> (GlyphRaster, FontId, impl Fn(char) -> GlyphId) {
        let mut fonts = FontIndex::new();
        let face = fonts
            .add_face(
                "Inter",
                FontWeight(400),
                FontStyle::Normal,
                Arc::new(INTER.to_vec()),
                0,
            )
            .unwrap();
        let font = fonts.instance(face, FontWeight(400), &[]).unwrap();
        let ids: HashMap<char, u32> = ['H', 'l', ' ']
            .into_iter()
            .map(|ch| {
                let id = fonts
                    .font(font)
                    .unwrap()
                    .charmap()
                    .map_unicode(ch)
                    .unwrap()
                    .to_u32();
                (ch, id)
            })
            .collect();
        let system = Arc::new(TextSystem::new(fonts, "Inter"));
        (GlyphRaster::new(system), font, move |ch| GlyphId(ids[&ch]))
    }

    fn key(font: FontId, glyph: GlyphId, smoothing: bool, fill: f32, x: f64) -> GlyphKey {
        GlyphKey::for_draw(font, glyph, 24.0, x, smoothing, [fill; 3])
    }

    fn coverage(glyph: &RasterGlyph) -> u32 {
        glyph.pixels.iter().map(|byte| u32::from(*byte)).sum()
    }

    /// A second rasterizer on the same text system, as a window made again has, finds the glyphs the first drew,
    /// with the same pixels, and draws none of them again; a glyph it has not seen it draws once.
    #[test]
    fn rasterizers_on_one_text_system_share_their_glyphs() {
        let (mut first, font, id) = raster();
        let keys = [
            key(font, id('H'), true, 0.0, 0.0),
            key(font, id('l'), true, 0.0, 0.25),
            key(font, id(' '), false, 1.0, 0.0),
        ];
        let drawn: Vec<_> = keys.iter().map(|k| first.rasterize(k).unwrap()).collect();
        let system = Arc::clone(&first.system);
        assert_eq!(system.shared_rasters().drawn, 3);
        let mut second = GlyphRaster::new(Arc::clone(&system));
        let found: Vec<_> = keys.iter().map(|k| second.rasterize(k).unwrap()).collect();
        assert_eq!(found, drawn);
        let stats = system.shared_rasters();
        assert_eq!((stats.drawn, stats.found, stats.glyphs), (3, 3, 3));
        // the same glyph drawn by a rasterizer that keeps nothing
        let mut alone = GlyphRaster::new(Arc::clone(&system));
        assert_eq!(alone.draw(&keys[0]).unwrap(), drawn[0]);
        second
            .rasterize(&key(font, id('H'), true, 0.0, 0.5))
            .unwrap();
        assert_eq!(system.shared_rasters().drawn, 4);
    }

    /// The shared rasters stay within their bytes: past them, the least recently used quarter goes, and a glyph
    /// used since stays.
    #[test]
    fn the_shared_rasters_stay_within_their_bytes() {
        let (raster, font, id) = raster();
        let mut shared = SharedRasters::default();
        let glyph = |side: u32| RasterGlyph {
            format: GlyphFormat::Coverage,
            width: side,
            height: side,
            stride: side,
            bearing_x: 0,
            bearing_y: 0,
            pixels: vec![7; (side * side) as usize].into(),
        };
        let held = key(font, id('H'), true, 0.0, 0.0);
        shared.insert(held, Some(glyph(64)));
        for i in 0..2000u32 {
            let k = GlyphKey::for_draw(font, GlyphId(1000 + i), 24.0, 0.0, true, [0.0; 3]);
            shared.insert(k, Some(glyph(64)));
            assert!(
                shared.get(&held).is_some(),
                "the glyph used every time stays"
            );
            assert!(shared.stats().bytes <= SHARED_RASTER_BYTES);
        }
        assert!(shared.stats().glyphs < 2001);
        drop(raster);
    }

    #[test]
    fn a_glyph_has_coverage_above_its_baseline() {
        let (mut raster, font, id) = raster();
        let glyph = raster
            .rasterize(&key(font, id('H'), true, 0.0, 0.0))
            .unwrap()
            .unwrap();
        assert_eq!(glyph.format, GlyphFormat::Coverage);
        assert_eq!(glyph.stride, glyph.width);
        assert_eq!(glyph.pixels.len(), (glyph.width * glyph.height) as usize);
        assert!(
            glyph.bearing_y > 10 && glyph.bearing_y < 30,
            "{}",
            glyph.bearing_y
        );
        assert!(coverage(&glyph) > 0);
    }

    #[test]
    fn a_space_has_no_pixels() {
        let (mut raster, font, id) = raster();
        assert_eq!(
            raster.rasterize(&key(font, id(' '), true, 0.0, 0.0)),
            Ok(None)
        );
    }

    #[test]
    fn light_text_is_darkened_more_than_dark_text_and_smoothing_off_least() {
        let (mut raster, font, id) = raster();
        let mut total = |smoothing, fill| {
            coverage(
                &raster
                    .rasterize(&key(font, id('l'), smoothing, fill, 0.0))
                    .unwrap()
                    .unwrap(),
            )
        };
        let (off, dark, light) = (total(false, 1.0), total(true, 0.0), total(true, 1.0));
        assert!(off < dark, "{off} {dark}");
        assert!(dark < light, "{dark} {light}");
    }

    #[test]
    fn a_subpixel_phase_moves_the_coverage_right() {
        let (mut raster, font, id) = raster();
        let mut centre = |x| {
            let glyph = raster
                .rasterize(&key(font, id('l'), true, 0.0, x))
                .unwrap()
                .unwrap();
            let (mut weight, mut sum) = (0u64, 0u64);
            for (i, byte) in glyph.pixels.iter().enumerate() {
                let column = i as u64 % u64::from(glyph.width);
                weight += u64::from(*byte) * (column * 4 + 2);
                sum += u64::from(*byte) * 4;
            }
            weight as f64 / sum as f64 + f64::from(glyph.bearing_x)
        };
        let (flat, shifted) = (centre(0.0), centre(0.5));
        assert!((shifted - flat - 0.5).abs() < 0.3, "{flat} {shifted}");
    }

    #[test]
    fn an_unknown_font_is_an_error() {
        let (mut raster, _, id) = raster();
        let key = key(FontId(9), id('H'), true, 0.0, 0.0);
        assert_eq!(
            raster.rasterize(&key),
            Err(TextError::UnknownFont(FontId(9)))
        );
    }

    #[test]
    fn equal_draws_share_a_key_and_the_fill_changes_it_in_quarters() {
        let (font, glyph) = (FontId(0), GlyphId(1));
        let key =
            |smoothing, fill| GlyphKey::for_draw(font, glyph, 12.0, 0.3, smoothing, [fill; 3]);
        assert_eq!(key(true, 0.0).dilation_q, 1);
        assert_eq!(key(true, 1.0).dilation_q, 5);
        assert_eq!(key(false, 1.0).dilation_q, 0);
        assert_eq!(key(true, 0.50), key(true, 0.51));
        assert_ne!(key(true, 0.0), key(true, 1.0));
        assert_eq!(key(true, 0.0).subpixel_x, 1);
    }

    #[test]
    fn premultiplying_scales_the_colour_by_the_alpha() {
        assert_eq!(
            &*premultiplied(&[255, 128, 0, 128, 10, 20, 30, 0]),
            &[128, 64, 0, 128, 0, 0, 0, 0]
        );
    }

    #[test]
    fn the_cost_of_a_glyph_grows_with_its_area_not_faster() {
        let (mut raster, font, id) = raster();
        let mut best = |size_px: f64| {
            (0..9)
                .map(|_| {
                    let key = GlyphKey::for_draw(font, id('H'), size_px, 0.0, true, [0.0; 3]);
                    let start = std::time::Instant::now();
                    let glyph = raster.rasterize(&key).unwrap().unwrap();
                    let elapsed = start.elapsed();
                    assert!(glyph.height > 0);
                    elapsed
                })
                .min()
                .unwrap()
        };
        let (small, large) = (best(64.0), best(256.0));
        let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
        assert!(
            ratio < 100.0,
            "256 px took {large:?} against {small:?} for 64 px, {ratio:.0}x for 16x the area"
        );
    }

    #[test]
    fn a_size_past_the_cap_draws_at_the_cap() {
        let (mut raster, font, id) = raster();
        let mut draw = |size_q| {
            let key = GlyphKey {
                size_q,
                ..key(font, id('H'), true, 0.0, 0.0)
            };
            raster.rasterize(&key).unwrap().unwrap()
        };
        let capped = draw(MAX_SIZE_Q);
        assert!(capped.height <= cupertino_glyph::MAX_MASK_SIDE);
        assert_eq!(draw(u32::MAX), capped);
    }
}
