use crate::{FaceId, FontId, FontStyle, FontVariation, FontWeight, TextError};
use harfrust::font::{Font, Variation};
use harfrust::Tag;
use std::collections::HashMap;
use std::sync::Arc;
use swash::{FontDataRef, StringId, Style};

const WEIGHT_AXIS: [u8; 4] = *b"wght";

#[cfg(test)]
thread_local! {
    /// The charmap lookups of [`FontIndex::has_glyph`] on this thread: what a fallback costs.
    pub(crate) static GLYPH_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Vertical metrics of a font as fractions of the em; multiply by the size in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
    /// Positive, below the baseline.
    pub descent: f32,
    pub line_gap: f32,
    pub x_height: f32,
    pub cap_height: f32,
}

impl FontMetrics {
    /// The height of one line of text at `size`, with the gap.
    pub fn line_height(&self, size: f32) -> f32 {
        (self.ascent + self.descent + self.line_gap) * size
    }
}

/// A font instance as bytes: the file, the face's index in it and the variation coordinates the instance sets.
#[derive(Clone, Debug)]
pub struct FontSource {
    pub data: Arc<Vec<u8>>,
    pub index: u32,
    pub variations: Vec<FontVariation>,
}

struct Face {
    family: Arc<str>,
    data: Arc<Vec<u8>>,
    index: u32,
    weight: FontWeight,
    style: FontStyle,
    font: Font,
    weight_range: Option<(f32, f32)>,
    /// The face has colour glyphs (COLR, CBDT or sbix).
    colour: bool,
}

struct Instance {
    face: FaceId,
    variations: Vec<FontVariation>,
    font: Font,
    metrics: FontMetrics,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct InstanceKey {
    face: FaceId,
    variations: Vec<([u8; 4], u32)>,
}

/// The fonts the text engine can use: faces registered from font files, and the face instances (a face at a set of
/// variation coordinates) that layout and rasterization name by [`FontId`].
#[derive(Default)]
pub struct FontIndex {
    faces: Vec<Face>,
    instances: Vec<Instance>,
    instance_ids: HashMap<InstanceKey, FontId>,
    fallbacks: HashMap<(FontId, FontWeight), Arc<[FontId]>>,
}

impl FontIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the font at `index` in `data` (a font file, or a collection; collection members share the `Arc`) as a face of `family`. A variable
    /// font declares its weight range itself, and `weight` is the weight of a static face.
    pub fn add_face(
        &mut self,
        family: &str,
        weight: FontWeight,
        style: FontStyle,
        data: Arc<Vec<u8>>,
        index: u32,
    ) -> Result<FaceId, TextError> {
        let font = Font::new(
            harfrust::font::Blob::from(Arc::clone(&data) as Arc<dyn AsRef<[u8]> + Send + Sync>),
            index,
        )
        .ok_or_else(|| TextError::InvalidFont(family.into()))?;
        let weight_range = font
            .axes()
            .get_by_tag(Tag::new(&WEIGHT_AXIS))
            .map(|axis| (axis.min_value(), axis.max_value()));
        let colour = swash::FontRef::from_index(&data, index as usize).is_some_and(|font| {
            [b"COLR", b"CBDT", b"sbix"]
                .into_iter()
                .any(|tag| font.table(swash::tag_from_bytes(tag)).is_some())
        });
        let id = FaceId(self.faces.len() as u32);
        self.fallbacks.clear();
        self.faces.push(Face {
            family: family.into(),
            data,
            index,
            weight,
            style,
            font,
            weight_range,
            colour,
        });
        Ok(id)
    }

    /// Registers every face of a font file or collection under the family, weight and style its own tables name. A face
    /// without a family name is skipped. Returns the faces added.
    pub fn add_font(&mut self, data: Arc<Vec<u8>>) -> Result<Vec<FaceId>, TextError> {
        let file = FontDataRef::new(&data)
            .ok_or_else(|| TextError::InvalidFont("not a TrueType or OpenType file".into()))?;
        let mut named = Vec::new();
        // a collection's header states its face count; each face needs a 4-byte offset, so no more fit in the file
        for index in 0..file.len().min(data.len() / 4) {
            let Some(font) = file.get(index) else {
                break;
            };
            let name = |id| {
                font.localized_strings()
                    .find(|s| s.id() == id)
                    .map(|s| s.to_string())
            };
            let Some(family) = name(StringId::TypographicFamily).or_else(|| name(StringId::Family))
            else {
                continue;
            };
            let attributes = font.attributes();
            let style = match attributes.style() {
                Style::Normal => FontStyle::Normal,
                _ => FontStyle::Italic,
            };
            named.push((
                family,
                FontWeight(attributes.weight().0),
                style,
                index as u32,
            ));
        }
        named
            .into_iter()
            .map(|(family, weight, style, index)| {
                self.add_face(&family, weight, style, Arc::clone(&data), index)
            })
            .collect()
    }

