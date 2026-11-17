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
