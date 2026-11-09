//! The application browser: the launcher's view of every application, in the same panel as the search field. Under
//! the field (which filters it) sit a row of category filters and the applications, as a grid or as a list: first a
//! row of the recently used ones, a separator, then every application by name. The content scrolls under a scroll
//! indicator that shows while it moves.
//!
//! This module is the browser's model: what it holds, its geometry inside the panel, its keyboard, its scrolling and
//! what a click asks for. It is pure. The panel's transitions are [`crate::motion`]'s, Escape is [`crate::nav`]'s and
//! the Kit nodes are `view::browse`.
//!
//! Selection is kept by application, not by position: when the set of applications changes (an entry installed or
//! removed while the browser is up) the selected one stays selected wherever it moved, and when it is gone nothing is.
//! A filter (a query or a category) starts the selection and the scroll over; clearing it puts back the scroll the
//! unfiltered view had, so a look into a category does not lose the place in the full list. Switching between grid and
//! list keeps the selected application (or the first one shown) in view.
//!
//! Coordinates are points from the panel's top-left corner.

use crate::desktop::DesktopEntry;
use cupertino_kit::geometry::{Point, Rect};
use std::collections::HashMap;

/// Columns of the grid, the recents row included.
pub const COLUMNS: usize = 7;

/// How long the scroll indicator stays after the content last scrolled, then how long it takes to fade out.
pub const SCROLLER_LINGER: f64 = 0.8;
pub const SCROLLER_FADE: f64 = 0.25;

/// The categories a filter narrows the applications to, in the order the filters show. An application is in every
/// category one of its desktop entry's `Categories` maps to ([`Category::of`]), and in [`Category::Other`] when none
/// does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Productivity,
    Development,
    Creativity,
    Media,
    Internet,
    Games,
    Education,
    Utilities,
    System,
    Other,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::Productivity,
        Category::Development,
        Category::Creativity,
        Category::Media,
        Category::Internet,
        Category::Games,
        Category::Education,
        Category::Utilities,
        Category::System,
        Category::Other,
    ];

    /// What the filter says.
    pub fn name(self) -> &'static str {
        match self {
            Category::Productivity => "Productivity",
            Category::Development => "Development",
            Category::Creativity => "Creativity",
            Category::Media => "Media",
            Category::Internet => "Internet",
            Category::Games => "Games",
            Category::Education => "Education",
            Category::Utilities => "Utilities",
            Category::System => "System",
            Category::Other => "Other",
        }
    }

    fn bit(self) -> u16 {
        1 << self as u16
    }

    /// The category of one freedesktop category (main or additional), if any.
    pub fn of(category: &str) -> Option<Category> {
        Some(match category {
            "Office" | "Calendar" | "ContactManagement" | "Database" | "Finance" | "FlowChart"
            | "PDA" | "ProjectManagement" | "Presentation" | "Spreadsheet" | "WordProcessor" => {
                Category::Productivity
            }
            "Development" | "Building" | "Debugger" | "IDE" | "GUIDesigner" | "Profiling"
            | "RevisionControl" | "Translation" | "WebDevelopment" => Category::Development,
            "Graphics" | "2DGraphics" | "3DGraphics" | "VectorGraphics" | "RasterGraphics"
            | "Photography" | "Publishing" | "Scanning" | "OCR" | "ImageProcessing" => {
                Category::Creativity
            }
            "AudioVideo" | "Audio" | "Video" | "Midi" | "Mixer" | "Sequencer" | "Tuner" | "TV"
            | "AudioVideoEditing" | "Player" | "Recorder" | "DiscBurning" | "Music" => {
                Category::Media
            }
            "Network" | "Email" | "Dialup" | "InstantMessaging" | "Chat" | "IRCClient" | "Feed"
            | "FileTransfer" | "HamRadio" | "News" | "P2P" | "RemoteAccess" | "Telephony"
            | "VideoConference" | "WebBrowser" => Category::Internet,
            "Game" | "ActionGame" | "AdventureGame" | "ArcadeGame" | "BoardGame" | "BlocksGame"
            | "CardGame" | "KidsGame" | "LogicGame" | "RolePlaying" | "Shooter" | "Simulation"
            | "SportsGame" | "StrategyGame" => Category::Games,
            "Education"
            | "Science"
            | "Art"
            | "Construction"
            | "Languages"
            | "ArtificialIntelligence"
            | "Astronomy"
            | "Biology"
            | "Chemistry"
            | "ComputerScience"
            | "DataVisualization"
            | "Economy"
            | "Electricity"
            | "Geography"
            | "Geology"
            | "Geoscience"
            | "History"
            | "Humanities"
            | "Literature"
            | "Maps"
            | "Math"
            | "NumericalAnalysis"
            | "MedicalSoftware"
            | "Physics"
            | "Robotics"
            | "Spirituality"
            | "Sports"
            | "ParallelComputing"
            | "Dictionary" => Category::Education,
            "Utility" | "TextTools" | "Archiving" | "Compression" | "FileTools"
            | "Accessibility" | "Calculator" | "Clock" | "TextEditor" | "FileManager"
            | "TerminalEmulator" => Category::Utilities,
            "System" | "Settings" | "DesktopSettings" | "HardwareSettings" | "Printing"
            | "PackageManager" | "Emulator" | "Filesystem" | "Monitor" | "Security" => {
                Category::System
            }
            _ => return None,
        })
    }

    /// The categories of an application with these `Categories`, as a set of bits; [`Category::Other`] alone when none
    /// maps.
    pub fn set_of<S: AsRef<str>>(categories: &[S]) -> u16 {
        let bits = categories
            .iter()
            .filter_map(|c| Category::of(c.as_ref()))
            .fold(0, |bits, c| bits | c.bit());
        if bits == 0 {
            Category::Other.bit()
        } else {
            bits
        }
    }
}

