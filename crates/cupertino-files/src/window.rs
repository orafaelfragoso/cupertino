//! The browser window, whose surface is the whole window (the server's unified title bar style): the
//! sidebar of places runs the full height on the left, under the window control buttons, over a
//! material the server draws; the 52 pt toolbar with back / forward, the folder's title and the view
//! switch sits above the folder's icon or list view. Layout, hit testing, keyboard steps and CPU
//! drawing; no toolkit (the window process puts the pixels on its surface).
//!
//! Values: sidebar default width 200 pt with a 128 pt minimum, medium rows of
//! 28 pt with 13 pt text, 19 pt group rows with 11 pt text, 6 pt indent and 13 pt between groups,
//! the 17 pt row icon with its text at 22 pt; the back /
//! forward control is 53 × 28 pt; the browser minimum of 316 × 100 pt gives the window minimum. The toolbar is the unified 52 pt bar (table
//! value, that the window uses it is ). The other insets, the title, the view switch
//! and the highlight shapes are approximate (marked below).
//!
//! Under the folder's view the window can show the 28 pt path bar and the 20 pt status bar, both hidden
//! by default; the path bar sits above the status bar (approximate, the
//! screenshot; has both in one bottom stack).

use crate::browser::Browser;
use crate::context_menu::{tag_dots, view_segments, ItemKind, Menu};
use crate::entry::Entry;
use crate::geom::{Direction, Point, Rect, Size};
use crate::layout::icon::{IconLayout, SectionSpec};
use crate::layout::list::{ListLayout, RowSpec};
use crate::metrics::{list_view, marquee, menu as menu_metrics, sidebar, window};
use crate::name_edit::NameEdit;
use crate::places::{Place, PlaceGroup, PlaceKind};
use crate::render::{
    available_size, copy_opaque, fill_rect, fill_rounded, paint, stroke_rounded, truncate_middle,
    truncate_tail, Palette, Renderer,
};
use crate::settings::{ColumnDesc, ColumnId, IconViewSettings, ListViewSettings};
use crate::tags::TagColor;
use cupertino_decorations::field::{
    caret_offsets, centred_baseline, draw_field, FieldContent, FieldStyle,
};
use cupertino_decorations::palette::Rgba;
use cupertino_geometry::Rect as GeoRect;
use cupertino_text_field::TextField;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tiny_skia::{PathBuilder, Pixmap, Stroke, Transform};

/// Window chrome metrics.
pub mod metrics {
    use crate::metrics::{sidebar, window};

    /// Height of the toolbar strip, which is also the title bar: the unified bar (table value,
    /// the choice of the style is ).
    pub const TOOLBAR_HEIGHT: f64 = window::UNIFIED_TOOLBAR_HEIGHT;
    /// Separator lines between sidebar, toolbar and content. approximate.
    pub const SEPARATOR: f64 = 1.0;
    /// Horizontal inset of the toolbar controls from the toolbar's edges: the leading and trailing toolbar
    /// insets, fields 0 and 1 of the unified metrics record.
    pub const TOOLBAR_INSET: f64 = 8.0;
    /// Gap between the back / forward control and the title. approximate.
    pub const TITLE_GAP: f64 = 12.0;
    /// Gap between the title and the first toolbar item, the title's width limit. approximate.
    pub const TITLE_TRAILING_GAP: f64 = 12.0;
    /// Room kept for the title before toolbar items are dropped. approximate.
    pub const TITLE_MIN: f64 = 60.0;
    /// Space between two toolbar items. approximate (fields 4 to 12 of the unified metrics record, the item viewer
    /// layout, are not separated yet).
    pub const ITEM_GAP: f64 = 12.0;
    /// Width of the chevrons at the right of a popup item, and the search field's corner radius, glyph inset and
    /// glyph size. approximate.
    pub const POPUP_CHEVRON: f64 = 12.0;
    pub const SEARCH_RADIUS: f64 = 6.0;
    pub const SEARCH_INSET: f64 = 6.0;
    pub const SEARCH_GLYPH: f64 = 12.0;
    /// Gap between a toolbar item and the menu it opens. approximate.
    pub const POPUP_GAP: f64 = 4.0;
    /// Size of the title text: the Title3 style, 15 pt, of a unified toolbar without a subtitle. Its weight (semibold) is approximate: the style's weight trait is not resolved.
    pub const TITLE_SIZE: f64 = 15.0;
    pub const TITLE_WEIGHT: u16 = 600;
    /// Space between the toolbar strip (the sidebar's top, under the window control buttons) and the first
    /// sidebar group. approximate.
    pub const SIDEBAR_TOP: f64 = 10.0;
    /// Horizontal inset of a sidebar row's highlight from the sidebar's edges. approximate.
    pub const SIDEBAR_ROW_INSET: f64 = 10.0;
    /// Corner radius of the sidebar highlight and of the selected view segment. approximate.
    pub const HIGHLIGHT_RADIUS: f64 = 5.0;
    /// Smallest sidebar: its minimum thickness; overlay scrollers add nothing.
    pub const SIDEBAR_MIN: f64 = sidebar::MIN_WIDTH_BASE;
    /// Smallest browser area, its scroll-view content insets are taken as zero (approximate).
    pub const BROWSER_MIN: (f64, f64) = (316.0, 100.0);
    /// Smallest window content: the minimum sidebar, the divider, the minimum browser and the toolbar..
    pub const WINDOW_MIN: (f64, f64) = (
        SIDEBAR_MIN + SEPARATOR + BROWSER_MIN.0,
        TOOLBAR_HEIGHT + SEPARATOR + BROWSER_MIN.1,
    );
    /// Path bar and status bar heights, each with a 1 pt separator at its top edge. heights and status bar separator; the path bar's separator is approximate (screenshot).
    pub const PATH_BAR_HEIGHT: f64 = window::PATH_BAR_HEIGHT;
    pub const STATUS_BAR_HEIGHT: f64 = window::STATUS_BAR_HEIGHT;
    /// Text size of both bars. approximate (screenshot; the controls' fonts is not known).
    pub const BAR_TEXT_SIZE: f64 = 11.0;
    /// Path bar: leading and trailing inset, segment icon, gap between icon and name, the space between two
    /// segments and the chevron drawn in it. approximate (screenshot).
    pub const PATH_BAR_INSET: f64 = 8.0;
    pub const PATH_ICON: f64 = 16.0;
    pub const PATH_ICON_GAP: f64 = 4.0;
    pub const PATH_SEPARATOR: f64 = 16.0;
    pub const PATH_CHEVRON: f64 = 8.0;
}

use metrics as m;

/// The bars under the folder's view that are shown. Both are hidden by default
/// (`ShowStatusBar` and `ShowPathbar` default to false).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bars {
    pub path: bool,
    pub status: bool,
}

impl Bars {
    pub const HIDDEN: Bars = Bars {
        path: false,
        status: false,
    };
}

/// The toolbar's items after the title, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarItem {
    /// The view-mode popup.
    View,
    /// The Group By popup: disabled until the window groups items.
    GroupBy,
    Tags,
    /// The Action (…) popup: the menu of the selection, or of the folder without one.
    Action,
    Search,
}

impl ToolbarItem {
    /// The order of a screenshot without Share. approximate.
    pub const ORDER: [ToolbarItem; 5] = [
        ToolbarItem::View,
        ToolbarItem::GroupBy,
        ToolbarItem::Tags,
        ToolbarItem::Action,
        ToolbarItem::Search,
    ];
    /// Dropped first when the toolbar is too narrow; the view popup stays. approximate: the design moves items into an
    /// overflow menu, which is not built.
    const DROP_ORDER: [ToolbarItem; 4] = [
        ToolbarItem::GroupBy,
        ToolbarItem::Tags,
        ToolbarItem::Action,
        ToolbarItem::Search,
    ];

    /// `(width, height): Action and Search are as designed; the view popup and Group By take the arrange popup's
    /// size and Tags the share button's (approximate: their own items are not in the nibs).
    pub fn size(self) -> (f64, f64) {
        match self {
            ToolbarItem::View | ToolbarItem::GroupBy => window::TOOLBAR_ARRANGE_POPUP,
            ToolbarItem::Tags => window::TOOLBAR_SHARE,
            ToolbarItem::Action => window::TOOLBAR_ACTION_POPUP,
            ToolbarItem::Search => window::TOOLBAR_SEARCH_FIELD,
        }
    }

    pub fn enabled(self) -> bool {
        self != ToolbarItem::GroupBy
    }
}

/// How the folder is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Icons,
    List,
}

/// A keyboard move of the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
    First,
    Last,
}

/// What a point of the window is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Back,
    Forward,
    /// An enabled toolbar item.
    Toolbar(ToolbarItem),
    /// A sidebar row: the index into the places.
    Place(usize),
    /// An item of the folder: its index in display order.
    Item(usize),
    /// The disclosure triangle of a folder row of the list: its index in display order.
    Disclosure(usize),
    /// A list column header.
    Column(ColumnId),
    /// A folder of the path bar: its index, outermost first.
    PathSegment(usize),
    /// Content area without an item.
    Empty,
    /// The toolbar strip without a control, over the sidebar's top too: it moves the window, and a double click
    /// zooms it.
    Titlebar,
    /// Sidebar or separators without a control.
    Chrome,
}

/// The width of the toolbar search field: its resting width, or [`window::SEARCH_GROWTH`] times that while it has the keyboard
/// or holds text, changing in [`window::SEARCH_GROW_SECONDS`] with an ease-out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchWidth {
    from: f64,
    to: f64,
    start: f64,
    open: bool,
}

impl Default for SearchWidth {
    fn default() -> Self {
        let rest = window::TOOLBAR_SEARCH_FIELD.0;
        SearchWidth {
            from: rest,
            to: rest,
            start: 0.0,
            open: false,
        }
    }
}

impl SearchWidth {
    /// The field wants to be open (`open`) or at rest at `now` seconds. Without `animate` (reduce motion) the width
    /// changes at once; a change in mid-flight starts from the width shown.
    pub fn set_open(&mut self, open: bool, now: f64, animate: bool) {
        if open == self.open {
            return;
        }
        let rest = window::TOOLBAR_SEARCH_FIELD.0;
        let shown = self.width(now);
        self.open = open;
        self.to = if open {
            rest * window::SEARCH_GROWTH
        } else {
            rest
        };
        self.from = if animate { shown } else { self.to };
        self.start = now;
    }

    /// The width at `now` seconds.
    pub fn width(&self, now: f64) -> f64 {
        let t = ((now - self.start) / window::SEARCH_GROW_SECONDS).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.from + (self.to - self.from) * eased
    }

    /// The width is still changing at `now`.
    pub fn animating(&self, now: f64) -> bool {
        self.from != self.to && now - self.start < window::SEARCH_GROW_SECONDS - 1e-9
    }
}

/// The fixed parts of the window for a size.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowLayout {
    pub size: Size,
    pub sidebar: Rect,
    pub toolbar: Rect,
    /// The folder's view.
    pub content: Rect,
    /// The path bar and the status bar under it, when shown.
    pub path_bar: Option<Rect>,
    pub status_bar: Option<Rect>,
    pub back: Rect,
    pub forward: Rect,
    /// Where the folder's title is drawn, between the back / forward control and the first toolbar item.
    pub title: Rect,
    /// The toolbar items that fit, left to right.
    pub items: Vec<(ToolbarItem, Rect)>,
    /// Group titles of the sidebar.
    pub groups: Vec<(PlaceGroup, Rect)>,
    /// Sidebar rows, in the order of the places.
    pub places: Vec<Rect>,
}

impl WindowLayout {
    pub fn new(size: Size, places: &[Place], bars: Bars) -> Self {
        Self::with_search(size, places, bars, window::TOOLBAR_SEARCH_FIELD.0)
    }

