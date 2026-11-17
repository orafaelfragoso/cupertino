use crate::bidi;
use crate::paragraph::Item;
use crate::raster::{SharedRasterStats, SharedRasters};
use crate::{
    AttributedText, Direction, FaceId, FontId, FontIndex, FontMetrics, FontStyle, Paragraph,
    ParagraphStyle, ShapeRequest, ShapedRun, TextError, TextStyle,
};
use cupertino_geometry::Size;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use unicode_bidi::{Level, ParagraphBidiInfo};
use unicode_segmentation::UnicodeSegmentation;

/// Paragraphs shaped before the cache starts a new generation.
const CACHE_LIMIT: usize = 512;
const ELLIPSIS: &str = "\u{2026}";

type CacheEntry = Arc<(AttributedText, ParagraphStyle, Paragraph)>;

/// Two generations: a full `current` becomes `previous` and the old `previous` is dropped, so a hit in either keeps the
/// paragraph and a view of more than the limit still reuses its recent ones.
#[derive(Default)]
struct Paragraphs {
    current: HashMap<u64, CacheEntry>,
    previous: HashMap<u64, CacheEntry>,
}

/// The fonts, the paragraph cache and the glyph rasters of one app, or of every instance of a process that shares
/// them. Methods take `&self`, so one `Arc<TextSystem>` serves measuring and painting on any thread, and every
/// renderer's rasterizer on it shares the rasters.
///
/// Every cache in it only takes whole entries that never change afterwards, and its lock is held only to look one up
/// or put one in: text is shaped, and glyphs drawn, outside every lock, so a panic while shaping or drawing leaves the
/// caches whole for every other user.
///
/// The cache keeps the plain text of every recent paragraph, so text that must not linger, a password above all, is
/// never shaped: a secure field shapes only its mask.
pub struct TextSystem {
    theme_family: Arc<str>,
    fonts: FontIndex,
    paragraphs: Mutex<Paragraphs>,
    /// The glyph rasters every rasterizer on this system shares.
    rasters: SharedRasters,
}

impl fmt::Debug for TextSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextSystem")
            .field("theme_family", &self.theme_family)
            .finish_non_exhaustive()
    }
}

impl TextSystem {
    /// `theme_family` is the family a style without one takes. A family no face matches falls back to the first
    /// registered face.
    pub fn new(fonts: FontIndex, theme_family: &str) -> Self {
        Self {
            theme_family: theme_family.into(),
            fonts,
            paragraphs: Mutex::default(),
            rasters: SharedRasters::default(),
        }
    }

    pub(crate) fn rasters(&self) -> &SharedRasters {
        &self.rasters
    }

    /// What the glyph rasters shared by this system's rasterizers hold and did.
    pub fn shared_rasters(&self) -> SharedRasterStats {
        self.rasters.stats()
    }

    fn paragraphs(&self) -> MutexGuard<'_, Paragraphs> {
        self.paragraphs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Shapes `text`, or returns the paragraph shaped for an equal request before.
    pub fn paragraph(
        &self,
        text: &AttributedText,
        style: &ParagraphStyle,
    ) -> Result<Paragraph, TextError> {
        check_values(text)?;
        let key = cache_key(text, style);
        {
            let mut cache = self.paragraphs();
            if let Some(entry) = lookup(&cache.current, key, text, style) {
                return Ok(entry.2.clone());
            }
            if let Some(entry) = lookup(&cache.previous, key, text, style) {
                let paragraph = entry.2.clone();
                insert(&mut cache, key, entry);
                return Ok(paragraph);
            }
        }
        let paragraph = shape_paragraph(&self.fonts, &self.theme_family, text, style)?;
        crate::TextWork::record(|work| work.shaped += 1);
        let entry = Arc::new((text.clone(), *style, paragraph.clone()));
        insert(&mut self.paragraphs(), key, entry);
        Ok(paragraph)
    }

    /// How many paragraphs the cache holds: two generations of at most 512 each, however many are shaped, so work that
    /// shapes text ahead of the frame that lays it out (a window's idle time) cannot grow it.
    pub fn cached_paragraphs(&self) -> usize {
        let cache = self.paragraphs();
        cache.current.len() + cache.previous.len()
    }

    /// The size of `text` laid out within `max_width`.
    pub fn measure(
        &self,
        text: &AttributedText,
        style: &ParagraphStyle,
        max_width: Option<f32>,
    ) -> Result<Size, TextError> {
        Ok(self.paragraph(text, style)?.layout(max_width).size())
    }

