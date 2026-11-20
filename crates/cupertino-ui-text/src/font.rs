use crate::installed::{read_font, FontFile, SystemFonts};
use crate::{FaceId, FontId, FontStyle, FontVariation, FontWeight, TextError};
use harfrust::font::{Font, Variation};
use harfrust::Tag;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use swash::{FontDataRef, StringId, Style};

const WEIGHT_AXIS: [u8; 4] = *b"wght";

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

/// The bytes of a font file, shared by every face, instance and rasterizer made from it: a file read into memory
/// (`Arc<Vec<u8>>`), or bytes the program itself holds (`Arc<&'static [u8]>`, a built-in font), which are never copied.
pub type FontData = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// A font instance as bytes: the file, the face's index in it and the variation coordinates the instance sets.
#[derive(Clone)]
pub struct FontSource {
    pub data: FontData,
    pub index: u32,
    pub variations: Vec<FontVariation>,
}

impl fmt::Debug for FontSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontSource")
            .field("bytes", &(*self.data).as_ref().len())
            .field("index", &self.index)
            .field("variations", &self.variations)
            .finish()
    }
}

struct Face {
    family: Arc<str>,
    data: FontData,
    index: u32,
    weight: FontWeight,
    style: FontStyle,
    font: Font,
    weight_range: Option<(f32, f32)>,
    /// The face has colour glyphs (COLR, CBDT or sbix).
    colour: bool,
}

pub(crate) struct Instance {
    face: FaceId,
    variations: Vec<FontVariation>,
    pub(crate) font: Font,
    metrics: FontMetrics,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct InstanceKey {
    face: FaceId,
    variations: Vec<([u8; 4], u32)>,
}

/// The fonts the text engine can use: faces registered from font files, and the face instances (a face at a set of
/// variation coordinates) that layout and rasterization name by [`FontId`].
///
/// One index serves every thread that lays out or draws text, so it only grows: a face, an instance or a fallback list,
/// once made, never changes, and the lock of each table is held only to look an entry up or put one in, never while a
/// font file is read, a face parsed or a list worked out. A panic anywhere leaves every table whole for the others.
#[derive(Default)]
pub struct FontIndex {
    faces: RwLock<Vec<Arc<Face>>>,
    instances: RwLock<Instances>,
    /// Kept per font, weight and the number of faces the list was made from: a face added later makes a new list.
    fallbacks: RwLock<HashMap<FallbackKey, Arc<[FontId]>>>,
    /// Asked from any thread with no lock held, the answer kept under the locks below; it keeps nothing the index
    /// relies on.
    system: Option<Box<dyn SystemFonts>>,
    /// The installed faces asked for so far, and the face each became (`None` for a file that did not load).
    system_faces: RwLock<HashMap<FontFile, Option<FaceId>>>,
    /// Characters and families the installed fonts were asked for and had nothing new for.
    system_misses: RwLock<HashSet<SystemQuery>>,
}

/// A font, a weight and how many faces there were when its fallback list was made.
type FallbackKey = (FontId, FontWeight, usize);

#[derive(Default)]
struct Instances {
    list: Vec<Arc<Instance>>,
    ids: HashMap<InstanceKey, FontId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SystemQuery {
    Char(char, Option<Box<str>>),
    Family(Box<str>, FontWeight, FontStyle),
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
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
        data: FontData,
        index: u32,
    ) -> Result<FaceId, TextError> {
        let face = parse_face(family, weight, style, data, index)?;
        let faces = self.faces.get_mut().unwrap_or_else(PoisonError::into_inner);
        faces.push(Arc::new(face));
        Ok(FaceId(faces.len() as u32 - 1))
    }

    /// Registers every face of a font file or collection under the family, weight and style its own tables name. A face
    /// without a family name is skipped. Returns the faces added.
    pub fn add_font(&mut self, data: FontData) -> Result<Vec<FaceId>, TextError> {
        let bytes: &[u8] = (*data).as_ref();
        let file = FontDataRef::new(bytes)
            .ok_or_else(|| TextError::InvalidFont("not a TrueType or OpenType file".into()))?;
        // a collection's header states its face count; each face needs a 4-byte offset, so no more fit in the file
        let named: Vec<_> = (0..file.len().min(bytes.len() / 4) as u32)
            .map_while(|index| file.get(index as usize).map(|_| index))
            .filter_map(|index| named_face(bytes, index).map(|named| (named, index)))
            .collect();
        named
            .into_iter()
            .map(|((family, weight, style), index)| {
                self.add_face(&family, weight, style, Arc::clone(&data), index)
            })
            .collect()
    }

