//! The launcher's windows as CupertinoKit draws them: the panel (a glass capsule holding the query, which grows to
//! show the sectioned results beside a preview of the selected one and its actions), the transparent catcher that
//! takes a press outside the panel, and the empty window the process keeps mapped while the panel is hidden.
//!
//! The panel is one enclosure for searching and for browsing the applications ([`browse`]): the same query row heads
//! both, and the enclosure changes shape between them as [`crate::motion`] presents it. The panel's window is larger
//! than the shape it heads for by room for its shadow on every side, and while the shape changes it holds both the old
//! and the new one; only the enclosure as it is drawn takes input.
//!
//! The panel's keys never reach the field: [`PanelRoot`] takes them in the capture phase and sends them to the model
//! as [`Input::Key`], so the panel's keyboard table ([`crate::panel`], and the browser's in [`crate::browse`]) holds
//! whatever the field would do with the key; which keys those are follows [`KeyMode`]. Every other key goes on to
//! Kit's [`TextField`], which reports the edited query as [`Input::Query`].

pub mod browse;
pub mod layers;
pub mod modes;
pub mod scope;

use crate::browse::{Arrangement, BrowseLayout, Browser, Category};
use crate::engine::{ItemAction, ItemKind, ResultItem};
use crate::modes::{self as mode_model, Mode};
use crate::motion::{Frame, Header, Shape as PanelShape, CLOSE_BLUR, GROUP_WIDTH};
use crate::nav::Nested;
use crate::panel::{takes_key, Focus, Key as PanelKey, Mods as PanelMods, Panel};
use crate::preview::{Preview, PreviewBody};
use crate::providers::Status;
use crate::scope::{FilesScope, Filter, ScopeLayout};
use cupertino_kit::controls::{
    BareField, Icon, IconDirection, Image, ImageFit, ImageSource, Stack, Text, TextChange,
    TextField, TextRole,
};
use cupertino_kit::core::{
    AccessibleStates, Action, Change, CommandRegistry, Element, Environment, Event, EventCx, Glass,
    Key, Live, MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, Overflow,
    SizeSpec,
};
use cupertino_kit::platform::{
    DecorationMode, KeyEvent, Mods, MouseButton, PointerEvent, WindowSpec,
};
use cupertino_kit::render::{Effect, FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{
    AttributedText, ContentPurpose, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign,
    TextStyle, Wrap,
};
use cupertino_kit::theme::{Color, ColorRole, StateVariant, Surface};
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

/// The `app_id` of the panel's window, which gives it its layer surface.
pub const APP_ID: &str = "cupertino-launcher";
/// The `app_id` of the window that takes a press outside the shown panel.
pub const CATCHER_APP_ID: &str = "cupertino-launcher.catcher";
/// The `app_id` of the window kept mapped while the panel is hidden.
pub const HIDDEN_APP_ID: &str = "cupertino-launcher.hidden";

/// Panel width: the search capsule's.
pub const WIDTH: f64 = 640.0;
/// The query row, which is the whole panel, a capsule, while there are no results. The one panel height off the
/// shape grid ([`SHAPE_GRID`]): the capsule is drawn at its design height.
pub const COLLAPSED_HEIGHT: f64 = 57.0;
/// The least height of the panel while it shows results.
pub const MIN_EXPANDED_HEIGHT: f64 = 430.0;
/// Corner radius of the panel: half the capsule's height, kept as it grows to show results.
pub const CORNER_RADIUS: f64 = COLLAPSED_HEIGHT / 2.0;
/// Between the query and the trailing edge, and the status text's inset.
pub const SIDE_PADDING: f64 = 22.0;
/// Where the query's text starts, from the panel's leading edge, behind every header's symbol.
pub const QUERY_INSET: f64 = 63.0;
/// Between the panel's leading edge and the search field's magnifier's box (its ink starts about 23.2 points in), and
/// between a scope's edge and its symbol.
pub const SEARCH_ICON_INSET: f64 = 19.5;
pub const SCOPE_ICON_INSET: f64 = 21.5;
/// The query's text size.
pub const QUERY_SIZE: f64 = 26.0;
/// Above the first row and below the last.
pub const LIST_PADDING: f64 = 8.0;
/// Width of the results list beside the preview.
pub const LIST_WIDTH: f64 = 288.0;
/// A section heading (derived from its paddings and one caption line).
pub const HEADER_HEIGHT: f64 = 32.0;
/// A result row.
pub const ROW_HEIGHT: f64 = 40.0;
/// The calculator's or a conversion's row, whose value is set large.
pub const CALCULATOR_ROW_HEIGHT: f64 = 64.0;
/// Result icons.
pub const ICON_SIZE: f64 = 32.0;
/// The search symbol's box before the query, whose drawn magnifier is about 23 points across, and a scope's symbol (the
/// applications', the files') in its place.
pub const SEARCH_ICON_SIZE: f64 = 29.0;
pub const SCOPE_ICON_SIZE: f64 = 25.0;
/// Between a row's selection and the list's edges.
pub const SELECTION_INSET: f64 = 8.0;
/// Between the selection's edge and the icon.
pub const CELL_INSET: f64 = 12.0;
/// Corner radius of a row's selection.
pub const SELECTION_RADIUS: f64 = 12.0;
/// Rows, headings included, shown at once.
pub const MAX_ROWS: usize = 9;
/// Distance of the panel's top edge from the top of the output, as a fraction of its height (203 points on an output
/// 982 points tall).
pub const TOP_FRACTION: f64 = 203.0 / 982.0;
/// The query field's placeholder and accessible name.
pub const PLACEHOLDER: &str = "Search";
/// The query row's placeholder while browsing the applications.
pub const BROWSE_PLACEHOLDER: &str = browse::TITLE;
/// The browsing query row's symbol.
pub const BROWSE_SYMBOL: &str = "symbol.applications";
/// The files scope's query row: its placeholder and symbol.
pub const FILES_PLACEHOLDER: &str = crate::scope::TITLE;
pub const FILES_SYMBOL: &str = "symbol.folder";
/// The row that says why nothing shows: its height, and what it says.
pub const STATUS_HEIGHT: f64 = 64.0;
pub const SEARCHING: &str = "Searching";
pub const NO_RESULTS: &str = "No Results";
pub const INCOMPLETE: &str = "Some results could not be searched";
/// The preview a result opens over the list: what it says when the host gave nothing in time.
pub const PREVIEW_UNAVAILABLE: &str = "No preview";
/// The panel's shadow: its blur, how far below the panel it falls and its opacity in each appearance.
pub const SHADOW_BLUR: f64 = 40.0;
pub const SHADOW_OFFSET: f64 = 16.0;
pub const SHADOW_ALPHA_DARK: f32 = 0.5;
pub const SHADOW_ALPHA_LIGHT: f32 = 0.22;
/// Room the window keeps around the panel for its shadow: the blur reaches three halves of `SHADOW_BLUR` past the
/// shape, moved down by the offset.
pub const SHADOW_SIDE: f64 = 1.5 * SHADOW_BLUR;
pub const SHADOW_TOP: f64 = 1.5 * SHADOW_BLUR - SHADOW_OFFSET;
pub const SHADOW_BOTTOM: f64 = 1.5 * SHADOW_BLUR + SHADOW_OFFSET;
/// The thin dark edge just outside the panel, and its opacity in each appearance: near black in the dark one over glass,
/// where it sits against the glass's bright inner rim, so the edge reads as a bevel over any scene.
pub const EDGE_WIDTH: f64 = 0.5;
pub const EDGE_ALPHA_DARK: f32 = 0.93;
/// The dark edge around the opaque fill drawn when there is no glass: no bright rim inside it to answer, so a softer
/// line that only separates the fill from the scene.
pub const EDGE_ALPHA_DARK_OPAQUE: f32 = 0.55;
pub const EDGE_ALPHA_LIGHT: f32 = 0.27;

/// The opacity of the edge outside a panel in the dark or light appearance, over glass or around the opaque fill.
pub fn edge_alpha(dark: bool, glass: bool) -> f32 {
    match (dark, glass) {
        (true, true) => EDGE_ALPHA_DARK,
        (true, false) => EDGE_ALPHA_DARK_OPAQUE,
        (false, _) => EDGE_ALPHA_LIGHT,
    }
}
/// A separator line over the panel's glass: white at this opacity in the dark appearance, black at this one in the
/// light appearance, so it reads as a faint line on any backdrop. Off the glass (the panel's opaque fill) and with
/// Increase Contrast it takes the theme's separator colour.
pub const GLASS_SEPARATOR_DARK: f32 = 0.175;
pub const GLASS_SEPARATOR_LIGHT: f32 = 0.14;
/// The selected result's fill over the panel's glass: white at this opacity in the dark appearance, black at this one
/// in the light appearance, so the backdrop's tint shows through it as it does through the rest of the panel. Off the
/// glass and with Increase Contrast it takes the theme's opaque selection colour.
pub const GLASS_SELECTION_DARK: f32 = 0.14;
pub const GLASS_SELECTION_LIGHT: f32 = 0.1;
/// An action button that is not the chosen one, over the glass: the same thin fill, fainter.
pub const GLASS_CONTROL_DARK: f32 = 0.12;
pub const GLASS_CONTROL_LIGHT: f32 = 0.06;
/// A category filter that is off, over the glass: white at this opacity in each appearance, a pill lighter than the
/// panel around it.
pub const GLASS_LIFT_DARK: f32 = 0.085;
pub const GLASS_LIFT_LIGHT: f32 = 0.3;
/// The preview pane: its padding, the largest picture or icon it shows, the text excerpt's size and lines, and the
/// action buttons' height and gap.
pub const PREVIEW_PADDING: f64 = 16.0;
pub const PREVIEW_IMAGE: Size = Size {
    width: 300.0,
    height: 136.0,
};
pub const PREVIEW_IMAGE_RADIUS: f64 = 6.0;
pub const PREVIEW_ICON_SIZE: f64 = 64.0;
pub const PREVIEW_TEXT_SIZE: f64 = 11.0;
pub const PREVIEW_TEXT_LINES: usize = 9;
pub const PREVIEW_TITLE_SIZE: f64 = 15.0;
pub const PREVIEW_VALUE_SIZE: f64 = 28.0;
/// Between a detail's label and its value.
pub const DETAIL_GAP: f64 = 8.0;
/// The width of a detail's label, which ends where its value starts: the labels end just short of the pane's middle
/// and the values start just past it.
pub const DETAIL_LABEL: f64 = (WIDTH - LIST_WIDTH - 1.0 - 2.0 * PREVIEW_PADDING - DETAIL_GAP) / 2.0;
pub const ACTION_HEIGHT: f64 = 26.0;
pub const ACTION_GAP: f64 = 4.0;
/// Action buttons shown at once.
pub const MAX_ACTIONS: usize = 4;

/// The symbol in front of the query and of the web search row.
pub const SEARCH_SYMBOL: &str = "symbol.magnifyingglass";
/// The raster icon of an application whose entry names none.
pub const GENERIC_APPLICATION_ICON: &str = "application.generic";
/// The raster icon of folder results.
pub const FOLDER_ICON: &str = "folder";
/// The raster icon of file results (they are not typed: the launcher holds no MIME database).
pub const DOCUMENT_ICON: &str = "document.generic";
const RETURN: u32 = 0xff0d;
const KP_ENTER: u32 = 0xff8d;
const ESCAPE: u32 = 0xff1b;
const BACKSPACE: u32 = 0xff08;
const DELETE: u32 = 0xffff;
const TAB: u32 = 0xff09;
const LEFT_TAB: u32 = 0xfe20;
const HOME: u32 = 0xff50;
const LEFT: u32 = 0xff51;
const UP: u32 = 0xff52;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;
const PAGE_UP: u32 = 0xff55;
const PAGE_DOWN: u32 = 0xff56;
const END: u32 = 0xff57;
const SUPER_L: u32 = 0xffeb;
const SUPER_R: u32 = 0xffec;

/// What the windows report to the model.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// A key of the panel's keyboard table.
    Key(PanelKey, PanelMods),
    /// The query field's text changed.
    Query(TextChange),
    /// The pointer moved onto the result at this flat index.
    Hover(usize),
    /// The result at this flat index was clicked, or activated by assistive technology.
    Activate(usize),
    /// Action `.1` of the result at flat index `.0` was clicked, or pressed by assistive technology.
    RunAction(usize, usize),
    /// A press outside the panel, or the panel's window was closed.
    Dismiss,
    /// The panel's window lost the keyboard to another window: the panel closes unless its view options menu, a popup
    /// of its window, took it. The model decides in the order of the window system's events, so a leave that follows
    /// the menu's dismissal in one batch still closes the panel.
    KeyboardLeft,
    /// The pointer moved onto the application at this index of the browser.
    BrowseHover(usize),
    /// The application at this index was pressed, or activated by assistive technology.
    BrowseActivate(usize),
    /// The application at this index was dragged out of the browser, held at this point of its icon.
    BrowseDrag(usize, Point),
    /// A category filter was pressed.
    BrowseCategory(Category),
    /// The wheel over the browser, by this many points down.
    BrowseScroll(f64),
    /// The view options button was pressed.
    ViewOptions,
    /// A view option was chosen.
    Arrange(Arrangement),
    /// The view options menu closed: an option was chosen, or Escape or a press outside dismissed it.
    CloseMenu,
    /// The enlarged preview's close was pressed.
    ClosePreview,
    /// The pointer moved onto the mode button at this index, or off them all.
    ModeHover(Option<usize>),
    /// The mode button at this index was pressed, or activated by assistive technology.
    ModePress(usize),
    /// The pointer moved onto the file at this index of the files scope.
    ScopeHover(usize),
    /// The file at this index was pressed, or activated by assistive technology.
    ScopeActivate(usize),
    /// A files filter was pressed.
    ScopeFilter(Filter),
    /// The primary modifier went down (`true`) or up.
    Primary(bool),
}

