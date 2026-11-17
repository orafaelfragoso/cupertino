//! Text rendering with the fonts of the asset packs.
//!
//! Faces are read from the asset packs' font files and the built-in font; no host font is ever loaded. The family comes
//! from the packs' font role. The tracking table of the design font
//! is applied as additional advance per glyph through [`TextRenderer::draw`]'s `letter_spacing_px`.
//!
//! The `ui` family is one variable font (axes `wdth`, `opsz`, `GRAD`, `wght`), so a weight is a variation coordinate, not a separate face. Shaping and rasterisation both
//! run through `swash` with the coordinates of the request: advances change with `wght` and with `opsz`, so
//! the coordinates must reach the shaper and not only the rasteriser.
//!
//! * Weight: the CSS-style weight (100..=900) is mapped through the font's own named instances at the default of
//!   every other axis, interpolated linearly between them. An instance stands for the CSS weight of its `wght`
//!   coordinate when the axis uses the standard 1..=1000 scale; otherwise its style name gives it (the CSS names Thin
//!   100 .. Black 900 and their usual synonyms, case-insensitive). A family of static faces uses each face's
//!   `usWeightClass`.
//! * Optical size: the design selects `opsz` from the point size and clamps it to the axis range
//!   (17..=96; section 1), so 13 pt text uses the 17 master. The
//!   renderer takes device pixels; the point size is `px / scale`, so the optical size is applied once the
//!   owner calls [`TextRenderer::set_scale`]. Without it the axis stays at its default (the display design).
//!
//! Glyphs are rasterised by [`crate::glyph`] with the design's dilation (stem darkening), which
//! depends on the size and on the fill colour's luminance, while [`TextRenderer::set_smoothing`] is on (the default,
//! [`DEFAULT_SMOOTHING`]), and their origins quantised to the subpixel positions of the size. Coverage is blended source-over on the premultiplied destination values, with no gamma, contrast
//! or lookup table.
//!
//! Limits: no autohinting (the design's 7-18 px vertical warp, is optional and not built); runs are shaped
//! left to right (no bidirectional reordering) and a character missing from the
//! family is taken from the first other loaded face that has it.

use crate::draws::{composite_glyphs, PlacedGlyph as PlacedImage};
use crate::glyph::{self, Mask, Smoothing};
use crate::palette::Rgba;
use cupertino_assets::{AssetError, Assets, FontRole, FontSource};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;
use swash::scale::ScaleContext;
use swash::shape::ShapeContext;
use swash::text::{Properties, Script};
use swash::{CacheKey, FontDataRef, FontRef, GlyphId, Setting, StringId, Style, Tag};
use tiny_skia::Pixmap;

const WGHT: Tag = swash::tag_from_bytes(b"wght");
const OPSZ: Tag = swash::tag_from_bytes(b"opsz");

/// CSS weights of the standard style names and their usual synonyms, compared in lower case without spaces, hyphens
/// or underscores. The fallback for instances of a `wght` axis that does not use the 1..=1000 scale.
const WEIGHT_NAMES: [(&str, u16); 15] = [
    ("thin", 100),
    ("hairline", 100),
    ("extralight", 200),
    ("ultralight", 200),
    ("light", 300),
    ("regular", 400),
    ("normal", 400),
    ("medium", 500),
    ("semibold", 600),
    ("demibold", 600),
    ("bold", 700),
    ("extrabold", 800),
    ("ultrabold", 800),
    ("black", 900),
    ("heavy", 900),
];

/// CSS weight a named instance stands for: its `wght` coordinate when the axis spans only the standard 1..=1000 scale,
/// else the weight of its style name; `None` for a name outside [`WEIGHT_NAMES`] on a non-standard axis.
/// An axis whose minimum is below 1 (for example one starting at 0) counts as non-standard and falls back to the names.
fn instance_weight(wght: f32, axis_range: (f32, f32), name: Option<&str>) -> Option<u16> {
    if axis_range.0 >= 1.0 && axis_range.1 <= 1000.0 {
        return Some(wght.round() as u16);
    }
    let key: String = name?
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect();
    WEIGHT_NAMES
        .iter()
        .find(|(n, _)| *n == key)
        .map(|(_, w)| *w)
}

