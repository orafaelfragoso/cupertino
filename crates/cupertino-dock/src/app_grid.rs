//! The application grid: a large floating panel centred over the windows, opened by the app-grid action the server
//! forwards (`toggle_app_grid`). Top to bottom it holds a search field over a separator, a row of category pills, a
//! first row of the recently used applications, a separator and then every application in alphabetical order, scrolled
//! under a scroll indicator that shows while it scrolls. The dock owns it whole: this module holds the geometry, the
//! categories, the search rule, the keyboard (selection, pills, the two-step Escape), the scrolling and the animations,
//! and says what a click or a key asks for. The Kit view is `view/grid.rs`.
//!
//! Escape steps back one level at a time: with a query typed it clears the query; otherwise the first Escape folds the
//! panel into its search capsule (its rows fade out at once, then the panel shrinks into the capsule along its top
//! edge) and the next one closes the grid. Typing in the capsule unfolds the panel again on the matches.
//!
//! Coordinates are top-down points in the grid's layer: the output within the dock bar's exclusive zone, so the dock
//! stays visible beside the panel, with the menu bar's strip at the top.

use cupertino_animation::CubicBezier;
use cupertino_wm::geometry::{Point, Rect};

use crate::desktop::DesktopIndex;

/// Columns of the grid, recents row included.
pub const COLUMNS: usize = 7;

/// Show and hide durations. The panel enters from opacity 0 at scale 1.05 with ease-in and leaves the reverse way with
/// ease-out; under Reduce Motion it only fades. The search capsule the panel folded into only fades out, quicker.
pub const SHOW_DURATION: f64 = 0.25;
pub const HIDE_DURATION: f64 = 0.25;
pub const CAPSULE_HIDE_DURATION: f64 = 0.08;
pub const ENTER_SCALE: f64 = 1.05;

/// How long the rows take to fade out when the panel folds into its capsule; the panel starts shrinking after them.
pub const FOLD_FADE: f64 = 0.05;
/// The fold's motion: a critically damped spring with this response, in seconds, from the panel to the capsule and
/// back. It is taken as settled after [`FOLD_SETTLE`].
pub const FOLD_RESPONSE: f64 = 0.24;
pub const FOLD_SETTLE: f64 = 0.4;

/// How long the scroll indicator stays after the content last scrolled, then how long it takes to fade out.
pub const SCROLLER_LINGER: f64 = 0.8;
pub const SCROLLER_FADE: f64 = 0.25;

/// Wayland layer-shell namespace of the grid's surface.
pub const NAMESPACE: &str = "cupertino-dock-app-grid";

/// The strip the menu bar keeps at the top of the output (the theme's menu bar height): the panel is centred below it.
pub const MENU_BAR_BAND: f64 = 24.0;

/// The categories a pill filters by, in the order the pills show. An application is in every category one of its
/// desktop entry's `Categories` maps to ([`Pill::of_category`]), and in [`Pill::Other`] when none does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pill {
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

impl Pill {
    pub const ALL: [Pill; 10] = [
        Pill::Productivity,
        Pill::Development,
        Pill::Creativity,
        Pill::Media,
        Pill::Internet,
        Pill::Games,
        Pill::Education,
        Pill::Utilities,
        Pill::System,
        Pill::Other,
    ];

    /// What the pill says.
    pub fn name(self) -> &'static str {
        match self {
            Pill::Productivity => "Productivity",
            Pill::Development => "Development",
            Pill::Creativity => "Creativity",
            Pill::Media => "Media",
            Pill::Internet => "Internet",
            Pill::Games => "Games",
            Pill::Education => "Education",
            Pill::Utilities => "Utilities",
            Pill::System => "System",
            Pill::Other => "Other",
        }
    }

    fn bit(self) -> u16 {
        1 << self as u16
    }

    /// The pill of one freedesktop category (main or additional), if any.
    pub fn of_category(category: &str) -> Option<Pill> {
        Some(match category {
            "Office" | "Calendar" | "ContactManagement" | "Database" | "Finance" | "FlowChart"
            | "PDA" | "ProjectManagement" | "Presentation" | "Spreadsheet" | "WordProcessor" => {
                Pill::Productivity
            }
            "Development" | "Building" | "Debugger" | "IDE" | "GUIDesigner" | "Profiling"
            | "RevisionControl" | "Translation" | "WebDevelopment" => Pill::Development,
            "Graphics" | "2DGraphics" | "3DGraphics" | "VectorGraphics" | "RasterGraphics"
            | "Photography" | "Publishing" | "Scanning" | "OCR" | "ImageProcessing" => {
                Pill::Creativity
            }
            "AudioVideo" | "Audio" | "Video" | "Midi" | "Mixer" | "Sequencer" | "Tuner" | "TV"
            | "AudioVideoEditing" | "Player" | "Recorder" | "DiscBurning" | "Music" => Pill::Media,
            "Network" | "Email" | "Dialup" | "InstantMessaging" | "Chat" | "IRCClient" | "Feed"
            | "FileTransfer" | "HamRadio" | "News" | "P2P" | "RemoteAccess" | "Telephony"
            | "VideoConference" | "WebBrowser" => Pill::Internet,
            "Game" | "ActionGame" | "AdventureGame" | "ArcadeGame" | "BoardGame" | "BlocksGame"
            | "CardGame" | "KidsGame" | "LogicGame" | "RolePlaying" | "Shooter" | "Simulation"
            | "SportsGame" | "StrategyGame" => Pill::Games,
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
            | "Dictionary" => Pill::Education,
            "Utility" | "TextTools" | "Archiving" | "Compression" | "FileTools"
            | "Accessibility" | "Calculator" | "Clock" | "TextEditor" | "FileManager"
            | "TerminalEmulator" => Pill::Utilities,
            "System" | "Settings" | "DesktopSettings" | "HardwareSettings" | "Printing"
            | "PackageManager" | "Emulator" | "Filesystem" | "Monitor" | "Security" => Pill::System,
            _ => return None,
        })
    }

    /// The pills of an application with these `Categories`, as a set of bits; [`Pill::Other`] alone when none maps.
    pub fn set_of<S: AsRef<str>>(categories: &[S]) -> u16 {
        let bits = categories
            .iter()
            .filter_map(|c| Pill::of_category(c.as_ref()))
            .fold(0, |bits, pill| bits | pill.bit());
        if bits == 0 {
            Pill::Other.bit()
        } else {
            bits
        }
    }
}

/// One application of the grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    /// Desktop file ID: what [`crate::session::Session::launch`] starts.
    pub key: String,
    pub name: String,
    /// Icon theme name (the entry's `Icon`, else the ID).
    pub icon: String,
    /// The pills it is under ([`Pill::set_of`]).
    pub pills: u16,
}

impl AppEntry {
    pub fn is_in(&self, pill: Pill) -> bool {
        self.pills & pill.bit() != 0
    }
}

/// Geometry of the panel on a layer of `width × height` points. The panel is as large as [`GridLayout::PANEL_SIZE`]
/// where the output allows, centred in the layer below the menu bar's strip; every part is placed from its top-left
/// corner. The rows scroll inside [`GridLayout::content`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLayout {
    pub width: f64,
    pub height: f64,
    /// The panel open.
    pub panel: Rect,
    /// The search field's row along the panel's top.
    pub search: Rect,
    /// The search capsule the panel folds into: as wide as [`GridLayout::CAPSULE_WIDTH`] where the panel allows,
    /// centred on it, along its top edge.
    pub capsule: Rect,
    /// The row the pills share.
    pub pills: Rect,
    /// The scrolled area: the recents row, the separator and every application.
    pub content: Rect,
    /// Where the first of the [`COLUMNS`] begins, and the width of each.
    pub columns_x: f64,
    pub column: f64,
}