    /// [`WindowLayout::new`] with the search field `search_width` wide: it grows to the left, and the toolbar drops items
    /// as it does for a narrow window when they no longer fit.
    pub fn with_search(size: Size, places: &[Place], bars: Bars, search_width: f64) -> Self {
        let item_size = |item: ToolbarItem| match item {
            ToolbarItem::Search => (search_width, window::TOOLBAR_SEARCH_FIELD.1),
            other => other.size(),
        };
        // the sidebar gives way to the browser minimum down to its own minimum
        let sidebar_w = (size.w - m::SEPARATOR - m::BROWSER_MIN.0)
            .clamp(m::SIDEBAR_MIN, sidebar::DEFAULT_WIDTH);
        let right = sidebar_w + m::SEPARATOR;
        let toolbar = Rect::new(right, 0.0, (size.w - right).max(0.0), m::TOOLBAR_HEIGHT);
        let content_y = m::TOOLBAR_HEIGHT + m::SEPARATOR;
        // the bars stack up from the window's bottom edge: the status bar, then the path bar
        let mut bottom = size.h;
        let status_bar = bars.status.then(|| {
            bottom -= m::STATUS_BAR_HEIGHT;
            Rect::new(right, bottom, toolbar.w, m::STATUS_BAR_HEIGHT)
        });
        let path_bar = bars.path.then(|| {
            bottom -= m::PATH_BAR_HEIGHT;
            Rect::new(right, bottom, toolbar.w, m::PATH_BAR_HEIGHT)
        });
        let content = Rect::new(right, content_y, toolbar.w, (bottom - content_y).max(0.0));
        let (bw, bh) = window::TOOLBAR_BACK_FORWARD;
        let control_y = (m::TOOLBAR_HEIGHT - bh) / 2.0;
        let back = Rect::new(toolbar.x + m::TOOLBAR_INSET, control_y, bw / 2.0, bh);
        let forward = Rect::new(back.right(), control_y, bw / 2.0, bh);
        let title_x = forward.right() + m::TITLE_GAP;
        let width = |shown: &[ToolbarItem]| {
            shown.iter().map(|i| item_size(*i).0).sum::<f64>()
                + shown.len().saturating_sub(1) as f64 * m::ITEM_GAP
        };
        let room =
            toolbar.right() - m::TOOLBAR_INSET - (title_x + m::TITLE_MIN + m::TITLE_TRAILING_GAP);
        let mut shown = ToolbarItem::ORDER.to_vec();
        for item in ToolbarItem::DROP_ORDER {
            if width(&shown) <= room {
                break;
            }
            shown.retain(|i| *i != item);
        }
        // laid out from the right edge
        let mut x = toolbar.right() - m::TOOLBAR_INSET;
        let mut items: Vec<(ToolbarItem, Rect)> = shown
            .iter()
            .rev()
            .map(|&item| {
                let (w, h) = item_size(item);
                x -= w;
                let r = Rect::new(x, (m::TOOLBAR_HEIGHT - h) / 2.0, w, h);
                x -= m::ITEM_GAP;
                (item, r)
            })
            .collect();
        items.reverse();
        let title_end = items
            .first()
            .map_or(toolbar.right() - m::TOOLBAR_INSET, |(_, r)| {
                r.x - m::TITLE_TRAILING_GAP
            });
        let title = Rect::new(
            title_x,
            0.0,
            (title_end - title_x).max(0.0),
            m::TOOLBAR_HEIGHT,
        );

        let row = sidebar::RowSize::DEFAULT.row_height();
        let (mut groups, mut rows) = (Vec::new(), Vec::with_capacity(places.len()));
        let mut y = m::TOOLBAR_HEIGHT + m::SIDEBAR_TOP;
        let mut current: Option<PlaceGroup> = None;
        for p in places {
            if current != Some(p.group) {
                if current.is_some() {
                    y += sidebar::INTERGROUP_SPACING;
                }
                groups.push((
                    p.group,
                    Rect::new(
                        m::SIDEBAR_ROW_INSET,
                        y,
                        sidebar_w - 2.0 * m::SIDEBAR_ROW_INSET,
                        sidebar::GROUP_ROW_HEIGHT,
                    ),
                ));
                y += sidebar::GROUP_ROW_HEIGHT;
                current = Some(p.group);
            }
            rows.push(Rect::new(
                m::SIDEBAR_ROW_INSET,
                y,
                sidebar_w - 2.0 * m::SIDEBAR_ROW_INSET,
                row,
            ));
            y += row;
        }
        WindowLayout {
            size,
            sidebar: Rect::new(0.0, 0.0, sidebar_w, size.h),
            toolbar,
            content,
            path_bar,
            status_bar,
            back,
            forward,
            title,
            items,
            groups,
            places: rows,
        }
    }

    /// The control or region under `p` (window points), not looking into the content.
    fn chrome_hit(&self, p: Point) -> Option<Hit> {
        let controls = [(self.back, Hit::Back), (self.forward, Hit::Forward)];
        if let Some((_, hit)) = controls.iter().find(|(r, _)| r.contains(p)) {
            return Some(*hit);
        }
        // a disabled item takes the press without moving the window
        if let Some((item, _)) = self.items.iter().find(|(_, r)| r.contains(p)) {
            return Some(if item.enabled() {
                Hit::Toolbar(*item)
            } else {
                Hit::Chrome
            });
        }
        if let Some(i) = self.places.iter().position(|r| r.contains(p)) {
            return Some(Hit::Place(i));
        }
        if p.y < m::TOOLBAR_HEIGHT {
            return Some(Hit::Titlebar);
        }
        (!self.content.contains(p)).then_some(Hit::Chrome)
    }
}

/// The folders the path bar shows for `dir`, outermost first, with their names. The chain starts at the file system
/// root under the computer place's name (the design starts at the startup volume, `PathBarRootAtHome`
/// false); a folder that is the Trash or Recents place starts it under the place's name instead.
pub fn path_chain(dir: &Path, places: &[Place]) -> Vec<(PathBuf, String)> {
    let mut chain = Vec::new();
    for folder in dir.ancestors() {
        if let Some(place) = places
            .iter()
            .find(|p| p.path == folder && matches!(p.kind, PlaceKind::Trash | PlaceKind::Recents))
        {
            chain.push((folder.to_path_buf(), place.name.clone()));
            break;
        }
        let name = match folder.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => places
                .iter()
                .find(|p| p.kind == PlaceKind::Computer && p.path == folder)
                .map_or_else(|| folder.display().to_string(), |p| p.name.clone()),
        };
        chain.push((folder.to_path_buf(), name));
    }
    chain.reverse();
    chain
}

/// One folder of the path bar, in window points.
#[derive(Clone, Debug, PartialEq)]
pub struct PathSegment {
    pub path: PathBuf,
    pub name: String,
    pub icon: Rect,
    /// Where the name is drawn: no width when the bar is too narrow and the segment shows its icon only.
    pub text: Rect,
    /// Icon and name: what a click goes to the folder on.
    pub frame: Rect,
}

/// Lay out `chain` with names `widths` points wide in `bar`. When the bar is too narrow the outer folders show their
/// icons only, outermost first, and the folder's own name is cut at the bar's end (approximate: the design
/// shrinks the segments, its rule is not known).
pub fn path_segments(bar: &Rect, chain: &[(PathBuf, String)], widths: &[f64]) -> Vec<PathSegment> {
    let n = chain.len();
    let room = bar.w - 2.0 * m::PATH_BAR_INSET;
    let total = |named: &[bool]| -> f64 {
        (0..n)
            .map(|i| {
                m::PATH_ICON
                    + if named[i] {
                        m::PATH_ICON_GAP + widths[i]
                    } else {
                        0.0
                    }
            })
            .sum::<f64>()
            + n.saturating_sub(1) as f64 * m::PATH_SEPARATOR
    };
    let mut named = vec![true; n];
    for i in 0..n.saturating_sub(1) {
        if total(&named) <= room {
            break;
        }
        named[i] = false;
    }
    let mut x = bar.x + m::PATH_BAR_INSET;
    let end = bar.right() - m::PATH_BAR_INSET;
    let mut out = Vec::with_capacity(n);
    for (i, (path, name)) in chain.iter().enumerate() {
        let icon = Rect::new(
            x,
            bar.y + (bar.h - m::PATH_ICON) / 2.0,
            m::PATH_ICON,
            m::PATH_ICON,
        );
        let text_x = icon.right() + m::PATH_ICON_GAP;
        let text_w = if named[i] {
            widths[i].min(end - text_x).max(0.0)
        } else {
            0.0
        };
        let text = Rect::new(text_x, bar.y, text_w, bar.h);
        let frame = Rect::new(
            x,
            bar.y,
            if named[i] {
                text.right() - x
            } else {
                m::PATH_ICON
            },
            bar.h,
        );
        out.push(PathSegment {
            path: path.clone(),
            name: name.clone(),
            icon,
            text,
            frame,
        });
        x = frame.right() + m::PATH_SEPARATOR;
    }
    out
}

/// The status bar's text: "1 item" or "N items", then the space left on the folder's file system when it has one,
/// "207 items, 227.27 GB available" (approximate, a screenshot; the selection count variant is not built).
pub fn status_text(items: usize, available: Option<u64>) -> String {
    let count = if items == 1 {
        "1 item".to_string()
    } else {
        format!("{items} items")
    };
    match available {
        Some(bytes) => format!("{count}, {} available", available_size(bytes)),
        None => count,
    }
}

/// The folder's view laid out for the content width.
#[derive(Clone, Debug, PartialEq)]
pub enum ContentLayout {
    Icons(IconLayout),
    /// The list with its columns (for the header hits).
    List(ListLayout, Vec<ColumnDesc>),
}

impl ContentLayout {
    /// Height of everything that scrolls, in points.
    pub fn height(&self) -> f64 {
        match self {
            ContentLayout::Icons(l) => l.content.h,
            ContentLayout::List(l, _) => l.content_height,
        }
    }

    /// Height at the top of the view that does not scroll (the list header).
    pub fn fixed_top(&self) -> f64 {
        match self {
            ContentLayout::Icons(_) => 0.0,
            ContentLayout::List(..) => list_view::HEADER_HEIGHT,
        }
    }

    /// Where the focus goes for a key: from no focus the first item (the last one for Up, Left and End);
    /// in the list Left and Right do not move it (they collapse and expand folders, [`Browser::disclose_focus`]); in the icon grid the nearest cell
    /// in the direction (approximate, the design rules is not known).
    pub fn step(&self, focus: Option<usize>, len: usize, key: Move) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let Some(from) = focus.filter(|f| *f < len) else {
            return Some(match key {
                Move::Up | Move::Left | Move::Last => len - 1,
                Move::Down | Move::Right | Move::First => 0,
            });
        };
        match (self, key) {
            (_, Move::First) => Some(0),
            (_, Move::Last) => Some(len - 1),
            (ContentLayout::List(..), Move::Up) => from.checked_sub(1),
            (ContentLayout::List(..), Move::Down) => (from + 1 < len).then_some(from + 1),
            (ContentLayout::List(..), Move::Left | Move::Right) => None,
            (ContentLayout::Icons(l), Move::Up) => l.neighbour(from, Direction::Up),
            (ContentLayout::Icons(l), Move::Down) => l.neighbour(from, Direction::Down),
            (ContentLayout::Icons(l), Move::Left) => l.neighbour(from, Direction::Left),
            (ContentLayout::Icons(l), Move::Right) => l.neighbour(from, Direction::Right),
        }
    }

    fn frame_of(&self, index: usize) -> Option<Rect> {
        match self {
            ContentLayout::Icons(l) => l.frame_of(index),
            ContentLayout::List(l, _) => l.rows.get(index).map(|r| r.frame),
        }
    }
}

/// Opacity of the accent over the folder a drag is over. approximate.
const DROP_TARGET_ALPHA: f32 = 0.3;
/// Opacity of the text of a disabled menu row. approximate.
const DISABLED_ALPHA: f32 = 0.4;
/// Name editor: padding either side of the text, minimum text width, border width and corner radius. approximate.
/// Longest side of a toolbar symbol (the chevrons, the two view modes) in points. approximate: the symbols' own
/// point size in the design toolbar is not known (the template boxes are 11 to 13 points).
const TOOLBAR_GLYPH: f64 = 13.0;
/// The up-down chevrons of the view popup, the search field's text size and its focus ring. approximate.
const POPUP_CHEVRON_GLYPH: f64 = 9.0;
const SEARCH_TEXT_SIZE: f64 = 13.0;
/// The words and the size of the message of a search without a match. approximate.
const NO_RESULTS: &str = "No Results";
const NO_RESULTS_SIZE: f64 = 17.0;
/// The most bytes the toolbar search holds.
pub const SEARCH_MAX_LEN: usize = 1024;
const SEARCH_RING: f64 = 2.0;
/// Longest side of a sidebar symbol in points: 16 of the 17 pt icon field; the symbol's size inside the field
/// and the accent tint of the sidebar symbols are approximate (not known).
const SIDEBAR_GLYPH: f64 = 16.0;
const EDITOR_PADDING: f64 = 4.0;
const EDITOR_MIN_WIDTH: f64 = 48.0;
const EDITOR_BORDER: f64 = 2.0;
const EDITOR_RADIUS: f64 = 3.0;

/// The rubber band over the content: a quaternary label fill inside a 1 pt tertiary label border, square corners,
/// clipped to the content area (the view under the band is the default collection view band).
fn paint_marquee(pm: &mut Pixmap, scale: f32, band: &Rect, content: &Rect, pal: &Palette) {
    let Some(r) = band.intersection(content) else {
        return;
    };
    fill_rect(pm, &r, scale, pal.quaternary_label);
    let w = marquee::BORDER_WIDTH;
    for edge in [
        Rect::new(r.x, r.y, r.w, w),
        Rect::new(r.x, r.bottom() - w, r.w, w),
        Rect::new(r.x, r.y, w, r.h),
        Rect::new(r.right() - w, r.y, w, r.h),
    ] {
        fill_rect(pm, &edge, scale, pal.tertiary_label);
    }
}

/// One frame of the window: its layout, the content layout and the scroll position.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub window: WindowLayout,
    pub content: ContentLayout,
    /// Points of content scrolled out at the top, within `0..=max_scroll()`.
    pub scroll: f64,
    /// The path bar's folders (empty when the bar is hidden).
    pub path: Vec<PathSegment>,
}

impl Frame {
    pub fn max_scroll(&self) -> f64 {
        (self.content.height() - self.window.content.h).max(0.0)
    }

    /// Scroll by `dy` points, staying within the content.
    pub fn scroll_by(&mut self, dy: f64) {
        self.scroll = (self.scroll + dy).clamp(0.0, self.max_scroll());
    }

    /// Scroll as little as needed for item `index` to be in sight.
    pub fn reveal(&mut self, index: usize) {
        let Some(r) = self.content.frame_of(index) else {
            return;
        };
        let top = self.scroll + self.content.fixed_top();
        let bottom = self.scroll + self.window.content.h;
        if r.y < top {
            self.scroll_by(r.y - top);
        } else if r.bottom() > bottom {
            self.scroll_by(r.bottom() - bottom);
        }
    }

