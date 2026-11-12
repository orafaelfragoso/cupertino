use crate::{
    Affinity, Direction, GlyphRun, ParagraphStyle, PositionedGlyph, ShapedRun, TextAlign,
    TextPosition, TextRange, Wrap,
};
use cupertino_geometry::{Point, Rect, Size};
use icu_segmenter::options::LineBreakOptions;
use icu_segmenter::LineSegmenter;
use std::ops::Range;
use std::sync::Arc;
use unicode_bidi::{BidiInfo, Level};
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
pub const CARET_WIDTH: f64 = 2.0;

/// One shaped piece of a paragraph: a run of one style, one font and one embedding level on one hard line.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub range: Range<usize>,
    /// The UAX 9 embedding level; an odd level is shaped right to left.
    pub level: Level,
    /// The paragraph level of the item's hard line (UAX 9 P2–P3): its direction and the side `Start` aligns to.
    pub base: Level,
    pub run: ShapedRun,
    /// The ellipsis in the item's font, when the paragraph's style asks for one; its glyphs' cluster is the item's end.
    pub ellipsis: Option<ShapedRun>,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
}

#[derive(Debug)]
struct Shaped {
    text: Arc<str>,
    style: ParagraphStyle,
    /// The base direction: `style.direction`, or the text's first strong character's. Without `style.direction` each hard
    /// line takes its own from its first strong character.
    direction: Direction,
    items: Vec<Item>,
    hard_lines: Vec<HardLine>,
}

/// The clusters of one hard line, which do not depend on the width it is laid out at.
#[derive(Debug)]
struct HardLine {
    items: Range<usize>,
    clusters: Vec<Cluster>,
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
    /// In visual order, left to right.
    pub runs: Vec<LineRun>,
    /// Every cluster of the line in logical order with its box; an empty line has one empty stop.
    stops: Vec<Stop>,
    /// The end of the last character that is not a space.
    visible_end: usize,
    soft_wrapped: bool,
}

/// The bytes of one cluster and the edges of its box on the line. A right-to-left cluster starts at its right edge.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stop {
    start: usize,
    end: usize,
    left: f64,
    right: f64,
    rtl: bool,
}

/// A paragraph broken into lines at one width: line boxes, caret and selection geometry, hit testing. Each line is
/// reordered for display by UAX 9; positions stay logical byte offsets.
#[derive(Clone, Debug)]
pub struct ParagraphLayout {
    text: Arc<str>,
    lines: Vec<Line>,
    size: Size,
}

impl Paragraph {
    pub(crate) fn new(
        text: Arc<str>,
        style: ParagraphStyle,
        direction: Direction,
        items: Vec<Item>,
    ) -> Self {
        let breaks = break_opportunities(&text);
        let mut groups: Vec<Range<usize>> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            match groups.last_mut() {
                Some(group) if items[group.end - 1].range.end == item.range.start => {
                    group.end = index + 1
                }
                _ => groups.push(index..index + 1),
            }
        }
        let hard_lines = groups
            .into_iter()
            .map(|group| {
                let start = items[group.start].range.start;
                let breaks = &breaks[breaks.partition_point(|&b| b < start)..];
                HardLine {
                    clusters: clusters_of(&text, &items, group.clone(), breaks),
                    items: group,
                }
            })
            .collect();
        Self {
            shaped: Arc::new(Shaped {
                text,
                style,
                direction,
                items,
                hard_lines,
            }),
        }
    }

    pub fn text(&self) -> &str {
        &self.shaped.text
    }

    /// The base direction of the text: `ParagraphStyle::direction`, else its first strong character's. Without a set
    /// direction a hard line with a strong character of its own is laid out in that one's.
    pub fn direction(&self) -> Direction {
        self.shaped.direction
    }

    /// Breaks the paragraph into lines no wider than `max_width` (`None`, or a width that is not finite, for no limit).
    /// It reuses the shaping. `max_lines` of zero keeps one line.
    pub fn layout(&self, max_width: Option<f32>) -> ParagraphLayout {
        crate::TextWork::record(|work| work.laid_out += 1);
        layout(&self.shaped, max_width)
    }
}

/// Byte offsets where UAX 14 allows a line to start, without 0 and the text length.
fn break_opportunities(text: &str) -> Vec<usize> {
    LineSegmenter::new_auto(LineBreakOptions::default())
        .segment_str(text)
        .filter(|&at| at > 0 && at < text.len())
        .collect()
}