impl GridLayout {
    /// Size of the open panel, and the least room it keeps from the output's edges (the menu bar's strip and the dock
    /// bar's zone are not counted in).
    pub const PANEL_SIZE: (f64, f64) = (842.0, 576.0);
    pub const PANEL_MARGIN: f64 = 24.0;
    /// Corner radius of the panel.
    pub const PANEL_RADIUS: f64 = 24.0;
    /// The search capsule's width; its height is the search row's.
    pub const CAPSULE_WIDTH: f64 = 640.0;
    /// The side inset of the pills and the separators.
    pub const INSET: f64 = 20.0;
    /// The columns' insets from the panel's left and right edges: the right one leaves the scroll indicator its lane.
    pub const COLUMNS_LEFT: f64 = 24.0;
    pub const COLUMNS_RIGHT: f64 = 18.0;
    /// The search row: its height, the magnifying glass's size and centre from the row's left, where the text begins,
    /// and the text's size and weight.
    pub const SEARCH_HEIGHT: f64 = 56.0;
    pub const SEARCH_SYMBOL: f64 = 20.0;
    pub const SEARCH_SYMBOL_X: f64 = 34.0;
    pub const SEARCH_TEXT_X: f64 = 61.0;
    pub const SEARCH_FONT_PT: f64 = 28.0;
    pub const SEARCH_WEIGHT: u16 = 400;
    /// The separator under the search row, from the panel's top.
    pub const SEARCH_SEPARATOR: f64 = 56.0;
    /// The pills: the row's top from the panel's top, the pills' height, corner radius, the text's size and weight,
    /// its padding at each end, the gap between pills and the most a pill grows to fill the row.
    pub const PILLS_TOP: f64 = 68.0;
    pub const PILL_HEIGHT: f64 = 26.0;
    pub const PILL_RADIUS: f64 = 7.0;
    pub const PILL_FONT_PT: f64 = 14.0;
    pub const PILL_WEIGHT: u16 = 500;
    pub const PILL_PAD: f64 = 10.0;
    pub const PILL_GAP: f64 = 10.0;
    pub const PILL_GROW: f64 = 24.0;
    /// Average advance of a character of the pills' text, in ems: what a pill's width is reckoned from.
    pub const PILL_CHAR_EM: f64 = 0.58;
    /// The scrolled area's top from the panel's top and the room left under the last row.
    pub const CONTENT_TOP: f64 = 104.0;
    pub const CONTENT_BOTTOM: f64 = 0.0;
    /// A row: its height, the icon's top in it and size, the label's centre line, size and weight.
    pub const ROW_HEIGHT: f64 = 102.0;
    pub const ICON_TOP: f64 = 18.0;
    pub const ICON: f64 = 60.0;
    pub const LABEL_CY: f64 = 96.0;
    pub const LABEL_FONT_PT: f64 = 13.0;
    pub const LABEL_WEIGHT: u16 = 400;
    /// The label's inset from its cell's sides.
    pub const LABEL_INSET: f64 = 6.0;
    /// The selection highlight's inset from its cell, horizontally and vertically, and its corner radius.
    pub const SELECTION_INSET: (f64, f64) = (6.0, 4.0);
    pub const SELECTION_RADIUS: f64 = 12.0;
    /// The block between the recents row and the rest, and the separator's line at its end.
    pub const SEPARATOR_BLOCK: f64 = 15.0;
    pub const SEPARATOR_LINE: f64 = 15.0;
    /// The scroll indicator: its width, its inset from the panel's right edge and from the ends of the scrolled area,
    /// and the least length of its thumb.
    pub const SCROLLER_WIDTH: f64 = 6.0;
    pub const SCROLLER_INSET: f64 = 3.0;
    pub const SCROLLER_END_INSET: f64 = 8.0;
    pub const SCROLLER_MIN: f64 = 32.0;
    /// "No Results": its size and weight.
    pub const NO_RESULTS_FONT_PT: f64 = 22.0;

    pub fn new(width: f64, height: f64) -> Self {
        let room_w = (width - 2.0 * Self::PANEL_MARGIN).max(0.0);
        let room_h = (height - MENU_BAR_BAND - 2.0 * Self::PANEL_MARGIN).max(0.0);
        let w = Self::PANEL_SIZE.0.min(room_w).floor();
        let h = Self::PANEL_SIZE.1.min(room_h).floor();
        let x = ((width - w) / 2.0).floor();
        let y = (MENU_BAR_BAND + (height - MENU_BAR_BAND - h) / 2.0).floor();
        let panel = Rect::new(x, y, w, h);
        let inner = (w - 2.0 * Self::INSET).max(0.0);
        let columns = (w - Self::COLUMNS_LEFT - Self::COLUMNS_RIGHT).max(0.0);
        let content_top = y + Self::CONTENT_TOP;
        let capsule_w = Self::CAPSULE_WIDTH.min(w);
        Self {
            width,
            height,
            panel,
            search: Rect::new(x, y, w, Self::SEARCH_HEIGHT),
            capsule: Rect::new(
                x + ((w - capsule_w) / 2.0).floor(),
                y,
                capsule_w,
                Self::SEARCH_HEIGHT.min(h),
            ),
            pills: Rect::new(
                x + Self::INSET,
                y + Self::PILLS_TOP,
                inner,
                Self::PILL_HEIGHT,
            ),
            content: Rect::new(
                x + Self::INSET,
                content_top,
                inner,
                (panel.bottom() - Self::CONTENT_BOTTOM - content_top).max(0.0),
            ),
            columns_x: x + Self::COLUMNS_LEFT,
            column: (columns / COLUMNS as f64).floor(),
        }
    }

    /// The panel's shape `fold` of the way into its capsule (0 the panel, 1 the capsule), and its corner radius: the
    /// top edge stays, the sides close in on the capsule and the bottom rises to it.
    pub fn folded(&self, fold: f64) -> (Rect, f64) {
        let f = fold.clamp(0.0, 1.0);
        let lerp = |a: f64, b: f64| a + (b - a) * f;
        let (p, c) = (self.panel, self.capsule);
        let rect = Rect::new(lerp(p.x, c.x), p.y, lerp(p.w, c.w), lerp(p.h, c.h));
        (rect, lerp(Self::PANEL_RADIUS, c.h / 2.0).min(rect.h / 2.0))
    }

    /// The separator under the search row, inset like the pills.
    pub fn search_separator(&self) -> Rect {
        Rect::new(
            self.panel.x + Self::INSET,
            self.panel.y + Self::SEARCH_SEPARATOR,
            (self.panel.w - 2.0 * Self::INSET).max(0.0),
            1.0,
        )
    }

    /// The pills' rectangles in the pill row. Each is as wide as its text is reckoned to be plus its padding; a row
    /// with room to spare widens them evenly, by at most [`Self::PILL_GROW`] each, and a row too narrow narrows them
    /// all in proportion.
    pub fn pill_rects(&self, pills: &[Pill]) -> Vec<Rect> {
        if pills.is_empty() {
            return Vec::new();
        }
        let natural: Vec<f64> = pills
            .iter()
            .map(|p| {
                (p.name().chars().count() as f64 * Self::PILL_CHAR_EM * Self::PILL_FONT_PT).ceil()
                    + 2.0 * Self::PILL_PAD
            })
            .collect();
        let gaps = Self::PILL_GAP * (pills.len() - 1) as f64;
        let sum: f64 = natural.iter().sum();
        let spare = self.pills.w - gaps - sum;
        let widths: Vec<f64> = if spare >= 0.0 {
            let grow = (spare / pills.len() as f64).min(Self::PILL_GROW).floor();
            natural.iter().map(|w| w + grow).collect()
        } else {
            let k = ((self.pills.w - gaps) / sum).max(0.0);
            natural.iter().map(|w| (w * k).floor()).collect()
        };
        let mut x = self.pills.x;
        widths
            .into_iter()
            .map(|w| {
                let r = Rect::new(x, self.pills.y, w, self.pills.h);
                x += w + Self::PILL_GAP;
                r
            })
            .collect()
    }