    /// The font file and coordinates behind a [`FontId`] that layout produced.
    pub fn font_source(&self, font: crate::FontId) -> Result<crate::FontSource, TextError> {
        self.fonts.source(font)
    }

    pub fn metrics(&self, style: &TextStyle) -> Result<FontMetrics, TextError> {
        let font = resolve(&self.fonts, &self.theme_family, style)?;
        self.fonts.metrics(font)
    }
}

fn lookup(
    generation: &HashMap<u64, CacheEntry>,
    key: u64,
    text: &AttributedText,
    style: &ParagraphStyle,
) -> Option<CacheEntry> {
    generation
        .get(&key)
        .filter(|entry| entry.0 == *text && entry.1 == *style)
        .cloned()
}

/// Puts `entry` in the current generation, which first becomes the previous one when it is full.
fn insert(cache: &mut Paragraphs, key: u64, entry: CacheEntry) {
    if cache.current.len() >= CACHE_LIMIT {
        cache.previous = std::mem::take(&mut cache.current);
    }
    cache.current.insert(key, entry);
}

fn check_values(text: &AttributedText) -> Result<(), TextError> {
    let valid = text.runs.iter().all(|(_, style)| {
        style.size.is_finite()
            && style.size > 0.0
            && style.letter_spacing.is_finite()
            && style
                .variations
                .iter()
                .all(|variation| variation.value.is_finite())
    });
    if valid {
        Ok(())
    } else {
        Err(TextError::InvalidValue)
    }
}

fn resolve(
    fonts: &FontIndex,
    theme_family: &str,
    style: &TextStyle,
) -> Result<crate::FontId, TextError> {
    let family = style.family.as_deref().unwrap_or(theme_family);
    let face = fonts
        .match_face(family, style.weight, style.style)
        .or_else(|| fonts.system_family(family, style.weight, style.style))
        .or_else(|| (fonts.face_count() > 0).then_some(FaceId(0)))
        .ok_or(TextError::NoFont)?;
    fonts.instance(face, style.weight, &style.variations)
}

fn shape_paragraph(
    fonts: &FontIndex,
    theme_family: &str,
    text: &AttributedText,
    paragraph_style: &ParagraphStyle,
) -> Result<Paragraph, TextError> {
    let segments = style_segments(text)?;
    let direction = bidi::base_direction(&text.text, paragraph_style.direction);
    let mut items = Vec::new();
    let mut line_start = 0;
    loop {
        let (line_end, next_start) = next_line(&text.text, line_start);
        let base = bidi::base_level(bidi::line_direction(
            &text.text[line_start..line_end],
            paragraph_style.direction,
            direction,
        ));
        let bidi = ParagraphBidiInfo::new(&text.text[line_start..line_end], Some(base));
        let pieces = pieces_of(&segments, line_start..line_end);
        for (piece, style) in pieces {
            let primary = resolve(fonts, theme_family, style)?;
            for (piece, level) in level_runs(&text.text, piece, &bidi.levels[..], line_start, base)
            {
                let direction = if level.is_rtl() {
                    Direction::Rtl
                } else {
                    Direction::Ltr
                };
                let runs = if piece.is_empty() {
                    vec![(piece, primary)]
                } else {
                    font_runs(fonts, &text.text, piece, primary, style)?
                };
                for (range, font) in runs {
                    let metrics = fonts.metrics(font)?;
                    let mut run = if range.is_empty() {
                        ShapedRun {
                            font,
                            size: style.size,
                            direction,
                            glyphs: Vec::new(),
                        }
                    } else {
                        fonts.shape(
                            &text.text[range.clone()],
                            &ShapeRequest {
                                font,
                                size: style.size,
                                direction,
                                features: &style.features,
                                lang: style.lang.as_deref(),
                                letter_spacing: style.letter_spacing,
                            },
                        )?
                    };
                    for glyph in &mut run.glyphs {
                        glyph.cluster += range.start;
                    }
                    let ellipsis = if paragraph_style.ellipsis {
                        let mark = if covers(fonts, font, ELLIPSIS)? {
                            ELLIPSIS
                        } else {
                            "..."
                        };
                        let mut ellipsis = fonts.shape(
                            mark,
                            &ShapeRequest {
                                font,
                                size: style.size,
                                direction: if base.is_rtl() {
                                    Direction::Rtl
                                } else {
                                    Direction::Ltr
                                },
                                features: &style.features,
                                lang: style.lang.as_deref(),
                                letter_spacing: style.letter_spacing,
                            },
                        )?;
                        for glyph in &mut ellipsis.glyphs {
                            glyph.cluster = range.end;
                        }
                        Some(ellipsis)
                    } else {
                        None
                    };
                    items.push(Item {
                        range,
                        level,
                        base,
                        run,
                        ellipsis,
                        ascent: metrics.ascent * style.size,
                        descent: metrics.descent * style.size,
                        line_gap: metrics.line_gap * style.size,
                    });
                }
            }
        }
        match next_start {
            Some(next) => line_start = next,
            None => break,
        }
    }
    Ok(Paragraph::new(
        Arc::from(text.text.as_str()),
        *paragraph_style,
        direction,
        items,
    ))
}

