//! The files scope: the launcher's view of recent files and the files a query finds, in the same panel as the search
//! field. Under the field (which filters the files) sit a row of filters and a scrolled grid: first a row of the
//! most recently changed files, a separator, then a heading over the files themselves, each a thumbnail over a
//! name of up to two lines.
//!
//! This module is the scope's model: what it holds, its geometry inside the panel, its keyboard and scrolling and
//! what a click asks for. It is pure. The files come from the file index through a [`ScopeRequest`], which a host
//! runs off the loop, so the panel shows its grid without waiting and without saying it is searching. The panel's
//! transitions are [`crate::motion`]'s, Escape is [`crate::nav`]'s and the Kit nodes are `view::scope`.
//!
//! Selection is kept by index in the files shown; a new answer starts it over. Coordinates are points from the
//! panel's top-left corner.

use crate::browse::Key;
use crate::engine::LauncherConfig;
use crate::files::{FileEntry, FileIndex};
use crate::matcher::Query;
use cupertino_kit::geometry::{Point, Rect};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Columns of the grid, the suggestions row included.
pub const COLUMNS: usize = 5;
/// The most files the scope shows in its grid, and the most it suggests.
pub const MAX_ITEMS: usize = 80;
pub const MAX_SUGGESTIONS: usize = COLUMNS;
/// A file counts as recent when it changed this many days ago or less.
pub const RECENT_DAYS: f64 = 14.0;
/// What the scope says over its grid: with nothing typed and no filter on, with the query, and under each filter.
pub const HEADING_ALL: &str = "Files";
pub const HEADING_RESULTS: &str = "Results";
/// What assistive technology calls the scope, the suggestions' group and what shows when no file is left.
pub const TITLE: &str = "Files";
pub const SUGGESTED: &str = "Suggested";
pub const NO_RESULTS: &str = "No Results";

/// The filters, in the order they show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Files that changed within [`RECENT_DAYS`].
    Recent,
    /// Text and office documents.
    Documents,
    Images,
}

const DOCUMENTS: &[&str] = &[
    "pdf", "doc", "docx", "odt", "ods", "odp", "rtf", "txt", "md", "xls", "xlsx", "ppt", "pptx",
    "csv", "epub", "tex",
];
const IMAGES: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tif", "tiff", "heic", "avif", "ico",
];

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::Recent, Filter::Documents, Filter::Images];

    /// What the filter says.
    pub fn name(self) -> &'static str {
        match self {
            Filter::Recent => "Recent",
            Filter::Documents => "Documents",
            Filter::Images => "Images",
        }
    }

    /// What the grid's heading says while the filter is on.
    pub fn heading(self) -> &'static str {
        match self {
            Filter::Recent => "Changed Recently",
            Filter::Documents => "Documents",
            Filter::Images => "Images",
        }
    }

    /// Whether `entry` passes the filter, `now` Unix seconds.
    pub fn admits(self, entry: &FileEntry, now: f64) -> bool {
        let extension = |list: &[&str]| {
            entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| list.iter().any(|l| e.eq_ignore_ascii_case(l)))
        };
        match self {
            Filter::Recent => {
                entry.modified > 0.0 && now - entry.modified <= RECENT_DAYS * 86_400.0
            }
            Filter::Documents => !entry.is_dir && extension(DOCUMENTS),
            Filter::Images => !entry.is_dir && extension(IMAGES),
        }
    }
}

/// One file or folder of the scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileItem {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

impl FileItem {
    fn of(entry: &FileEntry) -> FileItem {
        FileItem {
            path: entry.path.clone(),
            name: entry.name.clone(),
            is_dir: entry.is_dir,
        }
    }
}

/// A search of the file index for the scope, for the host to run off the loop: answered as a [`ScopeAnswer`].
#[derive(Clone, Debug)]
pub struct ScopeRequest {
    /// The request's number; an answer for an older one is dropped.
    pub generation: u64,
    pub text: String,
    pub filter: Option<Filter>,
    pub index: Arc<FileIndex>,
    pub config: LauncherConfig,
    pub unix: f64,
}

