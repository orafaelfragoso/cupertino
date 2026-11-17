//! Geometry of the panel in points.
//!
//! The modules are separate tiles on a grid of two 140 point columns and six 64 point rows, 12 points apart:
//! `2 * 140 + 12 = 292` wide and `6 * 64 + 5 * 12 = 444` high. A capsule fills one column and one row, a circle is one
//! 64 point row square, the media tile fills one column and two rows, and the display and sound tiles fill a row
//! across both columns. The window holds the grid 8 points in from its edges, with the Edit Controls capsule centred
//! 16 points below the grid.

use cupertino_geometry::{Point, Rect, Size};

use crate::model::{Detail, Module};

/// Height of a tile row, the diameter of a circular tile and the end diameter of a capsule.
pub const ROW: f64 = 64.0;
/// Width of a grid column.
pub const COLUMN: f64 = 140.0;
/// Space between two tiles, across and down.
pub const GAP: f64 = 12.0;
/// Rows of the grid.
pub const ROWS: usize = 6;
/// Size of the grid.
pub const GRID_WIDTH: f64 = 2.0 * COLUMN + GAP;
pub const GRID_HEIGHT: f64 = ROWS as f64 * ROW + (ROWS as f64 - 1.0) * GAP;
/// Corner radius of the media tile and of an open detail view.
pub const LARGE_RADIUS: f64 = 32.0;
/// Corner radius of the display and sound tiles.
pub const SLIDER_RADIUS: f64 = 24.0;
/// How far the grid sits in from the window's edges; also the padding of the one opaque panel drawn under the tiles
/// with Reduce Transparency.
pub const INSET: f64 = 8.0;
/// Corner radius of that opaque panel: the tiles' radius plus its padding, so the corners share a centre.
pub const GROUP_RADIUS: f64 = LARGE_RADIUS + INSET;
/// The Edit Controls capsule: its size and its distance below the grid.
pub const EDIT_SIZE: Size = Size::new(94.0, 24.0);
pub const EDIT_GAP: f64 = 16.0;
/// Size of the window: the grid and the Edit Controls capsule, with the inset around them.
pub const WINDOW: Size = Size::new(
    GRID_WIDTH + 2.0 * INSET,
    GRID_HEIGHT + EDIT_GAP + EDIT_SIZE.height + 2.0 * INSET,
);
/// How far the soft shadow around the modules reaches from their edge.
pub const SHADOW_REACH: f64 = 130.0;
/// The transparent margin the shadow is drawn in, left of the window's content and below it: the shadow's reach less
/// the [`INSET`] the window already keeps around the grid. Above the grid the margin only reaches up to the top of the
/// output (the bar is at least this tall), and to the right of it to the output's edge.
pub const SHADOW_LEFT: f64 = SHADOW_REACH - INSET;
pub const SHADOW_BOTTOM: f64 = SHADOW_REACH - INSET;
pub const SHADOW_TOP: f64 = 24.0;
pub const SHADOW_RIGHT: f64 = EDGE_MARGIN as f64;
/// Size of the surface: the window with the shadow's margin around it.
pub const SURFACE: Size = Size::new(
    SHADOW_LEFT + WINDOW.width + SHADOW_RIGHT,
    SHADOW_TOP + WINDOW.height + SHADOW_BOTTOM,
);
/// Where the window's own corner lies in the surface.
pub const WINDOW_ORIGIN: Point = Point::new(SHADOW_LEFT, SHADOW_TOP);
/// Where the grid's corner lies in the surface.
pub const GRID_ORIGIN: Point = Point::new(SHADOW_LEFT + INSET, SHADOW_TOP + INSET);

/// Distance of the window from the right edge of the output and below the menu bar; the tiles sit [`INSET`] further
/// in, so under the theme's 24 pt bar the first row of tiles starts 44 pt from the top of the output.
pub const EDGE_MARGIN: i32 = 6;
pub const BAR_MARGIN: i32 = 12;

/// The outline of a tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outline {
    /// Round ends: half the height.
    Capsule,
    /// Corners of this radius.
    Rounded(f64),
}

impl Outline {
    /// The corner radius on a tile of `size`.
    pub fn radius(self, size: Size) -> f64 {
        match self {
            Outline::Capsule => size.width.min(size.height) / 2.0,
            Outline::Rounded(radius) => radius,
        }
    }
}

