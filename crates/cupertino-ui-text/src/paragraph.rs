use crate::{
    Affinity, GlyphRun, ParagraphStyle, PositionedGlyph, ShapedRun, TextAlign, TextPosition,
    TextRange, Wrap,
};
use cupertino_geometry::{Point, Rect, Size};
use icu_segmenter::options::LineBreakOptions;
use icu_segmenter::LineSegmenter;
use std::ops::Range;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

#[cfg(test)]
mod work {
    use std::cell::Cell;

    thread_local!(static STEPS: Cell<usize> = const { Cell::new(0) });

    pub fn step() {
        STEPS.with(|steps| steps.set(steps.get() + 1));
    }

    pub fn take() -> usize {
        STEPS.with(|steps| steps.replace(0))
    }
}

#[cfg(not(test))]
mod work {
    #[inline(always)]
    pub fn step() {}
}

/// Width of the rectangle `caret_rect` returns, in points.
pub const CARET_WIDTH: f64 = 1.0;

/// One shaped piece of a paragraph: a run of one style on one hard line.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub range: Range<usize>,
    pub run: ShapedRun,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
}

#[derive(Debug)]
struct Shaped {
    text: Arc<str>,
    style: ParagraphStyle,
    items: Vec<Item>,
    /// Byte offsets where UAX 14 allows a line to start, without 0 and the text length.
    breaks: Vec<usize>,
}

/// Shaped text ready to lay out at any width. Cloning shares the shaping.
#[derive(Clone, Debug)]
pub struct Paragraph {
    shaped: Arc<Shaped>,
}

/// A glyph run placed on a line.
#[derive(Clone, Debug, PartialEq)]
pub struct LineRun {
    /// The run's origin on the baseline, relative to the layout's top-left corner.
    pub origin: Point,
    /// The bytes of the paragraph's text the run shows.
    pub range: TextRange,
    pub run: GlyphRun,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The bytes on the line, including the spaces that hang past a soft wrap and excluding the line break.
    pub range: TextRange,
    pub rect: Rect,
    /// The y of the baseline, relative to the layout's top.
    pub baseline: f64,
    pub runs: Vec<LineRun>,
    /// Byte offset and x of every cluster start, and of the line end.
    stops: Vec<(usize, f64)>,
    /// The end of the last character that is not a space.
    visible_end: usize,
    soft_wrapped: bool,
}

/// A paragraph broken into lines at one width: line boxes, caret and selection geometry, hit testing. Lines and
/// glyphs are laid out in logical order, left to right; direction-aware layout comes with bidi.
#[derive(Clone, Debug)]
pub struct ParagraphLayout {
    text: Arc<str>,
    lines: Vec<Line>,
    size: Size,
}

impl Paragraph {
    pub(crate) fn new(text: Arc<str>, style: ParagraphStyle, items: Vec<Item>) -> Self {
        let breaks = break_opportunities(&text);
        Self {
            shaped: Arc::new(Shaped {
                text,
                style,
                items,
                breaks,
            }),
        }
    }

    pub fn text(&self) -> &str {
        &self.shaped.text
    }

    /// Breaks the paragraph into lines no wider than `max_width` (`None`, or a width that is not finite, for no limit).
    /// It reuses the shaping. `max_lines` of zero keeps one line.
    pub fn layout(&self, max_width: Option<f32>) -> ParagraphLayout {
        layout(&self.shaped, max_width)
    }
}

fn break_opportunities(text: &str) -> Vec<usize> {
    LineSegmenter::new_auto(LineBreakOptions::default())
        .segment_str(text)
        .filter(|&at| at > 0 && at < text.len())
        .collect()
}

struct Cluster {
    start: usize,
    end: usize,
    width: f32,
    item: usize,
    glyphs: Range<usize>,
    space: bool,
}

struct LineSpec {
    clusters: Range<usize>,
    soft_wrapped: bool,
}

