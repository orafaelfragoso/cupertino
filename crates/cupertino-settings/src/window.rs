//! The settings window (§4.1): its size, the fixed sidebar column under a material
//! the server draws, the 1 pt divider and the content column with its title region, and the hit test.
//!
//! The surface is the whole window (the unified title bar style): the server draws the window control buttons over
//! the sidebar's top. The title region over the content column is not a strip: the content colour runs to the window's
//! top with no separator, the pane title sits on the window controls' line, and the back / forward group shows only
//! once there is history (a screenshot;: the window sets no
//! separator style). Values per row of the spec table; the region's height, the back / forward group and the
//! sidebar's top inset are approximate placeholders until O6 closes.

use crate::geom::{Point, Rect, Size};
use crate::pane::PaneId;
use crate::sidebar::{row, RowSize};

/// Window metrics (§4.1, §4.4).
pub mod metrics {
    /// The default content width before the scale factor and the height.
    pub const DEFAULT_WIDTH: f64 = 715.0;
    pub const DEFAULT_HEIGHT: f64 = 625.0;
    /// The sidebar for small and medium rows before the scale factor, the extra for large rows, and the extra for
    /// always-shown scroll bars by row size.
    pub const SIDEBAR_WIDTH: f64 = 215.0;
    pub const SIDEBAR_LARGE_EXTRA: f64 = 12.0;
    pub const SIDEBAR_SCROLLER_EXTRA: [f64; 3] = [0.0, 8.0, 15.0];
    /// Between the sidebar and the content (role ).
    pub const DIVIDER: f64 = 1.0;
    /// Smallest window height: approximate (the standard panel's minimum content height, by analogy; O6).
    pub const MIN_HEIGHT: f64 = 400.0;
    /// The title region over the content column, centred on the window control buttons: approximate (the file
    /// manager's 52 pt bar, whose centre the server's buttons use; this window's is unknown, O6). Not drawn.
    pub const TOOLBAR_HEIGHT: f64 = 52.0;
    /// The back / forward group: approximate (the file manager's 53 × 28 record, N3).
    pub const BACK_FORWARD: (f64, f64) = (53.0, 28.0);
    /// The toolbar controls' inset from the strip's leading edge (the file manager's, approximate here).
    pub const TOOLBAR_INSET: f64 = 8.0;
    /// Between the back / forward group and the title. approximate.
    pub const TITLE_GAP: f64 = 12.0;
    /// The title's distance from the content column's leading edge while the back / forward group is hidden
    /// (approximate: 13 pt measured on a screenshot).
    pub const TITLE_LEADING: f64 = 13.0;
    /// The pane title: the Title3 text style, 15 pt, of a unified toolbar window without a subtitle.
    /// Its weight trait was not resolved: semibold is approximate.
    pub const TITLE_SIZE: f64 = 15.0;
    pub const TITLE_WEIGHT: u16 = 600;
    pub const SEARCH_TOP: f64 = 52.0;
    pub const SEARCH_HEIGHT: f64 = 28.0;
    pub const SEARCH_LEADING: f64 = 8.5;
    pub const SEARCH_TRAILING: f64 = 10.0;
    /// Between the search field and the first row. approximate.
    pub const ROWS_TOP_GAP: f64 = 10.0;
}

use metrics as m;

/// The default window size at the interface width factor `f`: `(ceil(715 f), 625).
pub fn default_size(f: f64) -> Size {
    Size {
        w: (m::DEFAULT_WIDTH * f).ceil(),
        h: m::DEFAULT_HEIGHT,
    }
}

/// The sidebar width at factor `f` for `rows` and whether scroll bars are always shown.
pub fn sidebar_width(f: f64, rows: RowSize, scrollers_shown: bool) -> f64 {
    let large = if rows == RowSize::Large {
        m::SIDEBAR_LARGE_EXTRA
    } else {
        0.0
    };
    let index = RowSize::ALL
        .iter()
        .position(|r| *r == rows)
        .expect("every row size is in ALL");
    let scrollers = if scrollers_shown {
        m::SIDEBAR_SCROLLER_EXTRA[index]
    } else {
        0.0
    };
    m::SIDEBAR_WIDTH * f + large + scrollers
}

/// What a point of the window is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Back,
    Forward,
    Search,
    Row(PaneId),
    /// The toolbar strip or the sidebar's top under the window control buttons: the window moves from it.
    Titlebar,
    Content,
    Sidebar,
}