/// Which keys the panel takes from the query field now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMode {
    /// Searching, the keys in the field or the result list.
    Search(Focus),
    /// Browsing: the arrows that move along the query (Left, Right, Home, End) and Space stay the field's until an
    /// application, a filter or the options button is `selecting`.
    Browse { selecting: bool },
    /// The files scope: the same keys as the browser's.
    Files { selecting: bool },
    /// The keys are on the mode buttons beside the plain bar: Return, Space, the arrows and Escape are theirs.
    Modes,
    /// The view options menu: it takes every key.
    Menu,
    /// The enlarged preview, opened from the list.
    Preview,
}

impl KeyMode {
    /// Whether the panel takes `key` with `mods`, before the field sees it.
    pub fn takes(self, key: PanelKey, mods: PanelMods) -> bool {
        // the scopes, from wherever the keys are
        if mods.only_primary() && matches!(key, PanelKey::Char('1'..='9')) {
            return true;
        }
        match self {
            KeyMode::Search(focus) => takes_key(key, mods, focus),
            KeyMode::Preview => takes_key(key, mods, Focus::List),
            KeyMode::Menu => true,
            KeyMode::Modes => matches!(
                key,
                PanelKey::Escape
                    | PanelKey::Enter
                    | PanelKey::Tab
                    | PanelKey::Left
                    | PanelKey::Right
                    | PanelKey::Char(' ')
            ),
            KeyMode::Browse { selecting } | KeyMode::Files { selecting } => match key {
                PanelKey::Escape
                | PanelKey::Enter
                | PanelKey::Up
                | PanelKey::Down
                | PanelKey::PageUp
                | PanelKey::PageDown
                | PanelKey::Tab => true,
                PanelKey::Left | PanelKey::Right | PanelKey::Home | PanelKey::End => selecting,
                PanelKey::Char(' ') => selecting && !mods.ctrl && !mods.alt && !mods.primary,
                _ => false,
            },
        }
    }
}

/// The panel key and modifiers of a key press, or `None` for a release and for keys the panel has no name for
/// (modifiers, function keys). Named keys are recognised by their keysym; a key with a command modifier held (Ctrl+N,
/// Alt+1, Super+Y) is named by its lower-case character, anything else by the character the layout produced.
pub fn panel_key(key: &KeyEvent) -> Option<(PanelKey, PanelMods)> {
    if !key.pressed {
        return None;
    }
    let mods = PanelMods {
        ctrl: key.mods.contains(Mods::CTRL),
        alt: key.mods.contains(Mods::ALT),
        shift: key.mods.contains(Mods::SHIFT),
        primary: key.mods.contains(Mods::PRIMARY),
    };
    let named = match key.keysym.0 {
        RETURN | KP_ENTER => Some(PanelKey::Enter),
        ESCAPE => Some(PanelKey::Escape),
        BACKSPACE => Some(PanelKey::Backspace),
        DELETE => Some(PanelKey::Delete),
        LEFT => Some(PanelKey::Left),
        RIGHT => Some(PanelKey::Right),
        UP => Some(PanelKey::Up),
        DOWN => Some(PanelKey::Down),
        HOME => Some(PanelKey::Home),
        END => Some(PanelKey::End),
        PAGE_UP => Some(PanelKey::PageUp),
        PAGE_DOWN => Some(PanelKey::PageDown),
        TAB | LEFT_TAB => Some(PanelKey::Tab),
        _ => None,
    };
    if let Some(named) = named {
        return Some((named, mods));
    }
    // a chord may type a control character (Ctrl+N) or nothing: the keysym names its key then
    let character = key
        .text
        .filter(|c| !c.is_control())
        .or_else(|| keysym_char(key.keysym.0))?;
    let character = if mods.ctrl || mods.alt || mods.primary {
        character.to_lowercase().next()?
    } else {
        character
    };
    Some((PanelKey::Char(character), mods))
}

/// The character a keysym stands for: Latin-1 keysyms are their code points, Unicode keysyms carry theirs.
fn keysym_char(keysym: u32) -> Option<char> {
    let code = match keysym {
        0x20..=0x7e | 0xa0..=0xff => keysym,
        0x0100_0100..=0x0110_ffff => keysym - 0x0100_0000,
        _ => return None,
    };
    char::from_u32(code).filter(|c| !c.is_control())
}

/// One shown row of the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Header(String),
    Item { flat: usize, calculator: bool },
}

impl Row {
    fn height(&self) -> f64 {
        match self {
            Row::Header(_) => HEADER_HEIGHT,
            Row::Item {
                calculator: true, ..
            } => CALCULATOR_ROW_HEIGHT,
            Row::Item { .. } => ROW_HEIGHT,
        }
    }
}

/// The rows the panel shows: [`MAX_ROWS`] of the headings and results from the panel's scroll position.
pub fn visible_rows(panel: &Panel) -> Vec<Row> {
    let mut rows = Vec::new();
    let scroll = panel.scroll_row().min(panel.total_rows().saturating_sub(1));
    let mut row = 0usize;
    let mut flat = 0usize;
    for section in &panel.results().sections {
        if row >= scroll && rows.len() < MAX_ROWS {
            rows.push(Row::Header(section.title.clone()));
        }
        row += 1;
        for item in &section.items {
            if row >= scroll && rows.len() < MAX_ROWS {
                rows.push(Row::Item {
                    flat,
                    calculator: is_value(item.kind),
                });
            }
            row += 1;
            flat += 1;
        }
    }
    rows
}

/// A calculation's or a conversion's result: its value is set large, without an icon.
fn is_value(kind: ItemKind) -> bool {
    matches!(kind, ItemKind::Calculator | ItemKind::Conversion)
}

/// The panel's height: the query row alone without results, else the rows shown below it, never less than
/// [`MIN_EXPANDED_HEIGHT`].
pub fn panel_height(panel: &Panel) -> f64 {
    let rows = visible_rows(panel);
    if rows.is_empty() {
        return COLLAPSED_HEIGHT;
    }
    let list: f64 = rows.iter().map(Row::height).sum();
    (COLLAPSED_HEIGHT + 1.0 + 2.0 * LIST_PADDING + list).max(MIN_EXPANDED_HEIGHT)
}