/// One face of a loaded font file.
struct Face {
    data: Arc<[u8]>,
    offset: u32,
    key: CacheKey,
    /// Family names (name IDs 1 and 16).
    families: Vec<String>,
    /// OS/2 weight class, for faces without a `wght` axis.
    class_weight: u16,
    /// `(css weight, wght value)` of the named instances; empty when the face has no `wght` axis.
    weight_stops: Vec<(u16, f32)>,
}

impl Face {
    fn font(&self) -> FontRef<'_> {
        FontRef {
            data: &self.data,
            offset: self.offset,
            key: self.key,
        }
    }

    fn is_variable(&self) -> bool {
        !self.weight_stops.is_empty()
    }

    /// `wght` axis value for a CSS weight, interpolated between the named instances and clamped to them.
    fn wght(&self, css: u16) -> f32 {
        let css = f32::from(css);
        let stops = &self.weight_stops;
        let (first, last) = (stops[0], stops[stops.len() - 1]);
        if css <= f32::from(first.0) {
            return first.1;
        }
        for pair in stops.windows(2) {
            let ((c0, w0), (c1, w1)) = (pair[0], pair[1]);
            if css <= f32::from(c1) {
                return w0 + (w1 - w0) * (css - f32::from(c0)) / f32::from(c1 - c0);
            }
        }
        last.1
    }
}

/// Reads every face of a font file. Fails when the file is not a font.
fn load_faces(data: Arc<[u8]>, label: &str) -> Result<Vec<Face>, AssetError> {
    let not_a_font = || AssetError::Font {
        reason: format!("cannot load font {label}: not a TrueType or OpenType file"),
    };
    let file = FontDataRef::new(&data[..]).ok_or_else(not_a_font)?;
    let mut faces = Vec::new();
    for i in 0..file.len() {
        let font = file.get(i).ok_or_else(not_a_font)?;
        let families = font
            .localized_strings()
            .filter(|s| matches!(s.id(), StringId::Family | StringId::TypographicFamily))
            .map(|s| s.to_string())
            .collect();
        let normal = font.attributes().style() == Style::Normal;
        faces.push((
            font.offset,
            font.key,
            families,
            font.attributes().weight().0,
            weight_stops(&font),
            normal,
        ));
    }
    Ok(faces
        .into_iter()
        .filter(|f| f.5)
        .map(
            |(offset, key, families, class_weight, weight_stops, _)| Face {
                data: Arc::clone(&data),
                offset,
                key,
                families,
                class_weight,
                weight_stops,
            },
        )
        .collect())
}

/// Named instances at the default value of every axis but `wght`, as `(css weight, wght value)` sorted by weight
/// ([`instance_weight`]); the first instance of a weight wins.
fn weight_stops(font: &FontRef<'_>) -> Vec<(u16, f32)> {
    let axes: Vec<_> = font.variations().collect();
    let Some(wght_axis) = axes.iter().position(|a| a.tag() == WGHT) else {
        return Vec::new();
    };
    let range = (axes[wght_axis].min_value(), axes[wght_axis].max_value());
    let mut stops: Vec<(u16, f32)> = Vec::new();
    for inst in font.instances() {
        let values: Vec<f32> = inst.values().collect();
        let on_default = axes
            .iter()
            .zip(&values)
            .enumerate()
            .all(|(i, (axis, v))| i == wght_axis || (axis.default_value() - v).abs() < 1e-3);
        if !on_default {
            continue;
        }
        let name = inst.name(None).map(|n| n.to_string());
        if let Some(css) = instance_weight(values[wght_axis], range, name.as_deref()) {
            if stops.iter().all(|s| s.0 != css) {
                stops.push((css, values[wght_axis]));
            }
        }
    }
    stops.sort_by_key(|s| s.0);
    stops
}

/// One shaped glyph: the face it comes from, its id, and its pen position relative to the line start.
struct PlacedGlyph {
    face: usize,
    id: GlyphId,
    x: f32,
    y: f32,
}

