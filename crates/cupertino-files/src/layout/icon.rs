//! Icon view layout.
//!
//! [`ideal_cell_size`] ports the cell-size computation of the design: padding around the icon,
//! label width interpolated by the grid-spacing parameter, label height from a line count that
//! ramps with the same parameter, and the fixed additions of the two label positions. All
//! constants are as designed; font metrics (character
//! width of "0" rounded up, line height) come from the caller because they depend on the font.

use crate::geom::{Direction, Point, Rect, Size};
use crate::metrics::icon_view as m;

/// Font measures the cell computation needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    /// Width of the digit "0" at the label size, rounded up.
    pub char_width: f64,
    pub line_height: f64,
    pub subtitle_line_height: f64,
}

impl FontMetrics {
    /// Estimate for a proportional UI font when no shaper is available (approximate: digit width
    /// 0.62 em, line height 1.2 em).
    pub fn estimate(text_size: f64) -> Self {
        let sub = (text_size + m::SUBTITLE_SIZE_DELTA).max(m::SUBTITLE_MIN_SIZE);
        FontMetrics {
            char_width: (text_size * 0.62).ceil(),
            line_height: (text_size * 1.2).ceil(),
            subtitle_line_height: (sub * 1.2).ceil(),
        }
    }
}

/// Hidden-preference knobs of the label size (defaults).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LabelPrefs {
    pub max_chars: u32,
    pub min_chars: u32,
    pub extra_width: u32,
    pub gap: u32,
    pub lines_max: u32,
    pub lines_min: u32,
}

