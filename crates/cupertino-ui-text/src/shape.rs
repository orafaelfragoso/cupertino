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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontStyle, FontVariation, FontWeight};
    use std::sync::Arc;

    const INTER: &[u8] = include_bytes!("../../cupertino-assets/fonts/InterVariable-subset.ttf");

    fn inter() -> (FontIndex, crate::FaceId) {
        let mut index = FontIndex::new();
        let face = index
            .add_face(
                "Inter",
                FontWeight::REGULAR,
                FontStyle::Normal,
                Arc::new(INTER.to_vec()),
                0,
            )
            .unwrap();
        (index, face)
    }

    fn request(font: FontId) -> ShapeRequest<'static> {
        ShapeRequest {
            font,
            size: 16.0,
            direction: Direction::Ltr,
            features: &[],
            lang: None,
            letter_spacing: 0.0,
        }
    }

    #[test]
    fn an_instance_request_returns_the_same_font_id_and_a_new_weight_a_new_one() {
        let (mut index, face) = inter();
        let regular = index.instance(face, FontWeight::REGULAR, &[]).unwrap();
        assert_eq!(
            index.instance(face, FontWeight::REGULAR, &[]).unwrap(),
            regular
        );
        let bold = index.instance(face, FontWeight::BOLD, &[]).unwrap();
        assert_ne!(bold, regular);
        assert_eq!(index.face_of(bold).unwrap(), face);
    }

    #[test]
    fn a_heavier_weight_shapes_wider() {
        let (mut index, face) = inter();
        let regular = index.instance(face, FontWeight::REGULAR, &[]).unwrap();
        let bold = index.instance(face, FontWeight(800), &[]).unwrap();
        let narrow = index
            .shape("Hamburgefons", &request(regular))
            .unwrap()
            .advance();
        let wide = index
            .shape("Hamburgefons", &request(bold))
            .unwrap()
            .advance();
        assert!(wide > narrow, "{wide} vs {narrow}");
    }

    #[test]
    fn advance_scales_with_size_and_clusters_follow_bytes() {
        let (mut index, face) = inter();
        let font = index.instance(face, FontWeight::REGULAR, &[]).unwrap();
        let small = index.shape("héllo", &request(font)).unwrap();
        let big = index
            .shape(
                "héllo",
                &ShapeRequest {
                    size: 32.0,
                    ..request(font)
                },
            )
            .unwrap();
        assert!((big.advance() - 2.0 * small.advance()).abs() < 0.01);
        let clusters: Vec<usize> = small.glyphs.iter().map(|glyph| glyph.cluster).collect();
        assert_eq!(clusters, [0, 1, 3, 4, 5]);
    }

    #[test]
    fn a_right_to_left_run_lists_the_last_character_first() {
        let (mut index, face) = inter();
        let font = index.instance(face, FontWeight::REGULAR, &[]).unwrap();
        let run = index
            .shape(
                "abc",
                &ShapeRequest {
                    direction: Direction::Rtl,
                    ..request(font)
                },
            )
            .unwrap();
        let clusters: Vec<usize> = run.glyphs.iter().map(|glyph| glyph.cluster).collect();
        assert_eq!(clusters, [2, 1, 0]);
    }

    #[test]
    fn letter_spacing_is_added_after_every_glyph() {
        let (mut index, face) = inter();
        let font = index.instance(face, FontWeight::REGULAR, &[]).unwrap();
        let plain = index.shape("abcd", &request(font)).unwrap().advance();
        let spaced = index
            .shape(
                "abcd",
                &ShapeRequest {
                    letter_spacing: 1.5,
                    ..request(font)
                },
            )
            .unwrap()
            .advance();
        assert!((spaced - plain - 6.0).abs() < 0.001);
    }

    #[test]
    fn a_feature_changes_the_shaping() {
        let (mut index, face) = inter();
        let font = index.instance(face, FontWeight::REGULAR, &[]).unwrap();
        let tabular = [FontFeature {
            tag: *b"tnum",
            value: 1,
        }];
        let ones = |features: &[FontFeature]| {
            let run = index
                .shape(
                    "1111",
                    &ShapeRequest {
                        features,
                        ..request(font)
                    },
                )
                .unwrap();
            run.glyphs
                .iter()
                .map(|glyph| glyph.advance)
                .collect::<Vec<_>>()
        };
        assert_ne!(ones(&[]), ones(&tabular));
    }

    #[test]
    fn variation_values_within_a_quarter_unit_share_an_instance() {
        let (mut index, face) = inter();
        let at = |value: f32| {
            [FontVariation {
                tag: *b"wght",
                value,
            }]
        };
        let a = index
            .instance(face, FontWeight::REGULAR, &at(650.0))
            .unwrap();
        let b = index
            .instance(face, FontWeight::REGULAR, &at(650.05))
            .unwrap();
        let c = index
            .instance(face, FontWeight::REGULAR, &at(700.0))
            .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn coverage_and_unknown_fonts_are_reported() {
        let (mut index, face) = inter();
        let font = index.instance(face, FontWeight::REGULAR, &[]).unwrap();
        assert!(index.has_glyph(font, 'a').unwrap());
        assert!(!index.has_glyph(font, '日').unwrap());
        assert_eq!(
            index.shape("a", &request(FontId(99))).unwrap_err(),
            TextError::UnknownFont(FontId(99))
        );
        assert_eq!(
            FontIndex::new().add_face(
                "X",
                FontWeight::REGULAR,
                FontStyle::Normal,
                Arc::new(vec![0; 16]),
                0
            ),
            Err(TextError::InvalidFont("X".into()))
        );
    }

    #[test]
    fn faces_match_by_family_style_and_nearest_weight() {
        let (mut index, face) = inter();
        let other = index
            .add_face(
                "Inter",
                FontWeight::REGULAR,
                FontStyle::Italic,
                Arc::new(INTER.to_vec()),
                0,
            )
            .unwrap();
        assert_eq!(
            index.match_face("inter", FontWeight::BOLD, FontStyle::Normal),
            Some(face)
        );
        assert_eq!(
            index.match_face("INTER", FontWeight::BOLD, FontStyle::Italic),
            Some(other)
        );
        assert_eq!(
            index.match_face("Nope", FontWeight::REGULAR, FontStyle::Normal),
            None
        );
    }

    #[test]
    fn metrics_scale_with_size() {
        let (mut index, face) = inter();
        let font = index.instance(face, FontWeight::REGULAR, &[]).unwrap();
        let metrics = index.metrics(font).unwrap();
        assert!(
            metrics.ascent > 0.5 && metrics.descent > 0.0 && metrics.x_height < metrics.cap_height
        );
        assert!((metrics.line_height(20.0) - 2.0 * metrics.line_height(10.0)).abs() < 0.001);
    }
}