/// A settled panel's width and height are whole multiples of this many points, so the glass under it ends on whole
/// device pixels at every quarter-step backing scale (1.25, 1.5, 1.75 and 2) and its edges stay sharp.
pub const SHAPE_GRID: f64 = 4.0;

/// `length` rounded up to [`SHAPE_GRID`].
pub fn on_shape_grid(length: f64) -> f64 {
    (length / SHAPE_GRID).ceil() * SHAPE_GRID
}

/// The search field alone.
pub fn compact_shape() -> PanelShape {
    PanelShape::new(WIDTH, COLLAPSED_HEIGHT, CORNER_RADIUS)
}

/// The searching panel's shape: the query row and what shows below it (the results, the enlarged preview, or the row
/// saying why nothing shows).
pub fn search_shape(panel: &Panel, status: &Status, nested: Option<Nested>) -> PanelShape {
    let height = if nested == Some(Nested::Preview) && panel.selected_item().is_some() {
        panel_height(panel).max(MIN_EXPANDED_HEIGHT)
    } else if visible_rows(panel).is_empty() {
        match status {
            Status::Searching | Status::NoResults | Status::Failed(_) => {
                COLLAPSED_HEIGHT + 1.0 + STATUS_HEIGHT
            }
            Status::Idle | Status::Results => COLLAPSED_HEIGHT,
        }
    } else {
        panel_height(panel)
    };
    // the capsule keeps its design height; a panel with something below the field ends on the grid
    let height = if height <= COLLAPSED_HEIGHT {
        COLLAPSED_HEIGHT
    } else {
        on_shape_grid(height)
    };
    PanelShape::new(WIDTH, height, CORNER_RADIUS)
}

/// The browsing panel's shape, `height` points tall.
pub fn browse_shape(height: f64) -> PanelShape {
    PanelShape::new(BrowseLayout::WIDTH, height, BrowseLayout::RADIUS)
}

/// The files scope's shape, `height` points tall.
pub fn scope_shape(height: f64) -> PanelShape {
    PanelShape::new(ScopeLayout::WIDTH, height, ScopeLayout::RADIUS)
}

/// The panel's window for `shape`: the shape and the room around it for its shadow, in whole points (what an animation
/// leaves a hair past a whole point does not round up).
pub fn window_extent(shape: PanelShape) -> Size {
    let whole = |v: f64| ((v * 1000.0).round() / 1000.0).ceil();
    // never narrower than the search field and its buttons: they stand beside each other in the same window
    Size::new(
        whole(shape.width.max(GROUP_WIDTH) + 2.0 * SHADOW_SIDE),
        whole(shape.height + SHADOW_TOP + SHADOW_BOTTOM),
    )
}

/// The most the enclosure is scaled by: the window keeps room for it around the shape.
pub const MAX_SCALE: f64 = 1.25;

/// Where the enclosure lies in a window `extent` large as `frame` presents it, scaled about its centre: its top edge
/// below the shadow's room, and its left edge where the group it is placed by starts, half the frame's anchor width
/// left of the middle. The group is the enclosure alone, or the search field and its buttons.
pub fn enclosure_rect(extent: Size, frame: &Frame) -> Rect {
    let scale = frame.scale.clamp(0.0, MAX_SCALE);
    let (w, h) = (
        frame.shape.width * scale,
        frame.shape.height * frame.scale_y.clamp(0.0, MAX_SCALE),
    );
    let middle = extent.width / 2.0;
    let top = SHADOW_TOP + (frame.shape.height - h) / 2.0;
    Rect::new(middle - frame.anchor * scale / 2.0, top, w, h)
}

/// The enclosure's corner radius as the frame presents it: scaled with its shorter scale, so a capsule stays one.
pub fn enclosure_radius(frame: &Frame) -> f64 {
    frame.shape.radius * frame.scale.min(frame.scale_y).clamp(0.0, MAX_SCALE)
}

/// Where the panel goes on an output `output` points large: horizontally centred, its top edge at [`TOP_FRACTION`]
/// of the height.
pub fn placement(output: Size, height: f64) -> Rect {
    Rect::new(
        ((output.width - WIDTH) / 2.0).round(),
        (output.height * TOP_FRACTION).round(),
        WIDTH,
        height,
    )
}

/// The icon a result shows when the host gave no image for it: its kind's icon (the host normally gives the raster
/// ones as images), none for the calculator.
pub fn fallback_symbol(kind: ItemKind) -> Option<&'static str> {
    match kind {
        ItemKind::Application | ItemKind::ApplicationAction => Some(GENERIC_APPLICATION_ICON),
        ItemKind::Folder => Some(FOLDER_ICON),
        ItemKind::File => Some(DOCUMENT_ICON),
        ItemKind::WebSearch => Some(SEARCH_SYMBOL),
        ItemKind::Calculator | ItemKind::Conversion => None,
    }
}

/// What the panel window draws.
pub struct PanelView<'a> {
    pub panel: &'a Panel,
    /// The revision of the query the model last took from the field.
    pub revision: u64,
    /// How the enclosure is presented now.
    pub frame: Frame,
    /// The window's size.
    pub extent: Size,
    /// Whether the panel is open (not closing): only then does it take input.
    pub open: bool,
    /// Which keys the panel takes from the field.
    pub keys: KeyMode,
    /// What is open over the view.
    pub nested: Option<Nested>,
    /// Why nothing shows, when nothing does.
    pub status: Status,
    /// The status said aloud for the query, once it settled ([`crate::kit::Model::said`]).
    pub said: Option<Status>,
    /// Images of the results' icons by [`ResultItem::id`]; a result without one shows its kind's symbol.
    pub icons: &'a HashMap<String, Vec<ImageSource>>,
    /// The selected result's actions, its own activation first.
    pub actions: &'a [ItemAction],
    /// What the host found out about the selected result, once it has.
    pub preview: Option<&'a Preview>,
    /// The host gave nothing for the selected result in time.
    pub preview_unavailable: bool,
    /// The application browser, while it shows (browsing, or still fading out of a fold).
    pub browser: Option<&'a Browser>,
    /// Images of the applications' icons by desktop id.
    pub app_icons: &'a HashMap<String, Vec<ImageSource>>,
    /// The browser says "No Results" aloud: its filter settled with nothing to show
    /// ([`crate::kit::Model::browse_no_results_said`]).
    pub browse_no_results_said: bool,
    /// The monotonic clock, for the browser's scroll indicator.
    pub now: f64,
    /// The mode buttons there are, in order, and the one the pointer is over and the one the keys are on.
    pub modes: &'a [Mode],
    pub mode_hover: Option<usize>,
    pub mode_focus: Option<usize>,
    /// The files scope, while it shows (it is up, or still fading out of a fold), and the pictures of its files.
    pub scope: Option<&'a FilesScope>,
    pub file_images: &'a HashMap<PathBuf, Vec<ImageSource>>,
    /// The query is shown selected, as the last opening left it.
    pub select_query: bool,
    /// The primary modifier is held: the selected file's or folder's location shows in its preview.
    pub show_location: bool,
    /// The panel's parts draw into compositor layers of their own ([`layers`]), with the same pixels.
    pub layered: bool,
    /// The compositor runs the panel's close on its layers: the panel takes every key, so nothing in it draws again.
    pub handed: bool,
    /// The compositor runs the panel's open on its layers: the field holds its caret, which would draw the content's
    /// layer again.
    pub opening: bool,
}

fn window_spec(title: &str, app_id: &str, size: Size) -> WindowSpec {
    WindowSpec {
        title: title.to_owned(),
        app_id: app_id.to_owned(),
        size,
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: None,
    }
}

fn empty<M: 'static>() -> Element<M> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::default(),
        spacing: 0.0,
        align: Align::Start,
        children: Vec::new(),
    })
}

/// The panel's window's key.
pub fn panel_window_key() -> WindowKey {
    WindowKey(Key::from("panel"))
}

/// The key of the view options menu's popup, a popup of the panel's window.
pub fn options_menu_key() -> WindowKey {
    WindowKey(Key::from("view-options"))
}