    /// The cell of column `col` whose row starts `row_y` points down the scrolled content, scrolled by `scroll`.
    pub fn cell(&self, col: usize, row_y: f64, scroll: f64) -> Rect {
        Rect::new(
            self.columns_x + col as f64 * self.column,
            self.content.y + row_y - scroll,
            self.column,
            Self::ROW_HEIGHT,
        )
    }

    /// The icon inside `cell`, centred along it.
    pub fn icon_rect(&self, cell: &Rect) -> Rect {
        Rect::new(
            cell.x + ((cell.w - Self::ICON) / 2.0).floor(),
            cell.y + Self::ICON_TOP,
            Self::ICON,
            Self::ICON,
        )
    }

    /// What the selection highlight and a press cover in `cell`: the cell less its insets.
    pub fn item_rect(&self, cell: &Rect) -> Rect {
        let (ix, iy) = Self::SELECTION_INSET;
        Rect::new(
            cell.x + ix,
            cell.y + iy,
            (cell.w - 2.0 * ix).max(0.0),
            (cell.h - 2.0 * iy).max(0.0),
        )
    }

    /// The scroll indicator's track along the panel's right edge.
    pub fn scroller_track(&self) -> Rect {
        Rect::new(
            self.panel.right() - Self::SCROLLER_INSET - Self::SCROLLER_WIDTH,
            self.content.y + Self::SCROLLER_END_INSET,
            Self::SCROLLER_WIDTH,
            (self.content.h - 2.0 * Self::SCROLLER_END_INSET).max(0.0),
        )
    }
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
    Tab,
    BackTab,
    /// A printable character typed into the search field.
    Char(char),
}

/// What a key or click asks the process to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Open the application with this key (bring it forward, ask it for a window or start it) and close the grid.
    Launch(String),
    Close,
}

/// Where the keys go besides the search field: the applications, or the pills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Grid,
    Pills,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Entering(f64),
    Shown,
    /// Since when, and for how long.
    Leaving(f64, f64),
}

/// The panel folding into its search capsule, or unfolding out of it: 0 is the panel, 1 the capsule.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fold {
    /// Where the fold was when it set off, where it goes, and when it set off.
    from: f64,
    to: f64,
    at: f64,
}

impl Fold {
    const OPEN: Fold = Fold {
        from: 0.0,
        to: 0.0,
        at: 0.0,
    };

    /// When the shape starts moving: folding waits for the rows to fade out.
    fn start(&self) -> f64 {
        if self.to > self.from {
            self.at + FOLD_FADE
        } else {
            self.at
        }
    }

    fn settled(&self, now: f64) -> bool {
        now - self.start() >= FOLD_SETTLE
    }

    /// How far into the capsule the shape is at `now`, along a critically damped spring.
    fn value(&self, now: f64) -> f64 {
        let t = (now - self.start()).max(0.0);
        let x = if self.settled(now) {
            1.0
        } else {
            let w = std::f64::consts::TAU / FOLD_RESPONSE;
            1.0 - (1.0 + w * t) * (-w * t).exp()
        };
        self.from + (self.to - self.from) * x
    }
}

/// Opacity and scale of the whole grid this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visual {
    pub alpha: f64,
    pub scale: f64,
}

/// One application placed in the scrolled content.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// Index into [`AppGrid::items`].
    pub index: usize,
    /// The cell in the layer, scrolled.
    pub cell: Rect,
}

/// The open grid: the applications, the recents, the search, the pill, the selection and the scroll.
#[derive(Clone, Debug, PartialEq)]
pub struct AppGrid {
    /// Every application, by name.
    apps: Vec<AppEntry>,
    /// Indices into `apps`, the most recently used first, at most a row.
    recents: Vec<usize>,
    /// The pills that have an application, in their order.
    pills: Vec<Pill>,
    query: String,
    pill: Option<Pill>,
    /// Indices into `apps` the query and the pill leave, in order; every one while there is neither.
    results: Vec<usize>,
    zone: Zone,
    /// Index into `pills` of the pill the keys are on.
    pill_focus: usize,
    /// Index into [`Self::items`].
    selected: Option<usize>,
    /// How far the content is scrolled, in points.
    scroll: f64,
    layout: Option<GridLayout>,
    phase: Phase,
    fold: Fold,
    /// When the content last scrolled, for the scroll indicator.
    scrolled_at: Option<f64>,
    /// The time of the key or scroll being handled.
    clock: f64,
    /// Reduce Motion: the show and hide keep their opacity and drop the scale step.
    reduce_motion: bool,
}