#[derive(Debug)]
struct Cluster {
    start: usize,
    end: usize,
    width: f32,
    item: usize,
    glyphs: Range<usize>,
    space: bool,
    /// UAX 14 allows a line to start at `start`.
    breakable: bool,
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
    let mut specs: Vec<(&HardLine, LineSpec)> = Vec::new();
    let mut dropped = false;
    'collect: for hard in &shaped.hard_lines {
        for spec in break_lines(&hard.clusters, wrap_width) {
            if specs.len() == line_limit {
                dropped = true;
                break 'collect;
            }
            specs.push((hard, spec));
        }
    }

    let mut ellipsis = None;
    if let Some((hard, spec)) = specs.last_mut().filter(|_| shaped.style.ellipsis) {
        let overflows = max_width.is_some_and(|limit| trimmed_width(&hard.clusters, spec) > limit);
        if dropped || overflows {
            ellipsis = elide(shaped, hard, spec, max_width);
        }
    }
    let ellipsis_width = |index: usize| {
        ellipsis
            .filter(|_| index + 1 == specs.len())
            .map_or(0.0, ShapedRun::advance)
    };

    let natural_width = specs
        .iter()
        .enumerate()
        .map(|(index, (hard, spec))| trimmed_width(&hard.clusters, spec) + ellipsis_width(index))
        .fold(0.0_f32, f32::max);
    let container = f64::from(max_width.unwrap_or(natural_width).max(natural_width));

    let mut lines = Vec::with_capacity(specs.len());
    let mut top = 0.0_f64;
    for (index, (hard, spec)) in specs.iter().enumerate() {
        let line = build_line(
            shaped,
            hard.items.clone(),
            &hard.clusters,
            spec,
            ellipsis.filter(|_| index + 1 == specs.len()),
            top,
            container,
        );
        top += line.rect.height();
        lines.push(line);
    }

    ParagraphLayout {
        text: shaped.text.clone(),
        size: Size::new(f64::from(natural_width), top),
        lines,
    }
}

