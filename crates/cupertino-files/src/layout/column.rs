//! Column (browser) view layout.
//!
//! Columns have the preferred width of the settings (245 pt by default, minimum 100 pt, both
//! ); the last column of a file selection is replaced by the preview column (312 pt wide,
//! template). Rows are `icon size + 6` pt high.

use crate::geom::Rect;
use crate::metrics::column_view as m;

pub fn row_height(icon_size: u32) -> f64 {
    icon_size as f64 + m::ROW_EXTRA
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnLayout {
    pub columns: Vec<Rect>,
    pub preview: Option<Rect>,
    pub content_width: f64,
    /// Horizontal scroll offset that reveals the last column (and the preview).
    pub scroll_to_end: f64,
}

/// `widths` are the (possibly user-resized) widths of the folder columns; `show_preview` appends the preview column.
pub fn layout(widths: &[f64], show_preview: bool, viewport_width: f64) -> ColumnLayout {
    let mut x = 0.0;
    let mut columns = Vec::with_capacity(widths.len());
    for w in widths {
        let w = w.max(m::MIN_WIDTH);
        columns.push(Rect::new(x, 0.0, w, 0.0));
        x += w;
    }
    let preview = show_preview.then(|| {
        let r = Rect::new(x, 0.0, m::PREVIEW_COLUMN_WIDTH, 0.0);
        x += m::PREVIEW_COLUMN_WIDTH;
        r
    });
    ColumnLayout { columns, preview, content_width: x, scroll_to_end: (x - viewport_width).max(0.0) }
}

/// Width available to the preview image inside the preview column.
pub fn preview_image_width() -> f64 {
    m::PREVIEW_COLUMN_WIDTH - 2.0 * m::PREVIEW_SIDE_INSET
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_abut_and_preview_follows() {
        let l = layout(&[245.0, 245.0, 80.0], true, 600.0);
        assert_eq!(l.columns[1].x, 245.0);
        assert_eq!(l.columns[2].w, 100.0);
        assert_eq!(l.preview.unwrap().x, 590.0);
        assert_eq!(l.content_width, 902.0);
        assert_eq!(l.scroll_to_end, 302.0);
        assert_eq!(layout(&[245.0], false, 600.0).scroll_to_end, 0.0);
    }

    #[test]
    fn row_and_preview_metrics() {
        assert_eq!(row_height(16), 22.0);
        assert_eq!(preview_image_width(), 292.0);
    }
}