/// How the applications are laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Arrangement {
    #[default]
    Grid,
    List,
}

impl Arrangement {
    pub fn name(self) -> &'static str {
        match self {
            Arrangement::Grid => "Grid",
            Arrangement::List => "List",
        }
    }
}

/// One application of the browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    /// Desktop file ID, as the launcher's entries have it.
    pub id: String,
    pub name: String,
    /// The entry's `Icon`, if any.
    pub icon: Option<String>,
    /// The categories it is in ([`Category::set_of`]).
    pub categories: u16,
}

impl App {
    pub fn of(entry: &DesktopEntry) -> App {
        App {
            id: entry.id.clone(),
            name: entry.name.clone(),
            icon: entry.icon.clone(),
            categories: Category::set_of(&entry.categories),
        }
    }

    pub fn is_in(&self, category: Category) -> bool {
        self.categories & category.bit() != 0
    }

    /// The first category it is in, by the filters' order: what the list says under its name.
    pub fn first_category(&self) -> Category {
        Category::ALL
            .into_iter()
            .find(|&c| self.is_in(c))
            .unwrap_or(Category::Other)
    }
}

/// The browser's geometry inside a panel `height` points tall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrowseLayout {
    /// The panel, from its top-left corner.
    pub panel: Rect,
    /// The row the category filters share.
    pub categories: Rect,
    /// The scrolled area.
    pub content: Rect,
    /// Where the first column begins, and the width of each.
    pub columns_x: f64,
    pub column: f64,
}

impl BrowseLayout {
    /// The panel's size, its corner radius, and the least height it shrinks to on a short output.
    pub const WIDTH: f64 = 844.0;
    pub const HEIGHT: f64 = 576.0;
    pub const MIN_HEIGHT: f64 = 300.0;
    pub const RADIUS: f64 = 24.0;
    /// The side inset of the category row and the separators.
    pub const INSET: f64 = 20.0;
    /// The columns' insets from the panel's left and right edges: the right one leaves the scroll indicator its lane.
    pub const COLUMNS_LEFT: f64 = 24.0;
    pub const COLUMNS_RIGHT: f64 = 18.0;
    /// The header row (the query field's) and the separator under it.
    pub const HEADER: f64 = 56.0;
    /// The category filters: the row's top, the filters' height, corner radius, text size and weight, the padding at
    /// each end, the gap between filters and the most a filter grows to fill the row; the average advance of a
    /// character of their text in ems, which a filter's width is reckoned from.
    pub const CATEGORIES_TOP: f64 = 68.0;
    pub const FILTER_HEIGHT: f64 = 26.0;
    pub const FILTER_RADIUS: f64 = 7.0;
    pub const FILTER_FONT_PT: f64 = 14.0;
    pub const FILTER_WEIGHT: u16 = 400;
    pub const FILTER_PAD: f64 = 10.0;
    pub const FILTER_GAP: f64 = 10.0;
    pub const FILTER_GROW: f64 = 24.0;
    pub const FILTER_CHAR_EM: f64 = 0.58;
    /// The scrolled area's top.
    pub const CONTENT_TOP: f64 = 104.0;
    /// Room under the scrolled content's last row, also kept under a row the keys scroll into view: the side inset,
    /// so a selection never runs into the panel's bottom edge.
    pub const CONTENT_BOTTOM: f64 = Self::INSET;
    /// A grid row: its height, the icon's top in it and size, the label's centre line, size and weight, and its inset
    /// from its cell's sides.
    pub const ROW_HEIGHT: f64 = 102.0;
    pub const ICON_TOP: f64 = 10.0;
    pub const ICON: f64 = 60.0;
    pub const LABEL_CY: f64 = 84.0;
    pub const LABEL_FONT_PT: f64 = 13.0;
    pub const LABEL_WEIGHT: u16 = 400;
    pub const LABEL_INSET: f64 = 6.0;
    /// A list row: its height, the icon's size and its inset, and the gap to the name.
    pub const LIST_ROW_HEIGHT: f64 = 44.0;
    pub const LIST_ICON: f64 = 32.0;
    pub const LIST_INSET: f64 = 12.0;
    pub const LIST_GAP: f64 = 12.0;
    /// The selection's inset from its cell, horizontally and vertically, and its corner radius.
    pub const SELECTION_INSET: (f64, f64) = (6.0, 4.0);
    pub const SELECTION_RADIUS: f64 = 12.0;
    /// The block between the recents row and the rest, and the separator's line at its end.
    pub const SEPARATOR_BLOCK: f64 = 15.0;
    pub const SEPARATOR_LINE: f64 = 15.0;
    /// The scroll indicator: its width, its inset from the panel's right edge and from the scrolled area's ends, and
    /// the least length of its thumb.
    pub const SCROLLER_WIDTH: f64 = 6.0;
    pub const SCROLLER_INSET: f64 = 3.0;
    pub const SCROLLER_END_INSET: f64 = 8.0;
    pub const SCROLLER_MIN: f64 = 32.0;