    pub fn family(&self, face: FaceId) -> Option<&str> {
        self.faces.get(face.0 as usize).map(|f| &*f.family)
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    /// The face that best matches `family`, `weight` and `style`, by family name (case-insensitive), then the style,
    /// then the nearest weight. A variable face matches any weight inside its range.
    pub fn match_face(&self, family: &str, weight: FontWeight, style: FontStyle) -> Option<FaceId> {
        self.faces
            .iter()
            .enumerate()
            .filter(|(_, face)| face.family.eq_ignore_ascii_case(family))
            .min_by_key(|(_, face)| {
                let style_miss = u8::from(face.style != style);
                let weight_miss = match face.weight_range {
                    Some((low, high)) => {
                        let wanted = f32::from(weight.0);
                        (low - wanted).max(wanted - high).max(0.0) as u32
                    }
                    None => u32::from(face.weight.0.abs_diff(weight.0)),
                };
                (style_miss, weight_miss)
            })
            .map(|(index, _)| FaceId(index as u32))
    }

    /// The instance of `face` at `weight` and `variations`; a variation given for `wght` wins over `weight`. The same
    /// request always returns the same [`FontId`].
    pub fn instance(
        &mut self,
        face: FaceId,
        weight: FontWeight,
        variations: &[FontVariation],
    ) -> Result<FontId, TextError> {
        let entry = self.faces.get(face.0 as usize).ok_or(TextError::NoFont)?;
        let mut coords: Vec<([u8; 4], f32)> = Vec::with_capacity(variations.len() + 1);
        if let Some((low, high)) = entry.weight_range {
            if !variations
                .iter()
                .any(|variation| variation.tag == WEIGHT_AXIS)
            {
                coords.push((WEIGHT_AXIS, f32::from(weight.0).clamp(low, high)));
            }
        }
        coords.extend(
            variations
                .iter()
                .map(|variation| (variation.tag, quantize(variation.value))),
        );
        coords.sort_by_key(|(tag, _)| *tag);
        coords.dedup_by_key(|(tag, _)| *tag);
        let key = InstanceKey {
            face,
            variations: coords
                .iter()
                .map(|(tag, value)| (*tag, value.to_bits()))
                .collect(),
        };
        if let Some(id) = self.instance_ids.get(&key) {
            return Ok(*id);
        }
        let font = entry
            .font
            .instance_builder()
            .variations(
                coords
                    .iter()
                    .map(|(tag, value)| Variation::new(Tag::new(tag), *value)),
            )
            .build();
        let metrics = metrics_of(&font);
        let id = FontId(self.instances.len() as u32);
        self.instances.push(Instance {
            face,
            variations: coords
                .iter()
                .map(|(tag, value)| FontVariation {
                    tag: *tag,
                    value: *value,
                })
                .collect(),
            font,
            metrics,
        });
        self.instance_ids.insert(key, id);
        Ok(id)
    }

    /// The fonts that stand in for `font` where it has no glyph: every other face in load order, at `weight`. Kept per
    /// font and weight.
    pub(crate) fn fallbacks(
        &mut self,
        font: FontId,
        weight: FontWeight,
    ) -> Result<Arc<[FontId]>, TextError> {
        if let Some(fonts) = self.fallbacks.get(&(font, weight)) {
            return Ok(Arc::clone(fonts));
        }
        let own = self.face_of(font)?;
        let fonts: Arc<[FontId]> = (0..self.faces.len() as u32)
            .map(FaceId)
            .filter(|face| *face != own)
            .map(|face| self.instance(face, weight, &[]))
            .collect::<Result<_, _>>()?;
        self.fallbacks.insert((font, weight), Arc::clone(&fonts));
        Ok(fonts)
    }

    pub fn face_of(&self, font: FontId) -> Result<FaceId, TextError> {
        self.instance_entry(font).map(|instance| instance.face)
    }

    /// The font file and variation coordinates behind `font`, for a rasterizer.
    pub fn source(&self, font: FontId) -> Result<FontSource, TextError> {
        let instance = self.instance_entry(font)?;
        let face = &self.faces[instance.face.0 as usize];
        Ok(FontSource {
            data: Arc::clone(&face.data),
            index: face.index,
            variations: instance.variations.clone(),
        })
    }

    pub fn metrics(&self, font: FontId) -> Result<FontMetrics, TextError> {
        self.instance_entry(font).map(|instance| instance.metrics)
    }

    /// Whether the font's face has colour glyphs (COLR, CBDT or sbix).
    pub fn is_colour(&self, font: FontId) -> Result<bool, TextError> {
        let face = self.face_of(font)?;
        Ok(self.faces[face.0 as usize].colour)
    }

    /// Whether the font has a glyph for `ch`.
    pub fn has_glyph(&self, font: FontId, ch: char) -> Result<bool, TextError> {
        #[cfg(test)]
        GLYPH_LOOKUPS.with(|count| count.set(count.get() + 1));
        self.font(font)
            .map(|font| font.charmap().map_unicode(ch).is_some())
    }

    pub(crate) fn font(&self, font: FontId) -> Result<&Font, TextError> {
        self.instance_entry(font).map(|instance| &instance.font)
    }

    fn instance_entry(&self, font: FontId) -> Result<&Instance, TextError> {
        self.instances
            .get(font.0 as usize)
            .ok_or(TextError::UnknownFont(font))
    }
}

/// Coordinates are kept in quarter units, so an animated axis shares instances instead of making one per frame.
fn quantize(value: f32) -> f32 {
    (value * 4.0).round() / 4.0
}

fn metrics_of(font: &Font) -> FontMetrics {
    let metrics = font.metrics();
    let em = f32::from(metrics.units_per_em);
    let line = metrics
        .use_typo_metrics
        .then_some(metrics.typo_line)
        .flatten()
        .or(metrics.hhea_line)
        .or(metrics.typo_line);
    let (ascent, descent, line_gap) = line.map_or((0.8, 0.2, 0.0), |line| {
        (
            line.ascender.to_f32() / em,
            -line.descender.to_f32() / em,
            line.line_gap.to_f32() / em,
        )
    });
    FontMetrics {
        ascent,
        descent,
        line_gap,
        x_height: metrics.x_height.map_or(0.5, |height| height.to_f32() / em),
        cap_height: metrics
            .cap_height
            .map_or(0.7, |height| height.to_f32() / em),
    }
}