/// Result of shaping one line.
struct Line {
    glyphs: Vec<PlacedGlyph>,
    width: f32,
    /// Each cluster's source range (bytes of the line's text), its pen position and the glyphs drawn before it.
    clusters: Vec<(std::ops::Range<usize>, f32, usize)>,
}

/// Key of a rasterised glyph: face, glyph, size, variation coordinates, subpixel bins and dilation, all as exact bits.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    face: usize,
    id: GlyphId,
    px_bits: u32,
    wght_bits: u32,
    opsz_bits: u32,
    x_bin: u8,
    y_bin: u8,
    dilation_bits: u32,
}

/// Dilation of a new renderer: on, smoothing allowed and requested with the preference unset, with no high-density gate.
pub const DEFAULT_SMOOTHING: Smoothing = Smoothing::On;

pub struct TextRenderer {
    faces: Vec<Face>,
    family: Option<String>,
    scale: Option<f32>,
    shape_cx: ShapeContext,
    scale_cx: ScaleContext,
    smoothing: Smoothing,
    /// The text is the design's declarative toolkit's: its dilation size is floored.
    declarative: bool,
    images: HashMap<GlyphKey, Option<(u64, Rc<Mask>)>>,
    /// Glyph images rasterized into `images`.
    rasterized: u64,
}

impl TextRenderer {
    /// Load the fonts of `assets` and resolve the family of `role`: the family the packs assign to it, else the first
    /// usable family of the loaded faces (a variable font without named weight instances is not usable: its weight
    /// scale cannot be derived). A font file that cannot be loaded is skipped with a message on stderr, as is a
    /// fallback away from the assigned family. Fails only when no usable face is left.
    pub fn new(assets: &Assets, role: FontRole) -> Result<Self, AssetError> {
        let mut faces = Vec::new();
        for source in assets.fonts() {
            let data: Result<Arc<[u8]>, AssetError> = match &source {
                FontSource::File(path) => {
                    std::fs::read(path)
                        .map(Arc::from)
                        .map_err(|e| AssetError::Font {
                            reason: format!("cannot load font {}: {e}", path.display()),
                        })
                }
                FontSource::Builtin => Ok(Arc::from(cupertino_assets::BUILTIN_FONT)),
            };
            let loaded = data.and_then(|data| load_faces(data, &source.label()));
            match loaded {
                Ok(f) => faces.extend(f),
                Err(e) => eprintln!("cupertino-decorations: skipping a font: {e}"),
            }
        }
        let usable = |family: &str| {
            let in_family: Vec<&Face> = faces
                .iter()
                .filter(|f| f.families.iter().any(|n| n == family))
                .collect();
            !in_family.is_empty()
                && !in_family
                    .iter()
                    .any(|f| f.font().variations().any(|a| a.tag() == WGHT) && !f.is_variable())
        };
        let wanted = assets.font_family(role);
        let family = wanted
            .into_iter()
            .chain(faces.iter().flat_map(|f| f.families.iter().map(String::as_str)))
            .find(|f| usable(f))
            .map(str::to_string)
            .ok_or_else(|| AssetError::Font { reason: format!("no usable font for role {role:?}: no loaded face has named weights or static faces") })?;
        if let Some(w) = wanted.filter(|w| *w != family) {
            eprintln!("cupertino-decorations: the family {w:?} of font role {role:?} is not usable; drawing with {family:?}");
        }
        Ok(Self {
            faces,
            family: Some(family),
            scale: None,
            shape_cx: ShapeContext::new(),
            scale_cx: ScaleContext::new(),
            smoothing: DEFAULT_SMOOTHING,
            declarative: false,
            images: HashMap::new(),
            rasterized: 0,
        })
    }

    /// A renderer without fonts, for a decoration that draws no text. It shapes nothing: every width is 0.
    pub fn without_fonts() -> Self {
        Self {
            faces: Vec::new(),
            family: None,
            scale: None,
            shape_cx: ShapeContext::new(),
            scale_cx: ScaleContext::new(),
            smoothing: DEFAULT_SMOOTHING,
            declarative: false,
            images: HashMap::new(),
            rasterized: 0,
        }
    }