/// The clusters of a hard line, in logical order, with the glyphs of each.
fn clusters_of(
    text: &str,
    all_items: &[Item],
    items: Range<usize>,
    breaks: &[usize],
) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();
    for index in items {
        let item = &all_items[index];
        let first_of_item = clusters.len();
        let glyphs = &item.run.glyphs;
        // a right-to-left run lists its glyphs in visual order, the last cluster first
        let rtl = item.level.is_rtl();
        let mut graphemes = text[item.range.clone()]
            .grapheme_indices(true)
            .map(|(offset, _)| item.range.start + offset)
            .peekable();
        for step in 0..glyphs.len() {
            work::step();
            let glyph_index = if rtl { glyphs.len() - 1 - step } else { step };
            let glyph = &glyphs[glyph_index];
            while graphemes.next_if(|&start| start < glyph.cluster).is_some() {}
            // a shaping cluster that starts inside a grapheme (a conjunct's second half) joins the one before it
            let starts_grapheme = graphemes.peek() == Some(&glyph.cluster);
            match clusters.last_mut() {
                Some(last)
                    if last.item == index && (last.start == glyph.cluster || !starts_grapheme) =>
                {
                    last.width += glyph.advance;
                    if rtl {
                        last.glyphs.start = glyph_index;
                    } else {
                        last.glyphs.end = glyph_index + 1;
                    }
                }
                _ => clusters.push(Cluster {
                    start: glyph.cluster,
                    end: glyph.cluster,
                    width: glyph.advance,
                    item: index,
                    glyphs: glyph_index..glyph_index + 1,
                    space: false,
                    breakable: false,
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
    let mut breaks = breaks.iter().peekable();
    for cluster in &mut clusters {
        cluster.space = text[cluster.start..cluster.end]
            .chars()
            .all(char::is_whitespace);
        while breaks.next_if(|&&at| at < cluster.start).is_some() {}
        cluster.breakable = breaks.peek() == Some(&&cluster.start);
    }
    clusters
}

fn break_lines(clusters: &[Cluster], max_width: Option<f32>) -> Vec<LineSpec> {
    let mut specs = Vec::new();
    let mut start = 0;
    let mut width = 0.0_f32;
    let mut last_opportunity: Option<usize> = None;
    let mut i = 0;
    while i < clusters.len() {
        work::step();
        let cluster = &clusters[i];
        if i > start && cluster.breakable {
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

/// Cuts the last line `spec` to the clusters that fit before an ellipsis within `max_width`, at least none, and returns
/// the ellipsis of the last cluster it keeps.
fn elide<'a>(
    shaped: &'a Shaped,
    hard: &HardLine,
    spec: &mut LineSpec,
    max_width: Option<f32>,
) -> Option<&'a ShapedRun> {
    let ellipsis = |item: usize| shaped.items[item].ellipsis.as_ref();
    let mut kept = (spec.clusters.start, ellipsis(hard.items.start)?);
    let mut width = 0.0_f32;
    for (index, cluster) in hard.clusters[spec.clusters.clone()].iter().enumerate() {
        work::step();
        width += cluster.width;
        if max_width.is_some_and(|limit| width > limit) {
            break;
        }
        let Some(run) = ellipsis(cluster.item).filter(|_| !cluster.space) else {
            continue;
        };
        if max_width.is_none_or(|limit| width + run.advance() <= limit) {
            kept = (spec.clusters.start + index + 1, run);
        }
    }
    spec.clusters.end = kept.0;
    spec.soft_wrapped = false;
    Some(kept.1)
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
    ellipsis: Option<&ShapedRun>,
    top: f64,
    container: f64,
) -> Line {
    let line_clusters = &clusters[spec.clusters.clone()];
    let ellipsis_width = f64::from(ellipsis.map_or(0.0, ShapedRun::advance));
    let width = f64::from(trimmed_width(clusters, spec)) + ellipsis_width;
    let base = shaped.items[items.start].base;
    let rtl = base.is_rtl();
    let offset = match (shaped.style.align, rtl) {
        (TextAlign::Center, _) => (container - width) / 2.0,
        (TextAlign::Left, _)
        | (TextAlign::Start | TextAlign::Justify, false)
        | (TextAlign::End, true) => 0.0,
        (TextAlign::Right, _)
        | (TextAlign::Start | TextAlign::Justify, true)
        | (TextAlign::End, false) => container - width,
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

    // UAX 9 L1: the spaces that end the line take the paragraph's level, so they hang past its end edge
    let visible = line_clusters
        .iter()
        .rposition(|cluster| !cluster.space)
        .map_or(0, |last| last + 1);
    let mut pieces: Vec<(Range<usize>, Level)> = Vec::new();
    for (index, cluster) in line_clusters.iter().enumerate() {
        let level = if index < visible {
            shaped.items[cluster.item].level
        } else {
            base
        };
        match pieces.last_mut() {
            Some((piece, last))
                if *last == level && line_clusters[piece.start].item == cluster.item =>
            {
                piece.end = index + 1
            }
            _ => pieces.push((index..index + 1, level)),
        }
    }
    let levels: Vec<Level> = pieces.iter().map(|(_, level)| *level).collect();
    let hanging: f64 = line_clusters[visible..]
        .iter()
        .map(|cluster| f64::from(cluster.width))
        .sum();

    let mut runs = Vec::with_capacity(pieces.len());
    let mut stops = vec![
        Stop {
            start,
            end: start,
            left: offset,
            right: offset,
            rtl,
        };
        line_clusters.len().max(1)
    ];
    let mut x = if rtl {
        offset + ellipsis_width - hanging
    } else {
        offset
    };
    for piece_index in BidiInfo::reorder_visual(&levels) {
        let (piece, level) = &pieces[piece_index];
        let item = &shaped.items[line_clusters[piece.start].item];
        let origin_x = x;
        let mut glyphs = Vec::new();
        let mut pen = 0.0_f32;
        for step in 0..piece.len() {
            work::step();
            let index = if level.is_rtl() {
                piece.end - 1 - step
            } else {
                piece.start + step
            };
            let cluster = &line_clusters[index];
            for glyph in &item.run.glyphs[cluster.glyphs.clone()] {
                glyphs.push(PositionedGlyph {
                    id: glyph.id,
                    x: pen + glyph.x_offset,
                    y: -glyph.y_offset,
                    cluster: glyph.cluster,
                });
                pen += glyph.advance;
            }
            let right = x + f64::from(cluster.width);
            stops[index] = Stop {
                start: cluster.start,
                end: cluster.end,
                left: x,
                right,
                rtl: level.is_rtl(),
            };
            x = right;
        }
        runs.push(LineRun {
            origin: Point::new(origin_x, baseline),
            range: TextRange(line_clusters[piece.start].start..line_clusters[piece.end - 1].end),
            run: GlyphRun {
                font: item.run.font,
                size: item.run.size,
                glyphs,
            },
        });
    }
    if let Some(ellipsis) = ellipsis {
        let mut pen = 0.0_f32;
        let glyphs = ellipsis
            .glyphs
            .iter()
            .map(|glyph| {
                let placed = PositionedGlyph {
                    id: glyph.id,
                    x: pen + glyph.x_offset,
                    y: -glyph.y_offset,
                    cluster: end,
                };
                pen += glyph.advance;
                placed
            })
            .collect();
        let run = LineRun {
            origin: Point::new(if rtl { offset } else { x }, baseline),
            range: TextRange(end..end),
            run: GlyphRun {
                font: ellipsis.font,
                size: ellipsis.size,
                glyphs,
            },
        };
        if rtl {
            runs.insert(0, run);
        } else {
            runs.push(run);
        }
    }

    let visible_end = line_clusters[..visible]
        .last()
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

impl Stop {
    /// The x of the caret at `byte` inside the cluster, by its share of the cluster's bytes.
    fn x_at(&self, byte: usize) -> f64 {
        let along = if self.end > self.start {
            (byte - self.start) as f64 / (self.end - self.start) as f64 * (self.right - self.left)
        } else {
            0.0
        };
        if self.rtl {
            self.right - along
        } else {
            self.left + along
        }
    }
}

impl Line {
    /// The x of the caret at `at`: the start edge of the cluster that starts there, else the end edge of the last. An
    /// upstream position takes the end edge of the cluster that ends there.
    fn x_at(&self, at: TextPosition) -> f64 {
        let byte = at.byte;
        if at.affinity == Affinity::Upstream {
            let before = self.stops.partition_point(|stop| stop.end < byte);
            if let Some(stop) = self
                .stops
                .get(before)
                .filter(|stop| stop.end == byte && stop.start < byte)
            {
                return stop.x_at(byte);
            }
        }
        let at = self.stops.partition_point(|stop| stop.end <= byte);
        let stop = &self.stops[at.min(self.stops.len() - 1)];
        stop.x_at(byte.clamp(stop.start, stop.end))
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
    /// unless its affinity is upstream; at a direction change, upstream is drawn at the end edge of the character
    /// before it and downstream at the start edge of the character after it.
    pub fn caret_rect(&self, at: TextPosition) -> Rect {
        let line = self.line_of(at);
        let x = line.x_at(TextPosition {
            byte: at.byte.min(self.text.len()),
            ..at
        });
        Rect::new(x, line.rect.y(), CARET_WIDTH, line.rect.height())
    }

    /// The position nearest `point`, on a grapheme boundary. A point above or below the text takes the first or last
    /// line, and one beside a line takes the nearest edge of the cluster at that side.
    pub fn index_at(&self, point: Point) -> TextPosition {
        let line = self
            .lines
            .iter()
            .find(|line| point.y < line.rect.max_y())
            .unwrap_or_else(|| &self.lines[self.lines.len() - 1]);
        let visible = &line.stops[..line
            .stops
            .partition_point(|stop| stop.start < line.visible_end)];
        let hit = visible
            .iter()
            .find(|stop| stop.left <= point.x && point.x < stop.right)
            .or_else(|| {
                let leftmost = visible.iter().min_by(|a, b| a.left.total_cmp(&b.left))?;
                if point.x < leftmost.left {
                    Some(leftmost)
                } else {
                    visible.iter().max_by(|a, b| a.right.total_cmp(&b.right))
                }
            });
        let Some(stop) = hit else {
            return TextPosition {
                byte: line.range.0.start,
                affinity: Affinity::Downstream,
            };
        };
        let byte = {
            std::iter::once(stop.start)
                .chain(
                    self.text[stop.start..stop.end]
                        .grapheme_indices(true)
                        .map(|(offset, _)| stop.start + offset)
                        .skip(1),
                )
                .chain(std::iter::once(stop.end))
                .min_by(|a, b| {
                    let da = (stop.x_at(*a) - point.x).abs();
                    let db = (stop.x_at(*b) - point.x).abs();
                    da.total_cmp(&db)
                })
                .unwrap_or(stop.start)
        };
        let downstream = TextPosition {
            byte,
            affinity: Affinity::Downstream,
        };
        let elsewhere = byte == stop.end && (line.x_at(downstream) - stop.x_at(byte)).abs() > 1e-9;
        TextPosition {
            byte,
            affinity: if (line.soft_wrapped && byte == line.range.0.end) || elsewhere {
                Affinity::Upstream
            } else {
                Affinity::Downstream
            },
        }
    }

    /// The rectangles that highlight `range` (logical bytes), left to right on each line it touches; a range that
    /// crosses a direction change can be several rectangles on one line.
    pub fn selection_rects(&self, range: Range<usize>) -> Vec<Rect> {
        let mut rects = Vec::new();
        for line in &self.lines {
            let start = range.start.max(line.range.0.start);
            let end = range.end.min(line.range.0.end);
            if start >= end {
                continue;
            }
            let first = line.stops.partition_point(|stop| stop.end <= start);
            let mut spans: Vec<(f64, f64)> = line.stops[first..]
                .iter()
                .take_while(|stop| stop.start < end)
                .map(|stop| {
                    let a = stop.x_at(start.max(stop.start));
                    let b = stop.x_at(end.min(stop.end));
                    (a.min(b), a.max(b))
                })
                .collect();
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut merged: Vec<(f64, f64)> = Vec::with_capacity(spans.len());
            for (left, right) in spans {
                match merged.last_mut() {
                    Some(last) if left <= last.1 => last.1 = last.1.max(right),
                    _ => merged.push((left, right)),
                }
            }
            rects.extend(merged.into_iter().map(|(left, right)| {
                Rect::new(left, line.rect.y(), right - left, line.rect.height())
            }));
        }
        rects
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

    fn ellipsized(text: &str, style: ParagraphStyle, width: Option<f32>) -> ParagraphLayout {
        layout_with(
            text,
            ParagraphStyle {
                ellipsis: true,
                ..style
            },
            width,
        )
    }

    fn width_of(text: &str) -> f64 {
        layout(text, None).size().width
    }

    #[test]
    fn a_line_wider_than_the_limit_ends_in_an_ellipsis_after_the_characters_that_fit() {
        let text = "alpha beta gamma delta";
        let fits = width_of("alpha beta") + width_of("\u{2026}");
        let style = ParagraphStyle {
            wrap: Wrap::None,
            ..ParagraphStyle::default()
        };
        let layout = ellipsized(text, style, Some(fits as f32 + 0.5));
        assert_eq!(line_texts(&layout, text), ["alpha beta"]);
        assert!(
            (layout.size().width - fits).abs() < 1e-3,
            "{:?}",
            layout.size()
        );
        let runs = &layout.lines()[0].runs;
        let ellipsis = runs.last().unwrap();
        assert_eq!(ellipsis.range, TextRange(10..10));
        assert_eq!(ellipsis.run.glyphs.len(), 1);
        assert!((ellipsis.origin.x - width_of("alpha beta")).abs() < 1e-3);
    }

    #[test]
    fn text_past_max_lines_ends_its_last_line_in_an_ellipsis() {
        let text = "a\nb\nc";
        let style = ParagraphStyle {
            max_lines: Some(2),
            ..ParagraphStyle::default()
        };
        let layout = ellipsized(text, style, None);
        assert_eq!(line_texts(&layout, text), ["a", "b"]);
        let ranges = |line: &Line| {
            line.runs
                .iter()
                .map(|run| run.range.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ranges(&layout.lines()[0]), [TextRange(0..1)]);
        assert_eq!(
            ranges(&layout.lines()[1]),
            [TextRange(2..3), TextRange(3..3)]
        );
    }

    #[test]
    fn text_that_fits_shows_no_ellipsis() {
        let text = "alpha beta";
        let layout = ellipsized(text, ParagraphStyle::default(), Some(500.0));
        assert_eq!(line_texts(&layout, text), [text]);
        assert!(layout.lines()[0]
            .runs
            .iter()
            .all(|run| !run.range.0.is_empty()));
    }

    #[test]
    fn a_right_to_left_line_shows_its_ellipsis_at_its_left_end() {
        let text = "alpha beta gamma delta";
        let kept = width_of("alpha beta");
        let limit = kept + width_of("\u{2026}") + 0.5;
        let style = ParagraphStyle {
            direction: Some(Direction::Rtl),
            wrap: Wrap::None,
            ..ParagraphStyle::default()
        };
        let layout = ellipsized(text, style, Some(limit as f32));
        let line = &layout.lines()[0];
        assert_eq!(&text[line.range.0.clone()], "alpha beta");
        let ellipsis = &line.runs[0];
        assert_eq!(ellipsis.range, TextRange(10..10));
        let edge = f64::from(limit as f32) - (kept + width_of("\u{2026}"));
        assert!(
            (ellipsis.origin.x - edge).abs() < 1e-3,
            "{}",
            ellipsis.origin.x
        );
        assert!((line.runs[1].origin.x - (f64::from(limit as f32) - kept)).abs() < 1e-3);
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
    fn left_and_right_alignment_hold_in_either_base_direction() {
        let text = "hi";
        let width = 200.0;
        let natural = layout(text, None).size().width;
        let place = |align, direction| {
            let style = ParagraphStyle {
                align,
                direction: Some(direction),
                ..ParagraphStyle::default()
            };
            layout_with(text, style, Some(width)).lines()[0].runs[0]
                .origin
                .x
        };
        for direction in [Direction::Ltr, Direction::Rtl] {
            assert_eq!(place(TextAlign::Left, direction), 0.0, "{direction:?}");
            let right = place(TextAlign::Right, direction);
            assert!(
                (right - (f64::from(width) - natural)).abs() < 1e-3,
                "{direction:?}"
            );
        }
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

    /// The cluster of every glyph of `line`, left to right.
    fn visual_clusters(line: &Line) -> Vec<usize> {
        line.runs
            .iter()
            .flat_map(|run| run.run.glyphs.iter().map(|glyph| glyph.cluster))
            .collect()
    }

    const MIXED: &str = "abc \u{5D0}\u{5D1}\u{5D2} def";

    #[test]
    fn right_to_left_characters_are_shown_last_first_inside_left_to_right_text() {
        let layout = layout(MIXED, None);
        let runs: Vec<Range<usize>> = layout.lines()[0]
            .runs
            .iter()
            .map(|run| run.range.0.clone())
            .collect();
        assert_eq!(runs, [0..4, 4..10, 10..14]);
        assert_eq!(
            visual_clusters(&layout.lines()[0]),
            [0, 1, 2, 3, 8, 6, 4, 10, 11, 12, 13]
        );
        let x = |byte| layout.caret_rect(position(byte)).x();
        assert!(
            x(4) > x(6) && x(6) > x(8),
            "the caret moves left through the Hebrew"
        );
        assert!(x(11) > x(4));
    }

    #[test]
    fn the_first_strong_character_sets_the_base_direction_and_start_is_its_edge() {
        let text = "\u{5D0}\u{5D1}\u{5D2} abc";
        let paragraph = system()
            .paragraph(
                &crate::AttributedText::plain(text, TextStyle::new(16.0)),
                &ParagraphStyle::default(),
            )
            .unwrap();
        assert_eq!(paragraph.direction(), Direction::Rtl);
        let layout = paragraph.layout(Some(200.0));
        assert_eq!(visual_clusters(&layout.lines()[0]), [7, 8, 9, 6, 4, 2, 0]);
        assert!((layout.caret_rect(position(0)).x() - 200.0).abs() < 1e-3);
        assert!((layout.lines()[0].runs[0].origin.x - (200.0 - layout.size().width)).abs() < 1e-3);

        let forced = layout_with(
            "abc",
            ParagraphStyle {
                direction: Some(Direction::Rtl),
                ..ParagraphStyle::default()
            },
            Some(200.0),
        );
        assert!((forced.caret_rect(position(3)).x() - 200.0).abs() < 1e-3);
        assert!((forced.caret_rect(position(0)).x() - (200.0 - forced.size().width)).abs() < 1e-3);
    }

    #[test]
    fn each_hard_line_takes_its_direction_from_its_own_first_strong_character() {
        let text = "abc\n\u{5D0}\u{5D1}\u{5D2}\n123\n\u{5D3}\u{5D4} xyz";
        let layout = layout(text, Some(200.0));
        let starts: Vec<f64> = layout
            .lines()
            .iter()
            .map(|line| layout.caret_rect(position(line.range.0.start)).x())
            .collect();
        assert!(starts[0].abs() < 1e-3, "{starts:?}");
        assert!(
            (starts[1] - 200.0).abs() < 1e-3,
            "the Hebrew line starts at the right"
        );
        assert!(
            starts[2].abs() < 1e-3,
            "digits alone keep the text's direction"
        );
        assert!((starts[3] - 200.0).abs() < 1e-3);
        assert_eq!(
            visual_clusters(&layout.lines()[3]),
            [20, 21, 22, 19, 17, 15],
            "Latin inside a right-to-left line sits at its left"
        );

        let forced = layout_with(
            text,
            ParagraphStyle {
                direction: Some(Direction::Ltr),
                ..ParagraphStyle::default()
            },
            Some(200.0),
        );
        assert!(
            forced.caret_rect(position(4)).x() < 100.0,
            "a set direction holds for every line"
        );
    }

    #[test]
    fn spaces_that_end_a_wrapped_line_hang_at_its_end_edge_not_inside_a_right_to_left_word() {
        let text = "abc \u{5D0}\u{5D1}\u{5D2} \u{5D3}\u{5D4}\u{5D5}";
        let width = layout("abc \u{5D0}\u{5D1}\u{5D2}", None).size().width as f32 + 1.0;
        let layout = layout(text, Some(width));
        assert_eq!(line_texts(&layout, text)[0], "abc \u{5D0}\u{5D1}\u{5D2} ");
        assert_eq!(
            visual_clusters(&layout.lines()[0]),
            [0, 1, 2, 3, 8, 6, 4, 10]
        );
    }

    #[test]
    fn right_from_the_end_of_abc_moves_the_caret_one_stop_rightward_through_the_hebrew() {
        let layout = layout(MIXED, None);
        let mut buffer = crate::TextBuffer::new();
        buffer.set_text(MIXED);
        buffer.select(3..3);
        let mut stops = vec![buffer.caret_position()];
        for _ in 0..5 {
            buffer.move_visual(true, false);
            stops.push(buffer.caret_position());
        }
        let bytes: Vec<usize> = stops.iter().map(|at| at.byte).collect();
        assert_eq!(bytes, [3, 4, 8, 6, 4, 11]);
        assert_eq!(
            stops[1].affinity,
            Affinity::Upstream,
            "after the space, before the Hebrew"
        );
        let xs: Vec<f64> = stops.iter().map(|at| layout.caret_rect(*at).x()).collect();
        assert!(xs.windows(2).all(|pair| pair[1] > pair[0]), "{xs:?}");
        for _ in 0..5 {
            buffer.move_visual(false, false);
        }
        assert_eq!(buffer.caret(), 3);
    }

    #[test]
    fn right_walks_each_hard_line_in_the_order_that_line_is_drawn() {
        let text = "abc\n\u{5D0}\u{5D1}\u{5D2}";
        let layout = layout(text, None);
        let mut buffer = crate::TextBuffer::multiline();
        buffer.set_text(text);
        buffer.select(text.len()..text.len());
        let mut stops = vec![buffer.caret_position()];
        for _ in 0..4 {
            buffer.move_visual(true, false);
            stops.push(buffer.caret_position());
        }
        let bytes: Vec<usize> = stops.iter().map(|at| at.byte).collect();
        assert_eq!(
            bytes,
            [10, 8, 6, 4, 3],
            "through the Hebrew to its start, then to the end of the line before"
        );
        let carets: Vec<Rect> = stops.iter().map(|at| layout.caret_rect(*at)).collect();
        assert!(
            carets[..4]
                .windows(2)
                .all(|pair| pair[1].x() > pair[0].x() && pair[1].y() == pair[0].y()),
            "{carets:?}"
        );
        assert!(carets[4].y() < carets[3].y(), "{carets:?}");
        buffer.move_visual(true, false);
        assert_eq!(buffer.caret(), 4, "Right past abc starts the next line");
        buffer.move_visual(false, false);
        assert_eq!(buffer.caret(), 6, "Left from the line's start goes into it");
    }

    #[test]
    fn a_word_move_right_through_hebrew_words_only_moves_the_caret_right() {
        let text = "abc \u{5D0}\u{5D1}\u{5D2} \u{5D3}\u{5D4}\u{5D5} xyz";
        let layout = layout(text, None);
        let mut buffer = crate::TextBuffer::new();
        buffer.set_text(text);
        buffer.select(3..3);
        let mut xs = vec![layout.caret_rect(buffer.caret_position()).x()];
        for _ in 0..3 {
            buffer.move_word_visual(true, false);
            xs.push(layout.caret_rect(buffer.caret_position()).x());
        }
        assert!(xs.windows(2).all(|pair| pair[1] > pair[0]), "{xs:?}");
        assert_eq!(xs[3], layout.size().width, "{xs:?}");
    }

    #[test]
    fn a_word_move_into_a_right_to_left_line_walks_its_words_leftward() {
        let text = "abc def\n\u{5D0}\u{5D1}\u{5D2} \u{5D3}\u{5D4}\u{5D5}";
        let layout = layout(text, None);
        let mut buffer = crate::TextBuffer::multiline();
        buffer.set_text(text);
        buffer.select(7..7);
        buffer.move_word_visual(true, false);
        assert_eq!(
            buffer.caret(),
            8,
            "Option+Right past def starts the next line"
        );
        let mut stops = vec![buffer.caret_position()];
        for _ in 0..2 {
            buffer.move_word_visual(false, false);
            stops.push(buffer.caret_position());
        }
        let bytes: Vec<usize> = stops.iter().map(|at| at.byte).collect();
        assert_eq!(
            bytes,
            [8, 14, 21],
            "each Hebrew word in turn, right to left"
        );
        let carets: Vec<Rect> = stops.iter().map(|at| layout.caret_rect(*at)).collect();
        assert!(
            carets
                .windows(2)
                .all(|pair| pair[1].x() < pair[0].x() && pair[1].y() == pair[0].y()),
            "{carets:?}"
        );
    }

    #[test]
    fn right_walks_latin_text_laid_out_right_to_left_in_the_order_it_is_drawn() {
        let text = "abc!";
        let style = ParagraphStyle {
            direction: Some(Direction::Rtl),
            ..ParagraphStyle::default()
        };
        let layout = layout_with(text, style, None);
        let mut buffer = crate::TextBuffer::new();
        buffer.set_text(text);
        buffer.set_direction(Some(Direction::Rtl));
        buffer.select(text.len()..text.len());
        for _ in 0..text.len() + 1 {
            buffer.move_visual(false, false);
        }
        let mut xs = vec![layout.caret_rect(buffer.caret_position()).x()];
        for _ in 0..text.len() + 1 {
            buffer.move_visual(true, false);
            xs.push(layout.caret_rect(buffer.caret_position()).x());
        }
        assert!((xs[0] - 0.0).abs() < 1e-6, "{xs:?}");
        assert!(
            xs.windows(2).take(text.len()).all(|pair| pair[1] > pair[0]),
            "every Right moves right on screen: {xs:?}"
        );
        assert_eq!(
            xs[text.len()],
            xs[text.len() + 1],
            "and stops at the right edge"
        );
    }

    #[test]
    fn a_click_at_a_direction_change_draws_the_caret_where_it_hit() {
        let layout = layout(MIXED, None);
        let after_space = layout.caret_rect(TextPosition {
            byte: 4,
            affinity: Affinity::Upstream,
        });
        assert!(after_space.x() < layout.caret_rect(position(4)).x());
        let hit = layout.index_at(Point::new(after_space.x() + 0.1, after_space.y() + 1.0));
        assert_eq!(
            hit,
            TextPosition {
                byte: 10,
                affinity: Affinity::Upstream
            },
            "the end of the Hebrew, drawn after the space"
        );
        assert!((layout.caret_rect(hit).x() - after_space.x()).abs() < 1e-9);
    }

    #[test]
    fn a_click_on_a_right_to_left_character_maps_its_right_half_to_its_start() {
        let layout = layout(MIXED, None);
        let (a, b) = (
            layout.caret_rect(position(4)).x(),
            layout.caret_rect(position(6)).x(),
        );
        let (left, right) = (a.min(b), a.max(b));
        let y = layout.lines()[0].rect.y() + 1.0;
        let at = |share: f64| layout.index_at(Point::new(left + (right - left) * share, y));
        assert_eq!(at(0.75).byte, 4);
        assert_eq!(at(0.25).byte, 6);
    }

    #[test]
    fn a_selection_across_a_direction_change_is_one_rectangle_per_visual_span() {
        let layout = layout(MIXED, None);
        let x = |byte| layout.caret_rect(position(byte)).x();
        let rects = layout.selection_rects(2..6);
        assert_eq!(rects.len(), 2, "{rects:?}");
        assert!((rects[0].x() - x(2)).abs() < 1e-6);
        assert!(
            rects[0].max_x() < rects[1].x(),
            "the other Hebrew letters sit between them"
        );
        assert!((rects[1].x() - x(6)).abs() < 1e-6 && (rects[1].max_x() - x(4)).abs() < 1e-6);
        assert_eq!(layout.selection_rects(0..MIXED.len()).len(), 1);
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

    /// Steps of layout work (glyphs grouped, clusters visited by the line breaker, clusters placed) to shape
    /// `length` characters and lay them out at two widths.
    fn steps_for(length: usize) -> usize {
        let sentence = "The quick brown fox jumps over the lazy dog and keeps running far away. ";
        let text: String = sentence.chars().cycle().take(length).collect();
        let attributed = crate::AttributedText::plain(text, TextStyle::new(16.0));
        work::take();
        let paragraph = system()
            .paragraph(&attributed, &ParagraphStyle::default())
            .unwrap();
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
