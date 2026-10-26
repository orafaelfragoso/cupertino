//! Gallery view layout: a large preview above a strip of thumbnails.
//!
//! The strip cell is found like in the design: starting from the smallest grid-spacing
//! parameter, increase it by one until the label is at least 90 pt wide (only when titles are on),
//! at most up to 100. The cell then follows the icon-view cell computation with the label below.

use super::icon::{ideal_cell_size, FontMetrics, IconParams};
use crate::geom::{Rect, Size};
use crate::metrics::gallery as m;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripMetrics {
    pub cell: Size,
    pub grid_spacing: f64,
    pub inter_item: f64,
    /// Bottom inset of the strip section: 2 pt with titles, 5 pt without.
    pub bottom_inset: f64,
}

pub fn strip_metrics(
    icon_size: u32,
    text_size: u16,
    show_titles: bool,
    font: FontMetrics,
) -> StripMetrics {
    let mut spacing = m::SPACING_STEP;
    loop {
        let mut p = IconParams::new(icon_size, spacing as u32, text_size, true, false, font);
        p.title_enabled = show_titles;
        let cell = ideal_cell_size(&p);
        // the loop condition of the design compares the ideal width with 90 pt
        if !show_titles
            || cell.w >= m::MIN_TITLE_WIDTH
            || spacing + m::SPACING_STEP > m::SPACING_MAX
        {
            return StripMetrics {
                cell,
                grid_spacing: spacing,
                inter_item: m::MIN_INTER_ITEM_SPACING,
                bottom_inset: if show_titles { 2.0 } else { 5.0 },
            };
        }
        spacing += m::SPACING_STEP;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GalleryLayout {
    pub preview: Rect,
    pub strip: Rect,
    pub thumbs: Vec<Rect>,
    /// Scroll offset keeping `selected` centred in the strip.
    pub strip_scroll: f64,
}

/// `viewport` is the content area; the preview takes `PREVIEW_SIZE` at most and keeps the 32 pt side insets.
pub fn layout(
    viewport: Size,
    count: usize,
    selected: Option<usize>,
    strip: &StripMetrics,
) -> GalleryLayout {
    let strip_h = strip.cell.h + strip.bottom_inset + m::MIN_INTER_ITEM_SPACING;
    let preview_h = (viewport.h - strip_h).max(0.0);
    let side = ((m::PREVIEW_CONTAINER.0 - m::PREVIEW_SIZE) / 2.0).min(viewport.w / 4.0);
    let ps = m::PREVIEW_SIZE
        .min(preview_h)
        .min(viewport.w - 2.0 * side)
        .max(0.0);
    let preview = Rect::new((viewport.w - ps) / 2.0, (preview_h - ps) / 2.0, ps, ps);
    let strip_rect = Rect::new(0.0, preview_h, viewport.w, strip_h);
    let mut thumbs = Vec::with_capacity(count);
    for i in 0..count {
        thumbs.push(Rect::new(
            m::MIN_INTER_ITEM_SPACING + i as f64 * (strip.cell.w + strip.inter_item),
            preview_h + m::MIN_INTER_ITEM_SPACING / 2.0,
            strip.cell.w,
            strip.cell.h,
        ));
    }
    let total = m::MIN_INTER_ITEM_SPACING * 2.0 + count as f64 * (strip.cell.w + strip.inter_item)
        - strip.inter_item;
    let strip_scroll = match selected.and_then(|i| thumbs.get(i)) {
        Some(t) => (t.center().x - viewport.w / 2.0).clamp(0.0, (total - viewport.w).max(0.0)),
        None => 0.0,
    };
    GalleryLayout {
        preview,
        strip: strip_rect,
        thumbs,
        strip_scroll,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> FontMetrics {
        FontMetrics {
            char_width: 8.0,
            line_height: 15.0,
            subtitle_line_height: 13.0,
        }
    }

    #[test]
    fn strip_grows_until_the_label_is_wide_enough() {
        let m = strip_metrics(32, 12, true, font());
        assert!(m.cell.w >= 90.0);
        assert!(m.grid_spacing >= 1.0 && m.grid_spacing <= 100.0);
        // one step less is too narrow (unless the search started at the minimum)
        if m.grid_spacing > 1.0 {
            let mut p = IconParams::new(32, m.grid_spacing as u32 - 1, 12, true, false, font());
            p.title_enabled = true;
            assert!(ideal_cell_size(&p).w < 90.0);
        }
        assert_eq!(m.bottom_inset, 2.0);
    }

    #[test]
    fn without_titles_the_search_stops_immediately() {
        let m = strip_metrics(32, 12, false, font());
        assert_eq!(m.grid_spacing, 1.0);
        assert_eq!(m.bottom_inset, 5.0);
    }

    #[test]
    fn preview_is_square_and_centred() {
        let s = strip_metrics(48, 12, true, font());
        let l = layout(Size { w: 900.0, h: 700.0 }, 20, Some(10), &s);
        assert_eq!(l.preview.w, l.preview.h);
        assert!(l.preview.w <= 512.0);
        assert!((l.preview.center().x - 450.0).abs() < 1e-9);
        assert!(l.strip_scroll > 0.0);
        assert_eq!(l.thumbs.len(), 20);
        let l2 = layout(Size { w: 300.0, h: 200.0 }, 3, Some(0), &s);
        assert!(l2.preview.w <= 200.0);
    }
}