    /// The layout of a panel `height` points tall (at most [`Self::HEIGHT`]).
    pub fn new(height: f64) -> Self {
        let (w, h) = (Self::WIDTH, height.clamp(Self::MIN_HEIGHT, Self::HEIGHT));
        let inner = w - 2.0 * Self::INSET;
        Self {
            panel: Rect::new(0.0, 0.0, w, h),
            categories: Rect::new(
                Self::INSET,
                Self::CATEGORIES_TOP,
                inner,
                Self::FILTER_HEIGHT,
            ),
            content: Rect::new(Self::INSET, Self::CONTENT_TOP, inner, h - Self::CONTENT_TOP),
            columns_x: Self::COLUMNS_LEFT,
            column: ((w - Self::COLUMNS_LEFT - Self::COLUMNS_RIGHT) / COLUMNS as f64).floor(),
        }
    }

    /// The panel's height on an output `output_height` points tall whose top `top` points the panel starts at: as
    /// tall as it is designed where the output allows, keeping `bottom` points free below it (the dock's room), on the
    /// panel's shape grid ([`crate::view::SHAPE_GRID`]).
    pub fn height_on(output_height: f64, top: f64, bottom: f64) -> f64 {
        let grid = crate::view::SHAPE_GRID;
        let room = ((output_height - top - bottom) / grid).floor() * grid;
        room.clamp(Self::MIN_HEIGHT, Self::HEIGHT)
    }

    /// The separator under the header, inset like the category row.
    pub fn header_separator(&self) -> Rect {
        Rect::new(
            Self::INSET,
            Self::HEADER,
            self.panel.width() - 2.0 * Self::INSET,
            1.0,
        )
    }

