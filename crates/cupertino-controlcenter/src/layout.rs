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

/// Where the Edit Controls capsule lies, from the grid's top-left corner.
pub fn edit_frame() -> Rect {
    Rect::new(
        (GRID_WIDTH - EDIT_SIZE.width) / 2.0,
        GRID_HEIGHT + EDIT_GAP,
        EDIT_SIZE.width,
        EDIT_SIZE.height,
    )
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
    let (from, to) = (frame(detail.module()), detail_frame(rows));
    let t = progress.clamp(0.0, 1.0);
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    Rect::new(
        lerp(from.x(), to.x()),
        lerp(from.y(), to.y()),
        lerp(from.width(), to.width()),
        lerp(from.height(), to.height()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edges(r: Rect) -> (f64, f64, f64, f64) {
        (r.x(), r.y(), r.width(), r.height())
    }

    #[test]
    fn the_grid_is_two_columns_of_140_and_six_rows_of_64_twelve_apart() {
        assert_eq!((GRID_WIDTH, GRID_HEIGHT), (292.0, 444.0));
        assert_eq!(WINDOW, Size::new(308.0, 500.0));
    }

    #[test]
    fn the_tiles_start_44_points_below_the_top_and_14_in_from_the_right() {
        let bar = cupertino_theme::ShapeTokens::default().menu_bar_height;
        assert_eq!(bar + f64::from(BAR_MARGIN) + INSET, 44.0);
        assert_eq!(f64::from(EDGE_MARGIN) + INSET, 14.0);
    }

    #[test]
    fn each_module_has_its_cell() {
        assert_eq!(edges(frame(Module::WiFi)), (0.0, 0.0, 140.0, 64.0));
        assert_eq!(edges(frame(Module::Bluetooth)), (0.0, 76.0, 140.0, 64.0));
        assert_eq!(edges(frame(Module::NowPlaying)), (152.0, 0.0, 140.0, 140.0));
        assert_eq!(edges(frame(Module::Microphone)), (0.0, 152.0, 140.0, 64.0));
        assert_eq!(edges(frame(Module::DockHiding)), (152.0, 152.0, 64.0, 64.0));
        assert_eq!(edges(frame(Module::LockScreen)), (228.0, 152.0, 64.0, 64.0));
        assert_eq!(edges(frame(Module::Mute)), (0.0, 228.0, 64.0, 64.0));
        assert_eq!(
            edges(frame(Module::KeyboardBrightness)),
            (76.0, 228.0, 64.0, 64.0)
        );
        assert_eq!(edges(frame(Module::FocusMode)), (152.0, 228.0, 140.0, 64.0));
        assert_eq!(edges(frame(Module::Display)), (0.0, 304.0, 292.0, 64.0));
        assert_eq!(edges(frame(Module::Sound)), (0.0, 380.0, 292.0, 64.0));
        assert_eq!(edges(edit_frame()), (99.0, 460.0, 94.0, 24.0));
    }

    #[test]
    fn no_two_tiles_touch() {
        for a in Module::ALL {
            for b in Module::ALL.into_iter().filter(|b| *b != a) {
                let (a, b) = (frame(a), frame(b));
                let apart_x = a.max_x() + GAP <= b.x() || b.max_x() + GAP <= a.x();
                let apart_y = a.max_y() + GAP <= b.y() || b.max_y() + GAP <= a.y();
                assert!(apart_x || apart_y, "{a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn capsules_and_circles_have_round_ends_and_the_wide_tiles_rounded_corners() {
        assert_eq!(outline(Module::WiFi).radius(frame(Module::WiFi).size), 32.0);
        assert_eq!(outline(Module::Mute).radius(frame(Module::Mute).size), 32.0);
        assert_eq!(outline(Module::Display), Outline::Rounded(24.0));
        assert_eq!(outline(Module::NowPlaying), Outline::Rounded(32.0));
        assert_eq!(GROUP_RADIUS, 40.0);
    }

    #[test]
    fn a_detail_view_grows_with_its_list_up_to_the_grid() {
        assert_eq!(detail_rows(), 9);
        assert_eq!(edges(detail_frame(2)), (0.0, 0.0, 292.0, 205.0));
        assert_eq!(
            detail_frame(0),
            detail_frame(1),
            "an empty list shows one row"
        );
        assert_eq!(detail_frame(50).height(), detail_frame(9).height());
        assert!(detail_frame(9).height() <= GRID_HEIGHT);
    }

    #[test]
    fn the_morph_runs_from_the_tile_to_the_open_view() {
        assert_eq!(morph(Detail::Sound, 2, 0.0), frame(Module::Sound));
        assert_eq!(morph(Detail::Sound, 2, 1.0), detail_frame(2));
        assert_eq!(
            edges(morph(Detail::Sound, 2, 0.5)),
            (0.0, 190.0, 292.0, 134.5)
        );
        assert_eq!(
            morph(Detail::WiFi, 2, 7.0),
            detail_frame(2),
            "progress is clamped"
        );
    }
}