/// What a [`ScopeRequest`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopeAnswer {
    pub generation: u64,
    /// The most recently changed files, when nothing is typed and no filter is on.
    pub suggestions: Vec<FileItem>,
    /// The files below them: by recency with nothing typed, by match with a query.
    pub items: Vec<FileItem>,
}

impl ScopeRequest {
    /// Run the search here and now: what a host does on its worker.
    pub fn run(&self) -> ScopeAnswer {
        let query = Query::new(&self.text);
        let entries = self.index.entries();
        let admitted = |e: &FileEntry| self.filter.is_none_or(|f| f.admits(e, self.unix));
        if !query.is_empty() {
            let found = self
                .index
                .search(&query, MAX_ITEMS * 4, self.unix, &self.config.weights);
            let items = found
                .into_iter()
                .map(|m| self.index.entry(m.index))
                .filter(|e| admitted(e))
                .take(MAX_ITEMS)
                .map(FileItem::of)
                .collect();
            return ScopeAnswer {
                generation: self.generation,
                suggestions: Vec::new(),
                items,
            };
        }
        let mut files: Vec<&FileEntry> = entries
            .iter()
            .filter(|e| !e.is_dir && admitted(e))
            .collect();
        let wanted = MAX_SUGGESTIONS + MAX_ITEMS;
        let newest_first = |a: &&FileEntry, b: &&FileEntry| {
            b.modified
                .total_cmp(&a.modified)
                .then_with(|| a.name.cmp(&b.name))
        };
        if files.len() > wanted {
            files.select_nth_unstable_by(wanted, newest_first);
            files.truncate(wanted);
        }
        files.sort_unstable_by(newest_first);
        let split = if self.filter.is_none() {
            MAX_SUGGESTIONS.min(files.len())
        } else {
            0
        };
        let items = files.split_off(split);
        ScopeAnswer {
            generation: self.generation,
            suggestions: files.into_iter().map(FileItem::of).collect(),
            items: items.into_iter().map(FileItem::of).collect(),
        }
    }
}

/// The scope's geometry inside a panel `height` points tall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeLayout {
    /// The panel, from its top-left corner.
    pub panel: Rect,
    /// The row the filters share.
    pub filters: Rect,
    /// The scrolled area.
    pub content: Rect,
}

impl ScopeLayout {
    /// The panel's size, its corner radius, and the least height it shrinks to on a short output.
    pub const WIDTH: f64 = 640.0;
    pub const HEIGHT: f64 = 547.0;
    pub const MIN_HEIGHT: f64 = 300.0;
    pub const RADIUS: f64 = 28.5;
    /// The side inset of the filter row and the separators.
    pub const INSET: f64 = 20.0;
    /// The header row (the query field's).
    pub const HEADER: f64 = 56.0;
    /// The filters: the row's top, their height, corner radius, text size and weight, and the gap between them. They
    /// share the row's width equally.
    pub const FILTERS_TOP: f64 = 67.0;
    pub const FILTER_HEIGHT: f64 = 25.0;
    pub const FILTER_RADIUS: f64 = 8.5;
    pub const FILTER_FONT_PT: f64 = 15.0;
    pub const FILTER_WEIGHT: u16 = 400;
    pub const FILTER_GAP: f64 = 11.0;
    pub const FILTERS_INSET: f64 = 21.0;
    /// The scrolled area's top, under the separator below the filters.
    pub const CONTENT_TOP: f64 = 104.0;
    pub const SEPARATOR_Y: f64 = 103.0;
    /// Room under the scrolled content's last row.
    pub const CONTENT_BOTTOM: f64 = Self::INSET;
    /// The grid: where the first column begins, the columns' pitch and a row's height.
    pub const COLUMNS_X: f64 = 20.0;
    pub const COLUMN: f64 = 120.0;
    pub const ROW_HEIGHT: f64 = 126.0;
    /// In a cell: the thumbnail's box and its top, the label's top, its size, weight and line pitch, and the
    /// cell's inset from its sides.
    pub const THUMB: f64 = 67.0;
    pub const THUMB_TOP: f64 = 8.0;
    pub const LABEL_TOP: f64 = 84.0;
    pub const LABEL_FONT_PT: f64 = 14.0;
    pub const LABEL_WEIGHT: u16 = 400;
    pub const LABEL_LINE: f64 = 17.5;
    pub const LABEL_INSET: f64 = 6.0;
    /// The selection's inset from its cell, and its corner radius.
    pub const SELECTION_INSET: (f64, f64) = (4.0, 2.0);
    pub const SELECTION_RADIUS: f64 = 12.0;
    /// The suggestions row's separator, below the row, and the heading's block over the grid.
    pub const SEPARATOR_GAP: f64 = 16.0;
    pub const HEADING_TOP: f64 = 6.0;
    pub const HEADING_BLOCK: f64 = 32.0;
    pub const HEADING_FONT_PT: f64 = 13.0;
    /// The scroll indicator: its width, its inset from the panel's right edge and from the scrolled area's ends,
    /// and the least length of its thumb.
    pub const SCROLLER_WIDTH: f64 = 6.0;
    pub const SCROLLER_INSET: f64 = 3.0;
    pub const SCROLLER_END_INSET: f64 = 8.0;
    pub const SCROLLER_MIN: f64 = 32.0;