/// The cell at `column` and `row` covering `columns` by `rows` cells, a circle taking a 64 point square of its column.
fn cell(column: f64, row: usize, width: f64, rows: usize) -> Rect {
    let rows = rows as f64;
    Rect::new(
        column,
        row as f64 * (ROW + GAP),
        width,
        rows * ROW + (rows - 1.0) * GAP,
    )
}

const LEFT: f64 = 0.0;
const RIGHT: f64 = COLUMN + GAP;
/// The second circle of a column: one circle and one gap along.
const NEXT_CIRCLE: f64 = ROW + GAP;

/// Where `module`'s tile lies in the grid, from the grid's top-left corner.
pub fn frame(module: Module) -> Rect {
    match module {
        Module::WiFi => cell(LEFT, 0, COLUMN, 1),
        Module::Bluetooth => cell(LEFT, 1, COLUMN, 1),
        Module::NowPlaying => cell(RIGHT, 0, COLUMN, 2),
        Module::Microphone => cell(LEFT, 2, COLUMN, 1),
        Module::DockHiding => cell(RIGHT, 2, ROW, 1),
        Module::LockScreen => cell(RIGHT + NEXT_CIRCLE, 2, ROW, 1),
        Module::Mute => cell(LEFT, 3, ROW, 1),
        Module::KeyboardBrightness => cell(LEFT + NEXT_CIRCLE, 3, ROW, 1),
        Module::FocusMode => cell(RIGHT, 3, COLUMN, 1),
        Module::Display => cell(LEFT, 4, GRID_WIDTH, 1),
        Module::Sound => cell(LEFT, 5, GRID_WIDTH, 1),
    }
}

/// The outline of `module`'s tile.
pub fn outline(module: Module) -> Outline {
    match module {
        Module::NowPlaying => Outline::Rounded(LARGE_RADIUS),
        Module::Display | Module::Sound => Outline::Rounded(SLIDER_RADIUS),
        _ => Outline::Capsule,
    }
}

/// Where the Edit Controls capsule lies under every tile, from the grid's top-left corner.
pub fn edit_frame() -> Rect {
    edit_frame_under(GRID_HEIGHT)
}

/// The top of the row under tiles `height` high that holds Edit Controls: [`EDIT_GAP`] below them, or at the
/// grid's top when no tile shows.
fn edit_row(height: f64) -> f64 {
    if height > 0.0 {
        height + EDIT_GAP
    } else {
        0.0
    }
}

/// Where the Edit Controls capsule lies under tiles `height` high, centred on the grid.
pub fn edit_frame_under(height: f64) -> Rect {
    Rect::new(
        (GRID_WIDTH - EDIT_SIZE.width) / 2.0,
        edit_row(height),
        EDIT_SIZE.width,
        EDIT_SIZE.height,
    )
}

/// The parts the panel shows while its controls are edited.
pub mod editing {
    use super::{edit_row, Module, Rect, Size, EDIT_SIZE, GRID_WIDTH};

    /// The Cancel and Done capsules that take the place of Edit Controls, and the space between them.
    pub const CANCEL_SIZE: Size = Size::new(62.0, EDIT_SIZE.height);
    pub const DONE_SIZE: Size = Size::new(49.0, EDIT_SIZE.height);
    pub const BUTTON_GAP: f64 = super::GAP;
    /// The mark on a tile's corner that removes it: its diameter and its inset from the tile's edges.
    pub const MARK: f64 = 18.0;
    pub const MARK_INSET: f64 = 4.0;
    /// A control offered to add: a round chip of this diameter, [`CHIP_GAP`] from the next as the tiles are, at most
    /// [`CHIP_COLUMNS`] in a row, the first row [`CHIPS_GAP`] below the capsules.
    pub const CHIP: f64 = 36.0;
    pub const CHIP_GAP: f64 = super::GAP;
    pub const CHIP_COLUMNS: usize = 6;
    pub const CHIPS_GAP: f64 = 16.0;
    /// The mark on a chip that says it adds: its diameter.
    pub const CHIP_MARK: f64 = 14.0;
    /// How far the pointer moves with the button down before it holds a tile.
    pub const DRAG_SLOP: f64 = 3.0;

    /// Where the remove mark lies on `module`'s tile of `size`: on its leading top corner, over the corner of its
    /// badge or artwork; on the display and sound tiles, whose title starts there, on the trailing one.
    pub fn mark(module: Module, size: Size) -> Rect {
        let x = match module {
            Module::Display | Module::Sound => size.width - MARK_INSET - MARK,
            _ => MARK_INSET,
        };
        Rect::new(x, MARK_INSET, MARK, MARK)
    }