    /// The cell or row of item `index` in window points, scrolled as drawn.
    pub fn item_frame(&self, index: usize) -> Option<Rect> {
        let r = self.content.frame_of(index)?;
        Some(r.offset(self.window.content.x, self.window.content.y - self.scroll))
    }

    /// The name of item `index` in window points, scrolled as drawn: the label under the icon, or the text of the name column.
    pub fn name_frame(&self, index: usize) -> Option<Rect> {
        let local = match &self.content {
            ContentLayout::Icons(l) => l.cells.get(index)?.label_frame,
            ContentLayout::List(l, _) => {
                let row = l.rows.get(index)?;
                let column = l.columns.iter().find(|c| c.id == ColumnId::Name)?;
                Rect::new(
                    row.text_x,
                    row.frame.y,
                    (column.frame.right() - row.text_x).max(0.0),
                    row.frame.h,
                )
            }
        };
        Some(local.offset(self.window.content.x, self.window.content.y - self.scroll))
    }

    /// Content coordinates (points from the content area's top-left, scrolled with the content) of the window point `p`.
    pub fn to_content(&self, p: Point) -> Point {
        Point {
            x: p.x - self.window.content.x,
            y: p.y - self.window.content.y + self.scroll,
        }
    }

    /// Items a rubber band over `band` (content coordinates) touches: icons and labels that intersect it in the icon
    /// view, rows whose span it crosses in the list.
    pub fn hits_in(&self, band: &Rect) -> Vec<usize> {
        match &self.content {
            ContentLayout::Icons(l) => l.hits_in_rect(band),
            ContentLayout::List(l, _) => {
                let (top, bottom) = (band.y.max(list_view::HEADER_HEIGHT), band.bottom());
                if bottom <= top {
                    return Vec::new();
                }
                l.visible_rows(top, bottom).collect()
            }
        }
    }

    /// What `p` (window points) is over.
    pub fn hit(&self, p: Point) -> Hit {
        if let Some(i) = self.path.iter().position(|s| s.frame.contains(p)) {
            return Hit::PathSegment(i);
        }
        if let Some(hit) = self.window.chrome_hit(p) {
            return hit;
        }
        let local = Point {
            x: p.x - self.window.content.x,
            y: p.y - self.window.content.y,
        };
        match &self.content {
            ContentLayout::Icons(l) => l
                .hit(Point {
                    x: local.x,
                    y: local.y + self.scroll,
                })
                .map_or(Hit::Empty, Hit::Item),
            ContentLayout::List(l, _) if local.y < list_view::HEADER_HEIGHT => l
                .columns
                .iter()
                .find(|c| local.x >= c.frame.x && local.x < c.frame.right())
                .map_or(Hit::Chrome, |c| Hit::Column(c.id)),
            ContentLayout::List(l, _) => match l.row_at(local.y + self.scroll) {
                // the triangle's column of the row, its full height
                Some(i)
                    if l.rows[i]
                        .disclosure
                        .is_some_and(|d| local.x >= d.x && local.x < d.right()) =>
                {
                    Hit::Disclosure(i)
                }
                Some(i) => Hit::Item(i),
                None => Hit::Empty,
            },
        }
    }
}

/// Everything a frame shows besides the layout.
pub struct WindowState<'a> {
    pub browser: &'a Browser,
    pub places: &'a [Place],
    pub mode: ViewMode,
    /// The window has the focus: selections use the accent colour.
    pub active: bool,
    /// The search field, and whether it has the keyboard.
    pub search: &'a TextField,
    pub search_focused: bool,
    /// The search field's caret shows now (it has the keyboard, nothing is selected, the blink is on).
    pub search_caret: bool,
    /// The folder's name, the window's title.
    pub title: &'a str,
    pub now: SystemTime,
    /// The name being edited in place, if any.
    pub edit: Option<&'a NameEdit>,
    /// The name editor's caret shows now (the window is active, nothing is selected, the blink is on).
    pub edit_caret: bool,
    /// The folder item a drag is over: outlined as the place the items would go.
    pub drop_target: Option<usize>,
    /// The rubber band being dragged: where it started in content coordinates and where the pointer is now in window
    /// points.
    pub marquee: Option<(Point, Point)>,
    /// The status bar's text ([`status_text`]), drawn when the bar is shown.
    pub status: &'a str,
}

/// Lays out and draws browser windows; holds the text shaper and the view settings.
pub struct WindowPainter {
    pub renderer: Renderer,
    pub icons: IconViewSettings,
    pub list: ListViewSettings,
    /// The name editor's text area and the x of each of its caret positions, as last drawn (window points).
    editor_text: Option<(Rect, Vec<f64>)>,
    /// The search field's text area, the x of each of its caret positions and the scroll it was drawn with.
    search_text: Option<(Rect, Vec<f64>, f64)>,
}

impl WindowPainter {
    /// A painter with the measured default view settings.
    pub fn new(renderer: Renderer) -> Self {
        WindowPainter {
            renderer,
            icons: IconViewSettings::default(),
            list: ListViewSettings::default(),
            editor_text: None,
            search_text: None,
        }
    }

    /// The caret position of the search `field` under `at` as last drawn, `None` before it was drawn.
    pub fn search_index(&self, field: &TextField, at: Point) -> Option<usize> {
        let (area, offsets, scroll) = self.search_text.as_ref()?;
        Some(field.index_at(offsets, at.x - area.x + scroll))
    }

    /// Scroll the search `field` as it was last drawn, which keeps its caret in view.
    pub fn follow_search_caret(&self, field: &mut TextField) {
        let Some((area, offsets, _)) = &self.search_text else {
            return;
        };
        let caret_x = field
            .boundaries()
            .iter()
            .position(|b| *b == field.caret())
            .and_then(|i| offsets.get(i))
            .copied()
            .unwrap_or(0.0);
        field.follow_caret(caret_x, offsets.last().copied().unwrap_or(0.0), area.w);
    }

    /// The caret position of the name editor's `field` under `at` as last drawn, `None` outside its text area.
    pub fn editor_index(&self, field: &TextField, at: Point) -> Option<usize> {
        let (area, offsets) = self.editor_text.as_ref()?;
        let row = Rect::new(
            area.x - EDITOR_PADDING,
            area.y,
            area.w + 2.0 * EDITOR_PADDING,
            area.h,
        );
        row.contains(at)
            .then(|| field.index_at(offsets, at.x - area.x + field.scroll()))
    }