/// `range` of `text` cut where the UAX 9 embedding level changes; each grapheme cluster takes the level of its first
/// character, so a cluster is never split between directions. `levels` are by byte from `line_start`; an empty range
/// takes the paragraph's `base` level.
fn level_runs(
    text: &str,
    range: Range<usize>,
    levels: &[Level],
    line_start: usize,
    base: Level,
) -> Vec<(Range<usize>, Level)> {
    let mut runs: Vec<(Range<usize>, Level)> = Vec::new();
    for (offset, cluster) in text[range.clone()].grapheme_indices(true) {
        let start = range.start + offset;
        let level = levels[start - line_start];
        match runs.last_mut() {
            Some((run, last)) if *last == level => run.end = start + cluster.len(),
            _ => runs.push((start..start + cluster.len(), level)),
        }
    }
    if runs.is_empty() {
        runs.push((range, base));
    }
    runs
}

/// `range` of `text` cut where the font drawing it changes. Each grapheme cluster takes the first font that has every
/// character of it, `primary` first, then the other faces in load order, then an installed face the system picks for
/// its first character and the style's language; a cluster no font has stays in `primary`, and a cluster of white
/// space stays in the font before it, so a gap between words does not split a run.
fn font_runs(
    fonts: &FontIndex,
    text: &str,
    range: Range<usize>,
    primary: FontId,
    style: &TextStyle,
) -> Result<Vec<(Range<usize>, FontId)>, TextError> {
    let weight = style.weight;
    let mut fallbacks: Option<Arc<[FontId]>> = None;
    let mut runs: Vec<(Range<usize>, FontId)> = Vec::new();
    for (offset, cluster) in text[range.clone()].grapheme_indices(true) {
        let (start, end) = (range.start + offset, range.start + offset + cluster.len());
        // the emoji presentation selector asks for a colour glyph: a colour font that has the cluster goes first
        let emoji = cluster.contains('\u{FE0F}');
        let font = if cluster.chars().all(char::is_whitespace) {
            runs.last().map_or(primary, |(_, font)| *font)
        } else if !emoji && covers(fonts, primary, cluster)? {
            primary
        } else {
            let candidates = match &fallbacks {
                Some(candidates) => Arc::clone(candidates),
                None => Arc::clone(fallbacks.insert(fonts.fallbacks(primary, weight)?)),
            };
            let order: Vec<FontId> = std::iter::once(primary)
                .chain(candidates.iter().copied())
                .skip(usize::from(!emoji))
                .collect();
            let mut found = None;
            if emoji {
                for &candidate in &order {
                    if fonts.is_colour(candidate)? && covers(fonts, candidate, cluster)? {
                        found = Some(candidate);
                        break;
                    }
                }
            }
            if found.is_none() {
                for &candidate in &order {
                    if covers(fonts, candidate, cluster)? {
                        found = Some(candidate);
                        break;
                    }
                }
            }
            if found.is_none() {
                found = installed(fonts, cluster, style)?;
                if found.is_some() {
                    fallbacks = None;
                }
            }
            found.unwrap_or(primary)
        };
        match runs.last_mut() {
            Some((run, last)) if *last == font => run.end = end,
            _ => runs.push((start..end, font)),
        }
    }
    Ok(runs)
}

/// An installed face that has every character of `cluster`, loaded for its first character that draws.
fn installed(
    fonts: &FontIndex,
    cluster: &str,
    style: &TextStyle,
) -> Result<Option<FontId>, TextError> {
    let Some(ch) = cluster.chars().find(|ch| draws(*ch)) else {
        return Ok(None);
    };
    let Some(face) = fonts.system_covering(ch, style.lang.as_deref(), style.weight, style.style)
    else {
        return Ok(None);
    };
    let font = fonts.instance(face, style.weight, &[])?;
    Ok(covers(fonts, font, cluster)?.then_some(font))
}