    /// Lets the index load installed fonts: a family no registered face has, and, after every registered face, a face
    /// for a character none of them draws. Installed faces load once, when first needed.
    pub fn set_system_fonts(&mut self, system: Box<dyn SystemFonts>) {
        self.system = Some(system);
        self.system_misses
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// The installed fonts' answer to `ask`; `None` without installed fonts.
    fn ask_system(
        &self,
        ask: impl FnOnce(&dyn SystemFonts) -> Option<FontFile>,
    ) -> Option<FontFile> {
        ask(self.system.as_deref()?)
    }

    /// The installed face of `family`, loaded on first use; `None` without installed fonts or such a family.
    pub(crate) fn system_family(
        &self,
        family: &str,
        weight: FontWeight,
        style: FontStyle,
    ) -> Option<FaceId> {
        let query = SystemQuery::Family(family.into(), weight, style);
        if read(&self.system_misses).contains(&query) {
            return None;
        }
        self.system.as_ref()?;
        let file = self.ask_system(|system| system.family(family, weight, style));
        let face = file.and_then(|file| self.load_system(file));
        if face.is_none() {
            write(&self.system_misses).insert(query);
        }
        face
    }

    /// An installed face that draws `ch` and was not loaded before, loaded now; `None` when the system has nothing new.
    pub(crate) fn system_covering(
        &self,
        ch: char,
        lang: Option<&str>,
        weight: FontWeight,
        style: FontStyle,
    ) -> Option<FaceId> {
        let query = SystemQuery::Char(ch, lang.map(Into::into));
        if read(&self.system_misses).contains(&query) {
            return None;
        }
        self.system.as_ref()?;
        let file = self.ask_system(|system| system.covering(ch, lang, weight, style));
        let face = file
            .filter(|file| !read(&self.system_faces).contains_key(file))
            .and_then(|file| self.load_system(file));
        if face.is_none() {
            write(&self.system_misses).insert(query);
        }
        face
    }

    fn load_system(&self, file: FontFile) -> Option<FaceId> {
        if let Some(face) = read(&self.system_faces).get(&file) {
            return *face;
        }
        let parsed = read_font(&file.path).ok().and_then(|data| {
            let (family, weight, style) = named_face(&data, file.index)?;
            parse_face(
                &family,
                weight,
                style,
                Arc::new(data) as FontData,
                file.index,
            )
            .ok()
        });
        // another thread may have loaded the file meanwhile: its face stands
        let mut system_faces = write(&self.system_faces);
        if let Some(face) = system_faces.get(&file) {
            return *face;
        }
        let face = parsed.map(|face| {
            let mut faces = write(&self.faces);
            faces.push(Arc::new(face));
            FaceId(faces.len() as u32 - 1)
        });
        system_faces.insert(file, face);
        face
    }

    pub fn family(&self, face: FaceId) -> Option<Arc<str>> {
        self.face(face).map(|f| Arc::clone(&f.family))
    }

    pub fn face_count(&self) -> usize {
        read(&self.faces).len()
    }

    fn face(&self, face: FaceId) -> Option<Arc<Face>> {
        read(&self.faces).get(face.0 as usize).cloned()
    }

    /// The face that best matches `family`, `weight` and `style`, by family name (case-insensitive), then the style,
    /// then the nearest weight. A variable face matches any weight inside its range.
    pub fn match_face(&self, family: &str, weight: FontWeight, style: FontStyle) -> Option<FaceId> {
        read(&self.faces)
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
        &self,
        face: FaceId,
        weight: FontWeight,
        variations: &[FontVariation],
    ) -> Result<FontId, TextError> {
        let entry = self.face(face).ok_or(TextError::NoFont)?;
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
        if let Some(id) = read(&self.instances).ids.get(&key) {
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
        let made = Arc::new(Instance {
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
        // another thread may have made the same instance meanwhile: its id stands
        let mut instances = write(&self.instances);
        if let Some(id) = instances.ids.get(&key) {
            return Ok(*id);
        }
        let id = FontId(instances.list.len() as u32);
        instances.list.push(made);
        instances.ids.insert(key, id);
        Ok(id)
    }

    /// The fonts that stand in for `font` where it has no glyph: every other face in load order, at `weight`. Kept per
    /// font, weight and the faces there were.
    pub(crate) fn fallbacks(
        &self,
        font: FontId,
        weight: FontWeight,
    ) -> Result<Arc<[FontId]>, TextError> {
        let count = self.face_count();
        if let Some(fonts) = read(&self.fallbacks).get(&(font, weight, count)) {
            return Ok(Arc::clone(fonts));
        }
        let own = self.face_of(font)?;
        let fonts: Arc<[FontId]> = (0..count as u32)
            .map(FaceId)
            .filter(|face| *face != own)
            .map(|face| self.instance(face, weight, &[]))
            .collect::<Result<_, _>>()?;
        Ok(Arc::clone(
            write(&self.fallbacks)
                .entry((font, weight, count))
                .or_insert(fonts),
        ))
    }

    pub fn face_of(&self, font: FontId) -> Result<FaceId, TextError> {
        self.instance_entry(font).map(|instance| instance.face)
    }

    /// The font file and variation coordinates behind `font`, for a rasterizer.
    pub fn source(&self, font: FontId) -> Result<FontSource, TextError> {
        let instance = self.instance_entry(font)?;
        let face = self.face(instance.face).ok_or(TextError::NoFont)?;
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
        Ok(self.face(face).ok_or(TextError::NoFont)?.colour)
    }

    /// Whether the font has a glyph for `ch`.
    pub fn has_glyph(&self, font: FontId, ch: char) -> Result<bool, TextError> {
        self.instance_entry(font)
            .map(|instance| instance.font.charmap().map_unicode(ch).is_some())
    }

    /// The instance behind `font`, with its shaping font.
    pub(crate) fn instance_entry(&self, font: FontId) -> Result<Arc<Instance>, TextError> {
        read(&self.instances)
            .list
            .get(font.0 as usize)
            .cloned()
            .ok_or(TextError::UnknownFont(font))
    }
}

/// A face of `family` from the font at `index` in `data`, with what the index keeps of it.
fn parse_face(
    family: &str,
    weight: FontWeight,
    style: FontStyle,
    data: FontData,
    index: u32,
) -> Result<Face, TextError> {
    let font = Font::new(harfrust::font::Blob::from(Arc::clone(&data)), index)
        .ok_or_else(|| TextError::InvalidFont(family.into()))?;
    let weight_range = font
        .axes()
        .get_by_tag(Tag::new(&WEIGHT_AXIS))
        .map(|axis| (axis.min_value(), axis.max_value()));
    let colour = swash::FontRef::from_index((*data).as_ref(), index as usize).is_some_and(|font| {
        [b"COLR", b"CBDT", b"sbix"]
            .into_iter()
            .any(|tag| font.table(swash::tag_from_bytes(tag)).is_some())
    });
    Ok(Face {
        family: family.into(),
        data,
        index,
        weight,
        style,
        font,
        weight_range,
        colour,
    })
}

/// The family, weight and style the name and OS/2 tables of face `index` in `data` give; `None` without a family name.
fn named_face(data: &[u8], index: u32) -> Option<(String, FontWeight, FontStyle)> {
    let font = FontDataRef::new(data)?.get(index as usize)?;
    let name = |id| {
        font.localized_strings()
            .find(|s| s.id() == id)
            .map(|s| s.to_string())
    };
    let family = name(StringId::TypographicFamily).or_else(|| name(StringId::Family))?;
    let attributes = font.attributes();
    let style = match attributes.style() {
        Style::Normal => FontStyle::Normal,
        _ => FontStyle::Italic,
    };
    Some((family, FontWeight(attributes.weight().0), style))
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