impl AppGrid {
    /// The grid over `apps`, sorted by name, with `recent` (desktop file IDs, the most recent first) for its first
    /// row, entering from `now`.
    pub fn new(mut apps: Vec<AppEntry>, recent: &[String], now: f64) -> Self {
        apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.key.clone()));
        let mut recents: Vec<usize> = Vec::new();
        for id in recent {
            let id = id.strip_suffix(".desktop").unwrap_or(id);
            if let Some(i) = apps.iter().position(|a| a.key == id) {
                if !recents.contains(&i) {
                    recents.push(i);
                }
            }
            if recents.len() == COLUMNS {
                break;
            }
        }
        let pills = Pill::ALL
            .into_iter()
            .filter(|&p| apps.iter().any(|a| a.is_in(p)))
            .collect();
        let results = (0..apps.len()).collect();
        Self {
            apps,
            recents,
            pills,
            query: String::new(),
            pill: None,
            results,
            zone: Zone::Grid,
            pill_focus: 0,
            selected: None,
            scroll: 0.0,
            layout: None,
            phase: Phase::Entering(now),
            fold: Fold::OPEN,
            scrolled_at: None,
            clock: now,
            reduce_motion: false,
        }
    }

    /// Every desktop entry the index has that is not `NoDisplay`, with `recent` for the first row.
    pub fn from_desktop(desktop: &DesktopIndex, recent: &[String], now: f64) -> Self {
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
                pills: Pill::set_of(&e.categories),
            })
            .collect();
        Self::new(apps, recent, now)
    }

    /// Follow the session's Reduce Motion option from the next frame on.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.reduce_motion = on;
    }

    /// The layer is `width × height` points now.
    pub fn set_size(&mut self, width: f64, height: f64) {
        self.layout = Some(GridLayout::new(width, height));
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    pub fn layout(&self) -> Option<&GridLayout> {
        self.layout.as_ref()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn pills(&self) -> &[Pill] {
        &self.pills
    }

    /// The pill filtering the applications, if one is on.
    pub fn pill(&self) -> Option<Pill> {
        self.pill
    }

    pub fn zone(&self) -> Zone {
        self.zone
    }

    /// The pill the keys are on while they are on the pills.
    pub fn pill_focus(&self) -> Option<usize> {
        (self.zone == Zone::Pills && !self.pills.is_empty()).then_some(self.pill_focus)
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Whether the recents row and its separator show: there are recents, and neither a query nor a pill.
    pub fn shows_recents(&self) -> bool {
        !self.recents.is_empty() && self.query.is_empty() && self.pill.is_none()
    }

    /// How many items of the recents row show: the first ones of [`Self::items`].
    pub fn recents_shown(&self) -> usize {
        if self.shows_recents() {
            self.recents.len()
        } else {
            0
        }
    }

    /// Every item, in the order the keys walk them: the recents row when it shows, then the results.
    pub fn items(&self) -> impl Iterator<Item = &AppEntry> + '_ {
        let recents = &self.recents[..self.recents_shown()];
        recents.iter().chain(&self.results).map(|&i| &self.apps[i])
    }

    /// The item at `index` of [`Self::items`].
    pub fn item(&self, index: usize) -> Option<&AppEntry> {
        let r = self.recents_shown();
        let app = if index < r {
            self.recents.get(index)
        } else {
            self.results.get(index - r)
        };
        app.map(|&i| &self.apps[i])
    }

    pub fn item_count(&self) -> usize {
        self.recents_shown() + self.results.len()
    }

    /// The applications the query and the pill leave, by name.
    pub fn results(&self) -> impl Iterator<Item = &AppEntry> + '_ {
        self.results.iter().map(|&i| &self.apps[i])
    }

    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    /// Where the results' rows begin in the scrolled content.
    fn list_top(&self) -> f64 {
        if self.shows_recents() {
            GridLayout::ROW_HEIGHT + GridLayout::SEPARATOR_BLOCK
        } else {
            0.0
        }
    }

    /// The separator's line in the scrolled content, while the recents row shows.
    pub fn separator_y(&self) -> Option<f64> {
        self.shows_recents()
            .then_some(GridLayout::ROW_HEIGHT + GridLayout::SEPARATOR_LINE)
    }

    /// How tall the scrolled content is.
    pub fn content_height(&self) -> f64 {
        let rows = self.results.len().div_ceil(COLUMNS) as f64;
        self.list_top() + rows * GridLayout::ROW_HEIGHT
    }

    pub fn max_scroll(&self) -> f64 {
        self.layout
            .map_or(0.0, |l| (self.content_height() - l.content.h).max(0.0))
    }

    /// The row (the recents row is row 0 while it shows) and column of item `index`.
    fn position(&self, index: usize) -> (usize, usize) {
        let r = self.recents_shown();
        if index < r {
            (0, index)
        } else {
            let j = index - r;
            (usize::from(r > 0) + j / COLUMNS, j % COLUMNS)
        }
    }

    /// The first item of `row` and how many it holds.
    fn row(&self, row: usize) -> Option<(usize, usize)> {
        let r = self.recents_shown();
        if r > 0 && row == 0 {
            return Some((0, r));
        }
        let list_row = row - usize::from(r > 0);
        let first = list_row * COLUMNS;
        (first < self.results.len()).then(|| (r + first, (self.results.len() - first).min(COLUMNS)))
    }

    /// Where the row holding item `index` starts in the scrolled content.
    fn row_y(&self, index: usize) -> f64 {
        let (row, _) = self.position(index);
        let r = self.recents_shown();
        if r > 0 && row == 0 {
            0.0
        } else {
            self.list_top() + (row - usize::from(r > 0)) as f64 * GridLayout::ROW_HEIGHT
        }
    }

    /// The cell of item `index`, scrolled, in the layer.
    pub fn cell_of(&self, index: usize) -> Option<Rect> {
        let layout = self.layout?;
        (index < self.item_count())
            .then(|| layout.cell(self.position(index).1, self.row_y(index), self.scroll))
    }

    /// The items whose rows the scrolled area shows, even in part, with their cells.
    pub fn visible(&self) -> Vec<Placed> {
        let Some(layout) = self.layout else {
            return Vec::new();
        };
        let (top, bottom) = (self.scroll, self.scroll + layout.content.h);
        (0..self.item_count())
            .filter_map(|index| {
                let y = self.row_y(index);
                (y + GridLayout::ROW_HEIGHT > top && y < bottom).then(|| Placed {
                    index,
                    cell: layout.cell(self.position(index).1, y, self.scroll),
                })
            })
            .collect()
    }

    /// The items within a screenful of the shown rows, either way: the ones whose icons are worth loading.
    pub fn nearby(&self) -> Vec<usize> {
        let Some(layout) = self.layout else {
            return Vec::new();
        };
        let (top, bottom) = (
            self.scroll - layout.content.h,
            self.scroll + 2.0 * layout.content.h,
        );
        (0..self.item_count())
            .filter(|&index| {
                let y = self.row_y(index);
                y + GridLayout::ROW_HEIGHT > top && y < bottom
            })
            .collect()
    }

    /// The scroll indicator's thumb, while the content does not fit.
    pub fn scroller_thumb(&self) -> Option<Rect> {
        let layout = self.layout?;
        let max = self.max_scroll();
        if max <= 0.0 {
            return None;
        }
        let track = layout.scroller_track();
        let len = (track.h * layout.content.h / self.content_height())
            .max(GridLayout::SCROLLER_MIN)
            .min(track.h);
        let y = track.y + (track.h - len) * (self.scroll / max);
        Some(Rect::new(track.x, y, track.w, len))
    }

    /// The panel's rectangle, once sized.
    pub fn panel(&self) -> Option<Rect> {
        self.layout.map(|l| l.panel)
    }

    /// Whether the panel is folded into its search capsule, or folding: the next Escape closes the grid.
    pub fn is_folded(&self) -> bool {
        self.fold.to >= 1.0
    }

    /// How far the panel is into its capsule at `now`: 0 the panel, 1 the capsule.
    pub fn fold(&self, now: f64) -> f64 {
        self.fold.value(now)
    }

    /// The shape drawn at `now`, the panel folding into the capsule or out of it, and its corner radius.
    pub fn shape(&self, now: f64) -> Option<(Rect, f64)> {
        self.layout.map(|l| l.folded(self.fold(now)))
    }

    /// How much of the rows, the pills and the separators shows at `now`: they fade out at once when the panel folds,
    /// and come back as it unfolds.
    pub fn content_alpha(&self, now: f64) -> f64 {
        let f = self.fold;
        if f.to >= 1.0 {
            (1.0 - f.from) * (1.0 - (now - f.at) / FOLD_FADE).clamp(0.0, 1.0)
        } else {
            1.0 - f.value(now)
        }
    }

    /// Fold towards `to` from where the fold is at `now`; under Reduce Motion the shape changes at once and only the
    /// rows fade.
    fn fold_to(&mut self, to: f64, now: f64) {
        self.fold = if self.reduce_motion {
            Fold {
                from: to,
                to,
                at: now - FOLD_FADE - FOLD_SETTLE,
            }
        } else {
            Fold {
                from: self.fold.value(now),
                to,
                at: now,
            }
        };
    }

    /// How much the scroll indicator shows at `now`: whole while the content scrolls and for a moment after, then it
    /// fades out.
    pub fn scroller_alpha(&self, now: f64) -> f64 {
        self.scrolled_at.map_or(0.0, |at| {
            (1.0 - (now - at - SCROLLER_LINGER) / SCROLLER_FADE).clamp(0.0, 1.0)
        })
    }

    /// The content scrolled from `before`: the scroll indicator shows.
    fn scrolled(&mut self, before: f64) {
        if self.scroll != before {
            self.scrolled_at = Some(self.clock);
        }
    }

    /// The search field's row, once sized.
    pub fn search_rect(&self) -> Option<Rect> {
        self.layout.map(|l| l.search)
    }

    /// The pills with their rectangles.
    pub fn pill_rects(&self) -> Vec<(Pill, Rect)> {
        match self.layout {
            Some(l) => self
                .pills
                .iter()
                .copied()
                .zip(l.pill_rects(&self.pills))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Scroll so the row of item `index` shows whole.
    fn reveal(&mut self, index: usize) {
        let Some(layout) = self.layout else {
            return;
        };
        let before = self.scroll;
        let y = self.row_y(index);
        if y < self.scroll {
            self.scroll = y;
        } else if y + GridLayout::ROW_HEIGHT > self.scroll + layout.content.h {
            self.scroll = y + GridLayout::ROW_HEIGHT - layout.content.h;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
        self.scrolled(before);
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        self.reveal(index);
    }

    /// Scroll by `dy` points at `now`, within the content; the capsule does not scroll.
    pub fn scroll_by(&mut self, dy: f64, now: f64) {
        if !dy.is_finite() || self.is_folded() {
            return;
        }
        self.clock = now;
        let before = self.scroll;
        self.scroll = (self.scroll + dy).clamp(0.0, self.max_scroll());
        self.scrolled(before);
    }

    /// Turn `pill` on, or off when it is on: the results start over.
    pub fn toggle_pill(&mut self, pill: Pill) {
        self.pill = if self.pill == Some(pill) {
            None
        } else {
            Some(pill)
        };
        if let Some(i) = self.pills.iter().position(|&p| p == pill) {
            self.pill_focus = i;
        }
        self.refilter();
    }

    /// The query or the pill changed: the results, the selection and the scroll start over. An application whose
    /// name fails the query is tried by its desktop file ID, read as words.
    fn refilter(&mut self) {
        let query = self.query.clone();
        let pill = self.pill;
        self.results = (0..self.apps.len())
            .filter(|&i| {
                let app = &self.apps[i];
                pill.is_none_or(|p| app.is_in(p))
                    && (matches(&app.name, &query) || matches(&id_words(&app.key), &query))
            })
            .collect();
        self.selected = None;
        self.scroll = 0.0;
    }

    /// A key at `now`; `Char` types into the search field. Returns what the process has to do, if anything.
    pub fn key(&mut self, key: Key, now: f64) -> Option<Outcome> {
        if matches!(self.phase, Phase::Leaving(..)) {
            return None;
        }
        self.clock = now;
        if self.is_folded() {
            // the capsule: Escape closes, typing unfolds the panel on the matches, nothing else applies
            match key {
                Key::Escape => return Some(Outcome::Close),
                Key::Char(c) => {
                    self.fold_to(0.0, now);
                    self.query.push(c);
                    self.zone = Zone::Grid;
                    self.refilter();
                }
                _ => {}
            }
            return None;
        }
        match key {
            // with a query typed, Escape clears it and the grid shows every application again; else it folds the panel
            // into its search capsule
            Key::Escape => {
                if self.query.is_empty() {
                    self.fold_to(1.0, now);
                    self.selected = None;
                } else {
                    self.query.clear();
                    self.refilter();
                }
                self.zone = Zone::Grid;
                return None;
            }
            Key::Char(c) if !(c == ' ' && self.zone == Zone::Pills) => {
                self.query.push(c);
                self.zone = Zone::Grid;
                self.refilter();
                return None;
            }
            Key::Backspace => {
                if self.query.pop().is_some() {
                    self.zone = Zone::Grid;
                    self.refilter();
                }
                return None;
            }
            Key::Tab | Key::BackTab => {
                if !self.pills.is_empty() {
                    self.zone = match self.zone {
                        Zone::Grid => {
                            self.pill_focus = self
                                .pill
                                .and_then(|p| self.pills.iter().position(|&q| q == p))
                                .unwrap_or(0);
                            Zone::Pills
                        }
                        Zone::Pills => Zone::Grid,
                    };
                    self.enter_grid();
                }
                return None;
            }
            _ => {}
        }
        match self.zone {
            Zone::Pills => self.pill_key(key),
            Zone::Grid => self.grid_key(key),
        }
    }

    fn first_result(&self) -> Option<Outcome> {
        (!self.query.is_empty())
            .then(|| self.results.first())
            .flatten()
            .map(|&i| Outcome::Launch(self.apps[i].key.clone()))
    }

    fn pill_key(&mut self, key: Key) -> Option<Outcome> {
        let last = self.pills.len().saturating_sub(1);
        match key {
            Key::Left => self.pill_focus = self.pill_focus.saturating_sub(1),
            Key::Right => self.pill_focus = (self.pill_focus + 1).min(last),
            Key::Home => self.pill_focus = 0,
            Key::End => self.pill_focus = last,
            Key::Return | Key::Char(' ') => {
                if let Some(&pill) = self.pills.get(self.pill_focus) {
                    self.toggle_pill(pill);
                }
            }
            Key::Down => {
                self.zone = Zone::Grid;
                self.enter_grid();
            }
            _ => {}
        }
        None
    }

    /// The keys came to the applications: with nothing selected, the first one is.
    fn enter_grid(&mut self) {
        if self.zone == Zone::Grid && self.selected.is_none() && self.item_count() > 0 {
            self.select(0);
        }
    }

    fn grid_key(&mut self, key: Key) -> Option<Outcome> {
        if key == Key::Return {
            return match self.selected {
                Some(i) => self.item(i).map(|a| Outcome::Launch(a.key.clone())),
                None => self.first_result(),
            };
        }
        let count = self.item_count();
        let page_rows = self.layout.map_or(1, |l| {
            ((l.content.h / GridLayout::ROW_HEIGHT).floor() as usize).max(1)
        });
        let Some(current) = self.selected else {
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
        let in_row = |grid: &Self, row: usize| {
            grid.row(row)
                .map(|(first, len)| first + col.min(len.saturating_sub(1)))
        };
        let next = match key {
            Key::Right => (current + 1).min(count - 1),
            Key::Left => current.saturating_sub(1),
            Key::Down => in_row(self, row + 1).unwrap_or(current),
            Key::Up if row == 0 => {
                // above the first row are the pills
                if !self.pills.is_empty() {
                    self.zone = Zone::Pills;
                    self.pill_focus = self
                        .pill
                        .and_then(|p| self.pills.iter().position(|&q| q == p))
                        .unwrap_or(0);
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
        self.select(next);
        None
    }

    fn last_row(&self) -> Option<usize> {
        let count = self.item_count();
        (count > 0).then(|| self.position(count - 1).0)
    }

    /// Scroll a screenful down (`1`) or up (`-1`).
    fn page(&mut self, direction: i32) {
        if let Some(l) = self.layout {
            self.scroll_by(f64::from(direction) * l.content.h, self.clock);
        }
    }

    /// A left press at `p`. Outside the panel it closes the grid. On a pill
    /// it turns that pill on or off, on an application it opens it; anywhere else on the panel it does nothing.
    /// Presses during the exit are ignored.
    pub fn click(&mut self, p: Point) -> Option<Outcome> {
        if matches!(self.phase, Phase::Leaving(..)) {
            return None;
        }
        let layout = self.layout?;
        if self.is_folded() {
            // the capsule takes a press and does nothing with it; anywhere else closes
            return (!layout.capsule.contains(p)).then_some(Outcome::Close);
        }
        if !layout.panel.contains(p) {
            return Some(Outcome::Close);
        }
        if let Some((pill, _)) = self.pill_rects().into_iter().find(|(_, r)| r.contains(p)) {
            self.zone = Zone::Pills;
            self.toggle_pill(pill);
            return None;
        }
        if !layout.content.contains(p) {
            return None;
        }
        self.visible()
            .into_iter()
            .find(|v| layout.item_rect(&v.cell).contains(p))
            .and_then(|v| self.item(v.index))
            .map(|a| Outcome::Launch(a.key.clone()))
    }

    /// Start the entry over from `now`, once the surface is sized and the icons it shows first are loaded: the entry
    /// then runs whole on the frames after it, however long the loading took.
    pub fn restart_entry(&mut self, now: f64) {
        if matches!(self.phase, Phase::Entering(_)) {
            self.phase = Phase::Entering(now);
        }
    }

    /// Start the exit animation: the panel's, or the capsule's quicker fade once folded. A second close changes
    /// nothing.
    pub fn close(&mut self, now: f64) {
        if !matches!(self.phase, Phase::Leaving(..)) {
            let duration = if self.is_folded() {
                CAPSULE_HIDE_DURATION
            } else {
                HIDE_DURATION
            };
            self.phase = Phase::Leaving(now, duration);
        }
    }

    pub fn is_closing(&self) -> bool {
        matches!(self.phase, Phase::Leaving(..))
    }

    /// Whether the exit animation has finished and the surface can go.
    pub fn is_gone(&self, now: f64) -> bool {
        matches!(self.phase, Phase::Leaving(at, d) if now - at >= d)
    }

    /// Settle finished animations.
    pub fn tick(&mut self, now: f64) {
        if matches!(self.phase, Phase::Entering(at) if now - at >= SHOW_DURATION) {
            self.phase = Phase::Shown;
        }
    }

    /// Whether a frame per refresh is needed: an entry, an exit, a fold or the scroll indicator's fade under way.
    pub fn animating(&self, now: f64) -> bool {
        let shown = match self.phase {
            Phase::Entering(at) => now - at < SHOW_DURATION,
            Phase::Shown => false,
            Phase::Leaving(at, d) => now - at < d,
        };
        let scroller = self
            .scrolled_at
            .is_some_and(|at| now - at < SCROLLER_LINGER + SCROLLER_FADE);
        shown || !self.fold.settled(now) || scroller
    }

    /// Opacity and scale: in with ease-in from (0, 1.05), out with ease-out to the same. Under Reduce Motion the scale
    /// stays 1.
    pub fn visual(&self, now: f64) -> Visual {
        let (alpha, scale) = match self.phase {
            Phase::Entering(at) => {
                let e = CubicBezier::EASE_IN.eval(((now - at) / SHOW_DURATION).clamp(0.0, 1.0));
                (e, ENTER_SCALE - (ENTER_SCALE - 1.0) * e)
            }
            Phase::Shown => (1.0, 1.0),
            Phase::Leaving(at, d) => {
                let e = CubicBezier::EASE_OUT.eval(((now - at) / d).clamp(0.0, 1.0));
                // the capsule only fades
                let grow = if self.is_folded() { 0.0 } else { e };
                (1.0 - e, 1.0 + (ENTER_SCALE - 1.0) * grow)
            }
        };
        Visual {
            alpha,
            scale: if self.reduce_motion { 1.0 } else { scale },
        }
    }
}

/// Whether `name` matches `query`: for every term, the name has the term as a prefix, or a word or camel-case segment
/// of it has, or the term (2+ characters) is a prefix of the words' initials, or the space-stripped name has the term
/// as a prefix. Case-insensitive; diacritics are not folded.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn app(key: &str, name: &str, categories: &[&str]) -> AppEntry {
        AppEntry {
            key: key.into(),
            name: name.into(),
            icon: String::new(),
            pills: Pill::set_of(categories),
        }
    }

    fn apps(n: usize) -> Vec<AppEntry> {
        (0..n)
            .map(|i| app(&format!("app-{i:02}"), &format!("App {i:02}"), &[]))
            .collect()
    }

    /// A 1440 × 900 output with a 64 pt dock bar zone below.
    fn sized(mut g: AppGrid) -> AppGrid {
        g.set_size(1440.0, 900.0 - 70.0);
        g
    }

    fn names(g: &AppGrid) -> Vec<String> {
        g.items().map(|a| a.name.clone()).collect()
    }

    #[test]
    fn the_panel_floats_centred_below_the_menu_bar_and_shrinks_on_small_outputs() {
        let l = GridLayout::new(1440.0, 830.0);
        assert_eq!(l.panel, Rect::new(299.0, 139.0, 842.0, 576.0));
        assert_eq!(
            l.panel.cy(),
            MENU_BAR_BAND + (830.0 - MENU_BAR_BAND) / 2.0,
            "centred between the menu bar and the dock"
        );
        assert!(
            l.panel.w < 1440.0 && l.panel.h < 830.0,
            "not the whole output"
        );
        assert_eq!(l.search, Rect::new(299.0, 139.0, 842.0, 56.0));
        assert_eq!(l.pills, Rect::new(319.0, 207.0, 802.0, 26.0));
        assert_eq!(l.content.y, 139.0 + 104.0);
        assert_eq!(
            l.content.bottom(),
            l.panel.bottom(),
            "the rows run to the panel's edge"
        );
        assert_eq!(
            (l.content.x, l.content.w),
            (319.0, 802.0),
            "the rows' clip is inset like the pills"
        );
        assert_eq!(
            (l.columns_x, l.column),
            (299.0 + 24.0, 114.0),
            "seven columns between their insets"
        );
        assert_eq!(
            l.search_separator(),
            Rect::new(319.0, 139.0 + 56.0, 802.0, 1.0)
        );
        assert_eq!(l.capsule, Rect::new(299.0 + 101.0, 139.0, 640.0, 56.0));
        let small = GridLayout::new(800.0, 600.0);
        assert_eq!(
            (small.panel.w, small.panel.h),
            (752.0, 528.0),
            "the panel keeps its margins on a small output"
        );
        assert_eq!(small.column, ((752.0f64 - 24.0 - 18.0) / 7.0).floor());
        assert_eq!(
            small.capsule.w, 640.0,
            "the capsule fits the panel of a small output"
        );
        let tiny = GridLayout::new(500.0, 400.0);
        assert_eq!(tiny.capsule.w, tiny.panel.w, "never wider than the panel");
    }

    #[test]
    fn categories_map_to_pills_and_an_entry_without_one_is_other() {
        assert_eq!(
            Pill::set_of(&["GTK", "Development", "IDE", "TextEditor"]),
            (1 << Pill::Development as u16) | (1 << Pill::Utilities as u16)
        );
        assert_eq!(
            Pill::set_of(&["Office", "Finance"]),
            1 << Pill::Productivity as u16
        );
        assert_eq!(
            Pill::set_of(&["AudioVideo", "Player"]),
            1 << Pill::Media as u16
        );
        assert_eq!(
            Pill::set_of(&["Network", "WebBrowser"]),
            1 << Pill::Internet as u16
        );
        assert_eq!(
            Pill::set_of(&["Game", "LogicGame"]),
            1 << Pill::Games as u16
        );
        assert_eq!(
            Pill::set_of(&["Graphics", "Photography"]),
            1 << Pill::Creativity as u16
        );
        assert_eq!(
            Pill::set_of(&["Science", "Math"]),
            1 << Pill::Education as u16
        );
        assert_eq!(
            Pill::set_of(&["Settings", "HardwareSettings"]),
            1 << Pill::System as u16
        );
        assert_eq!(Pill::set_of::<&str>(&[]), 1 << Pill::Other as u16);
        assert_eq!(Pill::set_of(&["GTK", "Qt"]), 1 << Pill::Other as u16);
    }

    #[test]
    fn only_the_pills_with_an_application_show_in_their_order() {
        let g = AppGrid::new(
            vec![
                app("term", "Terminal", &["System", "TerminalEmulator"]),
                app("web", "Browser", &["Network", "WebBrowser"]),
                app("odd", "Odd", &[]),
            ],
            &[],
            0.0,
        );
        assert_eq!(
            g.pills(),
            [Pill::Internet, Pill::Utilities, Pill::System, Pill::Other]
        );
    }

    #[test]
    fn pills_fill_the_row_up_to_their_growth_and_narrow_when_they_do_not_fit() {
        let l = GridLayout::new(1440.0, 830.0);
        let few = l.pill_rects(&[Pill::Media, Pill::Games]);
        assert_eq!(few[0].x, l.pills.x);
        assert_eq!(few[1].x, few[0].right() + GridLayout::PILL_GAP);
        let natural = (5.0f64 * 0.58 * 14.0).ceil() + 20.0;
        assert_eq!(
            few[0].w,
            natural + GridLayout::PILL_GROW,
            "grows by at most its growth"
        );
        let all = l.pill_rects(&Pill::ALL);
        assert!(
            all.last().unwrap().right() <= l.pills.right() + 0.5,
            "{all:?}"
        );
        assert!(all.windows(2).all(|w| w[1].x >= w[0].right()), "no overlap");
    }

    #[test]
    fn the_recents_row_comes_first_then_a_separator_then_every_application_by_name() {
        let mut g = AppGrid::new(
            vec![
                app("c", "Clock", &[]),
                app("a", "Atlas", &[]),
                app("b", "Books", &[]),
            ],
            &["b.desktop".into(), "gone".into(), "c".into(), "b".into()],
            0.0,
        );
        g.set_size(1440.0, 830.0);
        assert!(g.shows_recents());
        assert_eq!(
            names(&g),
            ["Books", "Clock", "Atlas", "Books", "Clock"],
            "recents (known, once each), then all by name"
        );
        assert_eq!(g.separator_y(), Some(102.0 + 15.0));
        let l = *g.layout().unwrap();
        assert_eq!(g.cell_of(0), Some(l.cell(0, 0.0, 0.0)));
        assert_eq!(
            g.cell_of(2),
            Some(l.cell(0, 102.0 + 15.0, 0.0)),
            "the list starts under the separator block"
        );
        assert_eq!(g.content_height(), 102.0 + 15.0 + 102.0);
        g.key(Key::Char('a'), 0.0);
        assert!(!g.shows_recents(), "a search hides the recents");
        assert_eq!(names(&g), ["Atlas"]);
    }

    #[test]
    fn a_pill_filters_and_a_second_press_shows_everything_again() {
        let mut g = sized(AppGrid::new(
            vec![
                app("term", "Terminal", &["System", "TerminalEmulator"]),
                app("web", "Browser", &["Network", "WebBrowser"]),
                app("odd", "Odd", &[]),
            ],
            &["web".into()],
            0.0,
        ));
        g.toggle_pill(Pill::System);
        assert_eq!(names(&g), ["Terminal"], "no recents under a pill");
        g.toggle_pill(Pill::Other);
        assert_eq!(names(&g), ["Odd"]);
        g.toggle_pill(Pill::Other);
        assert_eq!(names(&g), ["Browser", "Browser", "Odd", "Terminal"]);
        let pill = g.pill_rects()[0].1;
        assert_eq!(g.click(pill.center()), None, "a press on a pill");
        assert_eq!(g.pill(), Some(Pill::Internet));
        assert_eq!(g.zone(), Zone::Pills);
    }

    #[test]
    fn the_first_escape_folds_the_panel_into_its_capsule_and_the_next_closes() {
        let mut g = sized(AppGrid::new(apps(3), &["app-01".into()], 0.0));
        g.tick(1.0);
        g.key(Key::Down, 1.0);
        assert_eq!(g.key(Key::Escape, 2.0), None, "the first Escape folds");
        assert!(g.is_folded());
        assert_eq!(g.selected(), None);
        let l = *g.layout().unwrap();
        assert_eq!(g.shape(2.0), Some((l.panel, GridLayout::PANEL_RADIUS)));
        assert!(g.animating(2.0));
        assert_eq!(g.content_alpha(2.0), 1.0);
        assert!(
            g.content_alpha(2.0 + FOLD_FADE / 2.0) < 0.6,
            "the rows fade at once"
        );
        assert!(g.content_alpha(2.0 + FOLD_FADE) < 1e-9);
        assert_eq!(
            g.shape(2.0 + FOLD_FADE).unwrap().0,
            l.panel,
            "the shape waits for the rows"
        );
        let mid = g.shape(2.0 + FOLD_FADE + 0.07).unwrap().0;
        assert!(
            mid.h < l.panel.h * 0.6 && mid.h > l.capsule.h,
            "half way and more after a third of the response: {mid:?}"
        );
        assert_eq!(mid.y, l.panel.y, "the top edge stays");
        assert!(
            (mid.cx() - l.panel.cx()).abs() < 1.0,
            "the sides close in evenly"
        );
        let end = 2.0 + FOLD_FADE + FOLD_SETTLE + 1e-9;
        assert_eq!(g.shape(end), Some((l.capsule, l.capsule.h / 2.0)));
        assert!(!g.animating(end));
        assert_eq!(
            (l.capsule.w, l.capsule.h, l.capsule.y, l.capsule.cx()),
            (640.0, 56.0, l.panel.y, l.panel.cx())
        );
        // the capsule ignores the grid's keys and the wheel, and takes a press
        for key in [Key::Down, Key::Return, Key::Tab, Key::Backspace] {
            assert_eq!(g.key(key, 3.0), None);
        }
        assert_eq!(g.selected(), None);
        g.scroll_by(500.0, 3.0);
        assert_eq!(g.scroll(), 0.0);
        assert_eq!(g.click(l.capsule.center()), None);
        assert_eq!(
            g.key(Key::Escape, 3.0),
            Some(Outcome::Close),
            "the next Escape closes"
        );
        g.close(3.0);
        assert_eq!(g.visual(3.04).scale, 1.0, "the capsule only fades");
        assert!(!g.is_gone(3.0 + CAPSULE_HIDE_DURATION - 1e-3));
        assert!(g.is_gone(3.0 + CAPSULE_HIDE_DURATION));
    }

    #[test]
    fn typing_in_the_capsule_unfolds_the_panel_on_the_matches_and_a_press_outside_closes() {
        let mut g = sized(AppGrid::new(apps(12), &[], 0.0));
        g.key(Key::Escape, 1.0);
        assert_eq!(
            g.click(g.layout().unwrap().panel.center()),
            Some(Outcome::Close),
            "under the capsule, where the panel was"
        );
        g.key(Key::Char('1'), 2.0);
        assert!(!g.is_folded());
        assert_eq!(g.query(), "1");
        assert_eq!(g.result_count(), 2, "App 10 and App 11");
        assert!(g.fold(2.0) > 0.99 && g.fold(2.0 + FOLD_SETTLE + 1e-9) == 0.0);
        assert!(
            g.content_alpha(2.0 + 0.1) > 0.5,
            "the rows come back as it opens"
        );
        // Escape clears the query first, then folds, then closes
        assert_eq!(g.key(Key::Escape, 3.0), None);
        assert_eq!((g.query(), g.is_folded()), ("", false));
        assert_eq!(g.key(Key::Escape, 3.1), None);
        assert!(g.is_folded());
        assert_eq!(g.key(Key::Escape, 3.2), Some(Outcome::Close));
    }

    #[test]
    fn under_reduce_motion_the_panel_becomes_the_capsule_at_once() {
        let mut g = sized(AppGrid::new(apps(3), &[], 0.0));
        g.set_reduce_motion(true);
        g.key(Key::Escape, 1.0);
        let l = *g.layout().unwrap();
        assert_eq!(g.shape(1.0).unwrap().0, l.capsule);
        assert_eq!(g.content_alpha(1.0), 0.0);
        assert!(!g.animating(1.0));
    }

    #[test]
    fn with_a_query_typed_the_first_escape_clears_it() {
        let mut g = sized(AppGrid::new(apps(3), &[], 0.0));
        g.key(Key::Char('0'), 0.0);
        g.key(Key::Char('1'), 0.0);
        assert_eq!(g.results().count(), 1);
        g.key(Key::Down, 0.0);
        assert_eq!(
            g.key(Key::Escape, 0.0),
            None,
            "the first Escape clears the query"
        );
        assert_eq!(g.query(), "");
        assert_eq!(g.results().count(), 3, "every application again");
        assert_eq!(g.selected(), None);
        assert_eq!(
            g.panel(),
            Some(g.layout().unwrap().panel),
            "the panel stays open"
        );
        assert!(!g.visible().is_empty());
        assert!(!g.is_folded());
        assert_eq!(g.key(Key::Escape, 0.0), None, "the next folds");
        assert!(g.is_folded());
        assert_eq!(
            g.key(Key::Escape, 0.0),
            Some(Outcome::Close),
            "the third closes"
        );
    }

    #[test]
    fn typing_filters_by_name_then_by_key_and_backspace_widens_again() {
        let mut g = sized(AppGrid::new(
            vec![
                app("org.gnome.Nautilus", "Files", &[]),
                app("foot", "Foot", &[]),
            ],
            &[],
            0.0,
        ));
        g.key(Key::Char('f'), 0.0);
        g.key(Key::Char('o'), 0.0);
        assert_eq!(names(&g), ["Foot"]);
        assert_eq!(
            g.key(Key::Return, 0.0),
            Some(Outcome::Launch("foot".into())),
            "Return with a query opens the first result"
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
            names(&g),
            ["Files"],
            "the desktop file ID is tried when the name fails"
        );
    }

    #[test]
    fn arrows_walk_the_rows_and_cross_from_the_recents_to_the_list() {
        let recent: Vec<String> = (0..3).map(|i| format!("app-{i:02}")).collect();
        let mut g = sized(AppGrid::new(apps(20), &recent, 0.0));
        g.key(Key::Right, 0.0);
        assert_eq!(
            g.selected(),
            Some(0),
            "the first arrow selects the first item"
        );
        g.key(Key::Right, 0.0);
        g.key(Key::Right, 0.0);
        g.key(Key::Down, 0.0);
        assert_eq!(
            g.selected(),
            Some(3 + 2),
            "down from the recents keeps the column"
        );
        g.key(Key::Down, 0.0);
        assert_eq!(g.selected(), Some(3 + 7 + 2));
        g.key(Key::Up, 0.0);
        g.key(Key::Up, 0.0);
        assert_eq!(g.selected(), Some(2), "up into the recents row");
        g.key(Key::End, 0.0);
        assert_eq!(g.selected(), Some(22));
        g.key(Key::Down, 0.0);
        assert_eq!(g.selected(), Some(22), "nothing below the last row");
        assert_eq!(
            g.key(Key::Return, 0.0),
            Some(Outcome::Launch("app-19".into()))
        );
        g.key(Key::Home, 0.0);
        g.key(Key::Up, 0.0);
        assert_eq!(
            g.zone(),
            Zone::Pills,
            "up from the first row reaches the pills"
        );
    }

    #[test]
    fn tab_moves_between_the_pills_and_the_grid_and_the_pills_take_their_keys() {
        let mut g = sized(AppGrid::new(
            vec![
                app("term", "Terminal", &["System"]),
                app("web", "Browser", &["WebBrowser"]),
            ],
            &[],
            0.0,
        ));
        g.key(Key::Tab, 0.0);
        assert_eq!((g.zone(), g.pill_focus()), (Zone::Pills, Some(0)));
        g.key(Key::Right, 0.0);
        assert_eq!(g.pill_focus(), Some(1));
        g.key(Key::Char(' '), 0.0);
        assert_eq!(g.pill(), Some(Pill::System), "Space turns the pill on");
        assert_eq!(names(&g), ["Terminal"]);
        g.key(Key::Return, 0.0);
        assert_eq!(g.pill(), None, "Return turns it off");
        g.key(Key::BackTab, 0.0);
        assert_eq!(g.zone(), Zone::Grid);
        g.key(Key::Tab, 0.0);
        g.key(Key::Char('w'), 0.0);
        assert_eq!(g.zone(), Zone::Grid, "typing goes to the search");
        assert_eq!(names(&g), ["Browser"]);
    }

    #[test]
    fn the_selection_scrolls_into_view_and_the_wheel_scrolls_within_the_content() {
        let mut g = sized(AppGrid::new(apps(100), &[], 0.0));
        let l = *g.layout().unwrap();
        assert_eq!(g.content_height(), 15.0 * 102.0);
        assert_eq!(g.max_scroll(), 15.0 * 102.0 - l.content.h);
        let thumb = g.scroller_thumb().expect("the content does not fit");
        assert_eq!(thumb.y, l.scroller_track().y, "at the top");
        assert_eq!(
            g.scroller_alpha(0.0),
            0.0,
            "the indicator shows only while scrolling"
        );
        g.scroll_by(-50.0, 0.0);
        assert_eq!(g.scroll(), 0.0);
        assert_eq!(g.scroller_alpha(0.0), 0.0, "nothing scrolled");
        g.scroll_by(1e6, 5.0);
        assert_eq!(g.scroll(), g.max_scroll());
        assert_eq!(g.scroller_alpha(5.0 + SCROLLER_LINGER), 1.0);
        assert!(g.animating(5.0 + SCROLLER_LINGER));
        assert!(g.scroller_alpha(5.0 + SCROLLER_LINGER + SCROLLER_FADE / 2.0) < 0.6);
        assert!(g.scroller_alpha(5.0 + SCROLLER_LINGER + SCROLLER_FADE) < 1e-9);
        assert_eq!(
            (thumb.w, l.panel.right() - thumb.right()),
            (6.0, 3.0),
            "the thumb's width and its inset from the panel's edge"
        );
        let thumb = g.scroller_thumb().unwrap();
        assert!(
            (thumb.bottom() - l.scroller_track().bottom()).abs() < 1e-9,
            "at the bottom"
        );
        g.key(Key::Home, 0.0);
        assert_eq!((g.selected(), g.scroll()), (Some(0), 0.0));
        for _ in 0..6 {
            g.key(Key::Down, 0.0);
        }
        let cell = g.cell_of(42).unwrap();
        assert!(
            cell.bottom() <= l.content.bottom() + 1e-9 && cell.y >= l.content.y - 1e-9,
            "the selected row shows whole: {cell:?} in {:?}",
            l.content
        );
        let shown: Vec<usize> = g.visible().iter().map(|v| v.index).collect();
        assert!(
            shown.contains(&42) && !shown.contains(&0),
            "only the rows shown are placed"
        );
        assert!(
            g.nearby().len() < 100,
            "icons load near the shown rows only"
        );
    }

    #[test]
    fn clicks_open_items_toggle_pills_and_close_outside_the_panel() {
        let mut g = sized(AppGrid::new(apps(10), &[], 0.0));
        g.tick(1.0);
        let l = *g.layout().unwrap();
        let cell = g.cell_of(5).unwrap();
        assert_eq!(
            g.click(l.icon_rect(&cell).center()),
            Some(Outcome::Launch("app-05".into()))
        );
        assert_eq!(
            g.click(Point::new(cell.x + 1.0, cell.y + 1.0)),
            None,
            "the cell's inset"
        );
        assert_eq!(
            g.click(l.search.center()),
            None,
            "the search field takes the press"
        );
        assert_eq!(g.click(Point::new(10.0, 10.0)), Some(Outcome::Close));
        g.close(2.0);
        assert_eq!(
            g.click(l.icon_rect(&cell).center()),
            None,
            "a leaving grid takes nothing"
        );
        assert_eq!(g.key(Key::Return, 0.0), None);
    }

    #[test]
    fn the_search_rule_matches_prefixes_segments_initials_and_stripped_names() {
        assert!(matches("Quick Time Player", "qui"));
        assert!(matches("Quick Time Player", "pla"), "a word prefix");
        assert!(matches("QuickTime", "time"), "a camel-case segment");
        assert!(matches("Quick Time Player", "qtp"), "initials");
        assert!(!matches("Quick Time Player", "tp"));
        assert!(matches("Quick Time Player", "quicktime"));
        assert!(matches("Quick Time Player", "time player"));
        assert!(!matches("Quick Time Player", "time nope"));
        assert!(!matches("Quick Time Player", "ick"), "no substring matches");
        assert!(matches("anything", ""));
    }

    #[test]
    fn the_grid_fades_and_scales_in_then_out() {
        let mut g = AppGrid::new(apps(3), &[], 10.0);
        assert_eq!(
            g.visual(10.0),
            Visual {
                alpha: 0.0,
                scale: 1.05
            }
        );
        let mid = g.visual(10.125);
        assert!(mid.alpha < 0.5 && mid.alpha > 0.0);
        assert!((mid.scale - (1.05 - 0.05 * mid.alpha)).abs() < 1e-9);
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
        assert!(!g.is_gone(11.249));
        assert!(g.is_gone(11.25));
        g.restart_entry(11.1);
        assert!(g.is_gone(11.25), "an exit keeps its start");
    }

    #[test]
    fn under_reduce_motion_the_grid_only_fades() {
        let mut g = AppGrid::new(apps(3), &[], 10.0);
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
            }
        );
        g.close(11.0);
        assert_eq!(g.visual(11.125).scale, 1.0);
        assert!(g.is_gone(11.25), "the hide keeps its duration");
    }
}