/// Where everything of the window lies, in window points.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowLayout {
    pub size: Size,
    pub row_size: RowSize,
    /// The sidebar column at the window's full height (the material lies under it).
    pub sidebar: Rect,
    pub divider: Rect,
    /// The title region over the content column (not drawn; the window moves from it).
    pub toolbar: Rect,
    /// The content column under the title region.
    pub content: Rect,
    /// The back / forward group, while there is history to go through.
    pub back: Option<Rect>,
    pub forward: Option<Rect>,
    pub title: Rect,
    pub search: Rect,
    /// The sidebar rows with their background rectangle.
    pub rows: Vec<(PaneId, Rect)>,
}

impl WindowLayout {
    pub fn new(
        size: Size,
        f: f64,
        row_size: RowSize,
        groups: &[Vec<PaneId>],
        navigation: bool,
    ) -> Self {
        let sw = sidebar_width(f, row_size, false).min(size.w);
        let sidebar = Rect::new(0.0, 0.0, sw, size.h);
        let divider = Rect::new(sw, 0.0, m::DIVIDER, size.h);
        let cx = sw + m::DIVIDER;
        let cw = (size.w - cx).max(0.0);
        let toolbar = Rect::new(cx, 0.0, cw, m::TOOLBAR_HEIGHT.min(size.h));
        let content = Rect::new(
            cx,
            toolbar.bottom(),
            cw,
            (size.h - toolbar.bottom()).max(0.0),
        );
        let (bw, bh) = m::BACK_FORWARD;
        let by = (m::TOOLBAR_HEIGHT - bh) / 2.0;
        let (back, forward, tx) = if navigation {
            let back = Rect::new(cx + m::TOOLBAR_INSET, by, bw / 2.0, bh);
            let forward = Rect::new(back.right(), by, bw / 2.0, bh);
            (Some(back), Some(forward), forward.right() + m::TITLE_GAP)
        } else {
            (None, None, cx + m::TITLE_LEADING)
        };
        let title = Rect::new(
            tx,
            by,
            (toolbar.right() - m::TOOLBAR_INSET - tx).max(0.0),
            bh,
        );
        let search = Rect::new(
            m::SEARCH_LEADING,
            m::SEARCH_TOP,
            (sw - m::SEARCH_LEADING - m::SEARCH_TRAILING).max(0.0),
            m::SEARCH_HEIGHT,
        );
        let mut rows = Vec::new();
        let mut y = search.bottom() + m::ROWS_TOP_GAP;
        for (i, group) in groups.iter().enumerate() {
            if i > 0 {
                y += row::GROUP_GAP;
            }
            for pane in group {
                rows.push((
                    *pane,
                    Rect::new(
                        row::BACKGROUND_INSET,
                        y,
                        (sw - 2.0 * row::BACKGROUND_INSET).max(0.0),
                        row_size.row_height(),
                    ),
                ));
                y += row_size.row_height();
            }
        }
        WindowLayout {
            size,
            row_size,
            sidebar,
            divider,
            toolbar,
            content,
            back,
            forward,
            title,
            search,
            rows,
        }
    }

    /// The rectangle of `pane`'s sidebar row.
    pub fn row(&self, pane: PaneId) -> Option<Rect> {
        self.rows.iter().find(|(p, _)| *p == pane).map(|(_, r)| *r)
    }

    /// The icon enclosure of a row: the row size's icon, `ICON_LEADING` into the row background, centred vertically.
    pub fn icon(&self, row_rect: &Rect) -> Rect {
        let side = self.row_size.icon();
        Rect::new(
            row_rect.x + row::ICON_LEADING,
            row_rect.y + (row_rect.h - side) / 2.0,
            side,
            side,
        )
    }