/// The panel's window: transparent, [`PanelView::extent`] large, the enclosure inside it at [`enclosure_rect`], which
/// alone takes input while the panel is open (none while it closes).
pub fn panel_window<M>(view: &PanelView<'_>) -> Window<M>
where
    M: From<Input> + Clone + 'static,
{
    let rect = enclosure_rect(view.extent, &view.frame);
    let mut spec = window_spec("Launcher", APP_ID, view.extent);
    let buttons = mode_buttons(view, &rect);
    spec.input_region = Some(if view.open {
        std::iter::once(rect)
            .chain(
                buttons
                    .iter()
                    .filter(|(_, active, _)| *active)
                    .map(|(r, _, _)| *r),
            )
            .collect()
    } else {
        Vec::new()
    });
    let mut root = vec![panel_content(view).key("panel")];
    if view.layered {
        root.push(
            Element::new(layers::EdgeLayer {
                key: layers::EDGE_LAYER,
                rect: layers::edge_layer_rect(view.extent, &view.frame),
                shape: rect,
                radius: enclosure_radius(&view.frame),
                joined: mode_rects(view, &rect)
                    .into_iter()
                    .map(|r| (r, r.width() / 2.0))
                    .collect(),
                focus: false,
                children: Vec::new(),
            })
            .key("panel-edge"),
        );
    }
    for (i, (_, _, button)) in buttons.into_iter().enumerate() {
        let ModeParts { button, face } = button;
        root.push(button.key(format!("mode-{i}")));
        if let Some(face) = face {
            root.push(face.key(format!("mode-{i}-face")));
        }
    }
    Window {
        key: panel_window_key(),
        spec,
        shortcuts: CommandRegistry::new(),
        on_close: Some(M::from(Input::Dismiss)),
        background: WindowBackground::None,
        content: Element::new(Plain::absolute(root)),
        popup: None,
        menus: None,
    }
}

/// A mode button's elements: the button, and while the panel is drawn as layers its face apart.
struct ModeParts<M> {
    button: Element<M>,
    face: Option<Element<M>>,
}

/// The mode buttons as the frame presents them: each with its rectangle in the window, whether it takes input and its
/// elements. They stand where the group's layout puts them (the search field's end and the gaps), scaled with the
/// enclosure about the window's middle, and are none once they merged into the field.
fn mode_buttons<M>(view: &PanelView<'_>, enclosure: &Rect) -> Vec<(Rect, bool, ModeParts<M>)>
where
    M: From<Input> + Clone + 'static,
{
    let frame = &view.frame;
    let rects = mode_rects(view, enclosure);
    let active = view.open && frame.modes.spread > 0.5;
    let field = (*enclosure, enclosure_radius(frame));
    rects
        .iter()
        .enumerate()
        .map(|(index, &rect)| {
            let joined = std::iter::once(field)
                .chain(
                    rects
                        .iter()
                        .enumerate()
                        .filter(|&(i, _)| i != index)
                        .map(|(_, r)| (*r, r.width() / 2.0)),
                )
                .collect::<Vec<_>>();
            let mode = view.modes[index];
            let focus = view.mode_focus == Some(index);
            let symbol = modes::glyph(mode, frame.modes.sharp);
            let (children, face) = if view.layered {
                let face = layers::button_face(index, rect, joined.clone(), focus, symbol);
                (Vec::new(), Some(face))
            } else {
                (vec![symbol], None)
            };
            let button = Element::new(modes::ModeButton {
                mode,
                index,
                rect,
                alpha: frame.alpha,
                sharp: frame.modes.sharp,
                focus,
                active,
                joined,
                layered: view.layered,
                children,
            });
            (rect, active, ModeParts { button, face })
        })
        .collect()
}

/// The mode buttons' rectangles in the window as the frame presents them, scaled with the enclosure about the window's
/// middle; none once they merged into the field.
fn mode_rects(view: &PanelView<'_>, enclosure: &Rect) -> Vec<Rect> {
    let count = view.frame.modes.count.min(view.modes.len());
    mode_rects_of(view.extent, &view.frame, count, enclosure)
}

/// The rectangles of `count` mode buttons in a window `extent` large as `frame` presents them beside the enclosure at
/// `enclosure`.
pub(crate) fn mode_rects_of(
    extent: Size,
    frame: &Frame,
    count: usize,
    enclosure: &Rect,
) -> Vec<Rect> {
    let count = count.min(frame.modes.count);
    let scale = frame.scale.clamp(0.0, MAX_SCALE);
    let middle = extent.width / 2.0;
    let centre_y = enclosure.y() + enclosure.height() / 2.0;
    (0..count)
        .map(|index| {
            let place = mode_model::button_rect(index, count, frame.modes.spread, COLLAPSED_HEIGHT);
            let side = place.width() * frame.scale_y.clamp(0.0, MAX_SCALE);
            Rect::new(
                middle + (place.x() - frame.anchor / 2.0) * scale,
                centre_y - side / 2.0,
                side,
                side,
            )
        })
        .collect()
}

/// The thin dark edge just outside the rounded `rect`, black at `alpha`, left out inside each of the `joined` shapes
/// (other shapes of the same joined glass, in the same coordinates) that come within the glass's join spacing: where
/// the shapes join into one, only the outline of the whole shows, not each part's.
pub(crate) fn edge_outside(
    cx: &mut PaintCx<'_>,
    rect: Rect,
    radius: f64,
    alpha: f32,
    joined: &[(Rect, f64)],
) {
    let near = edge_cuts(rect, joined);
    let room = rect.outset(EDGE_WIDTH + 1.0);
    for (other, r) in &near {
        let mut cut = rounded_path(room.union(other), 0.0);
        cut.extend(rounded_path(
            *other,
            r.min(other.width().min(other.height()) / 2.0),
        ));
        cx.push(PaintOp::PushClip(Shape::Path {
            verbs: cut,
            fill_rule: FillRule::EvenOdd,
        }));
    }
    cx.push(PaintOp::Stroke {
        shape: Shape::RoundedRect {
            rect: rect.outset(EDGE_WIDTH / 2.0),
            radius: radius + EDGE_WIDTH / 2.0,
        },
        width: EDGE_WIDTH,
        paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, alpha)),
    });
    for _ in &near {
        cx.push(PaintOp::PopClip);
    }
}

/// Which of the `joined` shapes the edge round `rect` is left out inside: those that come close enough for the glass
/// to join them into one shape with it.
pub(crate) fn edge_cuts(rect: Rect, joined: &[(Rect, f64)]) -> Vec<&(Rect, f64)> {
    let reach = rect.outset(modes::JOIN_SPACING + EDGE_WIDTH);
    joined
        .iter()
        .filter(|(r, _)| r.intersects(&reach))
        .collect()
}

/// The transparent window over the output, below the panel, that closes it on a press.
pub fn catcher_window<M>() -> Window<M>
where
    M: From<Input> + Clone + 'static,
{
    Window {
        key: WindowKey(Key::from("catcher")),
        spec: window_spec("Close the launcher", CATCHER_APP_ID, Size::new(1.0, 1.0)),
        shortcuts: CommandRegistry::new(),
        on_close: Some(M::from(Input::Dismiss)),
        // shown without a frame of the output's size: it draws nothing
        background: WindowBackground::INPUT_ONLY,
        content: Element::new(Catcher),
        popup: None,
        menus: None,
    }
}

/// The window kept mapped while the panel is hidden: one transparent point that takes no input. Each `generation`
/// is a new window; when the server closes one (its output went away) `on_close` arrives and the model asks for the
/// next generation, which the server puts on an output still there.
pub fn hidden_window<M: Clone + 'static>(generation: u64, on_close: M) -> Window<M> {
    Window {
        key: WindowKey(Key::from(format!("hidden-{generation}"))),
        spec: window_spec("Launcher", HIDDEN_APP_ID, Size::new(1.0, 1.0)),
        shortcuts: CommandRegistry::new(),
        on_close: Some(on_close),
        background: WindowBackground::None,
        content: empty(),
        popup: None,
        menus: None,
    }
}

/// The enclosure: the query row, then below a separator the results beside the selected one's preview (or the
/// enlarged preview, or why nothing shows), or the application browser. Its content keeps the shape the enclosure
/// heads for while the enclosure scales, cut at the enclosure's edge.
pub fn panel_content<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let frame = &view.frame;
    let rect = enclosure_rect(view.extent, frame);
    let browsing = frame.header == Header::Browse;
    let filing = frame.header == Header::Files;
    let hint = match frame.header {
        Header::Search => PLACEHOLDER,
        Header::Browse => BROWSE_PLACEHOLDER,
        Header::Files => FILES_PLACEHOLDER,
    };
    let selection: Option<Range<usize>> = view
        .select_query
        .then(|| 0..panel.query().len())
        .filter(|r| !r.is_empty());
    let field = Element::new(BareField {
        field: TextField {
            label: hint.to_owned(),
            placeholder: hint.to_owned(),
            value: panel.query().to_owned(),
            revision: view.revision,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: Some(Box::new(|change| M::from(Input::Query(change)))),
            on_submit: None,
            selection,
            initial: true,
        },
        size: QUERY_SIZE,
        // while the compositor runs the close, the caret's blink would draw the content's layer again
        hold_caret: view.handed || view.opening,
    })
    .key("query")
    .fill();
    // the magnifier sits a little further in than a scope's wider symbol; the text starts at the same place behind both
    let (icon_inset, icon_size) = match frame.header {
        Header::Search => (SEARCH_ICON_INSET, SEARCH_ICON_SIZE),
        Header::Browse | Header::Files => (SCOPE_ICON_INSET, SCOPE_ICON_SIZE),
    };
    let mut row = vec![
        Element::new(Icon {
            name: match frame.header {
                Header::Search => SEARCH_SYMBOL,
                Header::Browse => BROWSE_SYMBOL,
                Header::Files => FILES_SYMBOL,
            }
            .to_owned(),
            size: icon_size,
            direction: IconDirection::Fixed,
            label: None,
            tint: Some(ColorRole::SecondaryText),
        })
        .key(match frame.header {
            Header::Search => "search-symbol",
            Header::Browse => "browse-symbol",
            Header::Files => "files-symbol",
        }),
        field,
    ];
    if let Some(index) = view.mode_hover.filter(|_| frame.modes.count > 0) {
        row.push(
            Element::new(modes::Chips {
                digit: mode_model::digit(index),
            })
            .key("mode-chips"),
        );
    }
    if let Some(browser) = view.browser.filter(|_| browsing) {
        row.push(
            Element::new(browse::OptionsButton {
                focus: browser.zone() == crate::browse::Zone::Options
                    && view.nested != Some(Nested::ViewOptions),
                expanded: view.nested == Some(Nested::ViewOptions),
            })
            .key(browse::OPTIONS_BUTTON),
        );
    }
    let showing_browser = view.browser.filter(|_| browsing || frame.content > 0.0);
    let showing_scope = view
        .scope
        .filter(|_| (filing || frame.content > 0.0) && showing_browser.is_none());
    // over a scope the enclosure fades its two parts one by one rather than as one layer: they do not overlap,
    // so the look is the same, and the scope's own fade then needs no layer inside another
    let split = showing_browser.is_some() || showing_scope.is_some();
    let query_row = Element::new(Bar {
        height: COLLAPSED_HEIGHT,
        leading: icon_inset,
        spacing: QUERY_INSET - icon_inset - icon_size,
        alpha: if split { frame.alpha } else { 1.0 },
        children: row,
    })
    .key("query-row");
    let mut children = vec![query_row];
    if let Some(browser) = showing_browser {
        children.push(
            browse::body(
                browser,
                view.app_icons,
                view.browse_no_results_said,
                frame.content,
                frame.alpha,
                view.now,
            )
            .key("browser"),
        );
    } else if let Some(scope) = showing_scope {
        children.push(
            scope::body(
                scope,
                view.file_images,
                frame.content,
                frame.alpha,
                view.now,
            )
            .key("scope"),
        );
    } else {
        children.extend(search_body(view));
    }
    // while the panel scales in or out its content keeps its size, cut off by the glass, and moves on whole device
    // pixels, as one picture, as the content's layer does when the compositor runs the open
    let scaled = frame.scale != 1.0 || frame.scale_y != 1.0;
    let inner = Element::new(PanelRoot {
        keys: view.keys,
        alpha: frame.alpha,
        size: Size::new(frame.shape.width, frame.shape.height),
        offset: (rect.width() - frame.shape.width) / 2.0,
        layer: view.layered,
        handed: view.handed,
        scaled,
        children,
    });
    let mut children = vec![inner.key("content")];
    if view.layered {
        children.insert(
            0,
            Element::new(layers::GlassLayer {
                rect: Rect::new(0.0, 0.0, rect.width(), rect.height()),
                look: layers::glass_look(frame),
                key: layers::GLASS_LAYER,
            })
            .key("glass"),
        );
    }
    Element::new(Enclosure {
        rect,
        radius: enclosure_radius(frame),
        alpha: frame.alpha,
        blur: frame.blur,
        joins: frame.modes.count > 0,
        joined: mode_rects(view, &rect)
            .into_iter()
            .map(|r| (r, r.width() / 2.0))
            .collect(),
        split,
        layered: view.layered,
        crops: scaled,
        content: Size::new(frame.shape.width, frame.shape.height),
        children,
    })
}

