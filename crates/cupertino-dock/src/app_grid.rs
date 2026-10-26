//! The application grid: every installed application on pages of icons with a search field and
//! page dots, opened by the app-grid action the server forwards (`toggle_app_grid`). The dock owns
//! it whole: this module holds the geometry, the
//! show and hide animation, paging by keys, wheel and dots, the search
//! rule and what a click or key asks for. Drawing is in `render::paint_app_grid`.
//!
//! Coordinates are top-down points in the grid's layer: the output below its 30 pt top band
//! ([`TOP_BAND`]) and above the dock bar, whose exclusive zone stands for the
//! reference's dock extent `d`.

use cupertino_animation::CubicBezier;
use cupertino_wm::geometry::{Point, Rect};

use crate::desktop::DesktopIndex;

/// Columns and rows of a page (the design's column and row preferences, 7 and 5).
pub const COLUMNS: usize = 7;
pub const ROWS: usize = 5;
pub const PER_PAGE: usize = COLUMNS * ROWS;

/// Show and hide durations: the layer framework's transaction default (0.25 s). The grid enters from opacity 0 at scale 1.05 with ease-in and leaves the reverse way
/// with ease-out.
pub const SHOW_DURATION: f64 = 0.25;
pub const HIDE_DURATION: f64 = 0.25;
pub const ENTER_SCALE: f64 = 1.05;

/// A page flip takes 0.5 s on the curve (0, 0) (0.35841, 1.0019).
pub const PAGE_DURATION: f64 = 0.5;
pub const PAGE_CURVE: CubicBezier = CubicBezier::new(0.0, 0.0, 0.35841, 1.0019);

/// The wheel accumulates twice the dominant-axis delta and flips one page 75 ms after the last
/// event when it moved. A positive total (down or right) flips forward
/// (the design negates the delta under its default reverse preference).
pub const WHEEL_GAIN: f64 = 2.0;
pub const WHEEL_SETTLE: f64 = 0.075;

/// The grid's layer starts this far below the output top, over the menu bar's strip.
pub const TOP_BAND: f64 = 30.0;

/// Material of the background: the grid's own recipe in the server's design tokens.
pub const MATERIAL: &str = "AppGrid";

/// Wayland layer-shell namespace of the grid's surface.
pub const NAMESPACE: &str = "cupertino-dock-app-grid";

/// One application of the grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    /// Desktop file ID: what [`crate::session::Session::launch`] starts.
    pub key: String,
    pub name: String,
    /// Icon theme name (the entry's `Icon`, else the ID).
    pub icon: String,
}

/// Geometry of the grid on a layer of `width × height` points (the design's layer, grid and
/// item layout routines). The dock extent is not subtracted: the layer already excludes the dock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLayout {
    pub width: f64,
    pub height: f64,
    /// Adjusted side margins (zoom style 0 widens the sides only) and the top and bottom margins.
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    /// Frame of page 0; page `i` is `i × width` further right.
    pub page: Rect,
    /// Cell width and height (`trunc(page / columns | rows)`).
    pub cell: (f64, f64),
    /// Icon side (`min(cell) − 46` through the icon rule).
    pub icon: f64,
    /// Label rectangle bottom, up from the cell bottom.
    pub label_y: f64,
    pub search: Rect,
    /// The page-dot strip, full width, 15 pt tall.
    pub dots: Rect,
}

impl GridLayout {
    /// Label layer height: line 15 + 4 + (blur 2 − offset −1) (formula, line).
    pub const LABEL_HEIGHT: f64 = 22.0;
    /// The label is 13 pt medium with monospaced numbers.
    pub const LABEL_FONT_PT: f64 = 13.0;
    pub const LABEL_WEIGHT: u16 = 500;
    /// Label width: text plus 4, at most the cell minus 12.
    pub const LABEL_PAD: f64 = 4.0;
    pub const LABEL_CELL_INSET: f64 = 12.0;
    /// Search field: 250 × 25, light 16 pt.
    pub const SEARCH_SIZE: (f64, f64) = (250.0, 25.0);
    pub const SEARCH_FONT_PT: f64 = 16.0;
    pub const SEARCH_WEIGHT: u16 = 300;
    /// "No Results": light 40 pt.
    pub const NO_RESULTS_FONT_PT: f64 = 40.0;
    /// Dots: 15 × 15 hit frames at pitch 20; the dot is 8 × 8 at (3, 3) with radius 4.
    pub const DOT_HIT: f64 = 15.0;
    pub const DOT_PITCH: f64 = 20.0;
    pub const DOT_INSET: f64 = 3.0;
    pub const DOT_SIZE: f64 = 8.0;
    /// The keyboard selection: the union of image and label, 5 pt taller at each end (the
    /// reference's "height + 10", table; its inset is not known, approximate 0).
    pub const SELECTION_OUTSET_Y: f64 = 5.0;