    /// The frame of a window of `size` points on the folder `dir` with the `bars` shown, showing the items of `outline`
    /// (one row per displayed item, [`Browser::outline`]) in `mode` at `scale`, scrolled to `scroll` (clamped to the
    /// content).
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        size: Size,
        places: &[Place],
        bars: Bars,
        search_width: f64,
        dir: &Path,
        mode: ViewMode,
        outline: &[RowSpec],
        scroll: f64,
        scale: f32,
    ) -> Frame {
        self.renderer.text.set_scale(scale);
        let window = WindowLayout::with_search(size, places, bars, search_width);
        let path = match &window.path_bar {
            Some(bar) => {
                let chain = path_chain(dir, places);
                let px = m::BAR_TEXT_SIZE as f32 * scale;
                let widths: Vec<f64> = chain
                    .iter()
                    .map(|(_, name)| {
                        f64::from(self.renderer.text.measure(name, px, 400, 0.0) / scale)
                    })
                    .collect();
                path_segments(bar, &chain, &widths)
            }
            None => Vec::new(),
        };
        let content = match mode {
            ViewMode::Icons => ContentLayout::Icons(
                self.renderer
                    .icon_layout(
                        &self.icons,
                        &[SectionSpec {
                            title: None,
                            count: outline.len(),
                        }],
                        window.content.w,
                        scale,
                    )
                    .1,
            ),
            ViewMode::List => ContentLayout::List(
                self.renderer
                    .list_layout(outline, &self.list, window.content.w, scale),
                self.list.columns.clone(),
            ),
        };
        let mut frame = Frame {
            window,
            content,
            scroll: 0.0,
            path,
        };
        frame.scroll_by(scroll);
        frame
    }

    /// Draw `frame` of the window into `pm` (device pixels, `scale` per point).
    pub fn paint(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        frame: &Frame,
        state: &WindowState<'_>,
        pal: &Palette,
    ) {
        self.renderer.text.set_scale(scale);
        let w = &frame.window;
        // the sidebar is left clear: the server draws its material below the surface. An active window's strip is the
        // content background (the header's resting fill is the content background and the header reads
        // as part of the strip)
        let strip = if state.active {
            pal.background
        } else {
            pal.toolbar_inactive
        };
        fill_rect(pm, &w.toolbar, scale, strip);
        self.paint_sidebar(pm, scale, w, state, pal);
        fill_rect(
            pm,
            &Rect::new(w.sidebar.right(), 0.0, m::SEPARATOR, w.size.h),
            scale,
            pal.separator,
        );
        // a 1 pt line under the strip at all times: a screenshot shows one (a 222 grey line under the toolbar and
        // one under the list header, approximate), which is kept over 's reading of the automatic separator
        // (no line until content scrolls under the strip). The list header is part of the strip and takes its fill
        // (the resting colour)
        fill_rect(
            pm,
            &Rect::new(w.toolbar.x, w.toolbar.bottom(), w.toolbar.w, m::SEPARATOR),
            scale,
            pal.separator,
        );
        self.paint_toolbar(pm, scale, w, state, pal);
        self.paint_bars(pm, scale, frame, state, pal);

        let entries = state.browser.entries();
        let selected: Vec<usize> = state.browser.selection().indices().collect();
        let size = Size {
            w: w.content.w,
            h: w.content.h,
        };
        if size.w < 1.0 || size.h < 1.0 {
            return;
        }
        let content = match state.mode {
            ViewMode::Icons => self.renderer.icon_view(
                entries,
                None,
                &selected,
                state.active,
                &self.icons,
                size,
                frame.scroll,
                scale,
                pal,
            ),
            ViewMode::List => self.renderer.list_view(
                entries,
                state.browser.outline(),
                &selected,
                state.active,
                &self.list,
                state.browser.sort(),
                size,
                frame.scroll,
                scale,
                &Palette {
                    header: strip,
                    ..*pal
                },
                state.now,
            ),
        };
        let (x, y) = (
            (w.content.x as f32 * scale).round() as i32,
            (w.content.y as f32 * scale).round() as i32,
        );
        copy_opaque(pm, &content, x, y);
        self.renderer.recycle(content);
        if state.browser.no_results() {
            self.paint_no_results(pm, scale, &w.content, state.mode, pal);
        }
        if let Some(target) = state.drop_target.and_then(|i| frame.item_frame(i)) {
            let outline = [
                pal.accent[0],
                pal.accent[1],
                pal.accent[2],
                DROP_TARGET_ALPHA,
            ];
            fill_rounded(pm, &target, m::HIGHLIGHT_RADIUS, scale, outline);
        }
        if let Some((origin, at)) = state.marquee {
            let band = Rect::from_corners(origin, frame.to_content(at));
            // the list header covers the top of the content
            let top = if state.mode == ViewMode::List {
                list_view::HEADER_HEIGHT
            } else {
                0.0
            };
            let visible = Rect::new(
                w.content.x,
                w.content.y + top,
                w.content.w,
                (w.content.h - top).max(0.0),
            );
            paint_marquee(
                pm,
                scale,
                &band.offset(w.content.x, w.content.y - frame.scroll),
                &visible,
                pal,
            );
        }
        self.editor_text = None;
        if let Some(edit) = state.edit {
            self.paint_editor(pm, scale, frame, edit, state, pal);
        }
    }

    /// "No Results" centred in the content area (below the list header in the list view), in the secondary label colour.
    fn paint_no_results(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        content: &Rect,
        mode: ViewMode,
        pal: &Palette,
    ) {
        let px = NO_RESULTS_SIZE as f32 * scale;
        let width = self.renderer.text.measure(NO_RESULTS, px, 400, 0.0);
        let top = if mode == ViewMode::List {
            list_view::HEADER_HEIGHT
        } else {
            0.0
        };
        let centre_y = content.y + top + (content.h - top) / 2.0;
        self.renderer.text.draw(
            pm,
            NO_RESULTS,
            content.center().x as f32 * scale - width / 2.0,
            centre_y as f32 * scale + px * 0.36,
            px,
            400,
            0.0,
            pal.secondary_text,
        );
    }

    /// The path bar on the content background and the status bar on the toolbar's fill, each under a separator (the
    /// within-window and title bar materials of are drawn as these opaque fills, approximate).
    fn paint_bars(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        frame: &Frame,
        state: &WindowState<'_>,
        pal: &Palette,
    ) {
        let px = m::BAR_TEXT_SIZE as f32 * scale;
        if let Some(bar) = &frame.window.path_bar {
            fill_rect(pm, bar, scale, pal.background);
            fill_rect(
                pm,
                &Rect::new(bar.x, bar.y, bar.w, m::SEPARATOR),
                scale,
                pal.separator,
            );
            for (i, segment) in frame.path.iter().enumerate() {
                if i > 0 {
                    let chevron = Rect::new(
                        segment.icon.x - m::PATH_SEPARATOR,
                        bar.y,
                        m::PATH_SEPARATOR,
                        bar.h,
                    );
                    self.renderer.draw_symbol(
                        pm,
                        &chevron,
                        "symbol.chevron.right",
                        m::PATH_CHEVRON,
                        scale,
                        pal.secondary_text,
                    );
                }
                match state
                    .places
                    .iter()
                    .find(|p| p.path == segment.path && p.kind == PlaceKind::Recents)
                {
                    Some(recents) => self.renderer.draw_symbol(
                        pm,
                        &segment.icon,
                        recents.kind.symbol(),
                        m::PATH_ICON,
                        scale,
                        pal.accent,
                    ),
                    None => self.renderer.draw_item_icon(
                        pm,
                        &segment.icon,
                        &Entry {
                            path: segment.path.clone(),
                            ..Entry::synthetic(&segment.name, true, 0)
                        },
                        scale,
                    ),
                }
                if segment.text.w > 0.0 {
                    let name = truncate_tail(
                        &mut self.renderer.text,
                        &segment.name,
                        px,
                        segment.text.w as f32 * scale,
                    );
                    self.renderer.text.draw(
                        pm,
                        &name,
                        segment.text.x as f32 * scale,
                        segment.text.center().y as f32 * scale + px * 0.36,
                        px,
                        400,
                        0.0,
                        pal.secondary_text,
                    );
                }
            }
        }
        if let Some(bar) = &frame.window.status_bar {
            fill_rect(
                pm,
                bar,
                scale,
                if state.active {
                    pal.toolbar
                } else {
                    pal.toolbar_inactive
                },
            );
            fill_rect(
                pm,
                &Rect::new(bar.x, bar.y, bar.w, m::SEPARATOR),
                scale,
                pal.separator,
            );
            let text = truncate_tail(
                &mut self.renderer.text,
                state.status,
                px,
                (bar.w - 2.0 * window::STATUS_BAR_LEADING_INSET) as f32 * scale,
            );
            let width = self.renderer.text.measure(&text, px, 400, 0.0);
            // centred, as in a screenshot (approximate)
            self.renderer.text.draw(
                pm,
                &text,
                bar.center().x as f32 * scale - width / 2.0,
                bar.center().y as f32 * scale + px * 0.36,
                px,
                400,
                0.0,
                pal.secondary_text,
            );
        }
    }

    /// The name editor over the label of the item it edits: a field with the accent as its border, the selection
    /// and the caret. approximate shapes (the design's field editor is not measured).
    fn paint_editor(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        frame: &Frame,
        edit: &NameEdit,
        state: &WindowState<'_>,
        pal: &Palette,
    ) {
        let Some(label) = frame.name_frame(edit.index()) else {
            return;
        };
        let text_size = match state.mode {
            ViewMode::Icons => f64::from(self.icons.text_size),
            ViewMode::List => f64::from(self.list.text_size),
        };
        let field = edit.field();
        let style = FieldStyle {
            size: text_size,
            weight: 400,
            letter_spacing: 0.0,
            text: pal.text,
            selection: pal.text_selection,
            caret: pal.text,
            caret_width: 1.0,
        };
        let boundaries = field.boundaries();
        let offsets = caret_offsets(
            &mut self.renderer.text,
            field.text(),
            &boundaries,
            &style,
            scale,
        );
        let text_w = offsets.last().copied().unwrap_or(0.0);
        let inner_w = text_w
            .max(label.w - 2.0 * EDITOR_PADDING)
            .max(EDITOR_MIN_WIDTH);
        let (w, h) = (
            inner_w + 2.0 * EDITOR_PADDING,
            label.h.min(text_size * 1.6 + 4.0).max(text_size + 4.0),
        );
        let x = match state.mode {
            ViewMode::Icons => label.center().x - w / 2.0,
            ViewMode::List => label.x - EDITOR_PADDING,
        }
        .max(frame.window.content.x);
        let y = label.center().y - h / 2.0;
        fill_rounded(
            pm,
            &Rect::new(
                x - EDITOR_BORDER,
                y - EDITOR_BORDER,
                w + 2.0 * EDITOR_BORDER,
                h + 2.0 * EDITOR_BORDER,
            ),
            EDITOR_RADIUS + EDITOR_BORDER,
            scale,
            pal.accent,
        );
        fill_rounded(
            pm,
            &Rect::new(x, y, w, h),
            EDITOR_RADIUS,
            scale,
            pal.background,
        );
        let area = Rect::new(x + EDITOR_PADDING, y, inner_w, h);
        let geo = GeoRect::new(area.x, area.y, area.w, area.h);
        let content = FieldContent {
            text: field.text(),
            boundaries: &boundaries,
            offsets: &offsets,
            selection: field.selection(),
            composition: field.composition(),
            caret: state.edit_caret.then_some(field.caret()),
            scroll: field.scroll(),
        };
        draw_field(
            &mut self.renderer.text,
            pm,
            &geo,
            centred_baseline(&geo, text_size),
            scale,
            &content,
            &style,
        );
        self.editor_text = Some((area, offsets));
    }

    /// Draw level `level` of an open context menu into `pm` (device pixels, sized to the level's popup; the server draws the
    /// material behind it).
    pub fn paint_menu(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        menu: &Menu,
        level: usize,
        pal: &Palette,
    ) {
        self.renderer.text.set_scale(scale);
        let layout = menu.layout(level);
        let px = menu_metrics::TEXT_SIZE as f32 * scale;
        for (i, (item, row)) in menu.items(level).iter().zip(&layout.rows).enumerate() {
            let r = row.offset(-layout.rect.x, -layout.rect.y);
            match &item.kind {
                ItemKind::Separator => {
                    let line = Rect::new(
                        r.x + menu_metrics::SELECTION_INSET,
                        r.y + (r.h - 1.0) / 2.0,
                        r.w - 2.0 * menu_metrics::SELECTION_INSET,
                        1.0,
                    );
                    fill_rect(pm, &line, scale, pal.separator);
                    continue;
                }
                ItemKind::TagRow { on } => {
                    self.paint_tag_row(pm, scale, &r, &item.label, on, pal);
                    continue;
                }
                ItemKind::ViewRow { current } => {
                    self.paint_view_row(pm, scale, &r, *current, pal);
                    continue;
                }
                ItemKind::Command(_) | ItemKind::Submenu(_) | ItemKind::Unavailable => {}
            }
            let hot = menu.hovered(level) == Some(i) && item.is_selectable();
            if hot {
                let selection = Rect::new(
                    r.x + menu_metrics::SELECTION_INSET,
                    r.y,
                    r.w - 2.0 * menu_metrics::SELECTION_INSET,
                    r.h,
                );
                fill_rounded(
                    pm,
                    &selection,
                    menu_metrics::SELECTION_RADIUS,
                    scale,
                    pal.accent,
                );
            }
            let base = if hot { pal.accent_text } else { pal.text };
            let color = [
                base[0],
                base[1],
                base[2],
                base[3] * if item.enabled { 1.0 } else { DISABLED_ALPHA },
            ];
            let baseline = (r.y + r.h / 2.0) as f32 * scale + px * 0.36;
            if item.checked {
                self.paint_check(
                    pm,
                    scale,
                    &Rect::new(r.x + menu_metrics::SELECTION_INSET + 4.0, r.y, 9.0, r.h),
                    color,
                );
            }
            self.renderer.text.draw(
                pm,
                &item.label,
                (r.x + menu_metrics::TITLE_OFFSET) as f32 * scale,
                baseline,
                px,
                400,
                0.0,
                color,
            );
            if matches!(item.kind, ItemKind::Submenu(_)) {
                let arrow = Rect::new(
                    r.right() - menu_metrics::SUBMENU_ARROW - menu_metrics::SELECTION_INSET,
                    r.y,
                    menu_metrics::SUBMENU_ARROW,
                    r.h,
                );
                self.renderer.draw_symbol(
                    pm,
                    &arrow,
                    "symbol.chevron.right",
                    TOOLBAR_GLYPH,
                    scale,
                    color,
                );
            }
        }
    }

    /// A check mark in the state column, centred in `r`. approximate shape (the design draws a glyph).
    fn paint_check(&mut self, pm: &mut Pixmap, scale: f32, r: &Rect, color: Rgba) {
        let c = r.center();
        let (cx, cy) = (c.x as f32 * scale, c.y as f32 * scale);
        let mut pb = PathBuilder::new();
        pb.move_to(cx - 3.5 * scale, cy + 0.5 * scale);
        pb.line_to(cx - 1.0 * scale, cy + 3.5 * scale);
        pb.line_to(cx + 4.0 * scale, cy - 3.5 * scale);
        if let Some(p) = pb.finish() {
            pm.stroke_path(
                &p,
                &paint(color),
                &Stroke {
                    width: 1.5 * scale,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }

    /// The tag row: seven colour dots with the tags of the whole selection marked, the title under them (size,
    /// the dot size, the marking and the caption's place are approximate).
    fn paint_tag_row(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        r: &Rect,
        title: &str,
        on: &[TagColor],
        pal: &Palette,
    ) {
        for (color, dot) in tag_dots(r) {
            let fill = pal.tag_color(color);
            fill_rounded(pm, &dot, dot.w / 2.0, scale, fill);
            if on.contains(&color) {
                fill_rounded(
                    pm,
                    &dot.inset(dot.w * 0.3, dot.h * 0.3),
                    dot.w * 0.2,
                    scale,
                    [1.0, 1.0, 1.0, 0.9],
                );
            }
        }
        let px = menu_metrics::TEXT_SIZE as f32 * scale;
        let w = f64::from(self.renderer.text.measure(title, px, 400, 0.0)) / f64::from(scale);
        let y = r.y + menu_metrics::TAG_DOT_TOP + menu_metrics::TAG_DOT_DIAMETER + 6.0;
        self.renderer.text.draw(
            pm,
            title,
            (r.center().x - w / 2.0) as f32 * scale,
            (y + 13.0) as f32 * scale,
            px,
            400,
            0.0,
            pal.secondary_text,
        );
    }

    /// The view row: four segments, the current mode on a quaternary fill, those without a view dimmed. approximate shapes.
    fn paint_view_row(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        r: &Rect,
        current: ViewMode,
        pal: &Palette,
    ) {
        for (segment, rect) in view_segments(r) {
            let usable = segment.mode().is_some();
            if segment.mode() == Some(current) {
                fill_rounded(
                    pm,
                    &rect,
                    menu_metrics::SELECTION_RADIUS,
                    scale,
                    pal.tertiary_label,
                );
            }
            let c = pal.text;
            self.renderer.draw_symbol(
                pm,
                &rect,
                segment.symbol(),
                TOOLBAR_GLYPH,
                scale,
                [
                    c[0],
                    c[1],
                    c[2],
                    c[3] * if usable { 1.0 } else { DISABLED_ALPHA },
                ],
            );
        }
    }

    fn paint_sidebar(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        w: &WindowLayout,
        state: &WindowState<'_>,
        pal: &Palette,
    ) {
        let rows = sidebar::RowSize::DEFAULT;
        for (group, r) in &w.groups {
            let px = sidebar::GROUP_FONT_SIZE as f32 * scale;
            let baseline = (r.y + r.h / 2.0) as f32 * scale + px * 0.36;
            self.renderer.text.draw(
                pm,
                group.title(),
                (r.x + sidebar::GROUP_ROW_INDENT) as f32 * scale,
                baseline,
                px,
                600,
                0.0,
                pal.secondary_text,
            );
        }
        let here = state.browser.dir();
        for (place, r) in state.places.iter().zip(&w.places) {
            if place.path == here {
                fill_rounded(pm, r, m::HIGHLIGHT_RADIUS, scale, pal.inactive_selection);
            }
            let icon = Rect::new(
                r.x + sidebar::INDENT,
                r.y + (r.h - sidebar::CELL_ICON) / 2.0,
                sidebar::CELL_ICON,
                sidebar::CELL_ICON,
            );
            self.renderer.draw_symbol(
                pm,
                &icon,
                place.kind.symbol(),
                SIDEBAR_GLYPH,
                scale,
                pal.accent,
            );
            let px = rows.font_size() as f32 * scale;
            let x = icon.x + sidebar::CELL_TEXT_X;
            let name = truncate_tail(
                &mut self.renderer.text,
                &place.name,
                px,
                ((r.right() - x) as f32 - 4.0) * scale,
            );
            self.renderer.text.draw(
                pm,
                &name,
                x as f32 * scale,
                (r.y + r.h / 2.0) as f32 * scale + px * 0.36,
                px,
                400,
                0.0,
                pal.text,
            );
        }
    }

    fn paint_toolbar(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        w: &WindowLayout,
        state: &WindowState<'_>,
        pal: &Palette,
    ) {
        let history = state.browser.history();
        let dimmed = [pal.text[0], pal.text[1], pal.text[2], pal.text[3] * 0.3];
        self.renderer.draw_symbol(
            pm,
            &w.back,
            "symbol.chevron.left",
            TOOLBAR_GLYPH,
            scale,
            if history.back_target().is_some() {
                pal.text
            } else {
                dimmed
            },
        );
        self.renderer.draw_symbol(
            pm,
            &w.forward,
            "symbol.chevron.right",
            TOOLBAR_GLYPH,
            scale,
            if history.forward_target().is_some() {
                pal.text
            } else {
                dimmed
            },
        );
        let px = m::TITLE_SIZE as f32 * scale;
        let title = truncate_middle(
            &mut self.renderer.text,
            state.title,
            px,
            w.title.w as f32 * scale,
        );
        let color = if state.active {
            pal.text
        } else {
            pal.secondary_text
        };
        self.renderer.text.draw(
            pm,
            &title,
            w.title.x as f32 * scale,
            w.title.center().y as f32 * scale + px * 0.36,
            px,
            m::TITLE_WEIGHT,
            0.0,
            color,
        );
        for &(item, r) in &w.items {
            let color = if item.enabled() { pal.text } else { dimmed };
            // a popup's glyph, with its chevrons at the right
            let glyph = Rect::new(r.x, r.y, r.w - m::POPUP_CHEVRON, r.h);
            let chevron = Rect::new(glyph.right(), r.y, m::POPUP_CHEVRON, r.h);
            match item {
                ToolbarItem::View => {
                    let symbol = match state.mode {
                        ViewMode::Icons => "symbol.grid",
                        ViewMode::List => "symbol.list",
                    };
                    self.renderer
                        .draw_symbol(pm, &glyph, symbol, TOOLBAR_GLYPH, scale, color);
                    self.renderer.draw_symbol(
                        pm,
                        &chevron,
                        "symbol.chevron.up.down",
                        POPUP_CHEVRON_GLYPH,
                        scale,
                        color,
                    );
                }
                ToolbarItem::GroupBy => {
                    self.renderer.draw_symbol(
                        pm,
                        &glyph,
                        "symbol.columns",
                        TOOLBAR_GLYPH,
                        scale,
                        color,
                    );
                    paint_chevron_down(pm, chevron.center(), scale, color);
                }
                ToolbarItem::Tags => {
                    self.renderer
                        .draw_symbol(pm, &r, "symbol.tag", TOOLBAR_GLYPH, scale, color)
                }
                ToolbarItem::Action => {
                    self.renderer.draw_symbol(
                        pm,
                        &glyph,
                        "symbol.ellipsis.circle",
                        TOOLBAR_GLYPH,
                        scale,
                        color,
                    );
                    paint_chevron_down(pm, chevron.center(), scale, color);
                }
                ToolbarItem::Search => self.paint_search(pm, scale, &r, state, pal),
            }
        }
    }

    /// The search field: a rounded quaternary fill with the magnifying glass, the placeholder or the text with a caret,
    /// an accent ring while it has the keyboard. approximate shapes (the field's drawing is not measured).
    fn paint_search(
        &mut self,
        pm: &mut Pixmap,
        scale: f32,
        r: &Rect,
        state: &WindowState<'_>,
        pal: &Palette,
    ) {
        fill_rounded(pm, r, m::SEARCH_RADIUS, scale, pal.quaternary_label);
        if state.search_focused {
            stroke_rounded(pm, r, m::SEARCH_RADIUS, SEARCH_RING, scale, pal.accent);
        }
        let glass = Rect::new(r.x + m::SEARCH_INSET, r.y, m::SEARCH_GLYPH, r.h);
        self.renderer.draw_symbol(
            pm,
            &glass,
            "symbol.magnifyingglass",
            m::SEARCH_GLYPH,
            scale,
            pal.secondary_text,
        );
        let x = glass.right() + m::SEARCH_INSET;
        let area = Rect::new(x, r.y, (r.right() - m::SEARCH_INSET - x).max(0.0), r.h);
        let geo = GeoRect::new(area.x, area.y, area.w, area.h);
        let baseline = centred_baseline(&geo, SEARCH_TEXT_SIZE);
        let field = state.search;
        if field.is_empty() && !state.search_focused {
            self.renderer.text.draw(
                pm,
                "Search",
                x as f32 * scale,
                baseline as f32 * scale,
                SEARCH_TEXT_SIZE as f32 * scale,
                400,
                0.0,
                pal.secondary_text,
            );
            self.search_text = None;
            return;
        }
        let style = FieldStyle {
            size: SEARCH_TEXT_SIZE,
            weight: 400,
            letter_spacing: 0.0,
            text: pal.text,
            selection: pal.text_selection,
            caret: pal.text,
            caret_width: 1.0,
        };
        let boundaries = field.boundaries();
        let offsets = caret_offsets(
            &mut self.renderer.text,
            field.text(),
            &boundaries,
            &style,
            scale,
        );
        let caret_x = boundaries
            .iter()
            .position(|b| *b == field.caret())
            .map_or(0.0, |i| offsets[i]);
        let scroll = field.scroll_for(caret_x, offsets.last().copied().unwrap_or(0.0), area.w);
        let content = FieldContent {
            text: field.text(),
            boundaries: &boundaries,
            offsets: &offsets,
            selection: field.selection(),
            composition: field.composition(),
            caret: state.search_caret.then_some(field.caret()),
            scroll,
        };
        draw_field(
            &mut self.renderer.text,
            pm,
            &geo,
            baseline,
            scale,
            &content,
            &style,
        );
        self.search_text = Some((area, offsets, scroll));
    }
}

/// A small chevron pointing down centred on `c` (window points): the menu marker of a popup item. approximate shape.
fn paint_chevron_down(pm: &mut Pixmap, c: Point, scale: f32, color: Rgba) {
    let (cx, cy) = (c.x as f32 * scale, c.y as f32 * scale);
    let mut pb = PathBuilder::new();
    pb.move_to(cx - 3.5 * scale, cy - 1.75 * scale);
    pb.line_to(cx, cy + 1.75 * scale);
    pb.line_to(cx + 3.5 * scale, cy - 1.75 * scale);
    if let Some(p) = pb.finish() {
        pm.stroke_path(
            &p,
            &paint(color),
            &Stroke {
                width: 1.4 * scale,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::Browser;
    use crate::sort::SortSpec;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn places() -> Vec<Place> {
        let p = |name: &str, path: &str, group, kind| Place {
            name: name.into(),
            path: PathBuf::from(path),
            group,
            kind,
        };
        vec![
            p("Home", "/home/ada", PlaceGroup::Favorites, PlaceKind::Home),
            p(
                "Desktop",
                "/home/ada/Desktop",
                PlaceGroup::Favorites,
                PlaceKind::Desktop,
            ),
            p("Computer", "/", PlaceGroup::Locations, PlaceKind::Computer),
        ]
    }

    /// One flat outline row per entry, folders expandable.
    fn rows(entries: &[Entry]) -> Vec<RowSpec> {
        entries
            .iter()
            .map(|e| RowSpec {
                level: 0,
                expandable: e.is_dir,
                expanded: false,
            })
            .collect()
    }

    fn entries(n: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| Entry::synthetic(&format!("item {i:03}"), i % 3 == 0, 10))
            .collect()
    }

    #[test]
    fn the_layout_uses_the_measured_chrome_and_gives_way_down_to_the_minimums() {
        let l = WindowLayout::new(Size { w: 920.0, h: 436.0 }, &places(), Bars::HIDDEN);
        assert_eq!(l.sidebar.w, 200.0);
        assert_eq!(
            l.sidebar,
            Rect::new(0.0, 0.0, 200.0, 436.0),
            "the sidebar runs the full height under the window control buttons"
        );
        assert_eq!(l.toolbar, Rect::new(201.0, 0.0, 719.0, 52.0));
        assert_eq!(l.content, Rect::new(201.0, 53.0, 719.0, 383.0));
        assert_eq!((l.back.w + l.forward.w, l.back.h), (53.0, 28.0));
        // Favorites title, two rows, 13 pt, Locations title, one row
        assert_eq!(
            l.groups.iter().map(|(g, r)| (*g, r.y)).collect::<Vec<_>>(),
            [
                (PlaceGroup::Favorites, 62.0),
                (PlaceGroup::Locations, 62.0 + 19.0 + 56.0 + 13.0)
            ]
        );
        assert_eq!(
            l.places.iter().map(|r| (r.y, r.h)).collect::<Vec<_>>(),
            [(81.0, 28.0), (109.0, 28.0), (169.0, 28.0)]
        );

        let narrow = WindowLayout::new(
            Size {
                w: m::WINDOW_MIN.0,
                h: m::WINDOW_MIN.1,
            },
            &places(),
            Bars::HIDDEN,
        );
        assert_eq!(
            (narrow.sidebar.w, narrow.content.w, narrow.content.h),
            (128.0, 316.0, 100.0)
        );
    }

    #[test]
    fn hits_find_controls_places_items_and_list_headers() {
        let mut painter = WindowPainter::new(crate::render::renderer());
        let size = Size { w: 920.0, h: 436.0 };
        let items = entries(40);
        let frame = painter.frame(
            size,
            &places(),
            Bars::HIDDEN,
            window::TOOLBAR_SEARCH_FIELD.0,
            Path::new("/home/ada"),
            ViewMode::Icons,
            &rows(&items),
            0.0,
            1.0,
        );
        let at = |r: &Rect| r.center();
        assert_eq!(frame.hit(at(&frame.window.back)), Hit::Back);
        let item = |i: ToolbarItem| {
            frame
                .window
                .items
                .iter()
                .find(|(t, _)| *t == i)
                .map(|(_, r)| *r)
                .unwrap()
        };
        assert_eq!(
            frame.hit(at(&item(ToolbarItem::View))),
            Hit::Toolbar(ToolbarItem::View)
        );
        assert_eq!(
            frame.hit(at(&item(ToolbarItem::Search))),
            Hit::Toolbar(ToolbarItem::Search)
        );
        assert_eq!(
            frame.hit(at(&item(ToolbarItem::GroupBy))),
            Hit::Chrome,
            "Group By is disabled"
        );
        assert_eq!(frame.hit(at(&frame.window.places[2])), Hit::Place(2));
        // the strip without a control moves the window, over the sidebar's top and the toolbar alike
        assert_eq!(frame.hit(Point { x: 100.0, y: 20.0 }), Hit::Titlebar);
        assert_eq!(frame.hit(at(&frame.window.title)), Hit::Titlebar);
        assert_eq!(frame.hit(Point { x: 100.0, y: 300.0 }), Hit::Chrome);
        let ContentLayout::Icons(l) = &frame.content else {
            panic!("icon layout")
        };
        let icon = l.cells[5].icon_frame;
        let c = frame.window.content;
        assert_eq!(
            frame.hit(Point {
                x: c.x + icon.center().x,
                y: c.y + icon.center().y
            }),
            Hit::Item(5)
        );

        let mut list = painter.frame(
            size,
            &places(),
            Bars::HIDDEN,
            window::TOOLBAR_SEARCH_FIELD.0,
            Path::new("/home/ada"),
            ViewMode::List,
            &rows(&items),
            0.0,
            1.0,
        );
        assert_eq!(
            list.hit(Point {
                x: c.x + 20.0,
                y: c.y + 10.0
            }),
            Hit::Column(ColumnId::Name)
        );
        let ContentLayout::List(rows, _) = &list.content else {
            panic!("list layout")
        };
        let pitch = rows.row_height + list_view::INTERCELL.1;
        let first_row = Point {
            x: c.x + 20.0,
            y: c.y + list_view::HEADER_HEIGHT + 1.0,
        };
        assert_eq!(list.hit(first_row), Hit::Item(0));
        // scrolled by three rows the same point is over the fourth item
        list.scroll_by(3.0 * pitch);
        assert_eq!(list.hit(first_row), Hit::Item(3));
    }

    #[test]
    fn scrolling_stays_in_the_content_and_reveal_scrolls_as_little_as_needed() {
        let mut painter = WindowPainter::new(crate::render::renderer());
        let items = entries(200);
        let mut f = painter.frame(
            Size { w: 920.0, h: 436.0 },
            &places(),
            Bars::HIDDEN,
            window::TOOLBAR_SEARCH_FIELD.0,
            Path::new("/home/ada"),
            ViewMode::List,
            &rows(&items),
            -50.0,
            1.0,
        );
        assert_eq!(f.scroll, 0.0);
        f.scroll_by(1e9);
        assert_eq!(f.scroll, f.max_scroll());
        assert!(f.max_scroll() > 0.0);

        f.scroll = 0.0;
        f.reveal(199);
        let ContentLayout::List(rows, _) = &f.content else {
            panic!("list layout")
        };
        let (first, last) = (rows.rows[0].frame, rows.rows[199].frame);
        assert_eq!(f.scroll + f.window.content.h, last.bottom());
        // the first row reveals below the fixed header
        f.reveal(0);
        assert_eq!(f.scroll + list_view::HEADER_HEIGHT, first.y);
        let before = f.scroll;
        f.reveal(1);
        assert_eq!(f.scroll, before, "an item in sight does not scroll");
    }

    #[test]
    fn keyboard_steps_follow_the_view() {
        let mut painter = WindowPainter::new(crate::render::renderer());
        let items = entries(30);
        let size = Size { w: 920.0, h: 436.0 };
        let list = painter
            .frame(
                size,
                &places(),
                Bars::HIDDEN,
                window::TOOLBAR_SEARCH_FIELD.0,
                Path::new("/home/ada"),
                ViewMode::List,
                &rows(&items),
                0.0,
                1.0,
            )
            .content;
        assert_eq!(list.step(None, 30, Move::Down), Some(0));
        assert_eq!(list.step(None, 30, Move::Up), Some(29));
        assert_eq!(list.step(Some(4), 30, Move::Down), Some(5));
        assert_eq!(list.step(Some(0), 30, Move::Up), None);
        assert_eq!(list.step(Some(29), 30, Move::Down), None);
        assert_eq!(list.step(Some(4), 30, Move::Right), None);
        assert_eq!(list.step(Some(4), 30, Move::Last), Some(29));
        assert_eq!(list.step(None, 0, Move::Down), None);

        let icons = painter
            .frame(
                size,
                &places(),
                Bars::HIDDEN,
                window::TOOLBAR_SEARCH_FIELD.0,
                Path::new("/home/ada"),
                ViewMode::Icons,
                &rows(&items),
                0.0,
                1.0,
            )
            .content;
        let ContentLayout::Icons(l) = &icons else {
            panic!("icon layout")
        };
        let columns = l.columns;
        assert!(columns > 1);
        assert_eq!(icons.step(Some(0), 30, Move::Right), Some(1));
        assert_eq!(icons.step(Some(0), 30, Move::Down), Some(columns));
        assert_eq!(icons.step(Some(columns), 30, Move::Up), Some(0));
        assert_eq!(icons.step(Some(0), 30, Move::Left), None);
    }

    #[test]
    fn a_frame_paints_chrome_and_content() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), b"x").unwrap();
        let mut browser = Browser::open(
            d.path().to_path_buf(),
            SortSpec::default(),
            PathBuf::from("/nonexistent/recently-used.xbel"),
            None,
            Arc::new(|| {}),
        )
        .unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while browser.entries().is_empty() {
            assert!(
                std::time::Instant::now() < end,
                "timed out listing the folder"
            );
            browser.poll();
        }
        let places = vec![Place {
            name: "Here".into(),
            path: d.path().to_path_buf(),
            group: PlaceGroup::Favorites,
            kind: PlaceKind::Home,
        }];
        let mut painter = WindowPainter::new(crate::render::renderer());
        let pal = Palette::builtin(false);
        let size = Size { w: 600.0, h: 300.0 };
        let frame = painter.frame(
            size,
            &places,
            Bars::HIDDEN,
            window::TOOLBAR_SEARCH_FIELD.0,
            Path::new("/home/ada"),
            ViewMode::Icons,
            browser.outline(),
            0.0,
            2.0,
        );
        let mut pm = Pixmap::new(1200, 600).unwrap();
        let state = WindowState {
            browser: &browser,
            places: &places,
            mode: ViewMode::Icons,
            active: true,
            search: &TextField::new(),
            search_focused: false,
            search_caret: false,
            title: "Here",
            now: SystemTime::now(),
            edit: None,
            edit_caret: false,
            drop_target: None,
            marquee: None,
            status: "3 items",
        };
        painter.paint(&mut pm, 2.0, &frame, &state, &pal);
        let px = |p: Point| pm.pixel((p.x * 2.0) as u32, (p.y * 2.0) as u32).unwrap();
        // the sidebar is left clear for the server's material, the toolbar of an active window and the content have the
        // view background, the current place its highlight
        assert_eq!(px(Point { x: 2.0, y: 290.0 }).alpha(), 0);
        let toolbar = px(Point { x: 400.0, y: 4.0 });
        assert_eq!(
            (toolbar.red(), toolbar.green(), toolbar.blue()),
            (0xFF, 0xFF, 0xFF)
        );
        let content = px(Point { x: 590.0, y: 290.0 });
        assert_eq!(
            (content.red(), content.green(), content.blue()),
            (0xFF, 0xFF, 0xFF)
        );
        let row = frame.window.places[0];
        let highlight = px(Point {
            x: row.right() - 3.0,
            y: row.center().y,
        });
        assert!(highlight.alpha() > 0, "the current place is highlighted");
        // the place's symbol is tinted with the accent inside the icon field
        let field = (
            row.x + sidebar::INDENT,
            row.y + (row.h - sidebar::CELL_ICON) / 2.0,
        );
        let accent = pal.accent.map(|c| (c * 255.0).round() as u8);
        let tinted = (0..(sidebar::CELL_ICON * 2.0) as u32)
            .flat_map(|dy| (0..(sidebar::CELL_ICON * 2.0) as u32).map(move |dx| (dx, dy)))
            .any(|(dx, dy)| {
                let c = pm
                    .pixel((field.0 * 2.0) as u32 + dx, (field.1 * 2.0) as u32 + dy)
                    .unwrap();
                c.alpha() == 255
                    && [c.red(), c.green(), c.blue()] == [accent[0], accent[1], accent[2]]
            });
        assert!(tinted, "the home symbol is drawn in the accent colour");
    }

    #[test]
    fn item_and_name_frames_are_in_window_points_and_follow_the_scroll() {
        let mut painter = WindowPainter::new(crate::render::renderer());
        let size = Size { w: 920.0, h: 436.0 };
        let items = entries(200);
        for mode in [ViewMode::Icons, ViewMode::List] {
            let top = painter.frame(
                size,
                &places(),
                Bars::HIDDEN,
                window::TOOLBAR_SEARCH_FIELD.0,
                Path::new("/home/ada"),
                mode,
                &rows(&items),
                0.0,
                1.0,
            );
            let scrolled = painter.frame(
                size,
                &places(),
                Bars::HIDDEN,
                window::TOOLBAR_SEARCH_FIELD.0,
                Path::new("/home/ada"),
                mode,
                &rows(&items),
                100.0,
                1.0,
            );
            let (a, b) = (top.name_frame(3).unwrap(), scrolled.name_frame(3).unwrap());
            assert_eq!((b.x, b.y), (a.x, a.y - 100.0), "{mode:?}");
            let (item, name) = (top.item_frame(3).unwrap(), top.name_frame(3).unwrap());
            assert!(
                item.contains(name.center()),
                "{mode:?}: the name lies in its item"
            );
            assert!(top.window.content.contains(item.center()), "{mode:?}");
            assert_eq!(top.hit(name.center()), Hit::Item(3), "{mode:?}");
        }
    }

    /// A browser on a folder of one file, and the places of a window on it.
    fn fixture_browser() -> (Browser, Vec<Place>, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), b"x").unwrap();
        let mut browser = Browser::open(
            d.path().to_path_buf(),
            SortSpec::default(),
            PathBuf::from("/nonexistent/recently-used.xbel"),
            None,
            Arc::new(|| {}),
        )
        .unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while browser.entries().is_empty() {
            assert!(
                std::time::Instant::now() < end,
                "timed out listing the folder"
            );
            browser.poll();
        }
        let places = vec![Place {
            name: "Here".into(),
            path: d.path().to_path_buf(),
            group: PlaceGroup::Favorites,
            kind: PlaceKind::Home,
        }];
        (browser, places, d)
    }

    #[test]
    fn a_rubber_band_selects_the_icons_it_touches_and_the_rows_it_crosses() {
        let mut painter = WindowPainter::new(crate::render::renderer());
        let entries: Vec<Entry> = (0..12)
            .map(|i| Entry::synthetic(&format!("file{i:02}"), false, 1))
            .collect();
        let places = Vec::new();
        let size = Size { w: 900.0, h: 500.0 };
        let icons = painter.frame(
            size,
            &places,
            Bars::HIDDEN,
            window::TOOLBAR_SEARCH_FIELD.0,
            Path::new("/home/ada"),
            ViewMode::Icons,
            &rows(&entries),
            0.0,
            1.0,
        );
        let (a, b) = (
            icons.content.frame_of(0).unwrap(),
            icons.content.frame_of(1).unwrap(),
        );
        let band = Rect::from_corners(
            Point {
                x: a.center().x,
                y: a.center().y,
            },
            Point {
                x: b.center().x,
                y: b.center().y,
            },
        );
        assert_eq!(
            icons.hits_in(&band),
            [0, 1],
            "a band from the first cell's centre to the second's"
        );
        assert_eq!(
            icons.hits_in(&Rect::new(size.w - 5.0, size.h - 5.0, 3.0, 3.0)),
            Vec::<usize>::new(),
            "empty space touches nothing"
        );
        let list = painter.frame(
            size,
            &places,
            Bars::HIDDEN,
            window::TOOLBAR_SEARCH_FIELD.0,
            Path::new("/home/ada"),
            ViewMode::List,
            &rows(&entries),
            0.0,
            1.0,
        );
        let (r2, r4) = (
            list.content.frame_of(2).unwrap(),
            list.content.frame_of(4).unwrap(),
        );
        assert_eq!(
            list.hits_in(&Rect::new(
                10.0,
                r2.center().y,
                5.0,
                r4.center().y - r2.center().y
            )),
            [2, 3, 4]
        );
        assert_eq!(
            list.hits_in(&Rect::new(10.0, 0.0, 5.0, list_view::HEADER_HEIGHT - 1.0)),
            Vec::<usize>::new(),
            "the header holds no rows"
        );
    }

    #[test]
    fn the_rubber_band_is_a_quaternary_fill_inside_a_one_point_tertiary_border() {
        let (browser, places, _dir) = fixture_browser();
        let mut painter = WindowPainter::new(crate::render::renderer());
        let pal = Palette::builtin(false);
        let size = Size { w: 600.0, h: 300.0 };
        let frame = painter.frame(
            size,
            &places,
            Bars::HIDDEN,
            window::TOOLBAR_SEARCH_FIELD.0,
            Path::new("/home/ada"),
            ViewMode::Icons,
            browser.outline(),
            0.0,
            1.0,
        );
        let mut pm = Pixmap::new(600, 300).unwrap();
        let origin = Point { x: 200.0, y: 100.0 };
        let at = Point {
            x: frame.window.content.x + 320.0,
            y: frame.window.content.y + 200.0,
        };
        let state = WindowState {
            browser: &browser,
            places: &places,
            mode: ViewMode::Icons,
            active: true,
            search: &TextField::new(),
            search_focused: false,
            search_caret: false,
            title: "Here",
            now: SystemTime::now(),
            edit: None,
            edit_caret: false,
            drop_target: None,
            marquee: Some((origin, at)),
            status: "3 items",
        };
        painter.paint(&mut pm, 1.0, &frame, &state, &pal);
        // the list shows the band too, below its header
        let list = painter.frame(
            size,
            &places,
            Bars::HIDDEN,
            window::TOOLBAR_SEARCH_FIELD.0,
            Path::new("/home/ada"),
            ViewMode::List,
            browser.outline(),
            0.0,
            1.0,
        );
        let mut lpm = Pixmap::new(600, 300).unwrap();
        let lstate = WindowState {
            browser: &browser,
            places: &places,
            mode: ViewMode::List,
            active: true,
            search: &TextField::new(),
            search_focused: false,
            search_caret: false,
            title: "Here",
            now: SystemTime::now(),
            edit: None,
            edit_caret: false,
            drop_target: None,
            marquee: Some((origin, at)),
            status: "3 items",
        };
        painter.paint(&mut lpm, 1.0, &list, &lstate, &pal);
        let (lx, ly) = (
            (list.window.content.x + origin.x) as u32 + 20,
            (list.window.content.y + origin.y) as u32 + 20,
        );
        assert_eq!(
            lpm.pixel(lx, ly).unwrap().red(),
            230,
            "the band is over the list"
        );
        let (x0, y0) = (
            (frame.window.content.x + origin.x) as u32,
            (frame.window.content.y + origin.y) as u32,
        );
        let inside = pm.pixel(x0 + 20, y0 + 20).unwrap();
        let edge = pm.pixel(x0, y0 + 20).unwrap();
        let outside = pm.pixel(x0 - 3, y0 + 20).unwrap();
        // black at 0.098 over white: 230; black at 0.2588 over that fill: 170
        assert_eq!((inside.red(), outside.red()), (230, 255));
        assert!(
            edge.red() < inside.red() - 40,
            "the 1 pt border is darker than the fill: {} vs {}",
            edge.red(),
            inside.red()
        );
        assert_eq!(
            pm.pixel(x0 + 1, y0 + 20).unwrap().red(),
            inside.red(),
            "the border is one point wide"
        );
    }

    #[test]
    fn a_selected_icon_has_a_quaternary_bubble_with_a_white_border_in_every_window_state() {
        let pal = Palette::builtin(false);
        let mut pm = Pixmap::new(200, 200).unwrap();
        pm.fill(tiny_skia::Color::WHITE);
        let icon = Rect::new(60.0, 60.0, 64.0, 64.0);
        crate::render::paint_icon_bubble(&mut pm, &icon, 1.0, &pal);
        // padding and radius 4, border 2: the bubble spans 56 to 128
        assert_eq!(
            pm.pixel(92, 57).unwrap().red(),
            243,
            "the white 50 % border over the fill"
        );
        assert_eq!(
            pm.pixel(92, 70).unwrap().red(),
            230,
            "the fill inside the icon's frame"
        );
        assert_eq!(
            pm.pixel(92, 54).unwrap().red(),
            255,
            "nothing outside the bubble"
        );
        assert_eq!(
            pm.pixel(56, 56).unwrap().red(),
            255,
            "the corner is rounded"
        );
    }

    #[test]
    fn a_menu_level_highlights_its_hovered_row_and_draws_the_submenu_arrow() {
        use crate::context_menu::{AppChoice, Menu};
        use crate::menu_spec::{item_menu, Subject};
        let mut painter = WindowPainter::new(crate::render::renderer());
        let pal = Palette::builtin(false);
        let apps = [AppChoice {
            id: "a.desktop".into(),
            name: "Viewer".into(),
            is_default: false,
        }];
        let subject = Subject {
            count: 1,
            name: "a".into(),
            all_folders: false,
            single_link: false,
            in_trash: false,
            tags_on: Vec::new(),
        };
        let mut menu = Menu::open(
            item_menu(&subject, &apps),
            Point { x: 10.0, y: 10.0 },
            Rect::new(0.0, 0.0, 600.0, 400.0),
            &|l| l.len() as f64 * 7.0,
        );
        let layout = menu.layout(0).clone();
        let mut pm = Pixmap::new(layout.rect.w as u32, layout.rect.h as u32).unwrap();
        painter.paint_menu(&mut pm, 1.0, &menu, 0, &pal);
        let probe = |pm: &Pixmap, row: usize| {
            let r = layout.rows[row].offset(-layout.rect.x, -layout.rect.y);
            let c = pm
                .pixel((r.x + r.w / 2.0) as u32, (r.y + 2.0) as u32)
                .unwrap();
            (c.red(), c.green(), c.blue(), c.alpha())
        };
        // row 0 is New Folder with Selection, row 1 Open, row 2 Open With
        assert_eq!(
            probe(&pm, 1).3,
            0,
            "an idle row is left to the material behind it"
        );
        menu.hover(0, Some(1), 0.0);
        let mut hot = Pixmap::new(layout.rect.w as u32, layout.rect.h as u32).unwrap();
        painter.paint_menu(&mut hot, 1.0, &menu, 0, &pal);
        assert_eq!(
            probe(&hot, 1).3,
            0xFF,
            "the hovered row is filled with the accent"
        );
        let arrow_row = layout.rows[2].offset(-layout.rect.x, -layout.rect.y);
        let arrow = |pm: &Pixmap| {
            (0..arrow_row.h as u32).any(|dy| {
                (arrow_row.right() as u32 - 24..arrow_row.right() as u32 - 8)
                    .any(|x| pm.pixel(x, arrow_row.y as u32 + dy).unwrap().alpha() > 0)
            })
        };
        assert!(arrow(&pm), "Open With shows an arrow at its trailing edge");
    }

    #[test]
    fn the_bars_are_hidden_by_default_and_stack_under_the_content_at_their_measured_heights() {
        let size = Size { w: 920.0, h: 436.0 };
        let hidden = WindowLayout::new(size, &places(), Bars::HIDDEN);
        assert_eq!((hidden.path_bar, hidden.status_bar), (None, None));
        assert_eq!(hidden.content.bottom(), 436.0);
        let both = WindowLayout::new(
            size,
            &places(),
            Bars {
                path: true,
                status: true,
            },
        );
        assert_eq!(
            both.status_bar,
            Some(Rect::new(201.0, 416.0, 719.0, 20.0)),
            "20 pt status bar at the bottom"
        );
        assert_eq!(
            both.path_bar,
            Some(Rect::new(201.0, 388.0, 719.0, 28.0)),
            "28 pt path bar above it"
        );
        assert_eq!(both.content, Rect::new(201.0, 53.0, 719.0, 335.0));
        let status_only = WindowLayout::new(
            size,
            &places(),
            Bars {
                path: false,
                status: true,
            },
        );
        assert_eq!(status_only.content.bottom(), 416.0);
    }

    #[test]
    fn the_path_chain_starts_at_the_root_under_the_computer_name_or_at_the_trash_or_recents() {
        let mut ps = places();
        ps.push(Place {
            name: "Trash".into(),
            path: PathBuf::from("/home/ada/.local/share/Trash/files"),
            group: PlaceGroup::Locations,
            kind: PlaceKind::Trash,
        });
        ps.push(Place {
            name: "Recents".into(),
            path: PathBuf::from("recent:///"),
            group: PlaceGroup::Favorites,
            kind: PlaceKind::Recents,
        });
        let chain = |dir: &str| {
            path_chain(Path::new(dir), &ps)
                .into_iter()
                .map(|(p, n)| (p.display().to_string(), n))
                .collect::<Vec<_>>()
        };
        let s = |p: &str, n: &str| (p.to_string(), n.to_string());
        assert_eq!(
            chain("/home/ada/Desktop"),
            [
                s("/", "Computer"),
                s("/home", "home"),
                s("/home/ada", "ada"),
                s("/home/ada/Desktop", "Desktop")
            ]
        );
        assert_eq!(chain("/"), [s("/", "Computer")]);
        assert_eq!(
            chain("/home/ada/.local/share/Trash/files/old"),
            [
                s("/home/ada/.local/share/Trash/files", "Trash"),
                s("/home/ada/.local/share/Trash/files/old", "old")
            ]
        );
        assert_eq!(chain("recent:///"), [s("recent:///", "Recents")]);
    }

    #[test]
    fn path_segments_flow_with_their_icons_and_drop_outer_names_when_narrow() {
        let chain: Vec<(PathBuf, String)> = ["/", "/home", "/home/ada"]
            .iter()
            .map(|p| (PathBuf::from(p), p.to_string()))
            .collect();
        let bar = Rect::new(201.0, 388.0, 719.0, 28.0);
        let wide = path_segments(&bar, &chain, &[60.0, 40.0, 30.0]);
        assert_eq!(wide[0].icon, Rect::new(209.0, 394.0, 16.0, 16.0));
        assert_eq!(wide[0].text, Rect::new(229.0, 388.0, 60.0, 28.0));
        assert_eq!(wide[1].icon.x, 229.0 + 60.0 + 16.0);
        assert_eq!(
            wide[2].frame.right(),
            209.0 + 3.0 * 20.0 + 130.0 + 2.0 * 16.0
        );
        // 104 pt of room: the two outer names go, the folder's own name is cut at the bar's end (92 to 112)
        let narrow = path_segments(
            &Rect::new(0.0, 0.0, 120.0, 28.0),
            &chain,
            &[60.0, 40.0, 30.0],
        );
        assert_eq!(
            narrow.iter().map(|s| s.text.w).collect::<Vec<_>>(),
            [0.0, 0.0, 20.0]
        );
        assert_eq!(narrow[1].frame, Rect::new(40.0, 0.0, 16.0, 28.0));
        let tiny = path_segments(
            &Rect::new(0.0, 0.0, 100.0, 28.0),
            &chain,
            &[60.0, 40.0, 30.0],
        );
        assert_eq!(tiny[2].text, Rect::new(92.0, 0.0, 0.0, 28.0));
    }

    #[test]
    fn the_status_text_counts_the_items_and_the_space_left() {
        assert_eq!(
            status_text(207, Some(227_270_000_000)),
            "207 items, 227.27 GB available"
        );
        assert_eq!(
            status_text(1, Some(250_000_000)),
            "1 item, 250 MB available"
        );
        assert_eq!(status_text(0, None), "0 items");
    }

    #[test]
    fn a_click_finds_the_path_segment_and_the_disclosure_triangle() {
        let mut painter = WindowPainter::new(crate::render::renderer());
        let size = Size { w: 920.0, h: 436.0 };
        let items = entries(5);
        let frame = painter.frame(
            size,
            &places(),
            Bars {
                path: true,
                status: true,
            },
            window::TOOLBAR_SEARCH_FIELD.0,
            Path::new("/home/ada/Desktop"),
            ViewMode::List,
            &rows(&items),
            0.0,
            1.0,
        );
        assert_eq!(
            frame
                .path
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["Computer", "home", "ada", "Desktop"]
        );
        assert_eq!(frame.hit(frame.path[2].icon.center()), Hit::PathSegment(2));
        let status = frame.window.status_bar.unwrap();
        assert_eq!(frame.hit(status.center()), Hit::Chrome);
        // item 0 is a folder: its triangle is the first 12 pt of the row at level 0, the rest of the row is the item
        let row = frame.item_frame(0).unwrap();
        assert_eq!(
            frame.hit(Point {
                x: row.x + 6.0,
                y: row.center().y
            }),
            Hit::Disclosure(0)
        );
        assert_eq!(
            frame.hit(Point {
                x: row.x + 40.0,
                y: row.center().y
            }),
            Hit::Item(0)
        );
        let file = frame.item_frame(1).unwrap();
        assert_eq!(
            frame.hit(Point {
                x: file.x + 6.0,
                y: file.center().y
            }),
            Hit::Item(1),
            "a file has no triangle"
        );
    }

    #[test]
    fn toolbar_items_sit_at_the_right_at_their_sizes_and_drop_when_narrow() {
        let l = WindowLayout::new(Size { w: 920.0, h: 436.0 }, &places(), Bars::HIDDEN);
        assert_eq!(
            l.items,
            [
                (ToolbarItem::View, Rect::new(594.0, 14.0, 44.0, 24.0)),
                (ToolbarItem::GroupBy, Rect::new(650.0, 14.0, 44.0, 24.0)),
                (ToolbarItem::Tags, Rect::new(706.0, 13.5, 29.0, 25.0)),
                (ToolbarItem::Action, Rect::new(747.0, 14.0, 41.0, 24.0)),
                (ToolbarItem::Search, Rect::new(800.0, 15.0, 112.0, 22.0)),
            ]
        );
        assert_eq!(l.title, Rect::new(274.0, 0.0, 308.0, 52.0));
        // 163 pt for items at the minimum width: Group By, Tags, Action and Search give way, the view popup stays
        let narrow = WindowLayout::new(
            Size {
                w: m::WINDOW_MIN.0,
                h: m::WINDOW_MIN.1,
            },
            &places(),
            Bars::HIDDEN,
        );
        assert_eq!(
            narrow.items,
            [(ToolbarItem::View, Rect::new(393.0, 14.0, 44.0, 24.0))]
        );
        // 246 pt at 600 pt: Group By and Tags give way
        let middle = WindowLayout::new(Size { w: 600.0, h: 436.0 }, &places(), Bars::HIDDEN);
        assert_eq!(
            middle.items.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [ToolbarItem::View, ToolbarItem::Action, ToolbarItem::Search]
        );
    }

    #[test]
    fn the_focused_search_field_draws_its_ring_text_and_caret() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("editor.txt"), b"x").unwrap();
        let mut browser = Browser::open(
            d.path().to_path_buf(),
            SortSpec::default(),
            PathBuf::from("/nonexistent/recently-used.xbel"),
            None,
            Arc::new(|| {}),
        )
        .unwrap();
        while browser.entries().is_empty() {
            browser.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        browser.set_filter("ed");
        let places = places();
        let mut painter = WindowPainter::new(crate::render::renderer());
        let pal = Palette::builtin(false);
        let size = Size { w: 920.0, h: 436.0 };
        let frame = painter.frame(
            size,
            &places,
            Bars {
                path: true,
                status: true,
            },
            window::TOOLBAR_SEARCH_FIELD.0,
            d.path(),
            ViewMode::List,
            browser.outline(),
            0.0,
            1.0,
        );
        let mut pm = Pixmap::new(920, 436).unwrap();
        let mut search = TextField::new();
        search.insert("ed");
        let state = WindowState {
            browser: &browser,
            places: &places,
            mode: ViewMode::List,
            active: true,
            search: &search,
            search_focused: true,
            search_caret: true,
            title: "Here",
            now: SystemTime::now(),
            edit: None,
            edit_caret: false,
            drop_target: None,
            marquee: None,
            status: "1 item",
        };
        painter.paint(&mut pm, 1.0, &frame, &state, &pal);
        let field = frame
            .window
            .items
            .iter()
            .find(|(i, _)| *i == ToolbarItem::Search)
            .unwrap()
            .1;
        let ring = pm.pixel(field.center().x as u32, field.y as u32).unwrap();
        assert!(ring.blue() > ring.red(), "the accent ring");
    }

    /// A browser on a folder with one file, the painter and a frame of a 920 x 436 window at `scale`.
    fn search_window(scale: f32) -> (tempfile::TempDir, Browser, Vec<Place>, WindowPainter, Frame) {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("editor.txt"), b"x").unwrap();
        let mut browser = Browser::open(
            d.path().to_path_buf(),
            SortSpec::default(),
            PathBuf::from("/nonexistent/recently-used.xbel"),
            None,
            Arc::new(|| {}),
        )
        .unwrap();
        while browser.entries().is_empty() {
            browser.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let places = places();
        let mut painter = WindowPainter::new(crate::render::renderer());
        let frame = painter.frame(
            Size { w: 920.0, h: 436.0 },
            &places,
            Bars::HIDDEN,
            window::TOOLBAR_SEARCH_FIELD.0,
            d.path(),
            ViewMode::List,
            browser.outline(),
            0.0,
            scale,
        );
        (d, browser, places, painter, frame)
    }

    fn paint_search(
        painter: &mut WindowPainter,
        frame: &Frame,
        browser: &Browser,
        places: &[Place],
        search: &TextField,
        scale: f32,
    ) -> Pixmap {
        let mut pm = Pixmap::new((920.0 * scale) as u32, (436.0 * scale) as u32).unwrap();
        let state = WindowState {
            browser,
            places,
            mode: ViewMode::List,
            active: true,
            search,
            search_focused: true,
            search_caret: true,
            title: "Here",
            now: SystemTime::now(),
            edit: None,
            edit_caret: false,
            drop_target: None,
            marquee: None,
            status: "",
        };
        painter.paint(&mut pm, scale, frame, &state, &Palette::builtin(false));
        pm
    }

    #[test]
    fn the_search_selection_lies_on_the_token_and_a_press_lands_on_the_caret_drawn_there() {
        let scale = 1.5;
        let (_d, browser, places, mut painter, frame) = search_window(scale);
        let mut search = TextField::with_max_len(SEARCH_MAX_LEN);
        search.insert("notes editor");
        search.select(0..5);
        let pm = paint_search(&mut painter, &frame, &browser, &places, &search, scale);
        let (area, offsets, scroll) = painter.search_text.clone().unwrap();
        // inside the band, above the glyphs
        let y = ((area.y + area.h / 2.0 - SEARCH_TEXT_SIZE / 2.0 - 1.0) * 1.5) as u32;
        let x = ((area.x + offsets[2] - scroll) * 1.5) as u32;
        let p = pm.pixel(x, y).unwrap().demultiply();
        assert!(
            p.blue() > p.red().saturating_add(40),
            "the selected-text background: {:?}",
            (p.red(), p.green(), p.blue())
        );
        for (i, caret) in search.boundaries().into_iter().enumerate() {
            let at = Point {
                x: area.x + offsets[i] - scroll,
                y: area.y + area.h / 2.0,
            };
            assert_eq!(painter.search_index(&search, at), Some(caret), "at {caret}");
        }
    }

    #[test]
    fn a_long_paste_is_cut_to_the_search_cap_and_drawn_in_time_at_any_length() {
        let (_d, browser, places, mut painter, frame) = search_window(1.0);
        for n in [1_000, 2_000, 4_000, 10_000] {
            let mut search = TextField::with_max_len(SEARCH_MAX_LEN);
            let start = std::time::Instant::now();
            search.paste(&"word ".repeat(n / 5));
            paint_search(&mut painter, &frame, &browser, &places, &search, 1.0);
            painter.follow_search_caret(&mut search);
            let took = start.elapsed();
            eprintln!("{n} characters pasted: {took:?}");
            assert_eq!(search.text().len(), n.min(SEARCH_MAX_LEN));
            assert!(search.scroll() > 0.0, "the caret at the end stays in view");
            assert!(took < std::time::Duration::from_secs(1), "{n}: {took:?}");
        }
    }

    #[test]
    fn the_strip_keeps_its_line_and_the_list_header_takes_its_fill() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..60 {
            std::fs::write(d.path().join(format!("file {i:02}.txt")), b"x").unwrap();
        }
        let mut browser = Browser::open(
            d.path().to_path_buf(),
            SortSpec::default(),
            PathBuf::from("/nonexistent/recently-used.xbel"),
            None,
            Arc::new(|| {}),
        )
        .unwrap();
        while browser.entries().len() < 60 {
            browser.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let places = places();
        let mut painter = WindowPainter::new(crate::render::renderer());
        let pal = Palette::builtin(false);
        let size = Size { w: 920.0, h: 436.0 };
        let mut draw = |mode: ViewMode, scroll: f64, active: bool| {
            let frame = painter.frame(
                size,
                &places,
                Bars::HIDDEN,
                window::TOOLBAR_SEARCH_FIELD.0,
                d.path(),
                mode,
                browser.outline(),
                scroll,
                1.0,
            );
            let mut pm = Pixmap::new(920, 436).unwrap();
            let state = WindowState {
                browser: &browser,
                places: &places,
                mode,
                active,
                search: &TextField::new(),
                search_focused: false,
                search_caret: false,
                title: "Here",
                now: SystemTime::now(),
                edit: None,
                edit_caret: false,
                drop_target: None,
                marquee: None,
                status: "",
            };
            painter.paint(&mut pm, 1.0, &frame, &state, &pal);
            let at = |y: u32| {
                let p = pm.pixel(600, y).unwrap();
                (p.red(), p.green(), p.blue())
            };
            // the 1 pt line under the 52 pt strip, and the header 10 pt into the content
            (at(52), at(63))
        };
        let (line, header) = draw(ViewMode::List, 0.0, true);
        assert_ne!(line, (0xFF, 0xFF, 0xFF), "the line under the strip at rest");
        assert_eq!(
            line,
            draw(ViewMode::Icons, 100.0, true).0,
            "and the same line scrolled"
        );
        assert_eq!(header, (0xFF, 0xFF, 0xFF), "the header on the strip's fill");
        let (_, inactive_header) = draw(ViewMode::List, 0.0, false);
        assert_ne!(
            inactive_header,
            (0xFF, 0xFF, 0xFF),
            "an inactive strip's fill reaches its header"
        );
    }

    #[test]
    fn a_press_on_the_rename_editor_lands_on_the_caret_position_drawn_there_at_any_scale() {
        let d = tempfile::tempdir().unwrap();
        for name in ["a report.txt", "Notes 2026.md"] {
            std::fs::write(d.path().join(name), b"x").unwrap();
        }
        let mut browser = Browser::open(
            d.path().to_path_buf(),
            SortSpec::default(),
            PathBuf::from("/nonexistent/recently-used.xbel"),
            None,
            Arc::new(|| {}),
        )
        .unwrap();
        while browser.entries().len() < 2 {
            browser.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let places = places();
        let mut painter = WindowPainter::new(crate::render::renderer());
        let pal = Palette::builtin(false);
        // every caret position of both names, at fractional scales too: each used to land on a fraction of a pixel
        for scale in [1.0_f32, 1.5] {
            for mode in [ViewMode::List, ViewMode::Icons] {
                let frame = painter.frame(
                    Size { w: 600.0, h: 300.0 },
                    &places,
                    Bars::HIDDEN,
                    window::TOOLBAR_SEARCH_FIELD.0,
                    d.path(),
                    mode,
                    browser.outline(),
                    0.0,
                    scale,
                );
                let mut pm = Pixmap::new((600.0 * scale) as u32, (300.0 * scale) as u32).unwrap();
                for (index, e) in browser.entries().iter().enumerate() {
                    let mut edit = NameEdit::new(index, &e.name, false);
                    for (i, caret) in edit.field().boundaries().into_iter().enumerate() {
                        edit.field_mut().select(caret..caret);
                        let state = WindowState {
                            browser: &browser,
                            places: &places,
                            mode,
                            active: true,
                            search: &TextField::new(),
                            search_focused: false,
                            search_caret: false,
                            title: "Here",
                            now: SystemTime::now(),
                            edit: Some(&edit),
                            edit_caret: true,
                            drop_target: None,
                            marquee: None,
                            status: "",
                        };
                        painter.paint(&mut pm, scale, &frame, &state, &pal);
                        let (area, offsets) = painter.editor_text.clone().unwrap();
                        let at = Point {
                            x: area.x + offsets[i],
                            y: area.y + area.h / 2.0,
                        };
                        assert_eq!(
                            painter.editor_index(edit.field(), at),
                            Some(caret),
                            "{:?} at {caret}, scale {scale}, {mode:?}",
                            e.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_search_field_opens_with_an_ease_out_to_two_and_a_half_times_its_rest_width_and_closes_back(
    ) {
        let rest = window::TOOLBAR_SEARCH_FIELD.0;
        let open = rest * window::SEARCH_GROWTH;
        let mut field = SearchWidth::default();
        assert_eq!(field.width(0.0), rest);
        assert!(!field.animating(0.0));
        field.set_open(true, 10.0, true);
        assert_eq!(field.width(10.0), rest, "it starts at rest");
        let half = field.width(10.0 + window::SEARCH_GROW_SECONDS / 2.0);
        assert!(
            half > rest + (open - rest) / 2.0 && half < open,
            "ease-out: more than half way at half the time, got {half}"
        );
        assert!(field.animating(10.05));
        assert_eq!(field.width(10.0 + window::SEARCH_GROW_SECONDS), open);
        assert!(!field.animating(10.0 + window::SEARCH_GROW_SECONDS));
        // closing in mid-flight starts from the width shown
        field.set_open(false, 11.0, true);
        let shown = field.width(11.0);
        assert_eq!(shown, open);
        field.set_open(true, 11.1, true);
        field.set_open(false, 11.15, true);
        assert!(field.width(11.15) > rest && field.width(11.15) < open);
        assert_eq!(field.width(11.15 + window::SEARCH_GROW_SECONDS), rest);
        // reduce motion: at once
        let mut still = SearchWidth::default();
        still.set_open(true, 5.0, false);
        assert_eq!(still.width(5.0), open);
        assert!(!still.animating(5.0));
    }

    #[test]
    fn a_wider_search_field_grows_to_the_left_and_the_toolbar_never_overlaps_its_neighbours() {
        let places = places();
        let rest = window::TOOLBAR_SEARCH_FIELD.0;
        let open = rest * window::SEARCH_GROWTH;
        for width in [920.0, 700.0, 560.0, 480.0] {
            let size = Size { w: width, h: 436.0 };
            let at_rest = WindowLayout::with_search(size, &places, Bars::HIDDEN, rest);
            let wide = WindowLayout::with_search(size, &places, Bars::HIDDEN, open);
            let search = |l: &WindowLayout| {
                l.items
                    .iter()
                    .find(|(i, _)| *i == ToolbarItem::Search)
                    .map(|(_, r)| *r)
            };
            if let (Some(a), Some(b)) = (search(&at_rest), search(&wide)) {
                assert_eq!(
                    a.right(),
                    b.right(),
                    "it grows to the left from the same right edge ({width})"
                );
                assert_eq!(b.w, open);
            }
            for l in [&at_rest, &wide] {
                for pair in l.items.windows(2) {
                    assert!(
                        pair[0].1.right() <= pair[1].1.x,
                        "items do not overlap ({width})"
                    );
                }
                if let Some((_, first)) = l.items.first() {
                    assert!(
                        l.title.right() <= first.x,
                        "the title stays clear of the items ({width})"
                    );
                }
            }
        }
        assert_eq!(
            WindowLayout::new(Size { w: 920.0, h: 436.0 }, &places, Bars::HIDDEN),
            WindowLayout::with_search(Size { w: 920.0, h: 436.0 }, &places, Bars::HIDDEN, rest),
            "new is the layout at rest"
        );
    }

    #[test]
    fn a_search_without_a_match_draws_no_results_in_the_middle_of_the_content_and_a_match_does_not()
    {
        let (_d, mut browser, places, mut painter, frame) = search_window(1.0);
        let mut search = TextField::with_max_len(SEARCH_MAX_LEN);
        search.insert("zzz-nothing");
        let content = frame.window.content;
        let centre = |pm: &Pixmap| {
            let (cx, cy) = (
                content.center().x as u32,
                (content.y + content.h / 2.0) as u32,
            );
            let corner = pm.pixel(cx - 60, cy - 14).unwrap();
            (cx - 60..cx + 60)
                .flat_map(|x| (cy - 14..cy + 14).map(move |y| (x, y)))
                .any(|(x, y)| pm.pixel(x, y).unwrap() != corner)
        };
        assert!(!centre(&paint_search(
            &mut painter,
            &frame,
            &browser,
            &places,
            &search,
            1.0
        )));
        browser.set_filter("zzz-nothing");
        assert!(browser.no_results());
        assert!(
            centre(&paint_search(
                &mut painter,
                &frame,
                &browser,
                &places,
                &search,
                1.0
            )),
            "text in the middle"
        );
        browser.set_filter("editor");
        assert!(!centre(&paint_search(
            &mut painter,
            &frame,
            &browser,
            &places,
            &search,
            1.0
        )));
    }

    #[test]
    fn the_no_results_view_draws_in_time_at_any_window_size_and_scale() {
        let (_d, mut browser, places, mut painter, _frame) = search_window(1.0);
        browser.set_filter("zzz-nothing");
        assert!(browser.no_results());
        let mut search = TextField::with_max_len(SEARCH_MAX_LEN);
        search.insert("zzz-nothing");
        let mut per_pixel: Vec<(f64, f64, String)> = Vec::new();
        for (w, h, scale) in [
            (600.0, 400.0, 1.0),
            (920.0, 436.0, 1.0),
            (1920.0, 1080.0, 1.0),
            (1920.0, 1080.0, 2.0),
            (3840.0, 2160.0, 1.0),
        ] {
            let frame = painter.frame(
                Size { w, h },
                &places,
                Bars::HIDDEN,
                window::TOOLBAR_SEARCH_FIELD.0 * window::SEARCH_GROWTH,
                browser.dir(),
                ViewMode::List,
                browser.outline(),
                0.0,
                scale,
            );
            let mut pm =
                Pixmap::new((w * f64::from(scale)) as u32, (h * f64::from(scale)) as u32).unwrap();
            let state = WindowState {
                browser: &browser,
                places: &places,
                mode: ViewMode::List,
                active: true,
                search: &search,
                search_focused: true,
                search_caret: true,
                title: "Here",
                now: SystemTime::now(),
                edit: None,
                edit_caret: false,
                drop_target: None,
                marquee: None,
                status: "",
            };
            // a first frame fills the glyph and view caches; the cost of a frame is the best of the next three
            painter.paint(&mut pm, scale, &frame, &state, &Palette::builtin(false));
            let took = (0..3)
                .map(|_| {
                    let start = std::time::Instant::now();
                    painter.paint(&mut pm, scale, &frame, &state, &Palette::builtin(false));
                    start.elapsed().as_secs_f64()
                })
                .fold(f64::INFINITY, f64::min);
            let pixels = f64::from(pm.width()) * f64::from(pm.height());
            eprintln!("{w} x {h} at {scale}x: {took:.3} s");
            per_pixel.push((pixels, took / pixels, format!("{w} x {h} at {scale}x")));
        }
        // the cost grows no faster than the pixels drawn: no size takes much more per pixel than a smaller one (a
        // small frame costs more per pixel, its fixed work spread over fewer pixels)
        for (pixels, cost, what) in &per_pixel {
            for (smaller, smaller_cost, than) in per_pixel.iter().filter(|(p, _, _)| p <= pixels) {
                assert!(
                    *cost < smaller_cost * 2.0,
                    "{what}: {cost:e} s a pixel against {smaller_cost:e} for {than} ({smaller} pixels)"
                );
            }
        }
    }
}