/// What the searching panel shows below its query row: the results beside the selected one's preview, the enlarged
/// preview over them, or the row saying why nothing shows.
fn search_body<M>(view: &PanelView<'_>) -> Vec<Element<M>>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let rows = visible_rows(panel);
    let separator = || {
        Element::new(Hairline {
            axis: Axis::Horizontal,
        })
        .key("separator")
    };
    if view.nested == Some(Nested::Preview) && panel.selected_item().is_some() {
        return vec![separator(), enlarged_preview(view).key("enlarged-preview")];
    }
    if rows.is_empty() {
        let Some(text) = status_text(&view.status) else {
            return Vec::new();
        };
        // the row says it at once; aloud it is said once the query settled, by a node of its own
        let said = view.said.as_ref().and_then(|s| {
            Some(StatusAnnouncer {
                text: status_text(s)?,
                failed: matches!(s, Status::Failed(_)),
            })
        });
        let mut body = vec![
            separator(),
            Element::new(StatusRow {
                text,
                failed: matches!(view.status, Status::Failed(_)),
                live: false,
            })
            .key("status"),
        ];
        body.extend(said.map(|s| Element::new(s).key("status-said")));
        return body;
    }
    let body = vec![
        results(view, &rows).key("results"),
        Element::new(Hairline {
            axis: Axis::Vertical,
        })
        .key("divider"),
        preview_pane(view).key("preview"),
    ];
    vec![
        separator(),
        Element::new(Plain::row(body, Align::Stretch)).key("body"),
    ]
}

/// The selected result's preview over the whole body: its picture as large as it can be shown, its name and what is
/// known of it, and its actions; or that the host gave nothing in time.
fn enlarged_preview<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let mut children = vec![preview_pane(view).key("pane")];
    if view.preview_unavailable {
        children.insert(
            0,
            Element::new(StatusRow {
                text: PREVIEW_UNAVAILABLE,
                failed: false,
                live: true,
            })
            .key("unavailable"),
        );
    }
    Element::new(Plain::sheet(children))
}

/// The shown rows: a list of the sections, each a group named by its heading over its results.
fn results<M>(view: &PanelView<'_>, rows: &[Row]) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let mut sections: Vec<Element<M>> = Vec::new();
    let mut current: Option<(String, Vec<Element<M>>)> = None;
    let flush = |sections: &mut Vec<Element<M>>, current: Option<(String, Vec<Element<M>>)>| {
        if let Some((title, children)) = current {
            let key = format!("section:{title}");
            sections.push(Element::new(SectionGroup { title, children }).key(key));
        }
    };
    for row in rows {
        match row {
            Row::Header(title) => {
                flush(&mut sections, current.take());
                current = Some((
                    title.clone(),
                    vec![Element::new(Quiet::heading(title)).key("heading")],
                ));
            }
            &Row::Item { flat, calculator } => {
                let Some(item) = panel.results().get(flat) else {
                    continue;
                };
                let element = result_row(view, item, flat, calculator, flat == panel.selected());
                match &mut current {
                    Some((_, children)) => children.push(element),
                    // the list scrolled past this section's heading
                    None => {
                        current = Some((
                            panel
                                .results()
                                .locate(flat)
                                .and_then(|(s, _)| panel.results().sections.get(s))
                                .map_or_else(String::new, |s| s.title.clone()),
                            vec![element],
                        ))
                    }
                }
            }
        }
    }
    flush(&mut sections, current);
    Element::new(ResultList {
        count: panel.results().len(),
        children: sections,
    })
}

/// The selected result's preview: a picture of it, its name and what is known of it, and its actions at the foot.
fn preview_pane<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let Some(item) = panel.selected_item() else {
        return Element::new(Plain::pane(Vec::new()));
    };
    let preview = view.preview;
    let mut children: Vec<Element<M>> = Vec::new();
    if is_value(item.kind) {
        children.push(
            Element::new(Label::new(&item.title, PREVIEW_VALUE_SIZE, 500).centred(2)).key("value"),
        );
        children.push(
            Element::new(
                Label::new(item.subtitle.trim_end_matches(" ="), 13.0, 400)
                    .secondary()
                    .centred(2),
            )
            .key("expression"),
        );
    } else {
        children.push(preview_visual(view, item, preview).key("visual"));
        children.push(
            Element::new(Label::new(&item.title, PREVIEW_TITLE_SIZE, 600).centred(2)).key("name"),
        );
        let kind = match (preview, item.kind) {
            (Some(p), _) if !p.kind.is_empty() => p.kind.clone(),
            (_, ItemKind::Application) => "Application".to_owned(),
            (_, ItemKind::ApplicationAction) => item.subtitle.clone(),
            (_, ItemKind::WebSearch) => "Search the web".to_owned(),
            _ => String::new(),
        };
        if !kind.is_empty() {
            children.push(
                Element::new(Label::new(&kind, 12.0, 400).secondary().centred(1)).key("kind"),
            );
        }
        if let Some(p) = preview {
            for (label, value) in &p.details {
                children.push(
                    Element::new(Plain::detail(vec![
                        Element::new(
                            Label::new(label, 11.0, 400)
                                .secondary()
                                .ending(DETAIL_LABEL),
                        ),
                        Element::new(Label::new(value, 11.0, 400).lines(1)).fill(),
                    ]))
                    .key(format!("detail:{label}")),
                );
            }
        } else if matches!(item.kind, ItemKind::Application) && !item.subtitle.is_empty() {
            children.push(
                Element::new(Label::new(&item.subtitle, 11.0, 400).secondary().centred(2))
                    .key("description"),
            );
        }
    }
    // holding the primary modifier shows where a file or folder is
    if let (true, crate::engine::Action::OpenPath(path)) = (view.show_location, &item.action) {
        children.push(
            Element::new(
                Label::new(&crate::engine::home_relative(path), 11.0, 400)
                    .secondary()
                    .centred(2),
            )
            .key("location"),
        );
    }
    children.push(Element::new(Plain::spacer()));
    children.push(action_list(view).key("actions"));
    Element::new(Plain::pane(children))
}

/// The picture at the top of the preview: the file's own, the start of its text, or the result's icon.
fn preview_visual<M: 'static>(
    view: &PanelView<'_>,
    item: &ResultItem,
    preview: Option<&Preview>,
) -> Element<M> {
    match preview.map(|p| &p.body) {
        Some(PreviewBody::Image { sources, size }) => {
            return Element::new(Image {
                sources: sources.clone(),
                size: *size,
                fit: ImageFit::Contain,
                radius: PREVIEW_IMAGE_RADIUS,
                label: None,
            })
        }
        Some(PreviewBody::Text(text)) => return Element::new(TextBlock { text: text.clone() }),
        _ => {}
    }
    let size = Size::new(PREVIEW_ICON_SIZE, PREVIEW_ICON_SIZE);
    let sources = preview
        .map(|p| &p.icon)
        .filter(|icon| !icon.is_empty())
        .or_else(|| view.icons.get(&item.id));
    match (sources, fallback_symbol(item.kind)) {
        (Some(sources), _) => Element::new(Image {
            sources: sources.clone(),
            size,
            fit: ImageFit::Contain,
            radius: 0.0,
            label: None,
        }),
        (None, Some(symbol)) => Element::new(Icon {
            name: symbol.to_owned(),
            size: PREVIEW_ICON_SIZE,
            direction: IconDirection::Fixed,
            label: None,
            tint: (item.kind == ItemKind::WebSearch).then_some(ColorRole::SecondaryText),
        }),
        (None, None) => Element::new(Plain::spacer()),
    }
}

/// The selected result's actions as buttons, the chosen one filled; at most [`MAX_ACTIONS`] of them, the chosen one
/// always among them.
fn action_list<M>(view: &PanelView<'_>) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let panel = view.panel;
    let chosen = panel.action();
    let first = (chosen + 1).saturating_sub(MAX_ACTIONS);
    let buttons = view
        .actions
        .iter()
        .enumerate()
        .skip(first)
        .take(MAX_ACTIONS)
        .map(|(index, action)| {
            Element::new(ActionButton {
                flat: panel.selected(),
                index,
                label: action.label.clone(),
                chosen: index == chosen,
            })
            .key(format!("action:{index}"))
        })
        .collect();
    Element::new(ActionList {
        count: view.actions.len(),
        children: buttons,
    })
}

fn text<M: 'static>(text: &str, role: TextRole, secondary: bool) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(text, TextStyle::new(13.0)),
        role,
        wrap: false,
        secondary,
        selected: false,
    })
}