    /// The layout of a panel `height` points tall (at most [`Self::HEIGHT`]).
    pub fn new(height: f64) -> Self {
        let (w, h) = (Self::WIDTH, height.clamp(Self::MIN_HEIGHT, Self::HEIGHT));
        Self {
            panel: Rect::new(0.0, 0.0, w, h),
            filters: Rect::new(
                Self::FILTERS_INSET,
                Self::FILTERS_TOP,
                w - 2.0 * Self::FILTERS_INSET,
                Self::FILTER_HEIGHT,
            ),
            content: Rect::new(0.0, Self::CONTENT_TOP, w, h - Self::CONTENT_TOP),
        }
    }

    /// The panel's height on an output `output_height` points tall whose top `top` points the panel starts at: as
    /// tall as it is designed where the output allows, keeping `bottom` points free below it, on the panel's shape
    /// grid.
    pub fn height_on(output_height: f64, top: f64, bottom: f64) -> f64 {
        let grid = crate::view::SHAPE_GRID;
        let room = ((output_height - top - bottom) / grid).floor() * grid;
        room.clamp(Self::MIN_HEIGHT, Self::HEIGHT)
    }

    /// The separator under the header.
    pub fn header_separator(&self) -> Rect {
        Rect::new(
            Self::INSET,
            Self::HEADER,
            self.panel.width() - 2.0 * Self::INSET,
            1.0,
        )
    }

    /// The separator under the filters.
    pub fn filters_separator(&self) -> Rect {
        Rect::new(
            Self::INSET,
            Self::SEPARATOR_Y,
            self.panel.width() - 2.0 * Self::INSET,
            1.0,
        )
    }

    /// The filters' rectangles: equally wide across the row.
    pub fn filter_rects(&self, count: usize) -> Vec<Rect> {
        if count == 0 {
            return Vec::new();
        }
        let gaps = Self::FILTER_GAP * (count - 1) as f64;
        let width = (self.filters.width() - gaps) / count as f64;
        (0..count)
            .map(|i| {
                Rect::new(
                    self.filters.x() + i as f64 * (width + Self::FILTER_GAP),
                    self.filters.y(),
                    width,
                    self.filters.height(),
                )
            })
            .collect()
    }

    /// The selection's and a press's area in `cell`.
    pub fn item_rect(cell: &Rect) -> Rect {
        let (ix, iy) = Self::SELECTION_INSET;
        Rect::new(
            cell.x() + ix,
            cell.y() + iy,
            (cell.width() - 2.0 * ix).max(0.0),
            (cell.height() - 2.0 * iy).max(0.0),
        )
    }

    /// The thumbnail's box in a `cell`, centred along it.
    pub fn thumb(cell: &Rect) -> Rect {
        Rect::new(
            cell.x() + ((cell.width() - Self::THUMB) / 2.0).floor(),
            cell.y() + Self::THUMB_TOP,
            Self::THUMB,
            Self::THUMB,
        )
    }