fn layout(shaped: &Shaped, max_width: Option<f32>) -> ParagraphLayout {
    let line_limit = shaped
        .style
        .max_lines
        .map_or(usize::MAX, |limit| (limit as usize).max(1));
    let max_width = max_width.filter(|width| width.is_finite());
    let wrap_width = match shaped.style.wrap {
        Wrap::Word => max_width,
        Wrap::None => None,
    };
    let mut groups: Vec<Range<usize>> = Vec::new();
    for (index, item) in shaped.items.iter().enumerate() {
        match groups.last_mut() {
            Some(group) if shaped.items[group.end - 1].range.end == item.range.start => {
                group.end = index + 1
            }
            _ => groups.push(index..index + 1),
        }
    }

    struct HardLine {
        clusters: Vec<Cluster>,
        lines: Vec<LineSpec>,
        items: Range<usize>,
    }
    let mut hard_lines: Vec<HardLine> = Vec::with_capacity(groups.len());
    let mut line_count = 0;
    for group in groups {
        let clusters = clusters_of(shaped, group.clone());
        let lines = break_lines(shaped, &clusters, wrap_width);
        line_count += lines.len();
        hard_lines.push(HardLine {
            clusters,
            lines,
            items: group,
        });
        if line_count >= line_limit {
            break;
        }
    }

    let natural_width = hard_lines
        .iter()
        .flat_map(|hard| {
            hard.lines
                .iter()
                .map(|line| trimmed_width(&hard.clusters, line))
        })
        .fold(0.0_f32, f32::max);
    let container = f64::from(max_width.unwrap_or(natural_width).max(natural_width));

    let mut lines = Vec::with_capacity(line_count);
    let mut top = 0.0_f64;
    'build: for hard in &hard_lines {
        for spec in &hard.lines {
            if lines.len() >= line_limit {
                break 'build;
            }
            let line = build_line(
                shaped,
                hard.items.clone(),
                &hard.clusters,
                spec,
                top,
                container,
            );
            top += line.rect.height();
            lines.push(line);
        }
    }

    ParagraphLayout {
        text: shaped.text.clone(),
        size: Size::new(f64::from(natural_width), top),
        lines,
    }
}

/// The clusters of a hard line, in logical order, with the glyphs of each.
fn clusters_of(shaped: &Shaped, items: Range<usize>) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();
    for index in items {
        let item = &shaped.items[index];
        let first_of_item = clusters.len();
        for (glyph_index, glyph) in item.run.glyphs.iter().enumerate() {
            work::step();
            match clusters.last_mut() {
                Some(last) if last.item == index && last.start == glyph.cluster => {
                    last.width += glyph.advance;
                    last.glyphs.end = glyph_index + 1;
                }
                _ => clusters.push(Cluster {
                    start: glyph.cluster,
                    end: glyph.cluster,
                    width: glyph.advance,
                    item: index,
                    glyphs: glyph_index..glyph_index + 1,
                    space: false,
                }),
            }
        }
        if clusters.len() > first_of_item {
            clusters[first_of_item].start = item.range.start;
        }
        for position in first_of_item..clusters.len() {
            let end = clusters
                .get(position + 1)
                .filter(|next| next.item == index)
                .map_or(item.range.end, |next| next.start);
            clusters[position].end = end;
        }
    }
    for cluster in &mut clusters {
        cluster.space = shaped.text[cluster.start..cluster.end]
            .chars()
            .all(char::is_whitespace);
    }
    clusters
}

fn break_lines(shaped: &Shaped, clusters: &[Cluster], max_width: Option<f32>) -> Vec<LineSpec> {
    let mut specs = Vec::new();
    let mut start = 0;
    let mut width = 0.0_f32;
    let mut last_opportunity: Option<usize> = None;
    let mut i = 0;
    while i < clusters.len() {
        work::step();
        let cluster = &clusters[i];
        if i > start && shaped.breaks.binary_search(&cluster.start).is_ok() {
            last_opportunity = Some(i);
        }
        let overflows = max_width
            .is_some_and(|limit| !cluster.space && i > start && width + cluster.width > limit);
        if overflows {
            let at = last_opportunity.filter(|&at| at > start).unwrap_or(i);
            specs.push(LineSpec {
                clusters: start..at,
                soft_wrapped: true,
            });
            start = at;
            width = 0.0;
            last_opportunity = None;
            i = at;
            continue;
        }
        width += cluster.width;
        i += 1;
    }
    specs.push(LineSpec {
        clusters: start..clusters.len(),
        soft_wrapped: false,
    });
    specs
}