fn result_row<M>(
    view: &PanelView<'_>,
    item: &ResultItem,
    flat: usize,
    calculator: bool,
    selected: bool,
) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let icon = match (view.icons.get(&item.id), fallback_symbol(item.kind)) {
        (Some(sources), _) => Some(Element::new(Image {
            sources: sources.clone(),
            size: Size::new(ICON_SIZE, ICON_SIZE),
            fit: ImageFit::Contain,
            radius: 0.0,
            label: None,
        })),
        (None, Some(symbol)) => Some(Element::new(Icon {
            name: symbol.to_owned(),
            size: ICON_SIZE,
            direction: IconDirection::Fixed,
            label: None,
            tint: (item.kind == ItemKind::WebSearch).then_some(ColorRole::SecondaryText),
        })),
        // the calculator's value starts at the icon's place
        (None, None) => None,
    };
    let title_role = if calculator {
        TextRole::Title
    } else {
        TextRole::Body
    };
    let mut lines = vec![text(&item.title, title_role, false)];
    if !item.subtitle.is_empty() {
        lines.push(text(&item.subtitle, TextRole::Caption, true));
    }
    let label = Element::new(Quiet::lines(lines)).fill();
    Element::new(ResultRow {
        flat,
        title: item.title.clone(),
        subtitle: item.subtitle.clone(),
        height: if calculator {
            CALCULATOR_ROW_HEIGHT
        } else {
            ROW_HEIGHT
        },
        selected,
        children: icon.into_iter().chain([label]).collect(),
    })
    .key(format!("item:{}", item.id))
}

/// A container's leaf callbacks: layout sizes it.
fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn column(insets: Insets) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        insets,
        ..LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }
}

/// The enclosure as it is drawn: the panel's glass (or the panel material's fill without it), its shadow outside its
/// shape and its thin dark edge, at the presented opacity and scale; its content, laid out at the unscaled shape and
/// centred, is cut at its edge.
struct Enclosure<M> {
    rect: Rect,
    radius: f64,
    alpha: f64,
    /// How blurred its content is, from 0 to 1 (while it closes).
    blur: f64,
    /// Its glass joins the mode buttons' into one shape where they come close.
    joins: bool,
    /// The mode buttons, in the window, with their corner radii: its edge is not drawn inside them.
    joined: Vec<(Rect, f64)>,
    /// Its parts fade themselves at `alpha` ([`Bar`] and the browser's body): the enclosure draws its own edge (and
    /// without glass its shadow and fill) at `alpha` and lays no layer over them.
    split: bool,
    /// Its glass, edge and content are layers of their own ([`layers`]): it draws nothing itself.
    layered: bool,
    /// It scales in or out: its content keeps its size past its edge, cut off there ([`Overflow::Crop`]).
    crops: bool,
    content: Size,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Enclosure<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width().max(0.0)),
            height: SizeSpec::Fixed(self.rect.height().max(0.0)),
            position: self.rect.origin,
            overflow: if self.crops {
                Overflow::Crop
            } else {
                Overflow::Clip
            },
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.rect, self.content, self.crops) != (old.rect, old.content, old.crops) {
            Change::Layout
        } else if (
            self.radius,
            self.alpha,
            self.split,
            self.blur,
            self.joins,
            self.layered,
        ) != (
            old.radius,
            old.alpha,
            old.split,
            old.blur,
            old.joins,
            old.layered,
        ) || self.joined != old.joined
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let alpha = self.alpha.clamp(0.0, 1.0);
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = self.radius.min(size.height / 2.0).max(0.0);
        let theme = cx.env().theme.clone();
        let dark = theme.preferences.appearance.is_dark();
        // the compositor draws the large panel's glass and its shadow; a window without glass draws both itself
        let mut material = Glass::large_panel().radius(radius).alpha(alpha);
        if self.joins {
            material = material.container(modes::JOIN_CONTAINER, modes::JOIN_SPACING);
        }
        if self.layered {
            // the glass layer draws the glass and the edge layer the edge; the content lies on the glass as in place
            cx.layer_glass(material);
            return;
        }
        let glass = cx.glass(material);
        // what the enclosure's own drawing is multiplied by: the fade itself when its parts fade one by one
        let own = if self.split { alpha as f32 } else { 1.0 };
        let blur = self.blur.clamp(0.0, 1.0) * CLOSE_BLUR;
        let fade = if self.split { 1.0 } else { alpha as f32 };
        if fade < 1.0 || blur > 0.05 {
            let effects = if blur > 0.05 {
                vec![Effect::Blur { radius: blur }]
            } else {
                Vec::new()
            };
            cx.set_group(fade, effects);
        }
        let shape = Shape::RoundedRect { rect, radius };
        if !glass {
            let shadow = if dark {
                SHADOW_ALPHA_DARK
            } else {
                SHADOW_ALPHA_LIGHT
            };
            // the shadow is cast by an opaque copy of the shape that the clip hides, so it never darkens the panel
            let mut ring = rounded_path(rect.outset(2.0 * SHADOW_SIDE), 0.0);
            ring.extend(rounded_path(rect, radius));
            cx.push(PaintOp::PushClip(Shape::Path {
                verbs: ring,
                fill_rule: FillRule::EvenOdd,
            }));
            cx.push(PaintOp::PushLayer {
                opacity: own,
                effects: vec![Effect::DropShadow {
                    offset: Point::new(0.0, SHADOW_OFFSET),
                    blur: SHADOW_BLUR,
                    color: Color::new(0.0, 0.0, 0.0, shadow),
                }],
            });
            cx.push(PaintOp::Fill {
                shape: shape.clone(),
                paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)),
            });
            cx.push(PaintOp::PopLayer);
            cx.push(PaintOp::PopClip);
            if let Some(fill) = theme.surface(Surface::Menu).fill {
                cx.push(PaintOp::Fill {
                    shape,
                    paint: Paint::Solid(Color::new(fill.r, fill.g, fill.b, fill.a * own)),
                });
            }
        }
        let edge = edge_alpha(dark, glass);
        let joined: Vec<(Rect, f64)> = self
            .joined
            .iter()
            .map(|(r, radius)| (r.translate(-self.rect.x(), -self.rect.y()), *radius))
            .collect();
        edge_outside(cx, rect, radius, edge * own, &joined);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The panel's content: takes the panel's keys before the query field does (which keys follows its [`KeyMode`]), and
/// reports when its window loses the keyboard.
struct PanelRoot<M> {
    keys: KeyMode,
    alpha: f64,
    size: Size,
    /// Where it lies in the enclosure: centred across it.
    offset: f64,
    /// It is the content's own compositor layer ([`layers::CONTENT_LAYER`]).
    layer: bool,
    /// The compositor runs the close ([`PanelView::handed`]): a key, its release included, reaches nothing inside, as
    /// a field would restart its caret's blink and draw again.
    handed: bool,
    /// The panel scales in or out: the content lies on whole device pixels ([`Widget::on_pixels`]).
    scaled: bool,
    children: Vec<Element<M>>,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for PanelRoot<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width.max(0.0)),
            height: SizeSpec::Fixed(self.size.height.max(0.0)),
            position: Point::new(self.offset, 0.0),
            ..column(Insets::default())
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.size, self.offset, self.scaled) != (old.size, old.offset, old.scaled) {
            Change::Layout
        } else if (self.alpha, self.keys, self.layer) != (old.alpha, old.keys, old.layer) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Key(..) if self.handed => Propagation::Stop,
            // the primary modifier alone, held and let go: the preview shows a file's location meanwhile
            Event::Key(key, _)
                if matches!(key.keysym.0, SUPER_L | SUPER_R)
                    && (cx.phase() == Phase::Capture || cx.node() == cx.target()) =>
            {
                cx.emit(M::from(Input::Primary(key.pressed)));
                Propagation::Continue
            }
            Event::Key(key, _) if cx.phase() == Phase::Capture || cx.node() == cx.target() => {
                match panel_key(key) {
                    Some((k, mods)) if self.keys.takes(k, mods) => {
                        cx.emit(M::from(Input::Key(k, mods)));
                        Propagation::Stop
                    }
                    _ => Propagation::Continue,
                }
            }
            // another window took the keyboard: the model knows whether that was the view options menu
            Event::FocusWithin { gained: false, .. } if !cx.window_active() => {
                cx.emit(M::from(Input::KeyboardLeft));
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    /// The query field is where the panel's keys go: a press on a button or in the content leaves it editing.
    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.keeps_editing();
    }

    /// While the enclosure scales about its middle the content moves as one picture on whole device pixels, as the
    /// compositor moves its layer when it runs the open, rather than its text snapping to the pixels line by line
    /// while its shapes and images fall between them.
    fn on_pixels(&self, _: &()) -> bool {
        self.scaled
    }

    fn surface_layer(&self, _: &()) -> Option<cupertino_kit::core::SurfaceLayerMark> {
        self.layer.then_some(cupertino_kit::core::SurfaceLayerMark {
            key: layers::CONTENT_LAYER,
            look: cupertino_kit::platform::SurfaceLayerLook::default(),
        })
    }
}

/// The colour of a separator line where `cx` paints: translucent over the panel's glass, the theme's separator over
/// the opaque fill and with Increase Contrast.
pub(crate) fn separator_color(cx: &PaintCx<'_>) -> Option<Color> {
    thin_fill(
        cx,
        GLASS_SEPARATOR_DARK,
        GLASS_SEPARATOR_LIGHT,
        ColorRole::Separator,
    )
}

/// The fill of a selected result, row or cell where `cx` paints: a thin fill of the label's tone over the panel's
/// glass, the theme's selection colour over the opaque fill and with Increase Contrast.
pub(crate) fn selection_color(cx: &PaintCx<'_>) -> Option<Color> {
    thin_fill(
        cx,
        GLASS_SELECTION_DARK,
        GLASS_SELECTION_LIGHT,
        ColorRole::SecondarySelectedControlFill,
    )
}

