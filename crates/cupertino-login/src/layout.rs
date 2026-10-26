//! Where the elements of the lock screen sit, in points, from the size of the output.

use cupertino_geometry::Rect;

use crate::picker::Grid;

/// Date font size as a fraction of the short side `S`.
const DATE_SIZE: f64 = 0.028;
/// First baseline of the date as a fraction of the height `H`.
const DATE_BASELINE: f64 = 0.09;
/// Time font size as a fraction of `S`.
const TIME_SIZE: f64 = 0.12;
/// The time's first baseline is this many date font sizes below the date's.
const TIME_BASELINE_IN_DATE_SIZES: f64 = 4.0;
/// Space between the time's bottom and the top of the user view.
const USER_VIEW_GAP: f64 = 20.0;
/// Avatar diameter.
pub const AVATAR_DIAMETER: f64 = 70.0;
/// Border above and below the avatar inside the user view.
pub const USER_VIEW_BORDER: f64 = 7.0;
/// The user name's first baseline is this far below the user view.
pub const NAME_BASELINE_GAP: f64 = 23.0;
/// Side borders of the user view around the name (`18/7` borders), which with the name's widest
/// extent make a picker tile's width.
const USER_VIEW_SIDE_BORDER: f64 = 18.0;
/// Width and height of a picker tile (approximate: the grid's item frame is not known, has only rows and
/// columns; the width is the user view's, the height holds the avatar, its borders and the name line).
pub const TILE_WIDTH: f64 = NAME_MAX_WIDTH + 2.0 * USER_VIEW_SIDE_BORDER;
pub const TILE_HEIGHT: f64 = 112.0;
/// Name font size.
pub const NAME_SIZE: f64 = 15.0;
/// Widest the name draws.
pub const NAME_MAX_WIDTH: f64 = 150.0;
/// Password pill size.
pub const PILL_WIDTH: f64 = 160.0;
pub const PILL_HEIGHT: f64 = 28.0;
/// Space between the name line and the pill (the 10 pt below the name that gives the spinner).
const PILL_GAP: f64 = 10.0;
/// Space between the pill and a notice below it (approximate).
const NOTICE_GAP: f64 = 20.0;
/// The password help's bottom edge sits this far above the bottom of the screen (section 2,
/// `layout guide bottom - 32`), and its text is this size (approximate, the field's).
const HELP_BOTTOM_INSET: f64 = 32.0;
pub const HELP_SIZE: f64 = 13.0;

/// The status bar's items are 22 pt high and the last one ends 20 pt from the trailing edge; the gap
/// above the bar is approximate, the register has none.
const STATUS_HEIGHT: f64 = 22.0;
const STATUS_TRAILING: f64 = 20.0;
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
    pub pill: Rect,
    pub notice_baseline: f64,
    pub help_baseline: f64,
    /// The power item in the status bar: a square the height of the bar.
    pub power_item: Rect,
}

impl Layout {
    /// The layout on an output of `width` x `height` points. `descent_per_em` is the font's descent in em, which
    /// places the user view below the time's bottom.
    pub fn new(width: f64, height: f64, descent_per_em: f64) -> Self {
        let short_side = width.min(height);
        let date_size = DATE_SIZE * short_side;
        let date_baseline = DATE_BASELINE * height;
        let time_size = TIME_SIZE * short_side;
        let time_baseline = date_baseline + TIME_BASELINE_IN_DATE_SIZES * date_size;
        let time_bottom = time_baseline + descent_per_em * time_size;
        let center_x = width / 2.0;
        let avatar_top = time_bottom + USER_VIEW_GAP + USER_VIEW_BORDER;
        let user_view_bottom = avatar_top + AVATAR_DIAMETER + USER_VIEW_BORDER;
        let name_baseline = user_view_bottom + NAME_BASELINE_GAP;
        let pill_top = name_baseline + descent_per_em * NAME_SIZE + PILL_GAP;
        Layout {
            center_x,
            date_baseline,
            date_size,
            time_baseline,
            time_size,
            avatar: Rect::new(center_x - AVATAR_DIAMETER / 2.0, avatar_top, AVATAR_DIAMETER, AVATAR_DIAMETER),
            name_baseline,
            pill: Rect::new(center_x - PILL_WIDTH / 2.0, pill_top, PILL_WIDTH, PILL_HEIGHT),
            notice_baseline: pill_top + PILL_HEIGHT + NOTICE_GAP,
            help_baseline: height - HELP_BOTTOM_INSET - descent_per_em * HELP_SIZE,
            power_item: Rect::new(width - STATUS_TRAILING - STATUS_HEIGHT, STATUS_TOP, STATUS_HEIGHT, STATUS_HEIGHT),
        }
    }