    /// Glyph images rasterized so far. Text drawn again at the same size, weight and subpixel position rasterizes
    /// nothing: its glyphs come from the cache.
    pub fn glyphs_rasterized(&self) -> u64 {
        self.rasterized
    }

    /// Resolved family name; `None` for a renderer without fonts.
    pub fn family(&self) -> Option<&str> {
        self.family.as_deref()
    }

    /// Output scale (device pixels per point) of the surface this renderer draws to. It turns the pixel size of
    /// a request into the point size that selects the optical size; call it again when the scale changes.
    /// Until it is called the optical size stays at the axis default.
    pub fn set_scale(&mut self, scale: f32) {
        assert!(scale > 0.0, "output scale must be positive, got {scale}");
        self.scale = Some(scale);
    }

    /// Whether the text drawn from now on is dilated: [`DEFAULT_SMOOTHING`] until a preference turns
    /// smoothing off.
    pub fn set_smoothing(&mut self, smoothing: Smoothing) {
        self.smoothing = smoothing;
    }

    /// Draw as the design's declarative toolkit does: the dilation takes the size floored to a power of 1.25.
    /// For the surfaces whose reference counterpart is built with that toolkit; off by default.
    pub fn set_declarative(&mut self, declarative: bool) {
        self.declarative = declarative;
    }

    /// Weight drawn for a requested one: the request clamped to 100..=900 for a variable family (any weight
    /// in between is rendered), the weight of the nearest face for a family of static faces.
    pub fn effective_weight(&self, wanted: u16) -> u16 {
        match self.primary(wanted) {
            Some(i) if !self.faces[i].is_variable() => self.faces[i].class_weight,
            _ => wanted.clamp(100, 900),
        }
    }

    /// Face of the resolved family for a requested weight: the variable face when there is one, else the
    /// static face with the nearest weight class.
    fn primary(&self, wanted: u16) -> Option<usize> {
        let name = self.family.as_deref()?;
        let mut own =
            (0..self.faces.len()).filter(|&i| self.faces[i].families.iter().any(|n| n == name));
        let candidates: Vec<usize> = own.by_ref().collect();
        candidates
            .iter()
            .copied()
            .find(|&i| self.faces[i].is_variable())
            .or_else(|| {
                candidates
                    .into_iter()
                    .min_by_key(|&i| self.faces[i].class_weight.abs_diff(wanted))
            })
    }

    /// Variation coordinates of a request for `face`: `wght` from the weight scale and, once the scale is
    /// known, `opsz` from the point size. Empty for a static face.
    fn settings(&self, face: usize, px: f32, weight: u16) -> Vec<Setting<f32>> {
        let face = &self.faces[face];
        if !face.is_variable() {
            return Vec::new();
        }
        let mut s = vec![Setting {
            tag: WGHT,
            value: face.wght(weight),
        }];
        if let Some(scale) = self.scale {
            s.push(Setting {
                tag: OPSZ,
                value: px / scale,
            });
        }
        s
    }

