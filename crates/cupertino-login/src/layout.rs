//! Where the elements of the lock screen sit, in points, from the size of the output: `S` is its short side and `H`
//! its height.

use cupertino_geometry::Rect;

use crate::picker::Grid;

/// Date font size as a fraction of the short side `S`.
const DATE_SIZE: f64 = 0.028;
/// First baseline of the date as a fraction of the height `H`.
const DATE_BASELINE: f64 = 0.09;
/// Time font size as a fraction of `S`.
const TIME_SIZE: f64 = 0.14;
/// The time's first baseline is this many date font sizes below the date's.
const TIME_BASELINE_IN_DATE_SIZES: f64 = 4.3;
/// The widest the date and the time draw, in their own font sizes: room for a long localized date and for a time with
/// its day period. Their boxes are no wider, so what they repaint stays near the middle (approximate).
const DATE_WIDTH_IN_SIZES: f64 = 18.0;
const TIME_WIDTH_IN_SIZES: f64 = 5.0;
/// Space between the time's bottom and the top of the user view (approximate).
const USER_VIEW_GAP: f64 = 20.0;
/// Avatar diameter as a fraction of `S`, rounded down to whole points.
const AVATAR_SHARE: f64 = 0.05;
/// Border above and below the avatar inside the user view.
pub const USER_VIEW_BORDER: f64 = 7.0;
/// The user name's first baseline is this far below the user view.
pub const NAME_BASELINE_GAP: f64 = 23.0;
/// Side borders of the user view around the name (`18/7` borders), which with the name's widest
/// extent make a picker tile's width.
const USER_VIEW_SIDE_BORDER: f64 = 18.0;
/// Width of a picker tile (approximate: the grid's item frame is not known, has only rows and columns; the width is
/// the user view's).
pub const TILE_WIDTH: f64 = NAME_MAX_WIDTH + 2.0 * USER_VIEW_SIDE_BORDER;
/// A picker tile is this much taller than its avatar: the avatar's borders (7 pt each), the name's baseline 23 pt
/// below them and room under the name, so a selected tile's frame clears the name's descenders and the next row
/// (approximate).
const TILE_HEIGHT_OVER_AVATAR: f64 = 56.0;
/// Name font size.
pub const NAME_SIZE: f64 = 15.0;
/// Widest the name draws.
pub const NAME_MAX_WIDTH: f64 = 150.0;
/// The password field: its least width, its height and corner radius (a capsule), and how much wider than its
/// placeholder it grows when that needs more.
pub const PILL_WIDTH: f64 = 160.0;
pub const PILL_HEIGHT: f64 = 28.0;
pub const PILL_RADIUS: f64 = 14.0;
const PILL_PADDING: f64 = 28.0;
/// The field's top is this far below the user name's first baseline.
const PILL_GAP: f64 = 18.0;
/// The submit and help buttons: circles as tall as the field, the submit button inside the field's row at its end
/// after a gap, the help button this far past the field's trailing edge; and the glyph they hold.
pub const BUTTON_SIZE: f64 = 28.0;
pub const SUBMIT_GAP: f64 = 2.0;
const HELP_BUTTON_GAP: f64 = 6.0;
pub const BUTTON_GLYPH: f64 = 22.0;
/// Space between the field and a message below it (approximate).
const NOTICE_GAP: f64 = 20.0;
/// The password help's bottom edge sits this far above the bottom of the screen (section 2,
/// `layout guide bottom - 32`), and its text is this size (approximate, the field's).
const HELP_BOTTOM_INSET: f64 = 32.0;
pub const HELP_SIZE: f64 = 13.0;

/// The status bar's items are 22 pt high and the last one ends 20 pt from the trailing edge, the items 14 pt apart;
/// the gap above the bar is approximate, the register has none.
const STATUS_HEIGHT: f64 = 22.0;
const STATUS_TRAILING: f64 = 20.0;
const STATUS_SPACING: f64 = 14.0;
const STATUS_TOP: f64 = 6.0;
/// The power menu's size (approximate, the design draws a system menu): its width, a row's height, the padding
/// above the first and below the last row, and the gap to the item.
const MENU_WIDTH: f64 = 150.0;
const MENU_ROW_HEIGHT: f64 = 24.0;
const MENU_PADDING: f64 = 4.0;
const MENU_GAP: f64 = 4.0;