fn trimmed_width(clusters: &[Cluster], spec: &LineSpec) -> f32 {
    let line = &clusters[spec.clusters.clone()];
    let visible = line
        .iter()
        .rposition(|cluster| !cluster.space)
        .map_or(0, |last| last + 1);
    line[..visible].iter().map(|cluster| cluster.width).sum()
}

fn build_line(
    shaped: &Shaped,
    items: Range<usize>,
    clusters: &[Cluster],
    spec: &LineSpec,
    top: f64,
    container: f64,
) -> Line {
    let line_clusters = &clusters[spec.clusters.clone()];
    let width = f64::from(trimmed_width(clusters, spec));
    let offset = match shaped.style.align {
        TextAlign::Start | TextAlign::Justify => 0.0,
        TextAlign::Center => (container - width) / 2.0,
        TextAlign::End => container - width,
    };

    let (start, end) = match (line_clusters.first(), line_clusters.last()) {
        (Some(first), Some(last)) => (first.start, last.end),
        _ => {
            let item = &shaped.items[items.start];
            (item.range.start, item.range.start)
        }
    };

    let mut ascent = 0.0_f32;
    let mut descent = 0.0_f32;
    let mut line_gap = 0.0_f32;
    let metric_items = if line_clusters.is_empty() {
        items.start..items.start + 1
    } else {
        line_clusters[0].item..line_clusters[line_clusters.len() - 1].item + 1
    };
    for item in &shaped.items[metric_items] {
        ascent = ascent.max(item.ascent);
        descent = descent.max(item.descent);
        line_gap = line_gap.max(item.line_gap);
    }
    let height = f64::from(ascent + descent + line_gap);
    let baseline = top + f64::from(ascent);

    let mut runs = Vec::new();
    let mut stops = Vec::with_capacity(line_clusters.len() + 1);
    let mut x = offset;
    let mut piece_start = 0;
    while piece_start < line_clusters.len() {
        let item_index = line_clusters[piece_start].item;
        let piece_end = line_clusters[piece_start..]
            .iter()
            .position(|cluster| cluster.item != item_index)
            .map_or(line_clusters.len(), |count| piece_start + count);
        let item = &shaped.items[item_index];
        let origin_x = x;
        let mut glyphs = Vec::new();
        let mut pen = 0.0_f32;
        for cluster in &line_clusters[piece_start..piece_end] {
            work::step();
            stops.push((cluster.start, x));
            for glyph in &item.run.glyphs[cluster.glyphs.clone()] {
                glyphs.push(PositionedGlyph {
                    id: glyph.id,
                    x: pen + glyph.x_offset,
                    y: -glyph.y_offset,
                    cluster: glyph.cluster,
                });
                pen += glyph.advance;
            }
            x += f64::from(cluster.width);
        }
        runs.push(LineRun {
            origin: Point::new(origin_x, baseline),
            range: TextRange(line_clusters[piece_start].start..line_clusters[piece_end - 1].end),
            run: GlyphRun {
                font: item.run.font,
                size: item.run.size,
                glyphs,
            },
        });
        piece_start = piece_end;
    }
    stops.push((end, x));
    if line_clusters.is_empty() {
        stops[0].1 = offset;
    }

    let visible_end = line_clusters
        .iter()
        .rfind(|cluster| !cluster.space)
        .map_or(start, |cluster| cluster.end);
    Line {
        range: TextRange(start..end),
        rect: Rect::new(0.0, top, container.max(width), height),
        baseline,
        runs,
        stops,
        visible_end,
        soft_wrapped: spec.soft_wrapped,
    }
}

impl Line {
    fn x_at(&self, byte: usize) -> f64 {
        let at = self.stops.partition_point(|&(stop, _)| stop <= byte);
        match at {
            0 => self.stops[0].1,
            _ if at == self.stops.len() => self.stops[at - 1].1,
            _ => {
                let (from, x0) = self.stops[at - 1];
                let (to, x1) = self.stops[at];
                x0 + (x1 - x0) * (byte - from) as f64 / (to - from) as f64
            }
        }
    }
}