fn draws(ch: char) -> bool {
    !matches!(ch, '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// Whether `font` has every character of `cluster` that draws: joiners and variation selectors only steer the shaping.
fn covers(fonts: &FontIndex, font: FontId, cluster: &str) -> Result<bool, TextError> {
    for ch in cluster.chars().filter(|ch| draws(*ch)) {
        if !fonts.has_glyph(font, ch)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The text cut into consecutive ranges of one style; ranges the runs leave out take the style before them.
fn style_segments(text: &AttributedText) -> Result<Vec<(Range<usize>, &TextStyle)>, TextError> {
    let mut runs: Vec<&(crate::TextRange, TextStyle)> = text.runs.iter().collect();
    runs.sort_by_key(|(range, _)| range.0.start);
    let first: &TextStyle = &runs.first().ok_or(TextError::NoFont)?.1;
    let mut segments: Vec<(Range<usize>, &TextStyle)> = Vec::with_capacity(runs.len() + 1);
    let mut at = 0;
    for (range, style) in runs {
        let (start, end) = (range.0.start.max(at), range.0.end.min(text.text.len()));
        for byte in [start, end] {
            if !text.text.is_char_boundary(byte) {
                return Err(TextError::NotACharBoundary(byte));
            }
        }
        if start > at {
            let filler = segments.last().map_or(first, |(_, style)| *style);
            segments.push((at..start, filler));
        }
        if end > start {
            segments.push((start..end, style));
            at = end;
        }
    }
    if at < text.text.len() {
        let filler = segments.last().map_or(first, |(_, style)| *style);
        segments.push((at..text.text.len(), filler));
    }
    if segments.is_empty() {
        segments.push((0..0, first));
    }
    Ok(segments)
}

/// The characters that end a hard line; `\r\n` is one separator.
pub(crate) const LINE_SEPARATORS: [char; 4] = ['\n', '\r', '\u{2028}', '\u{2029}'];

/// The end of the hard line starting at `start`, and where the next one starts (`None` after the last).
pub(crate) fn next_line(text: &str, start: usize) -> (usize, Option<usize>) {
    let rest = &text[start..];
    match rest.find(LINE_SEPARATORS) {
        None => (text.len(), None),
        Some(offset) => {
            let end = start + offset;
            let separator = if text[end..].starts_with("\r\n") {
                2
            } else {
                text[end..].chars().next().map_or(0, char::len_utf8)
            };
            (end, Some(end + separator))
        }
    }
}

/// The pieces of one hard line, one per style segment it touches; an empty line is one empty piece.
fn pieces_of<'a>(
    segments: &[(Range<usize>, &'a TextStyle)],
    line: Range<usize>,
) -> Vec<(Range<usize>, &'a TextStyle)> {
    let mut pieces: Vec<_> = segments
        .iter()
        .filter_map(|(range, style)| {
            let (start, end) = (range.start.max(line.start), range.end.min(line.end));
            (start < end).then_some((start..end, *style))
        })
        .collect();
    if pieces.is_empty() {
        let style = segments
            .iter()
            .rfind(|(range, _)| range.start <= line.start)
            .or(segments.first())
            .map(|(_, style)| *style);
        if let Some(style) = style {
            pieces.push((line.start..line.start, style));
        }
    }
    pieces
}

fn cache_key(text: &AttributedText, style: &ParagraphStyle) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.text.hash(&mut hasher);
    for (range, style) in &text.runs {
        range.hash(&mut hasher);
        style.family.hash(&mut hasher);
        style.size.to_bits().hash(&mut hasher);
        style.weight.hash(&mut hasher);
        style.style.hash(&mut hasher);
        style.features.hash(&mut hasher);
        for variation in &style.variations {
            variation.tag.hash(&mut hasher);
            variation.value.to_bits().hash(&mut hasher);
        }
        style.lang.hash(&mut hasher);
        style.letter_spacing.to_bits().hash(&mut hasher);
    }
    style.align.hash(&mut hasher);
    style.direction.hash(&mut hasher);
    style.max_lines.hash(&mut hasher);
    style.wrap.hash(&mut hasher);
    style.ellipsis.hash(&mut hasher);
    hasher.finish()
}

impl TextStyle {
    /// A regular upright style of the theme's family at `size` points.
    pub fn new(size: f32) -> Self {
        Self {
            family: None,
            size,
            weight: crate::FontWeight::REGULAR,
            style: FontStyle::Normal,
            features: Vec::new(),
            variations: Vec::new(),
            lang: None,
            letter_spacing: 0.0,
        }
    }
}

impl AttributedText {
    /// `text` in one style.
    pub fn plain(text: impl Into<String>, style: TextStyle) -> Self {
        let text = text.into();
        let range = crate::TextRange(0..text.len());
        Self {
            text,
            runs: vec![(range, style)],
        }
    }
}