/// Frames and baselines in points; `x` is the horizontal centre line unless a field says otherwise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub center_x: f64,
    pub date_baseline: f64,
    pub date_size: f64,
    pub time_baseline: f64,
    pub time_size: f64,
    pub avatar: Rect,
    pub name_baseline: f64,
    /// The password field's capsule.
    pub pill: Rect,
    /// The submit button, inside the capsule at its trailing end.
    pub submit: Rect,
    /// The help button, past the capsule's trailing edge.
    pub help_button: Rect,
    pub notice_baseline: f64,
    pub help_baseline: f64,
    /// The power item in the status bar: a square the height of the bar.
    pub power_item: Rect,
}

/// The avatar's diameter on an output whose short side is `short_side` points.
pub fn avatar_diameter(short_side: f64) -> f64 {
    (AVATAR_SHARE * short_side).floor().max(1.0)
}

/// The password field's width for a placeholder `placeholder_width` points wide.
pub fn pill_width(placeholder_width: f64) -> f64 {
    (placeholder_width.ceil() + PILL_PADDING).max(PILL_WIDTH)
}

impl Layout {
    /// The layout on an output of `width` x `height` points. `descent_per_em` is the font's descent in em, which
    /// places the user view below the time's bottom; `placeholder_width` is the password field's placeholder's width at
    /// its size, which the field grows to hold.
    pub fn new(width: f64, height: f64, descent_per_em: f64, placeholder_width: f64) -> Self {
        let short_side = width.min(height);
        let date_size = DATE_SIZE * short_side;
        let date_baseline = DATE_BASELINE * height;
        let time_size = TIME_SIZE * short_side;
        let time_baseline = date_baseline + TIME_BASELINE_IN_DATE_SIZES * date_size;
        let time_bottom = time_baseline + descent_per_em * time_size;
        let center_x = width / 2.0;
        let diameter = avatar_diameter(short_side);
        let avatar_top = time_bottom + USER_VIEW_GAP + USER_VIEW_BORDER;
        let user_view_bottom = avatar_top + diameter + USER_VIEW_BORDER;
        let name_baseline = user_view_bottom + NAME_BASELINE_GAP;
        let pill_top = name_baseline + PILL_GAP;
        let pill_width = pill_width(placeholder_width);
        let pill = Rect::new(
            center_x - pill_width / 2.0,
            pill_top,
            pill_width,
            PILL_HEIGHT,
        );
        Layout {
            center_x,
            date_baseline,
            date_size,
            time_baseline,
            time_size,
            avatar: Rect::new(center_x - diameter / 2.0, avatar_top, diameter, diameter),
            name_baseline,
            pill,
            submit: Rect::new(
                pill.max_x() - BUTTON_SIZE,
                pill_top,
                BUTTON_SIZE,
                BUTTON_SIZE,
            ),
            help_button: Rect::new(
                pill.max_x() + HELP_BUTTON_GAP,
                pill_top,
                BUTTON_SIZE,
                BUTTON_SIZE,
            ),
            notice_baseline: pill_top + PILL_HEIGHT + NOTICE_GAP,
            help_baseline: height - HELP_BOTTOM_INSET - descent_per_em * HELP_SIZE,
            power_item: Layout::status_item(width, 0),
        }
    }

    /// The `index`th item of the status bar from its trailing end: squares the height of the bar, 14 pt apart.
    pub fn status_item(width: f64, index: usize) -> Rect {
        Rect::new(
            width
                - STATUS_TRAILING
                - STATUS_HEIGHT
                - index as f64 * (STATUS_HEIGHT + STATUS_SPACING),
            STATUS_TOP,
            STATUS_HEIGHT,
            STATUS_HEIGHT,
        )
    }