impl Default for LabelPrefs {
    fn default() -> Self {
        LabelPrefs {
            max_chars: m::LABEL_MAX_CHARS,
            min_chars: 0,
            extra_width: m::LABEL_EXTRA_WIDTH,
            gap: m::LABEL_GAP,
            lines_max: 2,
            lines_min: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconParams {
    pub icon_size: f64,
    /// 1..=100, default 54.
    pub grid_spacing: f64,
    pub text_size: f64,
    pub label_on_bottom: bool,
    pub title_enabled: bool,
    pub subtitle_enabled: bool,
    pub font: FontMetrics,
    pub prefs: LabelPrefs,
}

impl IconParams {
    pub fn new(
        icon_size: u32,
        grid_spacing: u32,
        text_size: u16,
        label_on_bottom: bool,
        show_item_info: bool,
        font: FontMetrics,
    ) -> Self {
        IconParams {
            icon_size: icon_size.clamp(m::ICON_SIZE_MIN, m::ICON_SIZE_MAX) as f64,
            grid_spacing: grid_spacing.clamp(m::GRID_SPACING_MIN, m::GRID_SPACING_MAX) as f64,
            text_size: text_size.clamp(m::TEXT_SIZE_MIN, m::TEXT_SIZE_MAX) as f64,
            label_on_bottom,
            title_enabled: true,
            subtitle_enabled: show_item_info,
            font,
            prefs: LabelPrefs::default(),
        }
    }

    fn pad(&self) -> f64 {
        m::PAD_FRACTION * self.icon_size
    }

    /// Icon box: the icon plus its padding.
    pub fn icon_box(&self) -> Size {
        Size {
            w: self.icon_size + self.pad(),
            h: self.icon_size + self.pad(),
        }
    }
}

/// Number of label lines (a real number, 1.0..=2.0) as a function of the grid spacing.
pub fn max_title_lines(p: &IconParams) -> f64 {
    if !p.label_on_bottom && p.icon_size <= m::MIN_ICON {
        return 1.0;
    }
    let (lo, hi) = (p.prefs.lines_min as f64, p.prefs.lines_max as f64);
    let t = if p.grid_spacing < 1.0 {
        p.grid_spacing
    } else {
        p.grid_spacing - 1.0
    };
    let lines = if t < 25.0 {
        lo + (hi - lo) * (t / 25.0)
    } else {
        hi
    };
    if lines < 1.0 {
        1.0
    } else {
        lines.min(2.0)
    }
}

/// Label block: `(width incl. extra, height, width incl. the 6 pt bubble padding)`.
pub fn max_text_size(p: &IconParams) -> (f64, f64, f64) {
    let cw = p.font.char_width;
    let chars = p.prefs.max_chars.clamp(5, 100) as f64;
    let mut w0 = chars * cw;
    if p.grid_spacing < 100.0 {
        let min_chars = p.prefs.min_chars.max(m::LABEL_MIN_CHARS) as f64;
        w0 = (w0 - min_chars * cw) / 100.0 * p.grid_spacing + min_chars * cw;
    }
    if p.label_on_bottom {
        w0 = w0.max(p.icon_box().w);
    }
    let mut h = 0.0;
    if p.title_enabled {
        h = (p.font.line_height * max_title_lines(p)).ceil();
    }
    if p.subtitle_enabled {
        h += p.font.subtitle_line_height.ceil();
    }
    let extra = if p.prefs.extra_width == 0 {
        1
    } else {
        p.prefs.extra_width.min(100)
    } as f64;
    let w = w0.ceil() + extra;
    (w, h, w + m::LABEL_WIDTH_PAD)
}

/// Size of one grid cell.
pub fn ideal_cell_size(p: &IconParams) -> Size {
    let b = p.icon_box();
    let (w, h);
    if !p.title_enabled && !p.subtitle_enabled {
        (w, h) = (b.w, b.h);
    } else {
        let (_, text_h, text_w) = max_text_size(p);
        let g = if p.prefs.gap == 0 {
            1
        } else {
            p.prefs.gap.min(100)
        } as f64;
        if p.label_on_bottom {
            w = text_w;
            h = b.h + m::BOTTOM_LABEL_PAD.0 + text_h + g + m::BOTTOM_LABEL_PAD.1;
        } else {
            w = b.w + text_w + m::RIGHT_LABEL_PAD;
            let block = text_h.max(b.h);
            h = if p.icon_size > m::MIN_ICON {
                block + 3.0 * g
            } else if b.h <= text_h + g {
                block + 2.0 * g
            } else {
                block + (g as u32 / 2) as f64
            };
        }
    }
    let (mut w, mut h) = (w.ceil(), h.ceil());
    w += w % 2.0;
    h += h % 2.0;
    Size { w, h }
}

/// Horizontal and vertical spacing between cells of the arranged (flow) layout. The two numbers
/// are as designed, the assignment to axes is.
pub fn inter_item_spacing(icon_size: f64, label_on_bottom: bool) -> (f64, f64) {
    if label_on_bottom {
        (2.0, (icon_size * 0.0625).ceil())
    } else {
        (
            (icon_size * 0.0625).ceil().max(2.0),
            (icon_size * 0.0625).ceil().max(2.0),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IconCell {
    pub index: usize,
    pub frame: Rect,
    pub icon_frame: Rect,
    pub label_frame: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SectionSpec {
    pub title: Option<String>,
    pub count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Flow {
    /// Cells are packed from the leading edge.
    Leading,
    /// Free width is distributed equally around the cells (the design's "justified" layout).
    Justified,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IconLayout {
    pub cell: Size,
    pub spacing: (f64, f64),
    pub columns: usize,
    pub cells: Vec<IconCell>,
    pub headers: Vec<(String, Rect)>,
    pub content: Size,
}

/// Margin at the grid's leading and trailing sides, and above its first row and below its last.
pub const MARGIN: f64 = m::GRID_INSET.0;
pub const TOP_MARGIN: f64 = m::GRID_INSET.1;

/// Flow layout of `sections` in a viewport of `width` points.
pub fn layout(p: &IconParams, width: f64, sections: &[SectionSpec], flow: Flow) -> IconLayout {
    let cell = ideal_cell_size(p);
    let spacing = inter_item_spacing(p.icon_size, p.label_on_bottom);
    let avail = (width - 2.0 * MARGIN).max(cell.w);
    let columns = (((avail + spacing.0) / (cell.w + spacing.0)).floor() as usize).max(1);
    let lead = match flow {
        Flow::Leading => MARGIN,
        Flow::Justified => {
            let free =
                (avail - columns as f64 * cell.w - (columns as f64 - 1.0) * spacing.0).max(0.0);
            MARGIN + free / (columns as f64 + 1.0)
        }
    };
    let gap_x = match flow {
        Flow::Leading => spacing.0,
        Flow::Justified => {
            let free =
                (avail - columns as f64 * cell.w - (columns as f64 - 1.0) * spacing.0).max(0.0);
            spacing.0 + free / (columns as f64 + 1.0)
        }
    };
    let mut cells = Vec::new();
    let mut headers = Vec::new();
    let mut y = TOP_MARGIN;
    let mut index = 0;
    for s in sections {
        if let Some(t) = &s.title {
            headers.push((t.clone(), Rect::new(0.0, y, width, m::GROUP_HEADER_HEIGHT)));
            y += m::GROUP_HEADER_HEIGHT;
        }
        for k in 0..s.count {
            let (col, row) = (k % columns, k / columns);
            let frame = Rect::new(
                lead + col as f64 * (cell.w + gap_x),
                y + row as f64 * (cell.h + spacing.1),
                cell.w,
                cell.h,
            );
            cells.push(cell_parts(p, index, frame, cell));
            index += 1;
        }
        let rows = s.count.div_ceil(columns);
        if rows > 0 {
            y += rows as f64 * cell.h + (rows as f64 - 1.0) * spacing.1 + spacing.1;
        }
    }
    IconLayout {
        cell,
        spacing,
        columns,
        cells,
        headers,
        content: Size {
            w: width,
            h: y + TOP_MARGIN,
        },
    }
}

/// Room between two pictures of a strip, and at its leading and trailing ends.
pub const STRIP_SPACING: f64 = 5.0;
/// Room above and below the pictures of a strip.
pub const STRIP_INSET: f64 = 8.0;

/// One row of `count` pictures `p.icon_size` square without names, from the leading edge: the strip under a gallery's
/// preview. Each cell is the icon with its padding.
pub fn strip(p: &IconParams, count: usize) -> IconLayout {
    let p = IconParams {
        title_enabled: false,
        subtitle_enabled: false,
        ..*p
    };
    let cell = ideal_cell_size(&p);
    let cells = (0..count)
        .map(|index| {
            let x = STRIP_SPACING + index as f64 * (cell.w + STRIP_SPACING);
            let frame = Rect::new(x, STRIP_INSET, cell.w, cell.h);
            let mut parts = cell_parts(&p, index, frame, cell);
            // no name: an empty label under the picture, which nothing hits
            parts.label_frame = Rect::new(frame.x, frame.bottom(), 0.0, 0.0);
            parts
        })
        .collect();
    IconLayout {
        cell,
        spacing: (STRIP_SPACING, 0.0),
        columns: count.max(1),
        cells,
        headers: Vec::new(),
        content: Size {
            w: STRIP_SPACING + count as f64 * (cell.w + STRIP_SPACING),
            h: cell.h + 2.0 * STRIP_INSET,
        },
    }
}

/// Icon and label rectangles inside a cell.
pub fn cell_parts(p: &IconParams, index: usize, frame: Rect, cell: Size) -> IconCell {
    let b = p.icon_box();
    let (_, text_h, text_w) = max_text_size(p);
    let icon_sz = p.icon_size;
    if p.label_on_bottom {
        let box_x = frame.x + (cell.w - b.w) / 2.0;
        let icon_frame = Rect::new(
            box_x + (b.w - icon_sz) / 2.0,
            frame.y + (b.h - icon_sz) / 2.0,
            icon_sz,
            icon_sz,
        );
        let lw = (text_w - m::LABEL_WIDTH_PAD).min(cell.w);
        let label_frame = Rect::new(
            frame.x + (cell.w - lw) / 2.0,
            frame.y + b.h + m::BOTTOM_LABEL_PAD.0,
            lw,
            text_h,
        );
        IconCell {
            index,
            frame,
            icon_frame,
            label_frame,
        }
    } else {
        let icon_frame = Rect::new(
            frame.x + (b.w - icon_sz) / 2.0,
            frame.y + (cell.h - icon_sz) / 2.0,
            icon_sz,
            icon_sz,
        );
        let label_frame = Rect::new(
            frame.x + b.w + m::RIGHT_LABEL_PAD,
            frame.y + (cell.h - text_h) / 2.0,
            text_w - m::LABEL_WIDTH_PAD,
            text_h,
        );
        IconCell {
            index,
            frame,
            icon_frame,
            label_frame,
        }
    }
}

impl IconLayout {
    /// Item whose icon or label contains `pt` (the design hit-tests icon and text, not the whole cell).
    pub fn hit(&self, pt: Point) -> Option<usize> {
        self.cells
            .iter()
            .find(|c| c.icon_frame.contains(pt) || c.label_frame.contains(pt))
            .map(|c| c.index)
    }

    /// Items whose icon or label intersects the rubber-band rectangle.
    pub fn hits_in_rect(&self, r: &Rect) -> Vec<usize> {
        self.cells
            .iter()
            .filter(|c| c.icon_frame.intersects(r) || c.label_frame.intersects(r))
            .map(|c| c.index)
            .collect()
    }

    /// Neighbour for arrow-key navigation: the nearest cell strictly in `dir`, preferring the same
    /// row / column.
    pub fn neighbour(&self, from: usize, dir: Direction) -> Option<usize> {
        let a = self.cells.get(from)?.frame.center();
        self.cells
            .iter()
            .filter(|c| c.index != from)
            .filter_map(|c| {
                let p = c.frame.center();
                let (along, across) = match dir {
                    Direction::Right => (p.x - a.x, (p.y - a.y).abs()),
                    Direction::Left => (a.x - p.x, (p.y - a.y).abs()),
                    Direction::Down => (p.y - a.y, (p.x - a.x).abs()),
                    Direction::Up => (a.y - p.y, (p.x - a.x).abs()),
                };
                (along > 0.5).then_some((c.index, across * 4.0 + along))
            })
            .min_by(|x, y| x.1.partial_cmp(&y.1).unwrap())
            .map(|x| x.0)
    }

    /// Frame of a cell, for scrolling it into view or placing the quick-preview origin.
    pub fn frame_of(&self, index: usize) -> Option<Rect> {
        self.cells.get(index).map(|c| c.frame)
    }
}

/// Free-form (desktop) grid: cell at a grid location and snapping a point to it. margins.
#[derive(Clone, Copy, Debug)]
pub struct FreeGrid {
    pub bounds: Rect,
    pub cell: Size,
    pub margin: f64,
}

impl FreeGrid {
    pub fn new(bounds: Rect, cell: Size) -> Self {
        FreeGrid {
            bounds,
            cell,
            margin: m::FREE_GRID_MARGIN,
        }
    }
    pub fn dimensions(&self) -> (usize, usize) {
        let w = self.bounds.w - 2.0 * self.margin;
        let h = self.bounds.h - 2.0 * self.margin;
        (
            ((w / self.cell.w).floor() as usize).max(1),
            ((h / self.cell.h).floor() as usize).max(1),
        )
    }
    /// Grid location nearest to `pt` (column, row), clamped to the grid.
    pub fn nearest(&self, pt: Point) -> (usize, usize) {
        let (cols, rows) = self.dimensions();
        let c = ((pt.x - self.bounds.x - self.margin) / self.cell.w)
            .round()
            .clamp(0.0, cols as f64 - 1.0);
        let r = ((pt.y - self.bounds.y - self.margin) / self.cell.h)
            .round()
            .clamp(0.0, rows as f64 - 1.0);
        (c as usize, r as usize)
    }
    pub fn cell_frame(&self, col: usize, row: usize) -> Rect {
        Rect::new(
            self.bounds.x + self.margin + col as f64 * self.cell.w,
            self.bounds.y + self.margin + row as f64 * self.cell.h,
            self.cell.w,
            self.cell.h,
        )
    }
}