    /// The scroll indicator's track along the panel's right edge.
    pub fn scroller_track(&self) -> Rect {
        Rect::new(
            self.panel.width() - Self::SCROLLER_INSET - Self::SCROLLER_WIDTH,
            self.content.y() + Self::SCROLLER_END_INSET,
            Self::SCROLLER_WIDTH,
            (self.content.height() - 2.0 * Self::SCROLLER_END_INSET).max(0.0),
        )
    }
}

/// What a key or a click asks the launcher to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Open this file or folder; the launcher closes.
    Open(PathBuf),
}

/// Where the keys go besides the query field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zone {
    /// The files.
    #[default]
    Content,
    /// The filters.
    Filters,
}

/// One file placed in the scrolled content.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// Index into [`FilesScope::item`].
    pub index: usize,
    /// Its cell in the panel, scrolled.
    pub cell: Rect,
}

/// The files scope: the suggestions, the files, the query and the filter, the selection and the scroll.
#[derive(Clone, Debug, PartialEq)]
pub struct FilesScope {
    suggestions: Vec<FileItem>,
    items: Vec<FileItem>,
    query: String,
    filter: Option<Filter>,
    zone: Zone,
    /// Index into [`Filter::ALL`] of the filter the keys are on.
    filter_focus: usize,
    /// The selected file, an index over the suggestions then the files.
    selected: Option<usize>,
    scroll: f64,
    layout: ScopeLayout,
    /// When the content last scrolled, for the scroll indicator.
    scrolled_at: Option<f64>,
    /// The time of the input being handled.
    clock: f64,
    /// Whether an answer has arrived since the query or the filter last changed.
    answered: bool,
}

impl FilesScope {
    /// The scope in a panel `height` points tall, with nothing found yet.
    pub fn new(height: f64) -> Self {
        Self {
            suggestions: Vec::new(),
            items: Vec::new(),
            query: String::new(),
            filter: None,
            zone: Zone::Content,
            filter_focus: 0,
            selected: None,
            scroll: 0.0,
            layout: ScopeLayout::new(height),
            scrolled_at: None,
            clock: 0.0,
            answered: false,
        }
    }

    /// The panel is now `height` points tall.
    pub fn set_height(&mut self, height: f64) {
        self.layout = ScopeLayout::new(height);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    pub fn layout(&self) -> &ScopeLayout {
        &self.layout
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn filter(&self) -> Option<Filter> {
        self.filter
    }

    pub fn zone(&self) -> Zone {
        self.zone
    }

    /// The filter the keys are on, while they are on the filters.
    pub fn filter_focus(&self) -> Option<usize> {
        (self.zone == Zone::Filters).then_some(self.filter_focus)
    }

    /// What the grid's heading says now.
    pub fn heading(&self) -> &'static str {
        if !self.query.is_empty() {
            HEADING_RESULTS
        } else {
            self.filter.map_or(HEADING_ALL, Filter::heading)
        }
    }

    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Whether an answer has arrived for the query and filter shown.
    pub fn answered(&self) -> bool {
        self.answered
    }

    /// How many files are suggested at the top.
    pub fn suggested(&self) -> usize {
        self.suggestions.len()
    }

    pub fn item_count(&self) -> usize {
        self.suggestions.len() + self.items.len()
    }

    /// The file at `index` over the suggestions then the grid.
    pub fn item(&self, index: usize) -> Option<&FileItem> {
        match index.checked_sub(self.suggestions.len()) {
            None => self.suggestions.get(index),
            Some(i) => self.items.get(i),
        }
    }

    pub fn items(&self) -> impl Iterator<Item = &FileItem> + '_ {
        self.suggestions.iter().chain(&self.items)
    }

    /// The selected file's index.
    pub fn selected(&self) -> Option<usize> {
        self.selected.filter(|&i| i < self.item_count())
    }

    pub fn selected_item(&self) -> Option<&FileItem> {
        self.selected().and_then(|i| self.item(i))
    }