impl ParagraphLayout {
    /// The width of the widest line without its hanging spaces, and the height of all lines.
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    fn line_of(&self, at: TextPosition) -> &Line {
        let index = self
            .lines
            .partition_point(|line| line.range.0.start <= at.byte)
            .saturating_sub(1);
        if at.affinity == Affinity::Upstream && index > 0 {
            let line = &self.lines[index];
            let before = &self.lines[index - 1];
            if line.range.0.start == at.byte && before.soft_wrapped {
                return before;
            }
        }
        &self.lines[index]
    }

    /// The caret at `at`: a thin rectangle the height of its line. A position at a soft wrap belongs to the next line
    /// unless its affinity is upstream.
    pub fn caret_rect(&self, at: TextPosition) -> Rect {
        let line = self.line_of(at);
        let x = line.x_at(at.byte.min(self.text.len()));
        Rect::new(x, line.rect.y(), CARET_WIDTH, line.rect.height())
    }

    /// The position nearest `point`, on a grapheme boundary. A point above or below the text takes the first or last
    /// line, and one past a line's end takes the line's end.
    pub fn index_at(&self, point: Point) -> TextPosition {
        let line = self
            .lines
            .iter()
            .find(|line| point.y < line.rect.max_y())
            .unwrap_or_else(|| &self.lines[self.lines.len() - 1]);
        let first = line.stops[0];
        if point.x <= first.1 {
            return TextPosition {
                byte: first.0,
                affinity: Affinity::Downstream,
            };
        }
        let visible_x = line.x_at(line.visible_end);
        if point.x >= visible_x {
            let upstream = line.soft_wrapped && line.visible_end == line.range.0.end;
            return TextPosition {
                byte: line.visible_end,
                affinity: if upstream {
                    Affinity::Upstream
                } else {
                    Affinity::Downstream
                },
            };
        }
        let at = line
            .stops
            .partition_point(|&(_, x)| x <= point.x)
            .clamp(1, line.stops.len() - 1);
        let (from, _) = line.stops[at - 1];
        let (to, _) = line.stops[at];
        let nearest = std::iter::once(from)
            .chain(
                self.text[from..to]
                    .grapheme_indices(true)
                    .map(|(offset, _)| from + offset)
                    .skip(1),
            )
            .chain(std::iter::once(to))
            .min_by(|a, b| {
                let da = (line.x_at(*a) - point.x).abs();
                let db = (line.x_at(*b) - point.x).abs();
                da.total_cmp(&db)
            })
            .unwrap_or(from);
        TextPosition {
            byte: nearest,
            affinity: Affinity::Downstream,
        }
    }