    pub fn new(width: f64, height: f64) -> Self {
        let cols = COLUMNS as f64;
        let rows = ROWS as f64;
        let m = (width / (2.0 * cols + 1.0)).floor();
        let spare = (height - rows * (m + 40.0)).floor();
        let side = (m * 0.5).floor().max(90.0);
        let top = (spare * 0.15).floor().max(35.0);
        let bottom = (spare * 0.25).floor().max(35.0);
        let zoom = zoom_factor(width);
        let adjusted_side = (width * 0.2 * zoom + side).floor();
        let page = Rect::new(
            adjusted_side,
            30.0 + top,
            width - 2.0 * adjusted_side,
            (height - 30.0) - (top + bottom),
        );
        let cell = ((page.w / cols).trunc(), (page.h / rows).trunc());
        let icon = icon_size(cell.0.min(cell.1) - 46.0);
        let label_y = (cell.1 * 0.125).ceil().min(20.0);
        // bottom-up (W/2 − 125, layerH − (top + 15), 250, 25): the field's bottom is 15 pt above the page
        let search = Rect::new(
            width / 2.0 - Self::SEARCH_SIZE.0 / 2.0,
            top + 15.0 - Self::SEARCH_SIZE.1,
            Self::SEARCH_SIZE.0,
            Self::SEARCH_SIZE.1,
        );
        let dots_from_bottom = (bottom * 0.5 - 7.5).floor().max(0.0);
        let dots = Rect::new(
            0.0,
            height - Self::DOT_HIT - dots_from_bottom,
            width,
            Self::DOT_HIT,
        );
        Self {
            width,
            height,
            left: adjusted_side,
            right: adjusted_side,
            top,
            bottom,
            page,
            cell,
            icon,
            label_y,
            search,
            dots,
        }
    }

    /// Frame of page `i` when the pages stand at `position` (page 0 at 0, fractional while flipping).
    pub fn page_frame(&self, i: usize, position: f64) -> Rect {
        Rect::new(
            self.page.x + (i as f64 - position) * self.width,
            self.page.y,
            self.page.w,
            self.page.h,
        )
    }

    /// Cell of `slot` (0-based, row-major) in a page frame.
    pub fn cell_rect(&self, frame: &Rect, slot: usize) -> Rect {
        let (c, r) = ((slot % COLUMNS) as f64, (slot / COLUMNS) as f64);
        Rect::new(
            frame.x + c * self.cell.0,
            frame.y + r * self.cell.1,
            self.cell.0,
            self.cell.1,
        )
    }

    /// Image frame inside `cell`: `(trunc(cellW/2 − icon/2), labelY + Lh, icon, icon)` bottom-up.
    pub fn image_rect(&self, cell: &Rect) -> Rect {
        let bottom = cell.bottom() - self.label_y - Self::LABEL_HEIGHT;
        Rect::new(
            cell.x + (self.cell.0 / 2.0 - self.icon / 2.0).trunc(),
            bottom - self.icon,
            self.icon,
            self.icon,
        )
    }

    /// Label rectangle inside `cell` for a text `text_width` wide: `min(text + 4, cellW − 12)`
    /// wide, centred (truncated when the width is even, rounded when odd), `label_y` above the cell bottom.
    pub fn label_rect(&self, cell: &Rect, text_width: f64) -> Rect {
        let w = (text_width + Self::LABEL_PAD).min(self.cell.0 - Self::LABEL_CELL_INSET);
        let centre = self.cell.0 / 2.0 - w / 2.0;
        let x = if (w as i64) % 2 == 0 {
            centre.trunc()
        } else {
            centre.round()
        };
        Rect::new(
            cell.x + x,
            cell.bottom() - self.label_y - Self::LABEL_HEIGHT,
            w,
            Self::LABEL_HEIGHT,
        )
    }

    /// What a click or the selection highlight covers: the image and the widest label, 5 pt taller
    /// at each end.
    pub fn item_rect(&self, cell: &Rect) -> Rect {
        let image = self.image_rect(cell);
        let label = self.label_rect(cell, self.cell.0);
        let union = image.union(&label);
        Rect::new(
            union.x,
            union.y - Self::SELECTION_OUTSET_Y,
            union.w,
            union.h + 2.0 * Self::SELECTION_OUTSET_Y,
        )
    }

    /// The slot whose item `p` is over on a page at `frame`.
    pub fn slot_at(&self, frame: &Rect, p: Point, filled: usize) -> Option<usize> {
        if !frame.contains(p) {
            return None;
        }
        let c = ((p.x - frame.x) / self.cell.0) as usize;
        let r = ((p.y - frame.y) / self.cell.1) as usize;
        let slot = r * COLUMNS + c;
        (c < COLUMNS
            && r < ROWS
            && slot < filled
            && self.item_rect(&self.cell_rect(frame, slot)).contains(p))
        .then_some(slot)
    }

    /// Hit frame of dot `i` of `count`: row width `N·15 + (N − 1)·5`, first x `trunc(W/2 − rowW/2)`.
    pub fn dot_rect(&self, i: usize, count: usize) -> Rect {
        let row = count as f64 * Self::DOT_HIT
            + (count.saturating_sub(1)) as f64 * (Self::DOT_PITCH - Self::DOT_HIT);
        let x = (self.width / 2.0 - row / 2.0).trunc() + i as f64 * Self::DOT_PITCH;
        Rect::new(
            x,
            self.dots.y + (Self::DOT_HIT / 2.0 - 7.5).trunc(),
            Self::DOT_HIT,
            Self::DOT_HIT,
        )
    }

