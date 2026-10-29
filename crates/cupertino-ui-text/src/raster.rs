use crate::{
    FontId, FontSource, GlyphFormat, GlyphKey, GlyphRasterizer, RasterGlyph, TextError, TextSystem,
};
use cupertino_glyph::{declarative_size, rasterize, Smoothing};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::{tag_from_bytes, CacheKey, FontRef, Setting};

struct Loaded {
    source: FontSource,
    offset: u32,
    key: CacheKey,
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
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        let size = key.size_q as f32 / 64.0;
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

        let colour = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
        ])
        .render(&mut scaler, glyph);
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
}
