use crate::paragraph::Item;
use crate::{
    AttributedText, Direction, FaceId, FontIndex, FontMetrics, FontStyle, Paragraph,
    ParagraphStyle, ShapeRequest, ShapedRun, TextError, TextStyle,
};
use cupertino_geometry::Size;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard};

/// Paragraphs shaped before the cache starts a new generation.
const CACHE_LIMIT: usize = 512;

type CacheEntry = (AttributedText, ParagraphStyle, Paragraph);

struct State {
    fonts: FontIndex,
    /// Two generations: a full `current` becomes `previous` and the old `previous` is dropped, so a hit in either
    /// keeps the paragraph and a view of more than the limit still reuses its recent ones.
    current: HashMap<u64, CacheEntry>,
    previous: HashMap<u64, CacheEntry>,
}

/// The fonts and the paragraph cache of one app. Methods take `&self`, so one `Arc<TextSystem>` serves measuring and
/// painting.
///
/// The cache keeps the plain text of every recent paragraph, so text that must not linger, a password above all, is
/// never shaped: a secure field shapes only its mask.
pub struct TextSystem {
    theme_family: Arc<str>,
    state: Mutex<State>,
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
            state: Mutex::new(State {
                fonts,
                current: HashMap::new(),
                previous: HashMap::new(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Shapes `text`, or returns the paragraph shaped for an equal request before.
    pub fn paragraph(
        &self,
        text: &AttributedText,
        style: &ParagraphStyle,
    ) -> Result<Paragraph, TextError> {
        check_values(text)?;
        let key = cache_key(text, style);
        let mut state = self.state();
        if let Some(paragraph) = lookup(&state.current, key, text, style) {
            return Ok(paragraph);
        }
        if let Some(paragraph) = lookup(&state.previous, key, text, style) {
            state
                .current
                .insert(key, (text.clone(), *style, paragraph.clone()));
            return Ok(paragraph);
        }
        let paragraph = shape_paragraph(&mut state.fonts, &self.theme_family, text, style)?;
        insert(&mut state, key, text, style, paragraph.clone());
        Ok(paragraph)
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
        self.state().fonts.source(font)
    }

    pub fn metrics(&self, style: &TextStyle) -> Result<FontMetrics, TextError> {
        let mut state = self.state();
        let font = resolve(&mut state.fonts, &self.theme_family, style)?;
        state.fonts.metrics(font)
    }
}

fn lookup(
    generation: &HashMap<u64, CacheEntry>,
    key: u64,
    text: &AttributedText,
    style: &ParagraphStyle,
) -> Option<Paragraph> {
    generation
        .get(&key)
        .filter(|(cached_text, cached_style, _)| cached_text == text && cached_style == style)
        .map(|(_, _, paragraph)| paragraph.clone())
}

fn insert(
    state: &mut State,
    key: u64,
    text: &AttributedText,
    style: &ParagraphStyle,
    paragraph: Paragraph,
) {
    if state.current.len() >= CACHE_LIMIT {
        state.previous = std::mem::take(&mut state.current);
    }
    state.current.insert(key, (text.clone(), *style, paragraph));
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
    fonts: &mut FontIndex,
    theme_family: &str,
    style: &TextStyle,
) -> Result<crate::FontId, TextError> {
    let family = style.family.as_deref().unwrap_or(theme_family);
    let face = fonts
        .match_face(family, style.weight, style.style)
        .or_else(|| (fonts.face_count() > 0).then_some(FaceId(0)))
        .ok_or(TextError::NoFont)?;
    fonts.instance(face, style.weight, &style.variations)
}

fn shape_paragraph(
    fonts: &mut FontIndex,
    theme_family: &str,
    text: &AttributedText,
    paragraph_style: &ParagraphStyle,
) -> Result<Paragraph, TextError> {
    let segments = style_segments(text)?;
    let mut items = Vec::new();
    let mut line_start = 0;
    loop {
        let (line_end, next_start) = next_line(&text.text, line_start);
        let pieces = pieces_of(&segments, line_start..line_end);
        for (range, style) in pieces {
            let font = resolve(fonts, theme_family, style)?;
            let metrics = fonts.metrics(font)?;
            let mut run = if range.is_empty() {
                ShapedRun {
                    font,
                    size: style.size,
                    direction: Direction::Ltr,
                    glyphs: Vec::new(),
                }
            } else {
                fonts.shape(
                    &text.text[range.clone()],
                    &ShapeRequest {
                        font,
                        size: style.size,
                        direction: Direction::Ltr,
                        features: &style.features,
                        lang: style.lang.as_deref(),
                        letter_spacing: style.letter_spacing,
                    },
                )?
            };
            for glyph in &mut run.glyphs {
                glyph.cluster += range.start;
            }
            items.push(Item {
                range,
                run,
                ascent: metrics.ascent * style.size,
                descent: metrics.descent * style.size,
                line_gap: metrics.line_gap * style.size,
            });
        }
        match next_start {
            Some(next) => line_start = next,
            None => break,
        }
    }
    Ok(Paragraph::new(
        Arc::from(text.text.as_str()),
        *paragraph_style,
        items,
    ))
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

/// The end of the hard line starting at `start`, and where the next one starts (`None` after the last).
fn next_line(text: &str, start: usize) -> (usize, Option<usize>) {
    let rest = &text[start..];
    match rest.find(['\n', '\r', '\u{2028}', '\u{2029}']) {
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