    /// The rectangles that highlight `range` (logical bytes), one per line it touches.
    pub fn selection_rects(&self, range: Range<usize>) -> Vec<Rect> {
        self.lines
            .iter()
            .filter_map(|line| {
                let start = range.start.max(line.range.0.start);
                let end = range.end.min(line.range.0.end);
                if start >= end {
                    return None;
                }
                let (x0, x1) = (line.x_at(start), line.x_at(end));
                Some(Rect::new(x0, line.rect.y(), x1 - x0, line.rect.height()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontIndex, FontStyle, FontWeight, TextStyle, TextSystem};

    const INTER: &[u8] = include_bytes!("../../cupertino-assets/fonts/InterVariable-subset.ttf");

    fn system() -> TextSystem {
        let mut fonts = FontIndex::new();
        fonts
            .add_face(
                "Inter",
                FontWeight::REGULAR,
                FontStyle::Normal,
                Arc::new(INTER.to_vec()),
                0,
            )
            .unwrap();
        TextSystem::new(fonts, "Inter")
    }

    fn layout(text: &str, width: Option<f32>) -> ParagraphLayout {
        layout_with(text, ParagraphStyle::default(), width)
    }

    fn layout_with(text: &str, style: ParagraphStyle, width: Option<f32>) -> ParagraphLayout {
        let attributed = crate::AttributedText::plain(text, TextStyle::new(16.0));
        system()
            .paragraph(&attributed, &style)
            .unwrap()
            .layout(width)
    }

    fn position(byte: usize) -> TextPosition {
        TextPosition {
            byte,
            affinity: Affinity::Downstream,
        }
    }

    fn line_texts<'a>(layout: &ParagraphLayout, text: &'a str) -> Vec<&'a str> {
        layout
            .lines()
            .iter()
            .map(|line| &text[line.range.0.clone()])
            .collect()
    }

    #[test]
    fn text_without_a_limit_is_one_line_with_its_natural_size() {
        let text = "Hello world";
        let layout = layout(text, None);
        assert_eq!(line_texts(&layout, text), [text]);
        let line = &layout.lines()[0];
        assert!(layout.size().width > 50.0);
        assert!((layout.size().height - line.rect.height()).abs() < 1e-9);
        assert!((line.baseline - line.rect.y()).abs() > 1.0);
    }

    #[test]
    fn a_limit_wraps_at_word_breaks_and_the_spaces_hang() {
        let text = "alpha beta gamma delta";
        let one_line = layout(text, None).size().width;
        let layout = layout(text, Some(one_line as f32 / 2.0));
        let lines = line_texts(&layout, text);
        assert!(lines.len() >= 2);
        assert_eq!(lines.concat(), text);
        assert!(lines[0].ends_with(' '), "{lines:?}");
        for line in layout.lines() {
            assert!(layout.size().width <= f64::from(one_line as f32 / 2.0) + 1e-6);
            assert!(line.rect.width() >= layout.size().width - 1e-6);
        }
    }

    #[test]
    fn a_word_wider_than_the_limit_breaks_between_characters() {
        let text = "abcdefghijklmnop";
        let layout = layout(text, Some(40.0));
        let lines = line_texts(&layout, text);
        assert!(lines.len() > 1);
        assert_eq!(lines.concat(), text);
        assert!(lines.iter().all(|line| !line.is_empty()));
    }

    #[test]
    fn hard_breaks_start_lines_and_empty_lines_keep_their_height() {
        let text = "one\n\ntwo\r\nthree";
        let layout = layout(text, None);
        assert_eq!(line_texts(&layout, text), ["one", "", "two", "three"]);
        let heights: Vec<f64> = layout
            .lines()
            .iter()
            .map(|line| line.rect.height())
            .collect();
        assert!(heights
            .windows(2)
            .all(|pair| (pair[0] - pair[1]).abs() < 1e-9));
        let tops: Vec<f64> = layout.lines().iter().map(|line| line.rect.y()).collect();
        assert!(tops.windows(2).all(|pair| pair[1] > pair[0]));
    }

    #[test]
    fn empty_text_is_one_empty_line_with_a_caret() {
        let layout = layout("", Some(100.0));
        assert_eq!(layout.lines().len(), 1);
        let caret = layout.caret_rect(position(0));
        assert!(caret.height() > 10.0 && caret.x() == 0.0);
        assert_eq!(layout.index_at(Point::new(50.0, 3.0)).byte, 0);
    }

    #[test]
    fn max_lines_drops_the_lines_beyond_it() {
        let text = "a\nb\nc\nd";
        let style = ParagraphStyle {
            max_lines: Some(2),
            ..ParagraphStyle::default()
        };
        assert_eq!(
            line_texts(&layout_with(text, style, None), text),
            ["a", "b"]
        );
    }

    #[test]
    fn wrap_none_ignores_the_limit() {
        let text = "alpha beta gamma delta";
        let style = ParagraphStyle {
            wrap: Wrap::None,
            ..ParagraphStyle::default()
        };
        assert_eq!(layout_with(text, style, Some(30.0)).lines().len(), 1);
    }

    #[test]
    fn alignment_shifts_a_line_inside_the_limit() {
        let text = "hi";
        let width = 200.0;
        let natural = layout(text, None).size().width;
        let place = |align| {
            let style = ParagraphStyle {
                align,
                ..ParagraphStyle::default()
            };
            layout_with(text, style, Some(width)).lines()[0].runs[0]
                .origin
                .x
        };
        assert_eq!(place(TextAlign::Start), 0.0);
        assert!((place(TextAlign::Center) - (f64::from(width) - natural) / 2.0).abs() < 1e-3);
        assert!((place(TextAlign::End) - (f64::from(width) - natural)).abs() < 1e-3);
    }

    #[test]
    fn a_caret_has_the_line_height_and_moves_right_with_the_text() {
        let text = "Hello";
        let layout = layout(text, None);
        let xs: Vec<f64> = (0..=text.len())
            .map(|byte| layout.caret_rect(position(byte)).x())
            .collect();
        assert!(xs.windows(2).all(|pair| pair[1] > pair[0]), "{xs:?}");
        assert!((xs[text.len()] - layout.size().width).abs() < 1e-3);
        assert!(
            (layout.caret_rect(position(2)).height() - layout.lines()[0].rect.height()).abs()
                < 1e-9
        );
    }

    #[test]
    fn index_at_inverts_caret_rect_on_every_grapheme_boundary() {
        let text = "Héllo wörld, fine\ne\u{0301}x wrap me please";
        let layout = layout(text, Some(90.0));
        let mut boundaries: Vec<usize> = text.grapheme_indices(true).map(|(at, _)| at).collect();
        boundaries.push(text.len());
        for byte in boundaries {
            for affinity in [Affinity::Downstream, Affinity::Upstream] {
                let hanging = affinity == Affinity::Upstream && text[..byte].ends_with(' ');
                if hanging {
                    continue;
                }
                let at = TextPosition { byte, affinity };
                let caret = layout.caret_rect(at);
                let found =
                    layout.index_at(Point::new(caret.x(), caret.y() + caret.height() / 2.0));
                let back = layout.caret_rect(found);
                assert_eq!(
                    (back.x(), back.y()),
                    (caret.x(), caret.y()),
                    "byte {byte} {affinity:?} found {found:?}"
                );
            }
        }
    }

    #[test]
    fn a_click_inside_a_glyph_picks_the_nearer_boundary_and_never_splits_a_grapheme() {
        let text = "e\u{0301}e\u{0301}";
        let layout = layout(text, None);
        let end = layout.caret_rect(position(text.len())).x();
        for step in 0..=40 {
            let x = end * f64::from(step) / 40.0;
            let found = layout.index_at(Point::new(x, 5.0)).byte;
            assert!(matches!(found, 0 | 3 | 6), "x {x} gave {found}");
        }
    }

    #[test]
    fn points_above_below_and_beside_the_text_clamp_to_its_ends() {
        let text = "ab\ncd";
        let layout = layout(text, None);
        assert_eq!(layout.index_at(Point::new(-20.0, -20.0)).byte, 0);
        assert_eq!(layout.index_at(Point::new(500.0, 500.0)).byte, text.len());
        assert_eq!(layout.index_at(Point::new(500.0, 3.0)).byte, 2);
    }

    #[test]
    fn a_click_past_a_soft_wrapped_line_stays_on_that_line() {
        let text = "alpha beta gamma delta";
        let layout = layout(text, Some(60.0));
        let first = &layout.lines()[0];
        let found = layout.index_at(Point::new(1000.0, first.rect.y() + 1.0));
        assert_eq!(layout.caret_rect(found).y(), first.rect.y());
        assert!(found.byte <= first.range.0.end);
    }

    #[test]
    fn a_position_at_a_character_wrap_follows_its_affinity() {
        let text = "abcdefghijklmnop";
        let layout = layout(text, Some(40.0));
        let wrap = layout.lines()[1].range.0.start;
        let down = layout.caret_rect(position(wrap));
        let up = layout.caret_rect(TextPosition {
            byte: wrap,
            affinity: Affinity::Upstream,
        });
        assert!(down.y() > up.y());
        assert_eq!(down.x(), 0.0);
        assert!(up.x() > 0.0);
    }

    #[test]
    fn selection_rects_cover_the_range_one_per_line() {
        let text = "alpha beta\ngamma delta";
        let layout = layout(text, Some(60.0));
        let rects = layout.selection_rects(2..text.len() - 2);
        assert_eq!(rects.len(), layout.lines().len());
        let whole = layout.selection_rects(0..text.len());
        for (rect, line) in whole.iter().zip(layout.lines()) {
            assert_eq!(rect.y(), line.rect.y());
            assert!(rect.width() > 0.0);
        }
        assert!(layout.selection_rects(3..3).is_empty());
        assert!(rects[0].x() > 0.0);
    }

    #[test]
    fn glyph_runs_hold_every_glyph_with_byte_clusters_of_the_paragraph() {
        let text = "ab\ncd";
        let layout = layout(text, None);
        let clusters: Vec<Vec<usize>> = layout
            .lines()
            .iter()
            .map(|line| {
                line.runs
                    .iter()
                    .flat_map(|run| run.run.glyphs.iter().map(|glyph| glyph.cluster))
                    .collect()
            })
            .collect();
        assert_eq!(clusters, [vec![0, 1], vec![3, 4]]);
        let second = &layout.lines()[1];
        assert_eq!(second.runs[0].origin.y, second.baseline);
        assert_eq!(second.runs[0].range.0, 3..5);
    }

    #[test]
    fn styles_split_a_line_into_runs_and_the_taller_one_sets_the_height() {
        let small = TextStyle::new(12.0);
        let large = TextStyle::new(30.0);
        let text = crate::AttributedText {
            text: "small LARGE".into(),
            runs: vec![
                (crate::TextRange(0..6), small),
                (crate::TextRange(6..11), large.clone()),
            ],
        };
        let system = system();
        let mixed = system
            .paragraph(&text, &ParagraphStyle::default())
            .unwrap()
            .layout(None);
        let line = &mixed.lines()[0];
        assert_eq!(line.runs.len(), 2);
        assert_eq!(line.runs[1].run.size, 30.0);
        assert!(line.runs[1].origin.x > line.runs[0].origin.x);
        let alone = system.metrics(&large).unwrap().line_height(30.0);
        assert!((line.rect.height() - f64::from(alone)).abs() < 1e-3);
    }

    #[test]
    fn the_same_request_returns_the_cached_shaping() {
        let system = system();
        let text = crate::AttributedText::plain("cached", TextStyle::new(16.0));
        let first = system.paragraph(&text, &ParagraphStyle::default()).unwrap();
        let second = system.paragraph(&text, &ParagraphStyle::default()).unwrap();
        assert!(Arc::ptr_eq(&first.shaped, &second.shaped));
        let other = crate::AttributedText::plain("cached", TextStyle::new(17.0));
        let third = system
            .paragraph(&other, &ParagraphStyle::default())
            .unwrap();
        assert!(!Arc::ptr_eq(&first.shaped, &third.shaped));
    }

    #[test]
    fn a_run_range_off_a_character_boundary_is_an_error() {
        let text = crate::AttributedText {
            text: "é".into(),
            runs: vec![(crate::TextRange(0..1), TextStyle::new(16.0))],
        };
        assert_eq!(
            system()
                .paragraph(&text, &ParagraphStyle::default())
                .unwrap_err(),
            crate::TextError::NotACharBoundary(1)
        );
    }

    #[test]
    fn zero_max_lines_and_empty_text_still_answer_every_query() {
        for (text, max_lines) in [
            ("abc def", Some(0)),
            ("", None),
            ("", Some(0)),
            ("a\nb", Some(0)),
        ] {
            let style = ParagraphStyle {
                max_lines,
                ..ParagraphStyle::default()
            };
            let layout = layout_with(text, style, Some(20.0));
            assert_eq!(layout.lines().len(), 1, "{text:?} {max_lines:?}");
            let caret = layout.caret_rect(position(0));
            assert!(caret.height() > 0.0);
            let found = layout.index_at(Point::new(100.0, 100.0));
            assert!(found.byte <= text.len());
            layout.selection_rects(0..text.len());
            layout.caret_rect(position(text.len() + 5));
        }
    }

    #[test]
    fn a_width_that_is_not_finite_means_no_limit() {
        let text = "alpha beta gamma";
        assert_eq!(layout(text, Some(f32::NAN)).lines().len(), 1);
        assert_eq!(layout(text, Some(f32::INFINITY)).lines().len(), 1);
    }

    #[test]
    fn sizes_and_spacing_that_are_not_finite_numbers_are_rejected() {
        let system = system();
        for style in [
            TextStyle::new(f32::NAN),
            TextStyle::new(0.0),
            TextStyle::new(-4.0),
            TextStyle {
                letter_spacing: f32::INFINITY,
                ..TextStyle::new(16.0)
            },
        ] {
            let text = crate::AttributedText::plain("x", style);
            assert_eq!(
                system
                    .paragraph(&text, &ParagraphStyle::default())
                    .unwrap_err(),
                crate::TextError::InvalidValue
            );
        }
    }

    #[test]
    fn a_view_of_more_paragraphs_than_one_cache_generation_still_hits() {
        let system = system();
        let style = ParagraphStyle::default();
        let texts: Vec<_> = (0..600)
            .map(|index| crate::AttributedText::plain(format!("row {index}"), TextStyle::new(16.0)))
            .collect();
        let first: Vec<_> = texts
            .iter()
            .map(|text| system.paragraph(text, &style).unwrap())
            .collect();
        for (text, before) in texts.iter().zip(&first) {
            let again = system.paragraph(text, &style).unwrap();
            assert!(Arc::ptr_eq(&before.shaped, &again.shaped));
        }
    }

    /// Steps of layout work (glyphs grouped, clusters visited by the line breaker, clusters placed) to lay out
    /// `length` characters at two widths.
    fn steps_for(length: usize) -> usize {
        let sentence = "The quick brown fox jumps over the lazy dog and keeps running far away. ";
        let text: String = sentence.chars().cycle().take(length).collect();
        let attributed = crate::AttributedText::plain(text, TextStyle::new(16.0));
        let paragraph = system()
            .paragraph(&attributed, &ParagraphStyle::default())
            .unwrap();
        work::take();
        paragraph.layout(Some(300.0));
        paragraph.layout(Some(420.0));
        work::take()
    }

    #[test]
    fn the_work_of_layout_grows_linearly_with_the_text() {
        let small = steps_for(200);
        let large = steps_for(5000);
        assert!(small > 200, "{small}");
        assert!(
            large <= small * 25 * 2,
            "5000 chars took {large} steps against {small} for 200"
        );
    }

    /// Run with `cargo test --release -p cupertino-ui-text -- --ignored --nocapture timing`.
    #[test]
    #[ignore = "timing, run by hand in release"]
    fn timing_of_shaping_layout_and_reflow() {
        use std::time::Instant;
        let median = |mut samples: Vec<f64>| {
            samples.sort_by(f64::total_cmp);
            samples[samples.len() / 2]
        };
        let sentence = "The quick brown fox jumps over the lazy dog and keeps running far away. ";
        for (name, length) in [("200 chars", 200), ("5000 chars", 5000)] {
            let text: String = sentence.chars().cycle().take(length).collect();
            let attributed = crate::AttributedText::plain(text, TextStyle::new(16.0));
            let style = ParagraphStyle::default();
            let shape = (0..15)
                .map(|_| {
                    let system = system();
                    let start = Instant::now();
                    system.paragraph(&attributed, &style).unwrap();
                    start.elapsed().as_secs_f64() * 1e6
                })
                .collect();
            let system = system();
            let paragraph = system.paragraph(&attributed, &style).unwrap();
            let reflow = (0..15)
                .map(|run| {
                    let start = Instant::now();
                    paragraph.layout(Some(300.0 + run as f32));
                    start.elapsed().as_secs_f64() * 1e6
                })
                .collect();
            let hit = (0..15)
                .map(|_| {
                    let start = Instant::now();
                    system.paragraph(&attributed, &style).unwrap();
                    start.elapsed().as_secs_f64() * 1e6
                })
                .collect();
            println!(
                "{name}: shape {:.0} us, reflow {:.0} us, cache hit {:.1} us (medians of 15)",
                median(shape),
                median(reflow),
                median(hit)
            );
        }
    }
}