    /// The 8 × 8 dot inside its hit frame.
    pub fn dot_shape(&self, i: usize, count: usize) -> Rect {
        let hit = self.dot_rect(i, count);
        Rect::new(
            hit.x + Self::DOT_INSET,
            hit.y + Self::DOT_INSET,
            Self::DOT_SIZE,
            Self::DOT_SIZE,
        )
    }

    pub fn dot_at(&self, p: Point, count: usize) -> Option<usize> {
        (0..count).find(|&i| self.dot_rect(i, count).contains(p))
    }
}

/// Zoom factor by layer width (bottom-dock column).
/// A side dock uses the smaller column in the design; the layer here already excludes the
/// dock, so the bottom-dock column applies to it too (approximate).
fn zoom_factor(width: f64) -> f64 {
    if width > 1920.0 {
        0.6
    } else if width > 1680.0 {
        0.3
    } else if width > 1440.0 {
        0.2
    } else {
        0.0
    }
}

/// The icon rule: `n` rounded down to a multiple of 8, at most 128; below 8 it
/// is 1; then at least 20.
pub fn icon_size(n: f64) -> f64 {
    let n = n.floor();
    let icon = if n < 8.0 {
        1.0
    } else {
        (n - n % 8.0).min(128.0)
    };
    icon.max(20.0)
}

/// Whether `name` matches `query` (rule): for every term, the name has the term
/// as a prefix, or a word or camel-case segment of it has, or the term (2+ characters) is a prefix
/// of the words' initials, or the space-stripped name has the term as a prefix. Case-insensitive;
/// the design is diacritic-insensitive too, which is not modelled (approximate).
pub fn matches(name: &str, query: &str) -> bool {
    let lower = name.to_lowercase();
    let words: Vec<String> = lower.split_whitespace().map(str::to_string).collect();
    let segments: Vec<String> = camel_segments(name)
        .into_iter()
        .map(|s| s.to_lowercase())
        .collect();
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    let stripped: String = lower.split_whitespace().collect();
    query.split_whitespace().map(str::to_lowercase).all(|term| {
        lower.starts_with(&term)
            || words.iter().chain(&segments).any(|w| w.starts_with(&term))
            || (term.chars().count() >= 2 && initials.starts_with(&term))
            || stripped.starts_with(&term)
    })
}

/// A desktop file ID as words: `org.gnome.Nautilus` → `org gnome Nautilus`.
fn id_words(id: &str) -> String {
    id.replace(['.', '-', '_'], " ")
}

/// Segments of `name` starting at an uppercase letter that follows a lowercase one (`QuickTime` → `Quick`, `Time`).
fn camel_segments(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in name.split_whitespace() {
        let mut current = String::new();
        let mut previous_lower = false;
        for ch in word.chars() {
            if ch.is_uppercase() && previous_lower && !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            previous_lower = ch.is_lowercase();
            current.push(ch);
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}

/// A key the grid reacts to, already mapped from the toolkit's codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Return,
    Escape,
    Backspace,
    /// A printable character typed into the search field.
    Char(char),
}

/// What a key or click asks the process to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Start the application with this key and close the grid.
    Launch(String),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Entering(f64),
    Shown,
    Leaving(f64),
}

/// A page flip under way: from the fractional page position `from` since `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Flip {
    from: f64,
    at: f64,
}

/// Wheel delta accumulated since the first event of a burst and when the last one came.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Wheel {
    total: f64,
    last: f64,
}

/// Opacity and scale of the whole grid this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visual {
    pub alpha: f64,
    pub scale: f64,
}

/// The open grid: the applications, the search, the page and the keyboard selection.
#[derive(Clone, Debug, PartialEq)]
pub struct AppGrid {
    apps: Vec<AppEntry>,
    query: String,
    /// Indices into `apps` the query leaves, in order; every one while the query is empty.
    results: Vec<usize>,
    page: usize,
    flip: Option<Flip>,
    /// Index into `results`.
    selected: Option<usize>,
    phase: Phase,
    wheel: Option<Wheel>,
    /// Reduce Motion: the show and hide keep their opacity and drop the scale step (LPD Reduce Motion row,
    /// ).
    reduce_motion: bool,
}