    /// Take what a request found, unless the query or the filter changed since. The selection starts over.
    pub fn answer(&mut self, answer: ScopeAnswer) {
        self.suggestions = answer.suggestions;
        self.items = answer.items;
        self.answered = true;
        self.selected = None;
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// Rows the suggestions take: one when there are any.
    fn suggestion_rows(&self) -> usize {
        usize::from(!self.suggestions.is_empty())
    }

    /// Where the heading's block begins in the scrolled content.
    fn heading_top(&self) -> f64 {
        if self.suggestions.is_empty() {
            ScopeLayout::HEADING_TOP
        } else {
            ScopeLayout::ROW_HEIGHT + ScopeLayout::SEPARATOR_GAP + 1.0
        }
    }

    /// Where the grid begins in the scrolled content.
    fn grid_top(&self) -> f64 {
        self.heading_top() + ScopeLayout::HEADING_BLOCK
    }

    /// The suggestions' separator line in the scrolled content, while there are suggestions.
    pub fn separator_y(&self) -> Option<f64> {
        (!self.suggestions.is_empty())
            .then_some(ScopeLayout::ROW_HEIGHT + ScopeLayout::SEPARATOR_GAP)
    }

    /// The heading's rectangle in the scrolled content, scrolled.
    pub fn heading_rect(&self) -> Rect {
        Rect::new(
            ScopeLayout::INSET + 1.0,
            self.layout.content.y() + self.heading_top() - self.scroll,
            self.layout.panel.width() - 2.0 * ScopeLayout::INSET,
            ScopeLayout::HEADING_BLOCK,
        )
    }

    /// How tall the scrolled content is, with the room under its last row.
    pub fn content_height(&self) -> f64 {
        let rows = self.items.len().div_ceil(COLUMNS) as f64;
        self.grid_top() + rows * ScopeLayout::ROW_HEIGHT + ScopeLayout::CONTENT_BOTTOM
    }

    pub fn max_scroll(&self) -> f64 {
        (self.content_height() - self.layout.content.height()).max(0.0)
    }

    /// The row (suggestions first) and column of item `index`.
    fn position(&self, index: usize) -> (usize, usize) {
        let s = self.suggestions.len();
        if index < s {
            (0, index % COLUMNS)
        } else {
            let j = index - s;
            (self.suggestion_rows() + j / COLUMNS, j % COLUMNS)
        }
    }

    /// Where the row of item `index` starts in the scrolled content.
    fn row_y(&self, index: usize) -> f64 {
        let s = self.suggestions.len();
        if index < s {
            0.0
        } else {
            self.grid_top() + ((index - s) / COLUMNS) as f64 * ScopeLayout::ROW_HEIGHT
        }
    }

    /// The first item of `row` and how many it holds.
    fn row(&self, row: usize) -> Option<(usize, usize)> {
        let s = self.suggestions.len();
        let sr = self.suggestion_rows();
        if row < sr {
            return Some((0, s));
        }
        let first = (row - sr) * COLUMNS;
        (first < self.items.len()).then(|| (s + first, (self.items.len() - first).min(COLUMNS)))
    }

    fn last_row(&self) -> Option<usize> {
        let count = self.item_count();
        (count > 0).then(|| self.position(count - 1).0)
    }

    fn cell(&self, col: usize, row_y: f64) -> Rect {
        Rect::new(
            ScopeLayout::COLUMNS_X + col as f64 * ScopeLayout::COLUMN,
            self.layout.content.y() + row_y - self.scroll,
            ScopeLayout::COLUMN,
            ScopeLayout::ROW_HEIGHT,
        )
    }

    /// The cell of item `index` in the panel, scrolled.
    pub fn cell_of(&self, index: usize) -> Option<Rect> {
        (index < self.item_count()).then(|| self.cell(self.position(index).1, self.row_y(index)))
    }

    /// The files whose rows the scrolled area shows, even in part, with their cells.
    pub fn visible(&self) -> Vec<Placed> {
        let (top, bottom) = (self.scroll, self.scroll + self.layout.content.height());
        (0..self.item_count())
            .filter_map(|index| {
                let y = self.row_y(index);
                (y + ScopeLayout::ROW_HEIGHT > top && y < bottom).then(|| Placed {
                    index,
                    cell: self.cell(self.position(index).1, y),
                })
            })
            .collect()
    }

    /// The files within a screenful of the shown rows either way: the ones whose thumbnails are worth loading.
    pub fn nearby(&self) -> Vec<usize> {
        let h = self.layout.content.height();
        let (top, bottom) = (self.scroll - h, self.scroll + 2.0 * h);
        (0..self.item_count())
            .filter(|&index| {
                let y = self.row_y(index);
                y + ScopeLayout::ROW_HEIGHT > top && y < bottom
            })
            .collect()
    }

    /// The scroll indicator's thumb, while the content does not fit.
    pub fn scroller_thumb(&self) -> Option<Rect> {
        let max = self.max_scroll();
        if max <= 0.0 {
            return None;
        }
        let track = self.layout.scroller_track();
        let len = (track.height() * self.layout.content.height() / self.content_height())
            .max(ScopeLayout::SCROLLER_MIN)
            .min(track.height());
        let y = track.y() + (track.height() - len) * (self.scroll / max);
        Some(Rect::new(track.x(), y, track.width(), len))
    }

    /// How much the scroll indicator shows at `now`: whole while the content scrolls and for a moment after.
    pub fn scroller_alpha(&self, now: f64) -> f64 {
        self.scrolled_at.map_or(0.0, |at| {
            (1.0 - (now - at - crate::browse::SCROLLER_LINGER) / crate::browse::SCROLLER_FADE)
                .clamp(0.0, 1.0)
        })
    }

    /// Whether the scroll indicator is still fading at `now`.
    pub fn animating(&self, now: f64) -> bool {
        self.scrolled_at.is_some_and(|at| {
            now - at < crate::browse::SCROLLER_LINGER + crate::browse::SCROLLER_FADE
        })
    }

    /// The filters with their rectangles.
    pub fn filter_rects(&self) -> Vec<(Filter, Rect)> {
        Filter::ALL
            .into_iter()
            .zip(self.layout.filter_rects(Filter::ALL.len()))
            .collect()
    }

    fn set_scroll(&mut self, scroll: f64) {
        let before = self.scroll;
        self.scroll = scroll.clamp(0.0, self.max_scroll());
        if self.scroll != before {
            self.scrolled_at = Some(self.clock);
        }
    }

    /// Scroll by `dy` points at `now`, within the content.
    pub fn scroll_by(&mut self, dy: f64, now: f64) {
        if !dy.is_finite() {
            return;
        }
        self.clock = now;
        self.set_scroll(self.scroll + dy);
    }

    /// Scroll so the row of item `index` shows whole, above the room kept at the scrolled area's bottom.
    fn reveal(&mut self, index: usize) {
        let y = self.row_y(index);
        let h = ScopeLayout::ROW_HEIGHT;
        let view = self.layout.content.height() - ScopeLayout::CONTENT_BOTTOM;
        let scroll = if y < self.scroll {
            y
        } else if y + h > self.scroll + view {
            y + h - view
        } else {
            self.scroll
        };
        self.set_scroll(scroll);
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        self.reveal(index);
    }

    /// The query typed into the field: the files shown stay until the answer for it arrives. Returns whether it
    /// changed.
    pub fn set_query(&mut self, query: &str) -> bool {
        if query == self.query {
            return false;
        }
        self.query = query.to_owned();
        self.restart();
        true
    }

    /// Turn `filter` on, or off when it is on. Returns whether the filter changed.
    pub fn toggle_filter(&mut self, filter: Filter) -> bool {
        self.set_filter(if self.filter == Some(filter) {
            None
        } else {
            Some(filter)
        })
    }

    pub fn set_filter(&mut self, filter: Option<Filter>) -> bool {
        if let Some(i) = filter.and_then(|f| Filter::ALL.iter().position(|&x| x == f)) {
            self.filter_focus = i;
        }
        if filter == self.filter {
            return false;
        }
        self.filter = filter;
        self.restart();
        true
    }

    /// The query or the filter changed: the scroll and the selection start over.
    fn restart(&mut self) {
        self.zone = Zone::Content;
        self.selected = None;
        self.scroll = 0.0;
        self.answered = false;
    }

    /// The keys go to the files or the filters.
    pub fn set_zone(&mut self, zone: Zone) {
        self.zone = zone;
        if zone == Zone::Filters {
            self.filter_focus = self
                .filter
                .and_then(|f| Filter::ALL.iter().position(|&x| x == f))
                .unwrap_or(self.filter_focus);
        } else if self.selected().is_none() && self.item_count() > 0 {
            self.select(0);
        }
    }

    /// Nothing is selected: the keys are back in the query field.
    pub fn deselect(&mut self) {
        self.selected = None;
        self.zone = Zone::Content;
    }

    /// A key at `now`. Returns what the launcher has to do, if anything.
    pub fn key(&mut self, key: Key, now: f64) -> Option<Outcome> {
        self.clock = now;
        if matches!(key, Key::Tab | Key::BackTab) {
            let next = match self.zone {
                Zone::Content => Zone::Filters,
                Zone::Filters => Zone::Content,
            };
            self.set_zone(next);
            return None;
        }
        match self.zone {
            Zone::Filters => self.filter_key(key),
            Zone::Content => self.content_key(key),
        }
    }

    fn filter_key(&mut self, key: Key) -> Option<Outcome> {
        let last = Filter::ALL.len() - 1;
        match key {
            Key::Left => self.filter_focus = self.filter_focus.saturating_sub(1),
            Key::Right => self.filter_focus = (self.filter_focus + 1).min(last),
            Key::Home => self.filter_focus = 0,
            Key::End => self.filter_focus = last,
            Key::Return | Key::Space => {
                let focus = self.filter_focus;
                self.toggle_filter(Filter::ALL[focus]);
                self.zone = Zone::Filters;
            }
            Key::Down => self.set_zone(Zone::Content),
            _ => {}
        }
        None
    }

    fn content_key(&mut self, key: Key) -> Option<Outcome> {
        if key == Key::Return {
            let item = self.selected_item().or_else(|| self.item(0));
            return item.map(|f| Outcome::Open(f.path.clone()));
        }
        let count = self.item_count();
        let page_rows =
            ((self.layout.content.height() / ScopeLayout::ROW_HEIGHT).floor() as usize).max(1);
        let Some(current) = self.selected() else {
            match key {
                Key::Left | Key::Right | Key::Up | Key::Down | Key::Home if count > 0 => {
                    self.select(0)
                }
                Key::End if count > 0 => self.select(count - 1),
                Key::PageDown => self.scroll_by(self.layout.content.height(), self.clock),
                Key::PageUp => self.scroll_by(-self.layout.content.height(), self.clock),
                _ => {}
            }
            return None;
        };
        let (row, col) = self.position(current);
        let in_row = |s: &Self, row: usize| {
            s.row(row)
                .map(|(first, len)| first + col.min(len.saturating_sub(1)))
        };
        let next = match key {
            Key::Right => (current + 1).min(count - 1),
            Key::Left => current.saturating_sub(1),
            Key::Down => in_row(self, row + 1).unwrap_or(current),
            Key::Up if row == 0 => {
                // above the first row are the filters
                self.set_zone(Zone::Filters);
                current
            }
            Key::Up => in_row(self, row - 1).unwrap_or(current),
            Key::PageDown => in_row(self, row + page_rows)
                .or_else(|| self.last_row().and_then(|r| in_row(self, r)))
                .unwrap_or(current),
            Key::PageUp => in_row(self, row.saturating_sub(page_rows)).unwrap_or(current),
            Key::Home => 0,
            Key::End => count - 1,
            _ => current,
        };
        if self.zone == Zone::Content {
            self.select(next);
        }
        None
    }

    /// The pointer moved onto item `index`: it is selected (without scrolling).
    pub fn hover(&mut self, index: usize) {
        if index < self.item_count() {
            self.selected = Some(index);
            self.zone = Zone::Content;
        }
    }

    /// A press on item `index` opens it.
    pub fn activate(&self, index: usize) -> Option<Outcome> {
        self.item(index).map(|f| Outcome::Open(f.path.clone()))
    }

    /// The file whose cell holds `p`, if any.
    pub fn item_at(&self, p: Point) -> Option<usize> {
        self.visible()
            .into_iter()
            .find(|placed| ScopeLayout::item_rect(&placed.cell).contains(p))
            .map(|placed| placed.index)
    }

    /// The file at `path`, when shown.
    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.items().position(|f| f.path == path)
    }
}