    /// Shape one line left to right. Characters the family lacks come from the first other face that has them.
    fn shape(&mut self, text: &str, px: f32, weight: u16) -> Line {
        let Some(primary) = self.primary(weight) else {
            return Line {
                glyphs: Vec::new(),
                width: 0.0,
                clusters: Vec::new(),
            };
        };
        let mut runs: Vec<(usize, String)> = Vec::new();
        for ch in text.chars() {
            let face = self.face_for(primary, ch);
            match runs.last_mut() {
                Some((f, s)) if *f == face => s.push(ch),
                _ => runs.push((face, ch.to_string())),
            }
        }
        let mut glyphs = Vec::new();
        let mut clusters = Vec::new();
        let mut pen = 0.0f32;
        let mut run_start = 0;
        for (face, run) in runs {
            let settings = self.settings(face, px, weight);
            let script = run
                .chars()
                .map(|c| Properties::from(c).script())
                .find(|s| !matches!(s, Script::Common | Script::Inherited))
                .unwrap_or(Script::Latin);
            let f = &self.faces[face];
            let font = FontRef {
                data: &f.data,
                offset: f.offset,
                key: f.key,
            };
            let mut shaper = self
                .shape_cx
                .builder(font)
                .script(script)
                .size(px)
                .variations(settings)
                .build();
            shaper.add_str(&run);
            shaper.shape_with(|cluster| {
                let source = cluster.source.to_range();
                clusters.push((
                    run_start + source.start..run_start + source.end,
                    pen,
                    glyphs.len(),
                ));
                for g in cluster.glyphs {
                    glyphs.push(PlacedGlyph {
                        face,
                        id: g.id,
                        x: pen + g.x,
                        y: g.y,
                    });
                    pen += g.advance;
                }
            });
            run_start += run.len();
        }
        Line {
            glyphs,
            width: pen,
            clusters,
        }
    }

    /// Face that draws `ch`: the primary one when it maps the character (or nothing else does), else the
    /// first face that does.
    fn face_for(&self, primary: usize, ch: char) -> usize {
        let has = |i: usize| self.faces[i].font().charmap().map(ch) != 0;
        if has(primary) {
            return primary;
        }
        (0..self.faces.len()).find(|&i| has(i)).unwrap_or(primary)
    }

    /// Advance width in pixels of `text` at `px` pixels per em with `letter_spacing_px` added
    /// between glyphs.
    pub fn measure(&mut self, text: &str, px: f32, weight: u16, letter_spacing_px: f32) -> f32 {
        let line = self.shape(text, px, weight);
        match line.glyphs.len() {
            0 => 0.0,
            n => line.width + (n - 1) as f32 * letter_spacing_px,
        }
    }

    /// The pen position in pixels at each of `boundaries` (ascending byte offsets of `text` on character
    /// boundaries), from one shaping pass: the position of the cluster that starts there, a share of the cluster's
    /// advance for a boundary inside one (a ligature), the line's width at the end. The same widths as
    /// [`TextRenderer::measure`] of each prefix give, short of kerning across the boundary.
    pub fn offsets(
        &mut self,
        text: &str,
        boundaries: &[usize],
        px: f32,
        weight: u16,
        letter_spacing_px: f32,
    ) -> Vec<f32> {
        let line = self.shape(text, px, weight);
        let spacing = |glyphs: usize| glyphs as f32 * letter_spacing_px;
        let end = line.width + spacing(line.glyphs.len().saturating_sub(1));
        let x_of = |i: usize| {
            line.clusters
                .get(i)
                .map_or(end, |(_, x, before)| x + spacing(*before))
        };
        let mut i = 0;
        boundaries
            .iter()
            .map(|&b| {
                while line
                    .clusters
                    .get(i)
                    .is_some_and(|(range, _, _)| range.end <= b)
                {
                    i += 1;
                }
                match line.clusters.get(i) {
                    None => end,
                    Some((range, _, _)) if b <= range.start => x_of(i),
                    Some((range, _, _)) => {
                        let (here, next) = (x_of(i), x_of(i + 1));
                        here + (next - here) * (b - range.start) as f32 / range.len() as f32
                    }
                }
            })
            .collect()
    }

    /// Longest prefix of `text` that fits in `max_px` once an ellipsis is appended; the text itself
    /// when it fits. Returns the string and its width.
    pub fn fit(
        &mut self,
        text: &str,
        px: f32,
        weight: u16,
        letter_spacing_px: f32,
        max_px: f32,
    ) -> (String, f32) {
        let full = self.measure(text, px, weight, letter_spacing_px);
        if full <= max_px {
            return (text.to_string(), full);
        }
        let chars: Vec<char> = text.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let s: String = chars[..mid]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_string()
                + "…";
            if self.measure(&s, px, weight, letter_spacing_px) <= max_px {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let s: String = chars[..lo]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_string()
            + "…";
        let w = self.measure(&s, px, weight, letter_spacing_px);
        if w > max_px {
            return (String::new(), 0.0);
        }
        (s, w)
    }

    /// Draw `text` with its left edge at `x` and its baseline at `baseline` (device pixels).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pm: &mut Pixmap,
        text: &str,
        x: f32,
        baseline: f32,
        px: f32,
        weight: u16,
        letter_spacing_px: f32,
        color: Rgba,
    ) {
        let glyphs = self.place(text, x, baseline, px, weight, letter_spacing_px, color);
        composite_glyphs(pm, &glyphs, color);
    }