    /// Where the Cancel and the Done capsule lie under tiles `height` high, the pair centred on the grid.
    pub fn buttons(height: f64) -> (Rect, Rect) {
        let width = CANCEL_SIZE.width + BUTTON_GAP + DONE_SIZE.width;
        let (x, y) = ((GRID_WIDTH - width) / 2.0, edit_row(height));
        (
            Rect::new(x, y, CANCEL_SIZE.width, CANCEL_SIZE.height),
            Rect::new(
                x + CANCEL_SIZE.width + BUTTON_GAP,
                y,
                DONE_SIZE.width,
                DONE_SIZE.height,
            ),
        )
    }

    /// Where chip `index` of `count` lies under tiles `height` high: rows of [`CHIP_COLUMNS`], each centred on the
    /// grid.
    pub fn chip(index: usize, count: usize, height: f64) -> Rect {
        let (row, column) = (index / CHIP_COLUMNS, index % CHIP_COLUMNS);
        let in_row = (count - row * CHIP_COLUMNS).min(CHIP_COLUMNS) as f64;
        let width = in_row * CHIP + (in_row - 1.0) * CHIP_GAP;
        Rect::new(
            (GRID_WIDTH - width) / 2.0 + column as f64 * (CHIP + CHIP_GAP),
            edit_row(height) + EDIT_SIZE.height + CHIPS_GAP + row as f64 * (CHIP + CHIP_GAP),
            CHIP,
            CHIP,
        )
    }

    /// The bottom of what shows while editing, from the grid's top: the capsules under tiles `height` high, or
    /// the last row of `count` chips.
    pub fn bottom(count: usize, height: f64) -> f64 {
        match count {
            0 => edit_row(height) + EDIT_SIZE.height,
            count => chip(count - 1, count, height).max_y(),
        }
    }
}

/// The parts of an open detail view, top to bottom.
pub mod detail {
    /// The module's own row: its switch or slider, as on its tile.
    pub const HEADER: f64 = 64.0;
    /// The list's heading.
    pub const HEADING: f64 = 28.0;
    /// One row of the list, and the settings row at the end.
    pub const ITEM: f64 = 32.0;
    /// Space above the separator, the separator line and the space below it.
    pub const SEPARATOR: f64 = 9.0;
    /// Space under the last row.
    pub const BOTTOM: f64 = 8.0;
    /// Inset of a row's highlight from the view's sides, and of its text from the highlight.
    pub const ROW_INSET: f64 = 8.0;
    pub const TEXT_INSET: f64 = 8.0;
}

/// The most list rows an open detail view shows: as many as fit in the grid's height.
pub fn detail_rows() -> usize {
    let fixed =
        detail::HEADER + detail::HEADING + detail::SEPARATOR + detail::ITEM + detail::BOTTOM;
    ((GRID_HEIGHT - fixed) / detail::ITEM).floor() as usize
}

/// Where an open detail view lies with `rows` list rows (an empty list shows one row saying so): across the grid from
/// its top, as high as its content, never past the grid.
pub fn detail_frame(rows: usize) -> Rect {
    let rows = rows.clamp(1, detail_rows()) as f64;
    let height = detail::HEADER
        + detail::HEADING
        + rows * detail::ITEM
        + detail::SEPARATOR
        + detail::ITEM
        + detail::BOTTOM;
    Rect::new(0.0, 0.0, GRID_WIDTH, height.min(GRID_HEIGHT))
}

/// The frame of `detail`'s tile at `progress` of its opening, 0 (the tile in the grid) to 1 (the open view of
/// `rows` rows): each edge moves straight from one to the other.
pub fn morph(detail: Detail, rows: usize, progress: f64) -> Rect {
    morph_from(frame(detail.module()), rows, progress)
}

/// [`morph`] for a tile that lies at `from`.
pub fn morph_from(from: Rect, rows: usize, progress: f64) -> Rect {
    let to = detail_frame(rows);
    let t = progress.clamp(0.0, 1.0);
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    Rect::new(
        lerp(from.x(), to.x()),
        lerp(from.y(), to.y()),
        lerp(from.width(), to.width()),
        lerp(from.height(), to.height()),
    )
}