    /// The open power menu of `rows` items: its body, right-aligned under the item, and the frame of each row.
    pub fn power_menu(&self, rows: usize) -> (Rect, Vec<Rect>) {
        let body = Rect::new(self.power_item.x() + self.power_item.width() - MENU_WIDTH, self.power_item.y() + self.power_item.height() + MENU_GAP, MENU_WIDTH, rows as f64 * MENU_ROW_HEIGHT + 2.0 * MENU_PADDING);
        let frames = (0..rows).map(|i| Rect::new(body.x(), body.y() + MENU_PADDING + i as f64 * MENU_ROW_HEIGHT, MENU_WIDTH, MENU_ROW_HEIGHT)).collect();
        (body, frames)
    }

    /// The frames of the first `count` tiles of a picker page of `grid`, row by row: the grid is centred and its top is
    /// where the user view sits on the password screen.
    pub fn tiles(&self, grid: Grid, count: usize) -> Vec<Rect> {
        let top = self.avatar.y() - USER_VIEW_BORDER;
        let left = self.center_x - grid.columns as f64 * TILE_WIDTH / 2.0;
        (0..count).map(|i| Rect::new(left + (i % grid.columns) as f64 * TILE_WIDTH, top + (i / grid.columns) as f64 * TILE_HEIGHT, TILE_WIDTH, TILE_HEIGHT)).collect()
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
        // 1920 x 1080 points, a font that descends a quarter of its size
        let layout = Layout::new(1920.0, 1080.0, 0.25);
        approx(layout.center_x, 960.0);
        approx(layout.date_size, 30.24);
        approx(layout.date_baseline, 97.2);
        approx(layout.time_size, 129.6);
        // date baseline + 4 date sizes
        approx(layout.time_baseline, 218.16);
        // time bottom 250.56, 20 to the user view, 7 border
        approx(layout.avatar.y(), 277.56);
        approx(layout.avatar.x(), 925.0);
        approx(layout.avatar.width(), 70.0);
        // avatar bottom 347.56, 7 border, 23 to the name baseline
        approx(layout.name_baseline, 377.56);
        approx(layout.pill.y(), 377.56 + 3.75 + 10.0);
        approx(layout.pill.x(), 880.0);
        // the help's bottom edge 32 above the screen's bottom, 13 pt text descending 3.25
        approx(layout.help_baseline, 1080.0 - 32.0 - 3.25);
    }

    #[test]
    fn the_power_item_ends_twenty_points_from_the_trailing_edge_and_its_menu_hangs_under_it() {
        let layout = Layout::new(1920.0, 1080.0, 0.25);
        assert_eq!((layout.power_item.x(), layout.power_item.width(), layout.power_item.height()), (1878.0, 22.0, 22.0));
        let (body, rows) = layout.power_menu(3);
        // right edges line up; 4 pt gap below the item; 4 pt padding and 3 rows of 24
        assert_eq!((body.x() + body.width(), body.y(), body.height()), (1900.0, 32.0, 80.0));
        assert_eq!((rows[0].y(), rows[2].y(), rows[1].x(), rows[1].width()), (36.0, 84.0, 1750.0, 150.0));
    }

    #[test]
    fn picker_tiles_form_a_centred_grid_starting_where_the_user_view_sits() {
        let layout = Layout::new(1920.0, 1080.0, 0.25);
        // five tiles are 3 rows of 2: the grid is 372 wide, from x 774; the user view starts 7 above the avatar
        let tiles = layout.tiles(Grid::for_count(5), 5);
        let frames: Vec<(f64, f64)> = tiles.iter().map(|t| (t.x(), t.y())).collect();
        let top = 277.56 - 7.0;
        assert_eq!(frames.len(), 5);
        approx(frames[0].0, 774.0);
        approx(frames[0].1, top);
        approx(frames[3].0, 960.0);
        approx(frames[3].1, top + 112.0);
        approx(frames[4].0, 774.0);
        approx(frames[4].1, top + 224.0);
        approx(tiles[0].width(), 186.0);
    }

    #[test]
    fn sizes_follow_the_short_side_of_a_portrait_output() {
        let layout = Layout::new(1080.0, 1920.0, 0.25);
        approx(layout.date_size, 30.24);
        approx(layout.date_baseline, 172.8);
    }
}