    /// The filters' rectangles. Each is as wide as its text is reckoned to be plus its padding; a row with room to
    /// spare widens them evenly, by at most [`Self::FILTER_GROW`] each, and a row too narrow narrows them all in
    /// proportion.
    pub fn filter_rects(&self, categories: &[Category]) -> Vec<Rect> {
        if categories.is_empty() {
            return Vec::new();
        }
        let natural: Vec<f64> = categories
            .iter()
            .map(|c| {
                (c.name().chars().count() as f64 * Self::FILTER_CHAR_EM * Self::FILTER_FONT_PT)
                    .ceil()
                    + 2.0 * Self::FILTER_PAD
            })
            .collect();
        let gaps = Self::FILTER_GAP * (categories.len() - 1) as f64;
        let sum: f64 = natural.iter().sum();
        let room = self.categories.width();
        let spare = room - gaps - sum;
        let widths: Vec<f64> = if spare >= 0.0 {
            let grow = (spare / categories.len() as f64)
                .min(Self::FILTER_GROW)
                .floor();
            natural.iter().map(|w| w + grow).collect()
        } else {
            let k = ((room - gaps) / sum).max(0.0);
            natural.iter().map(|w| (w * k).floor()).collect()
        };
        let mut x = self.categories.x();
        widths
            .into_iter()
            .map(|w| {
                let r = Rect::new(x, self.categories.y(), w, self.categories.height());
                x += w + Self::FILTER_GAP;
                r
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

    /// The icon in a grid `cell`, centred along it.
    pub fn grid_icon(cell: &Rect) -> Rect {
        Rect::new(
            cell.x() + ((cell.width() - Self::ICON) / 2.0).floor(),
            cell.y() + Self::ICON_TOP,
            Self::ICON,
            Self::ICON,
        )
    }

    /// The icon in a list `cell`, at its leading end.
    pub fn list_icon(cell: &Rect) -> Rect {
        Rect::new(
            cell.x() + Self::SELECTION_INSET.0 + Self::LIST_INSET,
            cell.y() + ((cell.height() - Self::LIST_ICON) / 2.0).floor(),
            Self::LIST_ICON,
            Self::LIST_ICON,
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

/// A key the browser takes, mapped from the panel's keys.
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
    /// Space: turns the focused filter on or off.
    Space,
    Tab,
    BackTab,
}

/// What a key or a click asks the launcher to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Open the application with this desktop file ID; the launcher closes.
    Launch(String),
    /// Open the view options (grid or list).
    ViewOptions,
}

/// Where the keys go besides the query field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zone {
    /// The applications.
    #[default]
    Content,
    /// The category filters.
    Categories,
    /// The view options button at the header's end.
    Options,
}

/// One application placed in the scrolled content.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// Index into [`Browser::items`].
    pub index: usize,
    /// Its cell in the panel, scrolled.
    pub cell: Rect,
}

/// The filters a scroll position is remembered for: the unfiltered view, by arrangement.
type ScrollKey = Arrangement;

/// The browser: the applications, the recents, the filter, the arrangement, the selection and the scroll.
#[derive(Clone, Debug, PartialEq)]
pub struct Browser {
    /// Every application, by name.
    apps: Vec<App>,
    /// Indices into `apps`, the most recently used first, at most a row.
    recents: Vec<usize>,
    /// The desktop IDs of the recents, kept to rebuild them when the applications change.
    recent_ids: Vec<String>,
    /// The categories that have an application, in their order.
    categories: Vec<Category>,
    query: String,
    category: Option<Category>,
    arrangement: Arrangement,
    /// Indices into `apps` the query and the category leave, in order.
    results: Vec<usize>,
    zone: Zone,
    /// Index into `categories` of the filter the keys are on.
    category_focus: usize,
    /// The selected application's desktop ID, and whether it is the one in the recents row (an application can be in
    /// both).
    selected: Option<(String, bool)>,
    scroll: f64,
    /// The scroll the unfiltered view had, put back when a filter clears.
    remembered: HashMap<ScrollKey, f64>,
    layout: BrowseLayout,
    /// When the content last scrolled, for the scroll indicator.
    scrolled_at: Option<f64>,
    /// The time of the input being handled.
    clock: f64,
}

impl Browser {
    /// The browser over `apps`, sorted by name, with `recent` (desktop file IDs, the most recent first) for its first
    /// row, in a panel `height` points tall.
    pub fn new(apps: Vec<App>, recent: &[String], height: f64) -> Self {
        let mut browser = Self {
            apps: Vec::new(),
            recents: Vec::new(),
            recent_ids: recent.to_vec(),
            categories: Vec::new(),
            query: String::new(),
            category: None,
            arrangement: Arrangement::Grid,
            results: Vec::new(),
            zone: Zone::Content,
            category_focus: 0,
            selected: None,
            scroll: 0.0,
            remembered: HashMap::new(),
            layout: BrowseLayout::new(height),
            scrolled_at: None,
            clock: 0.0,
        };
        browser.set_apps(apps);
        browser
    }

    /// Every entry of `entries`, with `recent` for the first row.
    pub fn from_entries(entries: &[DesktopEntry], recent: &[String], height: f64) -> Self {
        Self::new(entries.iter().map(App::of).collect(), recent, height)
    }

    /// The applications changed (entries installed or removed): the filter, the selection (by application) and the
    /// scroll stay where they can.
    pub fn set_apps(&mut self, mut apps: Vec<App>) {
        apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.id.clone()));
        apps.dedup_by(|a, b| a.id == b.id);
        self.apps = apps;
        self.recents.clear();
        for id in &self.recent_ids {
            let id = if id.ends_with(".desktop") {
                id.clone()
            } else {
                format!("{id}.desktop")
            };
            if let Some(i) = self.apps.iter().position(|a| a.id == id) {
                if !self.recents.contains(&i) {
                    self.recents.push(i);
                }
            }
            if self.recents.len() == COLUMNS {
                break;
            }
        }
        self.categories = Category::ALL
            .into_iter()
            .filter(|&c| self.apps.iter().any(|a| a.is_in(c)))
            .collect();
        if self.category.is_some_and(|c| !self.categories.contains(&c)) {
            self.category = None;
        }
        self.category_focus = self
            .category_focus
            .min(self.categories.len().saturating_sub(1));
        self.results = self.filtered();
        if self
            .selected
            .as_ref()
            .is_some_and(|(id, recent)| self.index_of(id, *recent).is_none())
        {
            self.selected = None;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// The panel is now `height` points tall.
    pub fn set_height(&mut self, height: f64) {
        self.layout = BrowseLayout::new(height);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    pub fn layout(&self) -> &BrowseLayout {
        &self.layout
    }

    pub fn apps(&self) -> &[App] {
        &self.apps
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn categories(&self) -> &[Category] {
        &self.categories
    }

    /// The category filtering the applications, if one is on.
    pub fn category(&self) -> Option<Category> {
        self.category
    }

    pub fn arrangement(&self) -> Arrangement {
        self.arrangement
    }

    pub fn zone(&self) -> Zone {
        self.zone
    }

    /// The filter the keys are on, while they are on the filters.
    pub fn category_focus(&self) -> Option<usize> {
        (self.zone == Zone::Categories && !self.categories.is_empty())
            .then_some(self.category_focus)
    }

    /// The selected application's index in [`Self::items`].
    pub fn selected(&self) -> Option<usize> {
        self.selected
            .as_ref()
            .and_then(|(id, recent)| self.index_of(id, *recent))
    }

    pub fn selected_app(&self) -> Option<&App> {
        self.selected().and_then(|i| self.item(i))
    }

    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Whether the recents row and its separator show: there are recents, and no filter.
    pub fn shows_recents(&self) -> bool {
        !self.recents.is_empty() && self.query.is_empty() && self.category.is_none()
    }

    /// How many items the recents row shows: the first ones of [`Self::items`].
    pub fn recents_shown(&self) -> usize {
        if self.shows_recents() {
            self.recents.len()
        } else {
            0
        }
    }

    /// The item at `index` of [`Self::items`].
    pub fn item(&self, index: usize) -> Option<&App> {
        let r = self.recents_shown();
        let app = if index < r {
            self.recents.get(index)
        } else {
            self.results.get(index - r)
        };
        app.map(|&i| &self.apps[i])
    }

    /// Every item, in the order the keys walk them: the recents row when it shows, then the results.
    pub fn items(&self) -> impl Iterator<Item = &App> + '_ {
        let recents = &self.recents[..self.recents_shown()];
        recents.iter().chain(&self.results).map(|&i| &self.apps[i])
    }

    pub fn item_count(&self) -> usize {
        self.recents_shown() + self.results.len()
    }

    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    /// The index in [`Self::items`] of the application `id`, in the recents row when `recent` and it shows there,
    /// else in the list.
    fn index_of(&self, id: &str, recent: bool) -> Option<usize> {
        let r = self.recents_shown();
        let in_recents = || {
            self.recents[..r]
                .iter()
                .position(|&i| self.apps[i].id == id)
        };
        let in_list = || {
            self.results
                .iter()
                .position(|&i| self.apps[i].id == id)
                .map(|p| p + r)
        };
        if recent {
            in_recents().or_else(in_list)
        } else {
            in_list()
        }
    }

    fn columns(&self) -> usize {
        match self.arrangement {
            Arrangement::Grid => COLUMNS,
            Arrangement::List => 1,
        }
    }

    fn row_height(&self) -> f64 {
        match self.arrangement {
            Arrangement::Grid => BrowseLayout::ROW_HEIGHT,
            Arrangement::List => BrowseLayout::LIST_ROW_HEIGHT,
        }
    }

    /// How many rows the recents take.
    fn recent_rows(&self) -> usize {
        self.recents_shown().div_ceil(self.columns())
    }

    /// Where the results' rows begin in the scrolled content.
    fn list_top(&self) -> f64 {
        if self.shows_recents() {
            self.recent_rows() as f64 * self.row_height() + BrowseLayout::SEPARATOR_BLOCK
        } else {
            0.0
        }
    }

    /// The separator's line in the scrolled content, while the recents show.
    pub fn separator_y(&self) -> Option<f64> {
        self.shows_recents()
            .then(|| self.recent_rows() as f64 * self.row_height() + BrowseLayout::SEPARATOR_LINE)
    }

    /// How tall the scrolled content is, with the room under its last row.
    pub fn content_height(&self) -> f64 {
        let rows = self.results.len().div_ceil(self.columns()) as f64;
        self.list_top() + rows * self.row_height() + BrowseLayout::CONTENT_BOTTOM
    }

    pub fn max_scroll(&self) -> f64 {
        (self.content_height() - self.layout.content.height()).max(0.0)
    }

    /// The row (recents first) and column of item `index`.
    fn position(&self, index: usize) -> (usize, usize) {
        let r = self.recents_shown();
        let c = self.columns();
        if index < r {
            (index / c, index % c)
        } else {
            let j = index - r;
            (self.recent_rows() + j / c, j % c)
        }
    }

    /// The first item of `row` and how many it holds.
    fn row(&self, row: usize) -> Option<(usize, usize)> {
        let r = self.recents_shown();
        let c = self.columns();
        let rr = self.recent_rows();
        if row < rr {
            let first = row * c;
            return Some((first, (r - first).min(c)));
        }
        let first = (row - rr) * c;
        (first < self.results.len()).then(|| (r + first, (self.results.len() - first).min(c)))
    }

    /// Where the row holding item `index` starts in the scrolled content.
    fn row_y(&self, index: usize) -> f64 {
        let (row, _) = self.position(index);
        let rr = self.recent_rows();
        if row < rr {
            row as f64 * self.row_height()
        } else {
            self.list_top() + (row - rr) as f64 * self.row_height()
        }
    }

    /// The cell of item `index` in the panel, scrolled.
    pub fn cell_of(&self, index: usize) -> Option<Rect> {
        (index < self.item_count()).then(|| self.cell(self.position(index).1, self.row_y(index)))
    }

    fn cell(&self, col: usize, row_y: f64) -> Rect {
        let l = &self.layout;
        match self.arrangement {
            Arrangement::Grid => Rect::new(
                l.columns_x + col as f64 * l.column,
                l.content.y() + row_y - self.scroll,
                l.column,
                BrowseLayout::ROW_HEIGHT,
            ),
            Arrangement::List => Rect::new(
                l.content.x(),
                l.content.y() + row_y - self.scroll,
                l.content.width() - BrowseLayout::COLUMNS_RIGHT,
                BrowseLayout::LIST_ROW_HEIGHT,
            ),
        }
    }

    /// The items whose rows the scrolled area shows, even in part, with their cells.
    pub fn visible(&self) -> Vec<Placed> {
        let (top, bottom) = (self.scroll, self.scroll + self.layout.content.height());
        let h = self.row_height();
        (0..self.item_count())
            .filter_map(|index| {
                let y = self.row_y(index);
                (y + h > top && y < bottom).then(|| Placed {
                    index,
                    cell: self.cell(self.position(index).1, y),
                })
            })
            .collect()
    }

    /// The items within a screenful of the shown rows either way: the ones whose icons are worth loading.
    pub fn nearby(&self) -> Vec<usize> {
        let h = self.layout.content.height();
        let (top, bottom) = (self.scroll - h, self.scroll + 2.0 * h);
        let rh = self.row_height();
        (0..self.item_count())
            .filter(|&index| {
                let y = self.row_y(index);
                y + rh > top && y < bottom
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
            .max(BrowseLayout::SCROLLER_MIN)
            .min(track.height());
        let y = track.y() + (track.height() - len) * (self.scroll / max);
        Some(Rect::new(track.x(), y, track.width(), len))
    }

    /// How much the scroll indicator shows at `now`: whole while the content scrolls and for a moment after.
    pub fn scroller_alpha(&self, now: f64) -> f64 {
        self.scrolled_at.map_or(0.0, |at| {
            (1.0 - (now - at - SCROLLER_LINGER) / SCROLLER_FADE).clamp(0.0, 1.0)
        })
    }

    /// Whether the scroll indicator is still fading at `now`.
    pub fn animating(&self, now: f64) -> bool {
        self.scrolled_at
            .is_some_and(|at| now - at < SCROLLER_LINGER + SCROLLER_FADE)
    }

    /// The filters with their rectangles.
    pub fn filter_rects(&self) -> Vec<(Category, Rect)> {
        self.categories
            .iter()
            .copied()
            .zip(self.layout.filter_rects(&self.categories))
            .collect()
    }

    fn scrolled(&mut self, before: f64) {
        if self.scroll != before {
            self.scrolled_at = Some(self.clock);
        }
    }

    fn set_scroll(&mut self, scroll: f64) {
        let before = self.scroll;
        self.scroll = scroll.clamp(0.0, self.max_scroll());
        self.scrolled(before);
        if self.unfiltered() {
            self.remembered.insert(self.arrangement, self.scroll);
        }
    }

    fn unfiltered(&self) -> bool {
        self.query.is_empty() && self.category.is_none()
    }

    /// Scroll so the row of item `index` shows whole, above the room kept at the scrolled area's bottom.
    fn reveal(&mut self, index: usize) {
        let y = self.row_y(index);
        let h = self.row_height();
        let view = self.layout.content.height() - BrowseLayout::CONTENT_BOTTOM;
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
        let recent = index < self.recents_shown();
        self.selected = self.item(index).map(|a| (a.id.clone(), recent));
        self.reveal(index);
    }

    /// Scroll by `dy` points at `now`, within the content.
    pub fn scroll_by(&mut self, dy: f64, now: f64) {
        if !dy.is_finite() {
            return;
        }
        self.clock = now;
        self.set_scroll(self.scroll + dy);
    }

    /// The applications the query and the category leave.
    fn filtered(&self) -> Vec<usize> {
        (0..self.apps.len())
            .filter(|&i| {
                let app = &self.apps[i];
                self.category.is_none_or(|c| app.is_in(c))
                    && (matches(&app.name, &self.query) || matches(&id_words(&app.id), &self.query))
            })
            .collect()
    }

    /// The query or the category changed: the results start over, the selection with them. A filter starts at the
    /// top; clearing every filter puts back the scroll the unfiltered view had.
    fn refilter(&mut self) {
        self.results = self.filtered();
        self.selected = None;
        let scroll = if self.unfiltered() {
            self.remembered
                .get(&self.arrangement)
                .copied()
                .unwrap_or(0.0)
        } else {
            0.0
        };
        self.scroll = scroll.clamp(0.0, self.max_scroll());
    }

    /// The query typed into the field.
    pub fn set_query(&mut self, query: &str) {
        if query != self.query {
            self.query = query.to_owned();
            self.zone = Zone::Content;
            self.refilter();
        }
    }

    /// Turn `category` on, or off when it is on.
    pub fn toggle_category(&mut self, category: Category) {
        self.set_category(if self.category == Some(category) {
            None
        } else {
            Some(category)
        });
    }

    pub fn set_category(&mut self, category: Option<Category>) {
        let category = category.filter(|c| self.categories.contains(c));
        if let Some(i) = category.and_then(|c| self.categories.iter().position(|&x| x == c)) {
            self.category_focus = i;
        }
        if category != self.category {
            self.category = category;
            self.refilter();
        }
    }

    /// Lay the applications out as `arrangement`, keeping the selected application, or else the first one shown, in
    /// view.
    pub fn set_arrangement(&mut self, arrangement: Arrangement) {
        if arrangement == self.arrangement {
            return;
        }
        let anchor = self
            .selected()
            .or_else(|| self.visible().first().map(|p| p.index));
        self.arrangement = arrangement;
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        match anchor {
            Some(index) => {
                // the anchor's row at the top, or as near as the content allows
                let y = self.row_y(index);
                if self.selected().is_some() {
                    self.reveal(index);
                } else {
                    self.set_scroll(y);
                }
            }
            None => self.set_scroll(0.0),
        }
    }

    /// The keys go to the content or the filters.
    pub fn set_zone(&mut self, zone: Zone) {
        self.zone = if zone == Zone::Categories && self.categories.is_empty() {
            Zone::Content
        } else {
            zone
        };
        if self.zone == Zone::Categories {
            self.category_focus = self
                .category
                .and_then(|c| self.categories.iter().position(|&x| x == c))
                .unwrap_or(self.category_focus);
        } else if self.selected().is_none() && self.item_count() > 0 {
            self.select(0);
        }
    }

    /// Nothing is selected: the keys are back in the query field.
    pub fn deselect(&mut self) {
        self.selected = None;
        self.zone = Zone::Content;
    }

    /// The first application a query left: what Return opens with nothing selected.
    fn first_result(&self) -> Option<Outcome> {
        (!self.query.is_empty())
            .then(|| self.results.first())
            .flatten()
            .map(|&i| Outcome::Launch(self.apps[i].id.clone()))
    }

    /// A key at `now`. Returns what the launcher has to do, if anything.
    pub fn key(&mut self, key: Key, now: f64) -> Option<Outcome> {
        self.clock = now;
        match key {
            Key::Tab | Key::BackTab => {
                // the applications, the filters, then the view options, round and round
                let order = [Zone::Content, Zone::Categories, Zone::Options];
                let at = order.iter().position(|&z| z == self.zone).unwrap_or(0);
                let step = if key == Key::Tab { 1 } else { order.len() - 1 };
                let mut next = order[(at + step) % order.len()];
                if next == Zone::Categories && self.categories.is_empty() {
                    next = order[(at + 2 * step) % order.len()];
                }
                self.set_zone(next);
                return None;
            }
            _ => {}
        }
        match self.zone {
            Zone::Categories => self.category_key(key),
            Zone::Content => self.content_key(key),
            Zone::Options => match key {
                Key::Return | Key::Space => Some(Outcome::ViewOptions),
                Key::Down => {
                    self.set_zone(Zone::Content);
                    None
                }
                Key::Left if !self.categories.is_empty() => {
                    self.set_zone(Zone::Categories);
                    None
                }
                _ => None,
            },
        }
    }

    fn category_key(&mut self, key: Key) -> Option<Outcome> {
        let last = self.categories.len().saturating_sub(1);
        match key {
            Key::Left => self.category_focus = self.category_focus.saturating_sub(1),
            Key::Right => self.category_focus = (self.category_focus + 1).min(last),
            Key::Home => self.category_focus = 0,
            Key::End => self.category_focus = last,
            Key::Return | Key::Space => {
                if let Some(&c) = self.categories.get(self.category_focus) {
                    self.toggle_category(c);
                }
            }
            Key::Down => self.set_zone(Zone::Content),
            _ => {}
        }
        None
    }

    fn content_key(&mut self, key: Key) -> Option<Outcome> {
        if key == Key::Return {
            return match self.selected_app() {
                Some(a) => Some(Outcome::Launch(a.id.clone())),
                None => self.first_result(),
            };
        }
        let count = self.item_count();
        let page_rows =
            ((self.layout.content.height() / self.row_height()).floor() as usize).max(1);
        let Some(current) = self.selected() else {
            match key {
                Key::Left | Key::Right | Key::Up | Key::Down | Key::Home if count > 0 => {
                    self.select(0)
                }
                Key::End if count > 0 => self.select(count - 1),
                Key::PageDown => self.page(1),
                Key::PageUp => self.page(-1),
                _ => {}
            }
            return None;
        };
        let (row, col) = self.position(current);
        let in_row = |b: &Self, row: usize| {
            b.row(row)
                .map(|(first, len)| first + col.min(len.saturating_sub(1)))
        };
        let next = match key {
            Key::Right if self.arrangement == Arrangement::Grid => (current + 1).min(count - 1),
            Key::Left if self.arrangement == Arrangement::Grid => current.saturating_sub(1),
            Key::Down => in_row(self, row + 1).unwrap_or(current),
            Key::Up if row == 0 => {
                // above the first row are the filters
                if !self.categories.is_empty() {
                    self.set_zone(Zone::Categories);
                }
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

    fn last_row(&self) -> Option<usize> {
        let count = self.item_count();
        (count > 0).then(|| self.position(count - 1).0)
    }

    fn page(&mut self, direction: i32) {
        let h = self.layout.content.height();
        self.scroll_by(f64::from(direction) * h, self.clock);
    }

    /// The pointer moved onto item `index`: it is selected (without scrolling).
    pub fn hover(&mut self, index: usize) {
        let recent = index < self.recents_shown();
        if let Some(app) = self.item(index) {
            self.selected = Some((app.id.clone(), recent));
            self.zone = Zone::Content;
        }
    }

    /// A press on item `index` opens it.
    pub fn activate(&self, index: usize) -> Option<Outcome> {
        self.item(index).map(|a| Outcome::Launch(a.id.clone()))
    }

    /// A left press at `p` in the panel: on a filter it turns it on or off, on an application it opens it.
    pub fn click(&mut self, p: Point) -> Option<Outcome> {
        if let Some((c, _)) = self.filter_rects().into_iter().find(|(_, r)| r.contains(p)) {
            self.zone = Zone::Categories;
            self.toggle_category(c);
            return None;
        }
        if !self.layout.content.contains(p) {
            return None;
        }
        self.visible()
            .into_iter()
            .find(|v| BrowseLayout::item_rect(&v.cell).contains(p))
            .and_then(|v| self.activate(v.index))
    }
}

/// Whether `name` matches `query`: for every term, the name has the term as a prefix, or a word or camel-case segment
/// of it has, or the term (2+ characters) is a prefix of the words' initials, or the space-stripped name has the term
/// as a prefix. Case and common Latin diacritics are folded.
pub fn matches(name: &str, query: &str) -> bool {
    let fold = |s: &str| -> String {
        s.chars()
            .flat_map(char::to_lowercase)
            .map(crate::matcher::fold_char)
            .collect()
    };
    let lower = fold(name);
    let words: Vec<String> = lower.split_whitespace().map(str::to_string).collect();
    let segments: Vec<String> = camel_segments(name).iter().map(|s| fold(s)).collect();
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    let stripped: String = lower.split_whitespace().collect();
    query.split_whitespace().map(fold).all(|term| {
        lower.starts_with(&term)
            || words.iter().chain(&segments).any(|w| w.starts_with(&term))
            || (term.chars().count() >= 2 && initials.starts_with(&term))
            || stripped.starts_with(&term)
    })
}

/// A desktop file ID as words: `org.example.Viewer.desktop` → `org example Viewer`.
fn id_words(id: &str) -> String {
    id.trim_end_matches(".desktop")
        .replace(['.', '-', '_'], " ")
}

/// Segments of `name` starting at an uppercase letter that follows a lowercase one (`QuickView` → `Quick`, `View`).
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

#[cfg(test)]
mod tests;