    /// The widest the date and the time draw, centred on the screen.
    pub fn date_width(&self) -> f64 {
        DATE_WIDTH_IN_SIZES * self.date_size
    }

    pub fn time_width(&self) -> f64 {
        TIME_WIDTH_IN_SIZES * self.time_size
    }

    /// The height of a picker tile.
    pub fn tile_height(&self) -> f64 {
        self.avatar.height() + TILE_HEIGHT_OVER_AVATAR
    }

    /// The open power menu of `rows` items: its body, right-aligned under the item, and the frame of each row.
    pub fn power_menu(&self, rows: usize) -> (Rect, Vec<Rect>) {
        let body = Rect::new(
            self.power_item.x() + self.power_item.width() - MENU_WIDTH,
            self.power_item.y() + self.power_item.height() + MENU_GAP,
            MENU_WIDTH,
            rows as f64 * MENU_ROW_HEIGHT + 2.0 * MENU_PADDING,
        );
        let frames = (0..rows)
            .map(|i| {
                Rect::new(
                    body.x(),
                    body.y() + MENU_PADDING + i as f64 * MENU_ROW_HEIGHT,
                    MENU_WIDTH,
                    MENU_ROW_HEIGHT,
                )
            })
            .collect();
        (body, frames)
    }

    /// The frames of the first `count` tiles of a picker page of `grid`, row by row: whole multiples of the tile's size
    /// from the grid's corner, with no gap between them; the grid is centred and its top is where the user view sits
    /// on the password screen.
    pub fn tiles(&self, grid: Grid, count: usize) -> Vec<Rect> {
        let top = self.avatar.y() - USER_VIEW_BORDER;
        let left = self.center_x - grid.columns as f64 * TILE_WIDTH / 2.0;
        let height = self.tile_height();
        (0..count)
            .map(|i| {
                Rect::new(
                    left + (i % grid.columns) as f64 * TILE_WIDTH,
                    top + (i / grid.columns) as f64 * height,
                    TILE_WIDTH,
                    height,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn the_clock_and_user_block_sit_where_the_register_puts_them() {
        // 1920 x 1080 points, a font that descends a quarter of its size, a placeholder narrower than the field
        let layout = Layout::new(1920.0, 1080.0, 0.25, 90.0);
        approx(layout.center_x, 960.0);
        // 0.028 S, first baseline at 0.09 H
        approx(layout.date_size, 30.24);
        approx(layout.date_baseline, 97.2);
        // 0.14 S, first baseline 4.3 date sizes below the date's
        approx(layout.time_size, 151.2);
        approx(layout.time_baseline, 97.2 + 4.3 * 30.24);
        // time bottom, 20 to the user view, 7 border; the avatar floor(0.05 S) = 54 points across
        let time_bottom = 97.2 + 4.3 * 30.24 + 0.25 * 151.2;
        approx(layout.avatar.y(), time_bottom + 27.0);
        approx(layout.avatar.width(), 54.0);
        approx(layout.avatar.x(), 960.0 - 27.0);
        // avatar bottom, 7 border, 23 to the name baseline; the field's top 18 below it
        approx(layout.name_baseline, time_bottom + 27.0 + 54.0 + 30.0);
        approx(layout.pill.y(), layout.name_baseline + 18.0);
        approx(layout.pill.x(), 880.0);
        approx(layout.pill.width(), 160.0);
        // the help's bottom edge 32 above the screen's bottom, 13 pt text descending 3.25
        approx(layout.help_baseline, 1080.0 - 32.0 - 3.25);
    }

    #[test]
    fn the_avatar_is_a_twentieth_of_the_short_side_in_whole_points() {
        approx(avatar_diameter(1440.0), 72.0);
        approx(avatar_diameter(900.0), 45.0);
        approx(avatar_diameter(1079.0), 53.0);
        let layout = Layout::new(2560.0, 1440.0, 0.25, 90.0);
        approx(layout.avatar.width(), 72.0);
        approx(layout.avatar.height(), 72.0);
        approx(layout.tile_height(), 128.0);
    }

    #[test]
    fn the_field_grows_past_160_points_only_for_a_placeholder_that_needs_it() {
        approx(pill_width(90.0), 160.0);
        approx(pill_width(132.0), 160.0);
        // ceil(140.2) + 28
        approx(pill_width(140.2), 169.0);
        let layout = Layout::new(1920.0, 1080.0, 0.25, 140.2);
        approx(layout.pill.width(), 169.0);
        approx(layout.pill.x(), 960.0 - 84.5);
    }

    #[test]
    fn the_submit_button_ends_the_field_and_the_help_button_follows_it() {
        let layout = Layout::new(1920.0, 1080.0, 0.25, 90.0);
        let (pill, submit, help) = (layout.pill, layout.submit, layout.help_button);
        // inside the 28 pt row at its end
        approx(submit.max_x(), pill.max_x());
        approx(submit.y(), pill.y());
        approx(submit.width(), 28.0);
        approx(submit.height(), 28.0);
        // 6 pt past the field's trailing edge
        approx(help.x(), pill.max_x() + 6.0);
        approx(help.y(), pill.y());
        approx(help.width(), 28.0);
    }

    #[test]
    fn the_clock_draws_no_wider_than_its_own_sizes_ask() {
        let layout = Layout::new(2560.0, 1440.0, 0.25, 90.0);
        approx(layout.date_width(), 18.0 * 0.028 * 1440.0);
        approx(layout.time_width(), 5.0 * 0.14 * 1440.0);
    }

    #[test]
    fn the_power_item_ends_twenty_points_from_the_trailing_edge_and_its_menu_hangs_under_it() {
        let layout = Layout::new(1920.0, 1080.0, 0.25, 90.0);
        assert_eq!(
            (
                layout.power_item.x(),
                layout.power_item.width(),
                layout.power_item.height()
            ),
            (1878.0, 22.0, 22.0)
        );
        let (body, rows) = layout.power_menu(3);
        // right edges line up; 4 pt gap below the item; 4 pt padding and 3 rows of 24
        assert_eq!(
            (body.x() + body.width(), body.y(), body.height()),
            (1900.0, 32.0, 80.0)
        );
        assert_eq!(
            (rows[0].y(), rows[2].y(), rows[1].x(), rows[1].width()),
            (36.0, 84.0, 1750.0, 150.0)
        );
    }

    #[test]
    fn the_status_items_stand_14_points_apart_from_the_trailing_edge() {
        let (last, before) = (
            Layout::status_item(1920.0, 0),
            Layout::status_item(1920.0, 1),
        );
        approx(last.max_x(), 1900.0);
        approx(last.x() - before.max_x(), 14.0);
        approx(before.y(), last.y());
    }

    #[test]
    fn picker_tiles_form_a_centred_grid_of_whole_cells_starting_where_the_user_view_sits() {
        let layout = Layout::new(1920.0, 1080.0, 0.25, 90.0);
        // five tiles are 3 rows of 2: the grid is 372 wide, from x 774; the user view starts 7 above the avatar
        let tiles = layout.tiles(Grid::for_count(5), 5);
        let frames: Vec<(f64, f64)> = tiles.iter().map(|t| (t.x(), t.y())).collect();
        let top = layout.avatar.y() - 7.0;
        let height = layout.tile_height();
        assert_eq!(frames.len(), 5);
        approx(frames[0].0, 774.0);
        approx(frames[0].1, top);
        approx(frames[3].0, 960.0);
        approx(frames[3].1, top + height);
        approx(frames[4].0, 774.0);
        approx(frames[4].1, top + 2.0 * height);
        approx(tiles[0].width(), 186.0);
        // each cell starts where the one before it ends
        approx(tiles[1].x(), tiles[0].max_x());
        approx(tiles[2].y(), tiles[0].max_y());
    }

    #[test]
    fn sizes_follow_the_short_side_of_a_portrait_output() {
        let layout = Layout::new(1080.0, 1920.0, 0.25, 90.0);
        approx(layout.date_size, 30.24);
        approx(layout.date_baseline, 172.8);
        approx(layout.time_size, 151.2);
    }
}