impl AppGrid {
    /// The grid over `apps`, sorted by name (the design keeps the user's arrangement, which has
    /// no counterpart here: approximate), entering from `now`.
    pub fn new(mut apps: Vec<AppEntry>, now: f64) -> Self {
        apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.key.clone()));
        let results = (0..apps.len()).collect();
        Self {
            apps,
            query: String::new(),
            results,
            page: 0,
            flip: None,
            selected: None,
            phase: Phase::Entering(now),
            wheel: None,
            reduce_motion: false,
        }
    }

    /// Follow the session's Reduce Motion option from the next frame on.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.reduce_motion = on;
    }

    /// Every desktop entry the index has that is not `NoDisplay`.
    pub fn from_desktop(desktop: &DesktopIndex, now: f64) -> Self {
        let apps = desktop
            .entries
            .iter()
            .filter(|e| !e.no_display)
            .map(|e| AppEntry {
                key: e.id.clone(),
                name: e.name.clone(),
                icon: if e.icon.is_empty() {
                    e.id.clone()
                } else {
                    e.icon.clone()
                },
            })
            .collect();
        Self::new(apps, now)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// The applications the query leaves, in page order.
    pub fn results(&self) -> impl Iterator<Item = &AppEntry> + '_ {
        self.results.iter().map(|&i| &self.apps[i])
    }

    /// The result at `index`.
    pub fn result(&self, index: usize) -> Option<&AppEntry> {
        self.results.get(index).map(|&i| &self.apps[i])
    }

    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    pub fn page(&self) -> usize {
        self.page
    }

    /// Pages the results fill; one, with no items, when there are none.
    pub fn page_count(&self) -> usize {
        self.results.len().div_ceil(PER_PAGE).max(1)
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Fractional page position: the flip's curve between its start and the current page.
    pub fn page_position(&self, now: f64) -> f64 {
        match self.flip {
            Some(flip) => {
                flip.from
                    + (self.page as f64 - flip.from)
                        * PAGE_CURVE.eval(((now - flip.at) / PAGE_DURATION).clamp(0.0, 1.0))
            }
            None => self.page as f64,
        }
    }

    /// Flip to `target`, animating from where the pages stand.
    pub fn go_to_page(&mut self, target: usize, now: f64) {
        let target = target.min(self.page_count() - 1);
        if target == self.page {
            return;
        }
        self.flip = Some(Flip {
            from: self.page_position(now),
            at: now,
        });
        self.page = target;
    }

    fn select(&mut self, index: usize, now: f64) {
        self.selected = Some(index);
        self.go_to_page(index / PER_PAGE, now);
    }

    /// Flip by `pages` and keep the selection in the same slot of the new page, clamped to the results.
    fn flip_by(&mut self, pages: isize, now: f64) {
        let target = (self.page as isize + pages).clamp(0, self.page_count() as isize - 1) as usize;
        self.flip_to(target, now);
    }

    fn flip_to(&mut self, target: usize, now: f64) {
        if let Some(selected) = self.selected {
            let slot = selected % PER_PAGE;
            self.selected =
                Some((target * PER_PAGE + slot).min(self.results.len().saturating_sub(1)));
        }
        self.go_to_page(target, now);
    }

    /// A key; `Char` types into the search field. Returns what the process has to do, if anything.
    pub fn key(&mut self, key: Key, now: f64) -> Option<Outcome> {
        if matches!(self.phase, Phase::Leaving(_)) {
            return None;
        }
        match key {
            Key::Escape => return Some(Outcome::Close),
            Key::Return => {
                let index = self.selected.or_else(|| {
                    (!self.query.is_empty() && !self.results.is_empty()).then_some(0)
                })?;
                return self.result(index).map(|a| Outcome::Launch(a.key.clone()));
            }
            Key::Char(c) => {
                self.query.push(c);
                self.refilter();
            }
            Key::Backspace => {
                if self.query.pop().is_some() {
                    self.refilter();
                }
            }
            Key::Left | Key::Right | Key::Up | Key::Down => {
                if self.results.is_empty() {
                    return None;
                }
                let last = self.results.len() - 1;
                let next = match (self.selected, key) {
                    (None, _) => (self.page * PER_PAGE).min(last),
                    (Some(i), Key::Right) => (i + 1).min(last),
                    (Some(i), Key::Left) => i.saturating_sub(1),
                    (Some(i), Key::Down) => {
                        if i + COLUMNS <= last {
                            i + COLUMNS
                        } else {
                            i
                        }
                    }
                    (Some(i), _) => {
                        if i >= COLUMNS {
                            i - COLUMNS
                        } else {
                            i
                        }
                    }
                };
                self.select(next, now);
            }
            Key::PageUp => self.flip_by(-1, now),
            Key::PageDown => self.flip_by(1, now),
            Key::Home => self.flip_to(0, now),
            Key::End => self.flip_to(self.page_count() - 1, now),
        }
        None
    }

    /// The query changed: the results and the pages start over. An application whose name fails
    /// is tried by its desktop file ID, read as words (the design tries the bundle file name).
    fn refilter(&mut self) {
        let query = self.query.clone();
        self.results = (0..self.apps.len())
            .filter(|&i| {
                matches(&self.apps[i].name, &query) || matches(&id_words(&self.apps[i].key), &query)
            })
            .collect();
        self.selected = None;
        self.page = 0;
        self.flip = None;
    }

    /// A left press at `p`: on a dot it flips there, on the search field nothing, on an item it
    /// launches, anywhere else it closes. Presses during a flip or the exit are ignored.
    pub fn click(&mut self, p: Point, layout: &GridLayout, now: f64) -> Option<Outcome> {
        if matches!(self.phase, Phase::Leaving(_))
            || self.flip.is_some_and(|f| now - f.at < PAGE_DURATION)
        {
            return None;
        }
        if let Some(dot) = layout.dot_at(p, self.page_count()) {
            self.go_to_page(dot, now);
            return None;
        }
        if layout.search.contains(p) {
            return None;
        }
        let first = self.page * PER_PAGE;
        let filled = self.results.len().saturating_sub(first).min(PER_PAGE);
        match layout.slot_at(&layout.page_frame(self.page, self.page as f64), p, filled) {
            Some(slot) => self
                .result(first + slot)
                .map(|a| Outcome::Launch(a.key.clone())),
            None => Some(Outcome::Close),
        }
    }

    /// A wheel or scroll-gesture step; the flip comes from [`Self::settle`] after the burst.
    pub fn scroll(&mut self, dx: f64, dy: f64, now: f64) {
        let dominant = if dx.abs() > dy.abs() { dx } else { dy };
        let wheel = self.wheel.get_or_insert(Wheel {
            total: 0.0,
            last: now,
        });
        wheel.total += WHEEL_GAIN * dominant;
        wheel.last = now;
    }

    /// Seconds until the wheel burst settles; `None` without one.
    pub fn until_settled(&self, now: f64) -> Option<f64> {
        self.wheel.map(|w| (w.last + WHEEL_SETTLE - now).max(0.0))
    }

    /// Flip one page in the direction of a wheel burst whose last event is 75 ms old. True when
    /// something changed.
    pub fn settle(&mut self, now: f64) -> bool {
        let Some(wheel) = self.wheel.filter(|w| now - w.last >= WHEEL_SETTLE) else {
            return false;
        };
        self.wheel = None;
        if wheel.total == 0.0 {
            return false;
        }
        let before = self.page;
        self.flip_by(if wheel.total > 0.0 { 1 } else { -1 }, now);
        self.page != before
    }

    /// Start the exit animation; a second close changes nothing.
    pub fn close(&mut self, now: f64) {
        if !matches!(self.phase, Phase::Leaving(_)) {
            self.phase = Phase::Leaving(now);
        }
    }

    pub fn is_closing(&self) -> bool {
        matches!(self.phase, Phase::Leaving(_))
    }

    /// Whether the exit animation has finished and the surface can go.
    pub fn is_gone(&self, now: f64) -> bool {
        matches!(self.phase, Phase::Leaving(at) if now - at >= HIDE_DURATION)
    }

    /// Settle finished animations.
    pub fn tick(&mut self, now: f64) {
        if matches!(self.phase, Phase::Entering(at) if now - at >= SHOW_DURATION) {
            self.phase = Phase::Shown;
        }
        if self.flip.is_some_and(|f| now - f.at >= PAGE_DURATION) {
            self.flip = None;
        }
    }

    /// Whether a frame per refresh is needed (an entry, exit or flip under way).
    pub fn animating(&self, now: f64) -> bool {
        let phase = match self.phase {
            Phase::Entering(at) => now - at < SHOW_DURATION,
            Phase::Shown => false,
            Phase::Leaving(at) => now - at < HIDE_DURATION,
        };
        phase || self.flip.is_some_and(|f| now - f.at < PAGE_DURATION)
    }

    /// Opacity and scale: in with ease-in from (0, 1.05), out with ease-out to the same. Under Reduce Motion
    /// the scale stays 1.
    pub fn visual(&self, now: f64) -> Visual {
        let full = self.full_visual(now);
        if self.reduce_motion {
            return Visual {
                alpha: full.alpha,
                scale: 1.0,
            };
        }
        full
    }

    fn full_visual(&self, now: f64) -> Visual {
        let (alpha, scale) = match self.phase {
            Phase::Entering(at) => {
                let e = CubicBezier::EASE_IN.eval(((now - at) / SHOW_DURATION).clamp(0.0, 1.0));
                (e, ENTER_SCALE - (ENTER_SCALE - 1.0) * e)
            }
            Phase::Shown => (1.0, 1.0),
            Phase::Leaving(at) => {
                let e = CubicBezier::EASE_OUT.eval(((now - at) / HIDE_DURATION).clamp(0.0, 1.0));
                (1.0 - e, 1.0 + (ENTER_SCALE - 1.0) * e)
            }
        };
        Visual { alpha, scale }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps(n: usize) -> Vec<AppEntry> {
        (0..n)
            .map(|i| AppEntry {
                key: format!("app-{i:02}"),
                name: format!("App {i:02}"),
                icon: String::new(),
            })
            .collect()
    }

    /// Layers of the computed table (`cupertino_lp_metrics.py table`, bottom dock, tile 64): the
    /// output height minus the dock extent 80 and the 30 pt band.
    fn layer(w: f64, h: f64) -> GridLayout {
        GridLayout::new(w, h - 80.0 - 30.0)
    }

    #[test]
    fn the_layout_reproduces_the_computed_table() {
        let l = layer(1440.0, 900.0);
        assert_eq!((l.left, l.right, l.top, l.bottom), (90.0, 90.0, 35.0, 35.0));
        assert_eq!(l.page, Rect::new(90.0, 65.0, 1260.0, 690.0));
        assert_eq!((l.cell, l.icon, l.label_y), ((180.0, 138.0), 88.0, 18.0));
        let l = layer(1024.0, 768.0);
        assert_eq!(
            (l.page.w, l.page.h, l.cell, l.icon, l.label_y),
            (844.0, 558.0, (120.0, 111.0), 64.0, 14.0)
        );
        // zoom 0.3 widens the sides: floor(1920 × 0.06 + 90)
        let l = layer(1920.0, 1080.0);
        assert_eq!((l.left, l.top, l.bottom), (205.0, 35.0, 35.0));
        assert_eq!(
            (l.page.w, l.page.h, l.cell, l.icon, l.label_y),
            (1510.0, 870.0, (215.0, 174.0), 128.0, 20.0)
        );
        let l = layer(2560.0, 1440.0);
        assert_eq!(
            (l.left, l.top, l.bottom, l.cell, l.icon),
            (397.0, 42.0, 70.0, (252.0, 237.0), 128.0)
        );
    }

    #[test]
    fn the_icon_rule_rounds_down_to_eights_between_20_and_128() {
        assert_eq!(icon_size(92.0), 88.0);
        assert_eq!(icon_size(128.0), 128.0);
        assert_eq!(icon_size(300.0), 128.0);
        assert_eq!(icon_size(23.0), 20.0, "16 is raised to 20");
        assert_eq!(
            icon_size(5.0),
            20.0,
            "below 8 the icon is 1, then at least 20"
        );
    }

    #[test]
    fn items_sit_in_their_cells_with_the_label_under_the_image() {
        let l = layer(1440.0, 900.0);
        let frame = l.page_frame(0, 0.0);
        let cell = l.cell_rect(&frame, COLUMNS + 2);
        assert_eq!(
            cell,
            Rect::new(90.0 + 2.0 * 180.0, 65.0 + 138.0, 180.0, 138.0)
        );
        let image = l.image_rect(&cell);
        assert_eq!(
            image,
            Rect::new(
                cell.x + 46.0,
                cell.bottom() - 18.0 - 22.0 - 88.0,
                88.0,
                88.0
            )
        );
        let label = l.label_rect(&cell, 60.0);
        assert_eq!(
            (label.w, label.h, label.bottom()),
            (64.0, 22.0, cell.bottom() - 18.0)
        );
        assert_eq!(label.x, cell.x + 58.0, "even width: truncated centre");
        assert_eq!(
            l.label_rect(&cell, 61.0).x,
            cell.x + (90.0 - 32.5f64).round(),
            "odd width: rounded centre"
        );
        assert_eq!(
            l.label_rect(&cell, 400.0).w,
            168.0,
            "never wider than the cell minus 12"
        );
        assert_eq!(
            l.page_frame(1, 0.0).x,
            frame.x + 1440.0,
            "pages are one layer width apart"
        );
        assert_eq!(l.page_frame(1, 0.5).x, frame.x + 720.0);
    }

    #[test]
    fn a_press_hits_an_item_only_inside_its_image_or_label() {
        let l = layer(1440.0, 900.0);
        let frame = l.page_frame(0, 0.0);
        let cell = l.cell_rect(&frame, 3);
        assert_eq!(l.slot_at(&frame, l.image_rect(&cell).center(), 7), Some(3));
        assert_eq!(
            l.slot_at(&frame, Point::new(cell.x + 2.0, cell.y + 2.0), 7),
            None,
            "the cell's corner is empty space"
        );
        assert_eq!(
            l.slot_at(&frame, l.image_rect(&cell).center(), 3),
            None,
            "an empty slot is empty space"
        );
        assert_eq!(
            l.slot_at(&frame, Point::new(frame.x - 1.0, frame.cy()), 35),
            None,
            "outside the page"
        );
    }

    #[test]
    fn dots_are_centred_at_pitch_20_and_the_search_field_sits_15_pt_above_the_page() {
        let l = layer(1440.0, 900.0);
        // row width 3 × 15 + 2 × 5 = 55: first x trunc(720 − 27.5)
        assert_eq!(l.dot_rect(0, 3), Rect::new(692.0, l.dots.y, 15.0, 15.0));
        assert_eq!(l.dot_rect(2, 3).x, 732.0);
        assert_eq!(
            l.dot_shape(1, 3),
            Rect::new(715.0, l.dots.y + 3.0, 8.0, 8.0)
        );
        assert_eq!(
            l.dots.y,
            790.0 - 15.0 - 10.0,
            "strip 10 pt up: floor(35 × 0.5 − 7.5)"
        );
        assert_eq!(l.dot_at(Point::new(720.0, l.dots.y + 7.0), 3), Some(1));
        assert_eq!(l.search, Rect::new(595.0, 25.0, 250.0, 25.0));
        assert_eq!(l.page.y - l.search.bottom(), 15.0);
    }

    #[test]
    fn the_search_rule_matches_prefixes_segments_initials_and_stripped_names() {
        assert!(matches("Quick Time Player", "qui"));
        assert!(matches("Quick Time Player", "pla"), "a word prefix");
        assert!(matches("QuickTime", "time"), "a camel-case segment");
        assert!(matches("Quick Time Player", "qtp"), "initials");
        assert!(matches("Quick Time Player", "qt"), "two initials");
        assert!(
            !matches("Quick Time Player", "tp"),
            "initials match from the first word only"
        );
        assert!(
            matches("Quick Time Player", "quicktime"),
            "the space-stripped name"
        );
        assert!(
            matches("Quick Time Player", "time player"),
            "every term has to match"
        );
        assert!(!matches("Quick Time Player", "time nope"));
        assert!(!matches("Quick Time Player", "ick"), "no substring matches");
        assert!(matches("anything", ""), "an empty query matches all");
    }

    #[test]
    fn typing_filters_by_name_then_by_key_and_backspace_widens_again() {
        let mut g = AppGrid::new(
            vec![
                AppEntry {
                    key: "org.gnome.Nautilus".into(),
                    name: "Files".into(),
                    icon: String::new(),
                },
                AppEntry {
                    key: "foot".into(),
                    name: "Foot".into(),
                    icon: String::new(),
                },
            ],
            0.0,
        );
        assert_eq!(
            g.results().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Files", "Foot"],
            "sorted by name"
        );
        g.key(Key::Char('f'), 0.0);
        g.key(Key::Char('o'), 0.0);
        assert_eq!(
            g.results().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Foot"]
        );
        assert_eq!(
            g.key(Key::Return, 0.0),
            Some(Outcome::Launch("foot".into())),
            "Return with a query launches the first result"
        );
        g.key(Key::Backspace, 0.0);
        g.key(Key::Backspace, 0.0);
        assert_eq!(g.result_count(), 2);
        assert_eq!(
            g.key(Key::Return, 0.0),
            None,
            "nothing selected and no query"
        );
        for c in "nautilus".chars() {
            g.key(Key::Char(c), 0.0);
        }
        assert_eq!(
            g.results().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Files"],
            "the desktop file ID is tried when the name fails"
        );
        g.key(Key::Char('x'), 0.0);
        assert_eq!(
            (g.result_count(), g.page_count()),
            (0, 1),
            "no results is one empty page"
        );
    }

    #[test]
    fn arrows_select_and_follow_the_selection_across_pages() {
        let mut g = AppGrid::new(apps(40), 0.0);
        assert_eq!(g.page_count(), 2);
        assert_eq!(g.key(Key::Right, 0.0), None);
        assert_eq!(
            g.selected(),
            Some(0),
            "the first arrow selects the page's first item"
        );
        g.key(Key::Down, 0.0);
        assert_eq!(g.selected(), Some(COLUMNS));
        g.key(Key::Up, 0.0);
        g.key(Key::Left, 0.0);
        assert_eq!(g.selected(), Some(0), "the first item stays at the edges");
        for _ in 0..PER_PAGE {
            g.key(Key::Right, 1.0);
        }
        assert_eq!(
            (g.selected(), g.page()),
            (Some(PER_PAGE), 1),
            "Right past the page's last item flips to the next page"
        );
        assert!(g.animating(1.1));
        g.key(Key::Down, 1.0);
        assert_eq!(
            g.selected(),
            Some(PER_PAGE),
            "Down with no item below stays"
        );
        for _ in 0..10 {
            g.key(Key::Right, 1.0);
        }
        assert_eq!(g.selected(), Some(39), "the last result is the end");
        assert_eq!(
            g.key(Key::Return, 1.0),
            Some(Outcome::Launch("app-39".into()))
        );
        assert_eq!(g.key(Key::Escape, 1.0), Some(Outcome::Close));
    }

    #[test]
    fn page_keys_flip_within_the_pages_and_keep_the_slot() {
        let mut g = AppGrid::new(apps(80), 0.0);
        assert_eq!(g.page_count(), 3);
        g.key(Key::PageUp, 0.0);
        assert_eq!(g.page(), 0, "clamped at the first page");
        g.key(Key::Right, 0.0);
        g.key(Key::PageDown, 0.0);
        assert_eq!((g.page(), g.selected()), (1, Some(PER_PAGE)));
        g.key(Key::End, 0.0);
        assert_eq!((g.page(), g.selected()), (2, Some(70)));
        g.key(Key::Right, 0.0);
        g.key(Key::Right, 0.0);
        g.key(Key::Right, 0.0);
        g.key(Key::Right, 0.0);
        g.key(Key::Right, 0.0);
        g.key(Key::Right, 0.0);
        g.key(Key::Right, 0.0);
        g.key(Key::Right, 0.0);
        g.key(Key::Right, 0.0);
        assert_eq!(g.selected(), Some(79));
        g.key(Key::PageUp, 0.0);
        assert_eq!(g.selected(), Some(PER_PAGE + 9), "the slot is kept");
        g.key(Key::Home, 0.0);
        assert_eq!((g.page(), g.selected()), (0, Some(9)));
    }

    #[test]
    fn the_page_flip_follows_its_curve_for_half_a_second() {
        let mut g = AppGrid::new(apps(80), 0.0);
        g.go_to_page(2, 10.0);
        assert_eq!(g.page_position(10.0), 0.0);
        let mid = g.page_position(10.25);
        assert!(
            mid > 1.5 && mid < 2.0,
            "at half the time the flip is past three quarters: {mid}"
        );
        assert!((g.page_position(10.5) - 2.0).abs() < 1e-9);
        g.tick(10.5);
        assert!(!g.animating(10.5));
        g.go_to_page(5, 10.5);
        assert_eq!(
            g.page(),
            2,
            "pages beyond the last are clamped and nothing moves"
        );
    }

    #[test]
    fn the_wheel_flips_once_per_burst_after_75_ms() {
        let mut g = AppGrid::new(apps(80), 0.0);
        g.scroll(0.0, 3.0, 1.0);
        g.scroll(0.0, 4.0, 1.05);
        assert!((g.until_settled(1.1).unwrap() - 0.025).abs() < 1e-9);
        assert!(!g.settle(1.1), "not settled yet");
        assert!(g.settle(1.13));
        assert_eq!(
            g.page(),
            1,
            "a positive total flips forward once, however many events"
        );
        assert_eq!(g.until_settled(1.2), None);
        g.scroll(-20.0, 1.0, 2.0);
        assert!(g.settle(2.1));
        assert_eq!(g.page(), 0, "the dominant axis decides");
        g.scroll(0.0, -5.0, 3.0);
        assert!(!g.settle(3.1), "nothing before the first page");
        assert_eq!(g.page(), 0);
    }

    #[test]
    fn clicks_launch_items_flip_on_dots_and_close_on_empty_space() {
        let l = layer(1440.0, 900.0);
        let mut g = AppGrid::new(apps(45), 0.0);
        g.tick(1.0);
        let frame = l.page_frame(0, 0.0);
        let image = l.image_rect(&l.cell_rect(&frame, 5));
        assert_eq!(
            g.click(image.center(), &l, 1.0),
            Some(Outcome::Launch("app-05".into()))
        );
        assert_eq!(
            g.click(Point::new(frame.x + 1.0, frame.y + 1.0), &l, 1.0),
            Some(Outcome::Close),
            "a cell corner is empty space"
        );
        assert_eq!(
            g.click(l.search.center(), &l, 1.0),
            None,
            "the search field takes the click"
        );
        assert_eq!(g.click(l.dot_rect(1, 2).center(), &l, 1.0), None);
        assert_eq!(g.page(), 1);
        assert_eq!(
            g.click(image.center(), &l, 1.1),
            None,
            "no clicks during the flip"
        );
        g.tick(1.6);
        assert_eq!(
            g.click(image.center(), &l, 1.6),
            Some(Outcome::Launch("app-40".into())),
            "page 1's slot 5"
        );
        g.close(2.0);
        assert_eq!(
            g.click(image.center(), &l, 2.1),
            None,
            "a leaving grid takes nothing"
        );
        assert_eq!(g.key(Key::Return, 2.1), None);
    }

    #[test]
    fn the_grid_fades_and_scales_in_then_out() {
        let mut g = AppGrid::new(apps(3), 10.0);
        assert_eq!(
            g.visual(10.0),
            Visual {
                alpha: 0.0,
                scale: 1.05
            }
        );
        let mid = g.visual(10.125);
        assert!(
            mid.alpha < 0.5 && mid.alpha > 0.0,
            "ease-in starts slowly: {}",
            mid.alpha
        );
        assert!(
            (mid.scale - (1.05 - 0.05 * mid.alpha)).abs() < 1e-9,
            "scale and opacity share the curve"
        );
        assert!(g.animating(10.2));
        g.tick(10.25);
        assert_eq!(
            g.visual(10.25),
            Visual {
                alpha: 1.0,
                scale: 1.0
            }
        );
        assert!(!g.animating(10.3));
        g.close(11.0);
        g.close(11.1);
        assert!(g.is_closing());
        let mid = g.visual(11.125);
        assert!(mid.alpha < 0.5, "ease-out leaves quickly: {}", mid.alpha);
        assert!(!g.is_gone(11.249));
        assert!(g.is_gone(11.25));
        assert_eq!(
            g.visual(11.25),
            Visual {
                alpha: 0.0,
                scale: 1.05
            }
        );
    }

    #[test]
    fn under_reduce_motion_the_grid_only_fades() {
        let mut g = AppGrid::new(apps(3), 10.0);
        let full = g.visual(10.125);
        g.set_reduce_motion(true);
        assert_eq!(
            g.visual(10.0),
            Visual {
                alpha: 0.0,
                scale: 1.0
            }
        );
        assert_eq!(
            g.visual(10.125),
            Visual {
                alpha: full.alpha,
                scale: 1.0
            },
            "the same fade, no scale step"
        );
        g.close(11.0);
        assert_eq!(g.visual(11.125).scale, 1.0);
        assert!(g.is_gone(11.25), "the hide keeps its duration");
    }
}
