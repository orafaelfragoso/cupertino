//! List view layout.
//!
//! Row height is `max(icon size, ceil(font ascent + descent)) + 4`; the
//! list view sets the table's intercell spacing to zero, so the pitch is the row
//! height: 20 pt for the default 16 pt icon and 13 pt font (derived from the font metrics). Columns
//! abut; a cell's text keeps its own inset from the column edge (the renderer's).


use crate::geom::Rect;
use crate::metrics::list_view as m;
use crate::settings::{ColumnDesc, ColumnId};

pub fn row_height(icon_size: u32, font_line: f64) -> f64 {
    (icon_size as f64).max(font_line.ceil()) + m::ROW_PADDING
}

pub fn row_pitch(icon_size: u32, font_line: f64) -> f64 {
    row_height(icon_size, font_line) + m::INTERCELL.1
}

/// Indentation per level: 15 pt in a hierarchy, 1 pt when the list is flat (grouped / searching).
pub fn indentation(hierarchical: bool) -> f64 {
    if hierarchical {
        m::INDENT_PER_LEVEL
    } else {
        m::INDENT_FLAT
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowSpec {
    /// Nesting level (0 for top level rows).
    pub level: u32,
    pub expandable: bool,
    /// Its items are shown under it (the triangle points down).
    pub expanded: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowRect {
    pub index: usize,
    pub frame: Rect,
    pub level: u32,
    /// Disclosure triangle (empty for leaves), icon and text origin inside the name column.
    pub disclosure: Option<Rect>,
    pub expanded: bool,
    pub icon: Rect,
    pub text_x: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnRect {
    pub id: ColumnId,
    pub frame: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListLayout {
    pub header: Rect,
    pub columns: Vec<ColumnRect>,
    pub rows: Vec<RowRect>,
    pub row_height: f64,
    pub content_width: f64,
    pub content_height: f64,
}

pub struct ListParams<'a> {
    pub icon_size: u32,
    pub font_line: f64,
    pub hierarchical: bool,
    pub columns: &'a [ColumnDesc],
    pub viewport_width: f64,
}

/// Width of the disclosure triangle area (approximate; the control metric is not part of the
/// data).
pub const DISCLOSURE: f64 = 12.0;
/// Gap between icon and text in the name cell. approximate (the template's 5 pt horizontal intercell spacing, the
/// cell view's own layout is not known).
pub const ICON_TEXT_GAP: f64 = m::INTERCELL_TEMPLATE.0;

pub fn layout(p: &ListParams<'_>, rows: &[RowSpec]) -> ListLayout {
    let visible: Vec<&ColumnDesc> = p.columns.iter().filter(|c| c.visible).collect();
    let mut x = 0.0;
    let mut columns = Vec::new();
    for c in &visible {
        columns.push(ColumnRect { id: c.id, frame: Rect::new(x, 0.0, c.width, m::HEADER_HEIGHT) });
        x += c.width + m::INTERCELL.0;
    }
    let content_width = (x - m::INTERCELL.0).max(0.0).max(p.viewport_width.min(x));
    let rh = row_height(p.icon_size, p.font_line);
    let pitch = rh + m::INTERCELL.1;
    let indent = indentation(p.hierarchical);
    let icon = p.icon_size as f64;
    let mut out = Vec::with_capacity(rows.len());
    for (i, r) in rows.iter().enumerate() {
        let y = m::HEADER_HEIGHT + i as f64 * pitch;
        let lvl_x = r.level as f64 * indent;
        let disclosure = (r.expandable && p.hierarchical).then(|| Rect::new(lvl_x, y + (rh - DISCLOSURE) / 2.0, DISCLOSURE, DISCLOSURE));
        let icon_x = (r.level as f64 + 1.0) * indent;
        out.push(RowRect {
            index: i,
            frame: Rect::new(0.0, y, content_width, rh),
            level: r.level,
            disclosure,
            expanded: r.expanded,
            icon: Rect::new(icon_x, y + (rh - icon) / 2.0, icon, icon),
            text_x: icon_x + icon + ICON_TEXT_GAP,
        });
    }
    ListLayout { header: Rect::new(0.0, 0.0, content_width, m::HEADER_HEIGHT), columns, rows: out, row_height: rh, content_width, content_height: m::HEADER_HEIGHT + rows.len() as f64 * pitch }
}

impl ListLayout {
    /// Row at vertical position `y` (content coordinates), counting the intercell gap as part of the row above.
    pub fn row_at(&self, y: f64) -> Option<usize> {
        if y < m::HEADER_HEIGHT || self.rows.is_empty() {
            return None;
        }
        let pitch = self.row_height + m::INTERCELL.1;
        let i = ((y - m::HEADER_HEIGHT) / pitch).floor() as usize;
        (i < self.rows.len()).then_some(i)
    }

    /// Rows intersecting the vertical span `[y0, y1)`, for virtualised drawing.
    pub fn visible_rows(&self, y0: f64, y1: f64) -> std::ops::Range<usize> {
        let pitch = self.row_height + m::INTERCELL.1;
        let first = (((y0 - m::HEADER_HEIGHT) / pitch).floor().max(0.0)) as usize;
        let last = (((y1 - m::HEADER_HEIGHT) / pitch).ceil().max(0.0)) as usize;
        first.min(self.rows.len())..last.min(self.rows.len())
    }

    /// Alternating background: odd rows are tinted (the default of the table).
    pub fn is_alternate(index: usize) -> bool {
        index % 2 == 1
    }
}

/// Minimum width of the name column so that the icon, indentation and a few characters fit
/// (approximate; the design sums font-dependent terms).
pub fn min_name_column_width(icon_size: u32, text_size: f64, deepest_level: u32, hierarchical: bool) -> f64 {
    (deepest_level as f64 + 1.0) * indentation(hierarchical) + icon_size as f64 + ICON_TEXT_GAP + text_size * 6.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::default_columns;

    #[test]
    fn row_height_formula() {
        assert_eq!(row_height(16, 15.5), 20.0);
        assert_eq!(row_height(16, 12.0), 20.0);
        assert_eq!(row_height(32, 15.5), 36.0);
        assert_eq!(row_height(16, 20.2), 25.0);
        assert_eq!(row_pitch(16, 15.5), 20.0, "no intercell spacing");
    }

    #[test]
    fn columns_abut_and_rows_follow_at_the_row_height() {
        let cols = default_columns();
        let rows: Vec<RowSpec> = (0..5).map(|i| RowSpec { level: (i % 2) as u32, expandable: i == 0, expanded: false }).collect();
        let l = layout(&ListParams { icon_size: 16, font_line: 15.0, hierarchical: true, columns: &cols, viewport_width: 800.0 }, &rows);
        assert_eq!(l.columns.len(), 4);
        assert_eq!(l.columns[0].frame.x, 0.0);
        assert_eq!(l.columns[1].frame.x, 300.0);
        assert_eq!(l.columns[3].frame.right(), 300.0 + 181.0 + 97.0 + 115.0);
        assert_eq!(l.rows[1].frame.y, 28.0 + 20.0);
        assert_eq!(l.content_height, 28.0 + 5.0 * 20.0);
    }

    #[test]
    fn indentation_and_disclosure() {
        let cols = default_columns();
        let rows = [RowSpec { level: 0, expandable: true, expanded: true }, RowSpec { level: 2, expandable: false, expanded: false }];
        let l = layout(&ListParams { icon_size: 16, font_line: 15.0, hierarchical: true, columns: &cols, viewport_width: 800.0 }, &rows);
        assert_eq!(l.rows[0].disclosure.unwrap().x, 0.0);
        assert!(l.rows[1].disclosure.is_none());
        assert!(l.rows[0].expanded && !l.rows[1].expanded);
        // 15 pt per level: the icon of a level-1 row starts at 30
        let child = layout(&ListParams { icon_size: 16, font_line: 15.0, hierarchical: true, columns: &cols, viewport_width: 800.0 }, &[RowSpec { level: 1, expandable: true, expanded: false }]);
        assert_eq!((child.rows[0].disclosure.unwrap().x, child.rows[0].icon.x), (15.0, 30.0));
        assert_eq!(l.rows[1].icon.x, 45.0);
        assert_eq!(l.rows[1].text_x, 45.0 + 16.0 + 5.0);
        let flat = layout(&ListParams { icon_size: 16, font_line: 15.0, hierarchical: false, columns: &cols, viewport_width: 800.0 }, &rows);
        assert_eq!(flat.rows[1].icon.x, 3.0);
        assert!(flat.rows[0].disclosure.is_none());
    }

    #[test]
    fn row_lookup_and_virtualisation() {
        let cols = default_columns();
        let rows: Vec<RowSpec> = (0..100).map(|_| RowSpec { level: 0, expandable: false, expanded: false }).collect();
        let l = layout(&ListParams { icon_size: 16, font_line: 15.0, hierarchical: true, columns: &cols, viewport_width: 800.0 }, &rows);
        assert_eq!(l.row_at(10.0), None);
        assert_eq!(l.row_at(28.0), Some(0));
        assert_eq!(l.row_at(28.0 + 20.0 * 3.0 + 19.0), Some(3));
        assert_eq!(l.row_at(1e9), None);
        assert_eq!(l.visible_rows(28.0, 28.0 + 20.0 * 10.0), 0..10);
        assert_eq!(l.visible_rows(28.0 + 20.0 * 95.0, 1e9), 95..100);
        assert!(ListLayout::is_alternate(1) && !ListLayout::is_alternate(2));
    }

    #[test]
    fn hidden_columns_are_skipped() {
        let mut cols = default_columns();
        cols[1].visible = false;
        let l = layout(&ListParams { icon_size: 16, font_line: 15.0, hierarchical: true, columns: &cols, viewport_width: 800.0 }, &[]);
        assert_eq!(l.columns.iter().map(|c| c.id).collect::<Vec<_>>(), [ColumnId::Name, ColumnId::Size, ColumnId::Kind]);
    }
}