    pub fn hit(&self, p: Point) -> Hit {
        if self.back.is_some_and(|b| b.contains(p)) {
            return Hit::Back;
        }
        if self.forward.is_some_and(|f| f.contains(p)) {
            return Hit::Forward;
        }
        if self.search.contains(p) {
            return Hit::Search;
        }
        if let Some((pane, _)) = self.rows.iter().find(|(_, r)| r.contains(p)) {
            return Hit::Row(*pane);
        }
        if self.toolbar.contains(p) || (self.sidebar.contains(p) && p.y < m::SEARCH_TOP) {
            return Hit::Titlebar;
        }
        if self.content.contains(p) {
            return Hit::Content;
        }
        Hit::Sidebar
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar;

    #[test]
    fn the_default_size_follows_the_width_factor() {
        let sizes: Vec<(f64, f64)> = [1.0, 1.1, 1.3]
            .iter()
            .map(|f| default_size(*f))
            .map(|s| (s.w, s.h))
            .collect();
        assert_eq!(sizes, [(715.0, 625.0), (787.0, 625.0), (930.0, 625.0)]);
    }

    #[test]
    fn the_sidebar_width_follows_row_size_and_scrollers_and_leaves_499_for_the_content() {
        let widths: Vec<(f64, f64)> = RowSize::ALL
            .iter()
            .map(|r| (sidebar_width(1.0, *r, false), sidebar_width(1.0, *r, true)))
            .collect();
        assert_eq!(widths, [(215.0, 215.0), (215.0, 223.0), (227.0, 242.0)]);
        let l = WindowLayout::new(
            default_size(1.0),
            1.0,
            RowSize::Medium,
            &sidebar::groups(&[]),
            false,
        );
        assert_eq!(
            (l.sidebar, l.divider.w, l.content.w),
            (Rect::new(0.0, 0.0, 215.0, 625.0), 1.0, 499.0)
        );
        assert_eq!(l.toolbar, Rect::new(216.0, 0.0, 499.0, 52.0));
        assert_eq!(l.content, Rect::new(216.0, 52.0, 499.0, 573.0));
    }

    #[test]
    fn rows_stack_in_groups_under_the_search_field() {
        let l = WindowLayout::new(
            default_size(1.0),
            1.0,
            RowSize::Medium,
            &sidebar::groups(&[]),
            false,
        );
        assert_eq!(
            l.search,
            Rect::new(8.5, 52.0, 196.5, 28.0),
            "the large field, 196.5 wide as on the photograph"
        );
        assert_eq!(l.rows.len(), 17);
        assert_eq!(
            l.row(PaneId::Wifi),
            Some(Rect::new(10.0, 90.0, 195.0, 28.0))
        );
        // four rows, then a group gap
        assert_eq!(
            l.row(PaneId::Accessibility).unwrap().y,
            90.0 + 4.0 * 28.0 + 13.0
        );
        assert_eq!(
            l.icon(&l.row(PaneId::Wifi).unwrap()),
            Rect::new(16.0, 94.0, 20.0, 20.0),
            "the medium icon field,"
        );
        let small = WindowLayout::new(
            default_size(1.0),
            1.0,
            RowSize::Small,
            &sidebar::groups(&[]),
            false,
        );
        assert_eq!(small.row(PaneId::Bluetooth).unwrap().h, 24.0);
    }

    #[test]
    fn hits_find_the_toolbar_controls_the_search_field_and_rows() {
        let l = WindowLayout::new(
            default_size(1.0),
            1.0,
            RowSize::Medium,
            &sidebar::groups(&[]),
            true,
        );
        assert_eq!(l.hit(l.back.unwrap().center()), Hit::Back);
        assert_eq!(l.hit(l.forward.unwrap().center()), Hit::Forward);
        assert_eq!(l.hit(l.search.center()), Hit::Search);
        assert_eq!(
            l.hit(l.row(PaneId::Sound).unwrap().center()),
            Hit::Row(PaneId::Sound)
        );
        assert_eq!(
            l.hit(Point { x: 100.0, y: 20.0 }),
            Hit::Titlebar,
            "the sidebar's top under the window control buttons"
        );
        assert_eq!(l.hit(Point { x: 600.0, y: 20.0 }), Hit::Titlebar);
        assert_eq!(l.hit(Point { x: 600.0, y: 300.0 }), Hit::Content);
        let below = l.row(PaneId::Trackpad).unwrap().bottom() + 4.0;
        assert!(below < m::DEFAULT_HEIGHT);
        assert_eq!(l.hit(Point { x: 100.0, y: below }), Hit::Sidebar);
    }

    #[test]
    fn without_history_the_title_takes_the_place_of_the_back_forward_group() {
        let first = WindowLayout::new(
            default_size(1.0),
            1.0,
            RowSize::Medium,
            &sidebar::groups(&[]),
            false,
        );
        assert_eq!((first.back, first.forward), (None, None));
        assert_eq!(
            first.title.x,
            216.0 + 13.0,
            "13 pt from the content's leading edge"
        );
        assert_eq!(
            first.hit(Point {
                x: 216.0 + 8.0 + 10.0,
                y: 26.0
            }),
            Hit::Titlebar,
            "no hidden button under the pointer"
        );
        let later = WindowLayout::new(
            default_size(1.0),
            1.0,
            RowSize::Medium,
            &sidebar::groups(&[]),
            true,
        );
        assert_eq!(later.title.x, 216.0 + 8.0 + 53.0 + 12.0);
        // the title is centred on the window control buttons' line: the middle of the 52 pt region
        assert_eq!(first.title.center().y, 26.0);
    }
}
