use crate::{Direction, FontFeature, FontId, FontIndex, GlyphId, TextError};
use harfrust::{Buffer, Feature, Language, ShapeOptions, ShaperFont, Tag};

/// What to shape: one run of text in one font instance.
#[derive(Clone, Copy, Debug)]
pub struct ShapeRequest<'a> {
    pub font: FontId,
    /// Logical points per em.
    pub size: f32,
    pub direction: Direction,
    pub features: &'a [FontFeature],
    /// A BCP 47 language tag.
    pub lang: Option<&'a str>,
    /// Extra advance added after every glyph, in points.
    pub letter_spacing: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub id: GlyphId,
    /// Points; the pen moves by this much after the glyph, in the run's direction.
    pub advance: f32,
    pub x_offset: f32,
    /// Positive up.
    pub y_offset: f32,
    /// Byte offset in the shaped text of the cluster this glyph belongs to.
    pub cluster: usize,
}

/// Glyphs of one run in visual order (a right-to-left run lists its last character first).
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedRun {
    pub font: FontId,
    pub size: f32,
    pub direction: Direction,
    pub glyphs: Vec<ShapedGlyph>,
}

impl ShapedRun {
    pub fn advance(&self) -> f32 {
        self.glyphs.iter().map(|glyph| glyph.advance).sum()
    }
}

impl FontIndex {
    /// Shapes `text` with one font instance. Glyphs the font lacks come out as its `.notdef` glyph; picking another
    /// font for them is the caller's job.
    pub fn shape(&self, text: &str, request: &ShapeRequest<'_>) -> Result<ShapedRun, TextError> {
        let font = self.font(request.font)?;
        let shaper = ShaperFont::new(font);
        let scale = request.size / f32::from(font.units_per_em());

        let mut buffer = Buffer::new();
        buffer.push_str(text);
        buffer.set_direction(match request.direction {
            Direction::Ltr => harfrust::Direction::LeftToRight,
            Direction::Rtl => harfrust::Direction::RightToLeft,
        });
        if let Some(lang) = request.lang.and_then(Language::new) {
            buffer.set_language(Some(lang));
        }
        buffer.guess_segment_properties();

        let features: Vec<Feature> = request
            .features
            .iter()
            .map(|feature| Feature::new(Tag::new(&feature.tag), feature.value, ..))
            .collect();
        harfrust::shape(
            &shaper,
            &mut buffer,
            ShapeOptions::new().features(&features),
        )
        .map_err(|_| TextError::ShapeFailed)?;

        let glyphs = buffer
            .glyph_infos()
            .iter()
            .zip(buffer.glyph_positions())
            .map(|(info, position)| ShapedGlyph {
                id: GlyphId(info.glyph_id),
                advance: position.x_advance as f32 * scale + request.letter_spacing,
                x_offset: position.x_offset as f32 * scale,
                y_offset: position.y_offset as f32 * scale,
                cluster: info.cluster as usize,
            })
            .collect();
        Ok(ShapedRun {
            font: request.font,
            size: request.size,
            direction: request.direction,
            glyphs,
        })
    }
}