    /// The coverage images of `text` with its left edge at `x` and its baseline at `baseline` (device pixels), each at
    /// the whole pixel [`TextRenderer::draw`] lays it, rasterized once per glyph, size, weight and subpixel position:
    /// what drawing the text in `color` (which sets the dilation) lays down.
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &mut self,
        text: &str,
        x: f32,
        baseline: f32,
        px: f32,
        weight: u16,
        letter_spacing_px: f32,
        color: Rgba,
    ) -> Vec<PlacedImage> {
        let line = self.shape(text, px, weight);
        let mut placed = Vec::with_capacity(line.glyphs.len());
        // the whole-pixel origin is truncated and its fraction quantised to the size's positions (truncating
        // the fraction too is ours, approximate)
        let (qx, qy) = glyph::quantization(px);
        // the dilation follows the fill colour's luminance
        let size = if self.declarative {
            glyph::declarative_size(px)
        } else {
            px
        };
        let dilation = self.smoothing.dilation(
            size,
            glyph::quantized_luminance([color[0], color[1], color[2]]),
        );
        for (i, g) in line.glyphs.iter().enumerate() {
            let pos = x + g.x + i as f32 * letter_spacing_px;
            let (whole, x_bin) = (
                pos.floor() as i32,
                ((pos - pos.floor()) * f32::from(qx)).floor() as u8,
            );
            let origin_y = baseline - g.y;
            let (whole_y, y_bin) = (
                origin_y.floor() as i32,
                ((origin_y - origin_y.floor()) * f32::from(qy)).floor() as u8,
            );
            let settings = self.settings(g.face, px, weight);
            let key = GlyphKey {
                face: g.face,
                id: g.id,
                px_bits: px.to_bits(),
                wght_bits: settings
                    .iter()
                    .find(|s| s.tag == WGHT)
                    .map_or(0, |s| s.value.to_bits()),
                opsz_bits: settings
                    .iter()
                    .find(|s| s.tag == OPSZ)
                    .map_or(0, |s| s.value.to_bits()),
                x_bin,
                y_bin,
                dilation_bits: dilation.to_bits(),
            };
            let faces = &self.faces;
            let scale_cx = &mut self.scale_cx;
            let rasterized = &mut self.rasterized;
            let img = self.images.entry(key).or_insert_with(|| {
                *rasterized += 1;
                let f = &faces[g.face];
                let font = FontRef {
                    data: &f.data,
                    offset: f.offset,
                    key: f.key,
                };
                // Unhinted: hinting programs of a variable font describe its default instance only.
                let mut scaler = scale_cx
                    .builder(font)
                    .size(px)
                    .hint(false)
                    .variations(settings)
                    .build();
                let outline = scaler.scale_outline(g.id)?;
                // a y-down origin lower by the fraction is a y-up offset down
                let offset = (
                    f32::from(x_bin) / f32::from(qx),
                    -f32::from(y_bin) / f32::from(qy),
                );
                let mask = glyph::rasterize(
                    outline.points(),
                    outline.verbs(),
                    offset,
                    (dilation, dilation),
                )?;
                // the face by its cache key, which no other loaded face shares
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (f.key.value(), key).hash(&mut h);
                Some((h.finish(), Rc::new(mask)))
            });
            let Some((id, img)) = img.as_ref() else {
                continue;
            };
            placed.push(PlacedImage {
                x: whole + img.left,
                y: whole_y - img.top,
                key: *id,
                mask: img.clone(),
            });
        }
        placed
    }
}
