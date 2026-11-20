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

mod body;
pub mod browse;
mod enclosure;
pub mod layers;
pub mod modes;
mod rows;
pub mod scope;

use body::*;
pub use enclosure::*;
use rows::*;

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
    BareField, CaretBlink, Icon, IconDirection, Image, ImageFit, ImageSource, Stack, Text,
    TextChange, TextField, TextRole,
};
use cupertino_kit::core::{
    glass_outline, AccessibleStates, Action, Change, CommandRegistry, Element, Environment, Event,
    EventCx, Glass, Key, Live, MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget,
    Window, WindowBackground, WindowKey,
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
    /// The query field's caret shows again ([`BareField::on_blink`]), with the blink of the field's theme.
    CaretShown(CaretBlink),
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
    /// The compositor runs the panel's open or a fold on its layers: the field holds its caret, which would draw the
    /// query row's layer again.
    pub opening: bool,
    /// The panel's own motion runs and it draws it itself: its content lies on whole device pixels as one picture, as
    /// the compositor puts the content's layers when it runs a motion.
    pub moving: bool,
    /// The compositor runs an unfold of the search field into a scope: the body is drawn at this, the scope's shape at
    /// rest, and its layer is cut to the glass as the glass grows ([`layers::fold_changes`]).
    pub unfold_body: Option<PanelShape>,
    /// The query field's caret blinks from the moment this starts ([`BareField::blink_start`]): the open's start.
    pub blink_start: Option<u64>,
    /// The query field's caret draws into a layer of its own ([`layers::CARET_LAYER`]), whose opacity the compositor
    /// takes through the blink while it runs the search field's open.
    pub caret_layer: bool,
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
    // drawn as layers, the panel's edge is the compositor's, along its glass ([`layers::glass_edge`])
    let mut root = vec![panel_content(view).key("panel")];
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

/// The browser's view options button at the query row's end, placed apart from the row in a layer of its own while the
/// panel is drawn as layers ([`layers::OPTIONS_LAYER`]): it keeps to the row's end as the enclosure's width changes,
/// where the rest of the row keeps to its start. While the panel `moves` it lies on whole device pixels as one picture,
/// as its layer does when the compositor moves it; it is drawn at `alpha`, as the row is.
fn options_button<M>(view: &PanelView<'_>, moves: bool, alpha: f64) -> Option<Element<M>>
where
    M: From<Input> + Clone + 'static,
{
    let frame = &view.frame;
    let browser = view.browser.filter(|_| frame.header == Header::Browse)?;
    let layer = layers::options_layer_rect(frame.shape.width);
    let at = layers::options_button(frame.shape.width).translate(-layer.x(), -layer.y());
    let button = Element::new(browse::OptionsButton {
        focus: browser.zone() == crate::browse::Zone::Options
            && view.nested != Some(Nested::ViewOptions),
        expanded: view.nested == Some(Nested::ViewOptions),
    })
    .key(browse::OPTIONS_BUTTON);
    Some(
        Element::new(layers::PlacedLayer {
            key: layers::OPTIONS_LAYER,
            layer: view.layered,
            rect: layer,
            alpha,
            on_pixels: moves,
            children: vec![Element::new(layers::SymbolBox {
                rect: at,
                on_pixels: false,
                children: vec![button],
            })],
        })
        .key("options"),
    )
}

/// A mode button's elements: the button, and while the panel is drawn as layers, or its symbol moves, its face apart.
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
            // where the button rests: while it moves its symbol lies on whole device pixels as its face layer does
            let rest = {
                let mut f = *frame;
                (f.modes.spread, f.scale, f.scale_y) = (1.0, 1.0, 1.0);
                layers::button_at(view.extent, &f, rects.len(), index)
            };
            let (children, face) = if view.layered {
                let symbol = modes::glyph(mode, frame.modes.sharp, 1.0);
                let face = layers::button_face(index, rect, joined.clone(), focus, false, symbol);
                (Vec::new(), Some(face))
            } else if rect != rest {
                let symbol = modes::glyph(mode, frame.modes.sharp, frame.alpha);
                (
                    Vec::new(),
                    Some(layers::symbol_in_place(rect, rest, symbol)),
                )
            } else {
                (vec![modes::glyph(mode, frame.modes.sharp, 1.0)], None)
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
/// the shapes join into one, only the outline of the whole shows, not each part's. On `glass` it runs along the
/// glass's own continuous outline ([`glass_outline`]), where the compositor draws a glass's edge; around the opaque
/// fill drawn without glass, along that fill's circular corners.
pub(crate) fn edge_outside(
    cx: &mut PaintCx<'_>,
    rect: Rect,
    radius: f64,
    alpha: f32,
    joined: &[(Rect, f64)],
    glass: bool,
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
    let shape = if glass {
        glass_outline(rect, radius, EDGE_WIDTH / 2.0)
    } else {
        Shape::RoundedRect {
            rect: rect.outset(EDGE_WIDTH / 2.0),
            radius: radius + EDGE_WIDTH / 2.0,
        }
    };
    cx.push(PaintOp::Stroke {
        shape,
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
        menu: true,
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
        // while the compositor runs the close, an open or a fold, the caret's blink would draw the row's layer again
        hold_caret: view.handed || view.opening,
        blink_start: view.blink_start,
        caret_layer: view.caret_layer.then_some(layers::CARET_LAYER),
        // the panel hears of the caret only while the compositor shows its blink
        on_blink: view
            .caret_layer
            .then(|| Box::new(|blink| M::from(Input::CaretShown(blink))) as Box<dyn Fn(_) -> M>),
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
    // the view options button at the row's end is placed apart from the row, which keeps its room for it
    // ([`options_button`])
    if view.browser.is_some() && browsing {
        row.push(
            Element::new(layers::Room(Size::new(
                browse::OPTIONS_SIZE,
                browse::OPTIONS_SIZE,
            )))
            .key("options-room"),
        );
    }
    let showing_browser = view.browser.filter(|_| browsing || frame.content > 0.0);
    let showing_scope = view
        .scope
        .filter(|_| (filing || frame.content > 0.0) && showing_browser.is_none());
    // over a scope the enclosure fades its two parts one by one rather than as one layer: they do not overlap,
    // so the look is the same, and the scope's own fade then needs no layer inside another
    let split = showing_browser.is_some() || showing_scope.is_some();
    // while the panel scales in or out, or its shape moves (a fold), each part of the content (the query row, the
    // view options button and the body) moves as one picture on whole device pixels, as the compositor moves its
    // layers when it runs the open or the fold, rather than its text snapping to the pixels line by line while its
    // shapes and images fall between them
    let moves = view.moving || frame.scale != 1.0 || frame.scale_y != 1.0;
    let query_row = Element::new(Bar {
        height: COLLAPSED_HEIGHT,
        leading: icon_inset,
        spacing: QUERY_INSET - icon_inset - icon_size,
        alpha: if split { frame.alpha } else { 1.0 },
        layer: view.layered.then_some(layers::ROW_LAYER),
        on_pixels: moves,
        children: row,
    })
    .key("query-row");
    let mut body = Vec::new();
    if let Some(browser) = showing_browser {
        body.push(
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
        body.push(
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
        body.extend(search_body(view));
    }
    // the query row keeps its height where the shape springs a little shorter than the field on its way to it
    let size = Size::new(frame.shape.width, frame.shape.height.max(COLLAPSED_HEIGHT));
    // the query row and the body are layers of their own, both from the content's top, on the device pixels it lies
    // on; the body's children start below the row. While the compositor unfolds the field into a scope the body is
    // drawn as the scope rests, its layer cut to the glass as the glass grows.
    let body_size = view.unfold_body.map_or(size, |shape| {
        Size::new(shape.width, shape.height.max(COLLAPSED_HEIGHT))
    });
    let unfolding = view.unfold_body.is_some();
    let mut parts = vec![query_row];
    parts.extend(options_button(
        view,
        moves,
        if split { frame.alpha } else { 1.0 },
    ));
    parts.push(
        Element::new(layers::ContentPart {
            key: layers::BODY_LAYER,
            layer: view.layered,
            size: body_size,
            top: COLLAPSED_HEIGHT,
            masks: unfolding.then(|| layers::held_radius(view.extent, frame)),
            on_pixels: moves,
            children: body,
        })
        .key("body-part"),
    );
    let inner = Element::new(PanelRoot {
        keys: view.keys,
        size,
        offset: (rect.width() - frame.shape.width) / 2.0,
        handed: view.handed,
        holding: view.opening,
        crops: unfolding,
        children: parts,
    });
    let mut children = vec![inner.key("content")];
    if view.layered {
        children.insert(
            0,
            Element::new(layers::GlassLayer {
                rect: Rect::new(0.0, 0.0, rect.width(), rect.height()),
                look: layers::glass_look(frame),
                key: layers::GLASS_LAYER,
                edge: true,
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
        crops: moves,
        content: Size::new(frame.shape.width, frame.shape.height),
        children,
    })
}