/// The fill of a category filter that is off where `cx` paints: over the panel's glass a white lift, lighter than the
/// panel around it in either appearance; over the opaque fill and with Increase Contrast none (the caller's own).
pub(crate) fn lift_color(cx: &PaintCx<'_>) -> Option<Color> {
    let appearance = cx.env().theme.preferences.appearance;
    (cx.on_glass() && !appearance.is_high_contrast()).then(|| {
        let a = if appearance.is_dark() {
            GLASS_LIFT_DARK
        } else {
            GLASS_LIFT_LIGHT
        };
        Color::new(1.0, 1.0, 1.0, a)
    })
}

/// Over the glass (and without Increase Contrast), white at `dark` opacity in the dark appearance and black at `light`
/// in the light one; elsewhere the theme's `role`.
fn thin_fill(cx: &PaintCx<'_>, dark: f32, light: f32, role: ColorRole) -> Option<Color> {
    let theme = &cx.env().theme;
    let appearance = theme.preferences.appearance;
    if cx.on_glass() && !appearance.is_high_contrast() {
        Some(if appearance.is_dark() {
            Color::new(1.0, 1.0, 1.0, dark)
        } else {
            Color::new(0.0, 0.0, 0.0, light)
        })
    } else {
        theme.colors.get(role, StateVariant::NORMAL)
    }
}

/// A hairline across the panel along `axis`, in [`separator_color`]: under the query row and between the results and
/// the preview.
struct Hairline {
    axis: Axis,
}

impl<M: 'static> Widget<M> for Hairline {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        let (width, height) = match self.axis {
            Axis::Horizontal => (SizeSpec::Fill, SizeSpec::Fixed(1.0)),
            Axis::Vertical => (SizeSpec::Fixed(1.0), SizeSpec::Fill),
        };
        LayoutSpec {
            width,
            height,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.axis != old.axis {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        if let Some(color) = separator_color(cx) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// What the row saying why nothing shows says of `status`; `None` when it does not show.
pub fn status_text(status: &Status) -> Option<&'static str> {
    match status {
        Status::Searching => Some(SEARCHING),
        Status::NoResults => Some(NO_RESULTS),
        Status::Failed(_) => Some(INCOMPLETE),
        Status::Idle | Status::Results => None,
    }
}

/// Why nothing shows, centred in its row: still searching, nothing found, or that a source failed (so what shows may be
/// incomplete); or that the preview did not come in time. A search's status is read where it is, and
/// [`StatusAnnouncer`] says it aloud once the query settled; the preview's is said as it shows (`live`).
struct StatusRow {
    text: &'static str,
    failed: bool,
    live: bool,
}

impl<M: 'static> Widget<M> for StatusRow {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(STATUS_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.text, self.failed, self.live) != (old.text, old.failed, old.live) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, STATUS_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let theme = &cx.env().theme;
        let color = theme
            .colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL)
            .unwrap_or(Color::new(0.0, 0.0, 0.0, 0.5));
        let style = TextStyle {
            weight: FontWeight(500),
            ..TextStyle::new(15.0)
        };
        let paragraph = ParagraphStyle {
            wrap: Wrap::None,
            max_lines: Some(1),
            ellipsis: true,
            align: TextAlign::Center,
            ..ParagraphStyle::default()
        };
        let Ok(text) = cx
            .env()
            .text
            .paragraph(&AttributedText::plain(self.text, style), &paragraph)
        else {
            return;
        };
        let layout = text.layout(Some((size.width - 2.0 * SIDE_PADDING).max(0.0) as f32));
        let h = layout.size().height;
        glyphs(
            cx,
            &layout,
            Point::new(SIDE_PADDING, (size.height - h) / 2.0),
            color,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.to_owned());
        if self.live {
            cx.live(Live::Polite);
        }
    }
}

/// The status said aloud for a settled query: a live region of no size that draws nothing, added once per query with
/// what it says, so a screen reader says it once. A failure is said at once, anything else politely.
struct StatusAnnouncer {
    text: &'static str,
    failed: bool,
}

impl<M: 'static> Widget<M> for StatusAnnouncer {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(0.0),
            height: SizeSpec::Fixed(0.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.text, self.failed) != (old.text, old.failed) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.to_owned());
        cx.live(if self.failed {
            Live::Assertive
        } else {
            Live::Polite
        });
    }
}

/// `rect` with its corners rounded by `radius`, as a path of four quarter circles.
fn rounded_path(rect: Rect, radius: f64) -> Vec<PathVerb> {
    // a quarter circle's control points sit this far along its tangents, as a fraction of the radius
    const KAPPA: f64 = 0.552_284_75;
    let (l, t, r, b) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let k = radius * KAPPA;
    vec![
        PathVerb::MoveTo(Point::new(l + radius, t)),
        PathVerb::LineTo(Point::new(r - radius, t)),
        PathVerb::CubicTo(
            Point::new(r - radius + k, t),
            Point::new(r, t + radius - k),
            Point::new(r, t + radius),
        ),
        PathVerb::LineTo(Point::new(r, b - radius)),
        PathVerb::CubicTo(
            Point::new(r, b - radius + k),
            Point::new(r - radius + k, b),
            Point::new(r - radius, b),
        ),
        PathVerb::LineTo(Point::new(l + radius, b)),
        PathVerb::CubicTo(
            Point::new(l + radius - k, b),
            Point::new(l, b - radius + k),
            Point::new(l, b - radius),
        ),
        PathVerb::LineTo(Point::new(l, t + radius)),
        PathVerb::CubicTo(
            Point::new(l, t + radius - k),
            Point::new(l + radius - k, t),
            Point::new(l + radius, t),
        ),
        PathVerb::Close,
    ]
}

/// A row of its children across the panel, centred vertically, `leading` points in from the leading edge and the
/// side padding in from the trailing one.
struct Bar<M> {
    height: f64,
    leading: f64,
    spacing: f64,
    /// The opacity it is drawn at, as one layer when under 1.
    alpha: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Bar<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.height),
            insets: Insets {
                top: 0.0,
                left: self.leading,
                bottom: 0.0,
                right: SIDE_PADDING,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: self.spacing,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.height, self.leading, self.spacing) != (old.height, old.leading, old.spacing) {
            Change::Layout
        } else if self.alpha != old.alpha {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.alpha < 1.0 {
            cx.set_group(self.alpha.clamp(0.0, 1.0) as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The results: a list of every result, of which the shown ones are children of their sections.
struct ResultList<M> {
    count: usize,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ResultList<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(LIST_WIDTH),
            ..column(Insets {
                top: LIST_PADDING,
                left: 0.0,
                bottom: LIST_PADDING,
                right: 0.0,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.count != old.count {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::List);
        cx.name("Results");
        cx.item_count(self.count);
    }
}

/// A section of the results, named by its heading.
struct SectionGroup<M> {
    title: String,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for SectionGroup<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        column(Insets::default())
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.title != old.title {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(self.title.clone());
    }
}

/// Lays its children out as `layout` says and leaves them out of the accessibility tree: a section's heading, which
/// its group is named by, and a row's lines, which the row is named and described by.
struct Quiet<M> {
    layout: LayoutSpec,
    children: Vec<Element<M>>,
}

impl<M: 'static> Quiet<M> {
    /// A section's heading: its title set as a section heading at the rows' text inset, on the heading's lower padding.
    fn heading(title: &str) -> Self {
        Self {
            layout: LayoutSpec {
                width: SizeSpec::Fill,
                height: SizeSpec::Fixed(HEADER_HEIGHT),
                insets: Insets {
                    top: 0.0,
                    left: SELECTION_INSET + CELL_INSET,
                    bottom: 6.0,
                    right: SELECTION_INSET + CELL_INSET,
                },
                ..LayoutSpec::new(ContainerKind::Stack {
                    axis: Axis::Vertical,
                    spacing: 0.0,
                    align: Align::Start,
                    justify: Justify::End,
                })
            },
            children: vec![text(title, TextRole::Section, true)],
        }
    }

    /// A row's title over its subtitle.
    fn lines(children: Vec<Element<M>>) -> Self {
        Self {
            layout: LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Start,
                justify: Justify::Center,
            }),
            children,
        }
    }
}

impl<M: 'static> Widget<M> for Quiet<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        self.layout.clone()
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.layout != old.layout {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// One result: its icon and its title over its subtitle, the selection behind them. Hover selects it and a press
/// activates it.
struct ResultRow<M> {
    flat: usize,
    title: String,
    subtitle: String,
    height: f64,
    selected: bool,
    children: Vec<Element<M>>,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for ResultRow<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.height),
            insets: Insets {
                top: 0.0,
                left: SELECTION_INSET + CELL_INSET,
                bottom: 0.0,
                right: SELECTION_INSET + CELL_INSET,
            },
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: CELL_INSET,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.height != old.height {
            Change::Layout
        } else if (self.flat, &self.title, &self.subtitle, self.selected)
            != (old.flat, &old.title, &old.subtitle, old.selected)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, self.height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(PointerEvent::Motion { .. }, _) if !self.selected => {
                cx.emit(M::from(Input::Hover(self.flat)));
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(M::from(Input::Activate(self.flat)));
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(M::from(Input::Activate(self.flat)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.selected {
            return;
        }
        let size = cx.size();
        let rect = Rect::new(
            SELECTION_INSET,
            0.0,
            (size.width - 2.0 * SELECTION_INSET).max(0.0),
            size.height,
        );
        let Some(color) = selection_color(cx) else {
            return;
        };
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect,
                radius: SELECTION_RADIUS,
            },
            paint: Paint::Solid(color),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ListItem);
        cx.name(self.title.clone());
        if !self.subtitle.is_empty() {
            cx.description(self.subtitle.clone());
        }
        cx.states(AccessibleStates {
            selected: self.selected,
            ..AccessibleStates::default()
        });
        cx.item_index(self.flat);
        cx.action(Action::Click);
    }
}

/// A container laid out as its `layout` says, with nothing of its own to paint; a named group to assistive
/// technology when it has a name.
struct Plain<M> {
    layout: LayoutSpec,
    group: Option<&'static str>,
    children: Vec<Element<M>>,
}

impl<M: 'static> Plain<M> {
    fn stack(axis: Axis, spacing: f64, align: Align, children: Vec<Element<M>>) -> Self {
        Self {
            layout: LayoutSpec::new(ContainerKind::Stack {
                axis,
                spacing,
                align,
                justify: Justify::Start,
            }),
            group: None,
            children,
        }
    }

    /// Its children side by side, taking the room left below the query row.
    fn row(children: Vec<Element<M>>, align: Align) -> Self {
        let mut row = Self::stack(Axis::Horizontal, 0.0, align, children);
        row.layout.width = SizeSpec::Fill;
        row.layout.height = SizeSpec::Fill;
        row
    }

    /// The preview pane: the width the list leaves, its children centred one above the other inside its padding.
    fn pane(children: Vec<Element<M>>) -> Self {
        let mut pane = Self::stack(Axis::Vertical, 6.0, Align::Center, children);
        pane.layout.width = SizeSpec::Fill;
        pane.layout.height = SizeSpec::Fill;
        pane.layout.flex = Flex::grow(1.0);
        pane.layout.insets = Insets {
            top: PREVIEW_PADDING,
            left: PREVIEW_PADDING,
            bottom: PREVIEW_PADDING,
            right: PREVIEW_PADDING,
        };
        pane.group = Some("Preview");
        pane
    }

    /// A detail of the preview: its label, ending at the value's start, then its value.
    fn detail(children: Vec<Element<M>>) -> Self {
        let mut detail = Self::stack(Axis::Horizontal, DETAIL_GAP, Align::Start, children);
        detail.layout.width = SizeSpec::Fill;
        detail
    }

    /// Its children placed where they say, over the whole window.
    fn absolute(children: Vec<Element<M>>) -> Self {
        Self {
            layout: LayoutSpec {
                width: SizeSpec::Fill,
                height: SizeSpec::Fill,
                ..LayoutSpec::new(ContainerKind::Absolute)
            },
            group: None,
            children,
        }
    }

    /// Its children one above the other, taking the room left below the query row.
    fn sheet(children: Vec<Element<M>>) -> Self {
        let mut sheet = Self::stack(Axis::Vertical, 0.0, Align::Stretch, children);
        sheet.layout.width = SizeSpec::Fill;
        sheet.layout.height = SizeSpec::Fill;
        sheet
    }

    /// Takes the room its column leaves.
    fn spacer() -> Self {
        let mut spacer = Self::stack(Axis::Vertical, 0.0, Align::Start, Vec::new());
        spacer.layout.height = SizeSpec::Fill;
        spacer
    }
}

impl<M: 'static> Widget<M> for Plain<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        self.layout.clone()
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.layout != old.layout {
            Change::Layout
        } else if self.group != old.group {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if let Some(name) = self.group {
            cx.role(Role::Group);
            cx.name(name);
        }
    }
}

/// Text at a size and weight of its own, in the label or secondary label colour: the preview's name, value and
/// details, which the text roles' sizes do not cover.
#[derive(Clone, Debug, PartialEq)]
struct Label {
    text: String,
    size: f64,
    weight: u16,
    secondary: bool,
    align: TextAlign,
    lines: u32,
    /// A fixed width, for a detail's label.
    width: Option<f64>,
}

impl Label {
    fn new(text: &str, size: f64, weight: u16) -> Self {
        Self {
            text: text.to_owned(),
            size,
            weight,
            secondary: false,
            align: TextAlign::Start,
            lines: 1,
            width: None,
        }
    }

    fn secondary(self) -> Self {
        Self {
            secondary: true,
            ..self
        }
    }

    /// Centred, wrapping onto at most `lines` lines.
    fn centred(self, lines: u32) -> Self {
        Self {
            align: TextAlign::Center,
            lines,
            ..self
        }
    }

    fn lines(self, lines: u32) -> Self {
        Self { lines, ..self }
    }

    /// `width` points wide, its text against the end.
    fn ending(self, width: f64) -> Self {
        Self {
            align: TextAlign::End,
            width: Some(width),
            ..self
        }
    }

    fn laid_out(&self, env: &Environment, width: f64) -> Option<ParagraphLayout> {
        if self.text.is_empty() {
            return None;
        }
        let style = TextStyle {
            weight: FontWeight(self.weight),
            ..TextStyle::new(self.size as f32)
        };
        let paragraph = ParagraphStyle {
            align: self.align,
            wrap: if self.lines > 1 {
                Wrap::Word
            } else {
                Wrap::None
            },
            max_lines: Some(self.lines.max(1)),
            ellipsis: true,
            ..ParagraphStyle::default()
        };
        let text = env
            .text
            .paragraph(&AttributedText::plain(&self.text, style), &paragraph)
            .ok()?;
        Some(text.layout(width.is_finite().then_some(width.max(0.0) as f32)))
    }
}

impl<M: 'static> Widget<M> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: self.width.map_or(SizeSpec::Fill, SizeSpec::Fixed),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (&self.text, self.size, self.weight, self.lines, self.width)
            != (&old.text, old.size, old.weight, old.lines, old.width)
        {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let width = self.width.unwrap_or(constraints.width.max());
        match self.laid_out(cx.env(), width) {
            Some(layout) => Measured {
                size: Size::new(
                    self.width.unwrap_or(layout.size().width),
                    layout.size().height,
                ),
                baseline: layout.lines().first().map(|line| line.baseline),
            },
            None => no_measure(),
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(layout) = self.laid_out(cx.env(), cx.size().width) else {
            return;
        };
        let role = if self.secondary {
            ColorRole::SecondaryText
        } else {
            ColorRole::Text
        };
        let Some(color) = cx.env().theme.colors.get(role, StateVariant::NORMAL) else {
            return;
        };
        glyphs(cx, &layout, Point::ZERO, color);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
    }
}

/// Draw `layout` with its top-left corner at `origin`.
pub(crate) fn glyphs(cx: &mut PaintCx<'_>, layout: &ParagraphLayout, origin: Point, color: Color) {
    for line in layout.lines() {
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                origin: origin + run.origin,
                run: run.run.clone(),
                color,
            });
        }
    }
}

/// The first lines of a text file, small, on a faint card the size of the preview's picture.
#[derive(Clone, Debug, PartialEq)]
struct TextBlock {
    text: String,
}

/// Between the excerpt's card and its text.
const TEXT_BLOCK_PADDING: f64 = 8.0;

impl TextBlock {
    fn laid_out(&self, env: &Environment) -> Option<ParagraphLayout> {
        let style = TextStyle::new(PREVIEW_TEXT_SIZE as f32);
        let paragraph = ParagraphStyle {
            wrap: Wrap::None,
            max_lines: Some(PREVIEW_TEXT_LINES as u32),
            ..ParagraphStyle::default()
        };
        let text = env
            .text
            .paragraph(&AttributedText::plain(&self.text, style), &paragraph)
            .ok()?;
        Some(text.layout(None))
    }
}

impl<M: 'static> Widget<M> for TextBlock {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(PREVIEW_IMAGE.height),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, PREVIEW_IMAGE.height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let card = Shape::RoundedRect {
            rect: Rect::new(0.0, 0.0, size.width, size.height),
            radius: PREVIEW_IMAGE_RADIUS,
        };
        let colors = cx.env().theme.colors.clone();
        if let Some(fill) = colors.get(ColorRole::QuaternaryLabel, StateVariant::NORMAL) {
            cx.push(PaintOp::Fill {
                shape: card.clone(),
                paint: Paint::Solid(Color::new(fill.r, fill.g, fill.b, fill.a * 0.5)),
            });
        }
        let (Some(layout), Some(color)) = (
            self.laid_out(cx.env()),
            colors.get(ColorRole::SecondaryText, StateVariant::NORMAL),
        ) else {
            return;
        };
        cx.push(PaintOp::PushClip(card));
        glyphs(
            cx,
            &layout,
            Point::new(TEXT_BLOCK_PADDING, TEXT_BLOCK_PADDING),
            color,
        );
        cx.push(PaintOp::PopClip);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
    }
}

/// The selected result's actions, one above the other.
struct ActionList<M> {
    count: usize,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ActionList<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: ACTION_GAP,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.count != old.count {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name("Actions");
        cx.item_count(self.count);
    }
}

/// One action of the selected result: a capsule, filled in the accent colour while it is the chosen one, which a
/// press or Return carries out.
#[derive(Clone, Debug, PartialEq)]
struct ActionButton {
    flat: usize,
    index: usize,
    label: String,
    chosen: bool,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for ActionButton {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(ACTION_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, ACTION_HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(M::from(Input::RunAction(self.flat, self.index)));
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(M::from(Input::RunAction(self.flat, self.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let colors = cx.env().theme.colors.clone();
        let (fill, text) = if self.chosen {
            (
                colors.get(ColorRole::DefaultButtonFill, StateVariant::NORMAL),
                ColorRole::DefaultButtonText,
            )
        } else {
            (
                thin_fill(
                    cx,
                    GLASS_CONTROL_DARK,
                    GLASS_CONTROL_LIGHT,
                    ColorRole::ControlFill,
                ),
                ColorRole::ControlText,
            )
        };
        if let Some(fill) = fill {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: size.height / 2.0,
                },
                paint: Paint::Solid(fill),
            });
        }
        let label = Label::new(&self.label, 13.0, 500).centred(1);
        let (Some(layout), Some(color)) = (
            label.laid_out(cx.env(), (size.width - size.height).max(0.0)),
            colors.get(text, StateVariant::NORMAL),
        ) else {
            return;
        };
        let top = ((size.height - layout.size().height) / 2.0).round();
        glyphs(cx, &layout, Point::new(size.height / 2.0, top), color);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            selected: self.chosen,
            ..AccessibleStates::default()
        });
        cx.item_index(self.index);
        cx.action(Action::Click);
    }
}

/// Fills the catcher window; a press anywhere dismisses the panel.
struct Catcher;

impl<M: From<Input> + Clone + 'static> Widget<M> for Catcher {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(constraints.width.ideal(), constraints.height.ideal()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _) => {
                cx.emit(M::from(Input::Dismiss));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name("Close the launcher");
    }
}

#[cfg(test)]
mod tests;
