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
/// The least space between the time's bottom and the top of the user view or the picker, when the screen is too short
/// for them to stand at the bottom (approximate).
const USER_VIEW_GAP: f64 = 20.0;
/// The password field's bottom edge, and the picker's last row's, stand this share of `H` above the screen's bottom
/// (approximate).
const BLOCK_BOTTOM: f64 = 0.1;
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
/// A picker tile's padding: above its avatar, below its name's descenders, the same (approximate); and the space
/// between tiles, no less than it, so a selected tile's frame keeps clear of the next one (approximate).
pub const TILE_PADDING: f64 = 10.0;
const TILE_GAP: f64 = 12.0;
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
/// Space between the field and the first baseline of a message below it (approximate), the messages' text size and
/// how many lines a message takes at most.
const NOTICE_GAP: f64 = 20.0;
pub const NOTICE_SIZE: f64 = 13.0;
pub const NOTICE_LINES: f64 = 3.0;
/// Lines of message the space under the field keeps clear of the help line, and the space between them and it: the
/// field stands higher than its share of the bottom when they would not fit (approximate).
const NOTICE_ROOM_LINES: f64 = 2.0;
const NOTICE_HELP_GAP: f64 = 4.0;
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
    /// The first message's first baseline under the field, and the lowest a message's text may reach, clear of the
    /// help line.
    pub notice_baseline: f64,
    pub notice_bottom: f64,
    pub help_baseline: f64,
    /// The highest the user view and the picker may start: clear of the time.
    pub block_top: f64,
    /// The bottom of the field and of the picker's last row.
    pub block_bottom: f64,
    /// A picker tile's height, its avatar's top and its name's first baseline in the tile.
    pub tile_height: f64,
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
    /// The layout on an output of `width` x `height` points. `ascent_per_em` and `descent_per_em` are the font's
    /// ascent and descent in em, which place lines by their baselines; `placeholder_width` is the password field's
    /// placeholder's width at its size, which the field grows to hold.
    ///
    /// The clock stands at the top; the user view (avatar, name, field) at the bottom, the field's bottom edge
    /// [`BLOCK_BOTTOM`] of the height above the screen's, the name and the avatar stacked above it, and the messages
    /// under it clear of the help line; on a screen too short for that the view stands lower, never over the time.
    pub fn new(
        width: f64,
        height: f64,
        ascent_per_em: f64,
        descent_per_em: f64,
        placeholder_width: f64,
    ) -> Self {
        let short_side = width.min(height);
        let date_size = DATE_SIZE * short_side;
        let date_baseline = DATE_BASELINE * height;
        let time_size = TIME_SIZE * short_side;
        let time_baseline = date_baseline + TIME_BASELINE_IN_DATE_SIZES * date_size;
        let time_bottom = time_baseline + descent_per_em * time_size;
        let center_x = width / 2.0;
        let diameter = avatar_diameter(short_side);
        let help_baseline = height - HELP_BOTTOM_INSET - descent_per_em * HELP_SIZE;
        let help_top = help_baseline - ascent_per_em * HELP_SIZE;
        let notice_line = (ascent_per_em + descent_per_em) * NOTICE_SIZE;
        let notice_bottom = help_top - NOTICE_HELP_GAP;
        // the messages' top is their first baseline less the ascent
        let notice_room = notice_bottom - NOTICE_ROOM_LINES * notice_line - NOTICE_GAP
            + ascent_per_em * NOTICE_SIZE;
        // from the field's bottom up to the user view's top
        let above_field =
            PILL_HEIGHT + PILL_GAP + NAME_BASELINE_GAP + 2.0 * USER_VIEW_BORDER + diameter;
        let block_top = time_bottom + USER_VIEW_GAP;
        let block_bottom = ((1.0 - BLOCK_BOTTOM) * height)
            .min(notice_room)
            .max(block_top + above_field);
        let pill_top = block_bottom - PILL_HEIGHT;
        let name_baseline = pill_top - PILL_GAP;
        let avatar_top = name_baseline - NAME_BASELINE_GAP - USER_VIEW_BORDER - diameter;
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
            notice_baseline: block_bottom + NOTICE_GAP,
            notice_bottom,
            help_baseline,
            block_top,
            block_bottom: (1.0 - BLOCK_BOTTOM) * height,
            tile_height: 2.0 * TILE_PADDING
                + diameter
                + USER_VIEW_BORDER
                + NAME_BASELINE_GAP
                + descent_per_em * NAME_SIZE,
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

    /// The avatar of a picker tile, in the tile's points: centred across, [`TILE_PADDING`] below its top.
    pub fn tile_avatar(&self) -> Rect {
        let diameter = self.avatar.width();
        Rect::new(
            (TILE_WIDTH - diameter) / 2.0,
            TILE_PADDING,
            diameter,
            diameter,
        )
    }

    /// The first baseline of a picker tile's name, in the tile's points: as far below its avatar as on the password
    /// screen.
    pub fn tile_name_baseline(&self) -> f64 {
        self.tile_avatar().max_y() + USER_VIEW_BORDER + NAME_BASELINE_GAP
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

    /// The frames of the `count` tiles of a picker page of `grid`, row by row, [`TILE_GAP`] apart: the grid is centred
    /// across, its last row's bottom where the password field's is, and it grows upward with its rows, never over the
    /// time.
    pub fn tiles(&self, grid: Grid, count: usize) -> Vec<Rect> {
        let columns = grid.columns.max(1);
        let rows = count.div_ceil(columns);
        let (width, height) = (TILE_WIDTH + TILE_GAP, self.tile_height + TILE_GAP);
        let tall = rows as f64 * height - TILE_GAP;
        let top = (self.block_bottom - tall).max(self.block_top);
        let left = self.center_x - (columns as f64 * width - TILE_GAP) / 2.0;
        (0..count)
            .map(|i| {
                Rect::new(
                    left + (i % columns) as f64 * width,
                    top + (i / columns) as f64 * height,
                    TILE_WIDTH,
                    self.tile_height,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font that rises 0.95 of its size above the baseline and descends a quarter of it.
    const ASCENT: f64 = 0.95;
    const DESCENT: f64 = 0.25;

    fn approx(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    fn layout(width: f64, height: f64) -> Layout {
        Layout::new(width, height, ASCENT, DESCENT, 90.0)
    }

    /// The bottom of the time's line.
    fn time_bottom(layout: &Layout) -> f64 {
        layout.time_baseline + DESCENT * layout.time_size
    }

    #[test]
    fn the_clock_stands_at_the_top_and_the_user_view_at_the_bottom() {
        // 1920 x 1080 points, a placeholder narrower than the field
        let layout = layout(1920.0, 1080.0);
        approx(layout.center_x, 960.0);
        // 0.028 S, first baseline at 0.09 H
        approx(layout.date_size, 30.24);
        approx(layout.date_baseline, 97.2);
        // 0.14 S, first baseline 4.3 date sizes below the date's
        approx(layout.time_size, 151.2);
        approx(layout.time_baseline, 97.2 + 4.3 * 30.24);
        // the field's bottom edge 0.1 H above the screen's bottom, 28 tall, its top 18 below the name's baseline
        approx(layout.pill.max_y(), 972.0);
        approx(layout.pill.y(), 944.0);
        approx(layout.name_baseline, 944.0 - 18.0);
        approx(layout.pill.x(), 880.0);
        approx(layout.pill.width(), 160.0);
        // the name's baseline 23 below the user view, the avatar floor(0.05 S) = 54 points across in its 7 pt borders
        approx(layout.avatar.max_y(), 926.0 - 23.0 - 7.0);
        approx(layout.avatar.width(), 54.0);
        approx(layout.avatar.x(), 960.0 - 27.0);
        // a message's first baseline 20 under the field
        approx(layout.notice_baseline, 992.0);
        // the help's bottom edge 32 above the screen's bottom, 13 pt text descending 3.25
        approx(layout.help_baseline, 1080.0 - 32.0 - 3.25);
    }

    #[test]
    fn nothing_of_the_password_screen_overlaps_on_common_and_portrait_outputs() {
        let line = (ASCENT + DESCENT) * NOTICE_SIZE;
        for (width, height) in [
            (2560.0, 1440.0),
            (1440.0, 900.0),
            (1920.0, 1080.0),
            (1080.0, 1920.0),
            (1024.0, 768.0),
        ] {
            let layout = layout(width, height);
            let at = format!("{width} x {height}");
            // the field stands at its share of the bottom, and from 900 points up at most a point higher to leave the
            // messages room; a shorter screen lifts it as far as they need
            assert!(
                layout.pill.max_y() <= 0.9 * height + 1e-9
                    && (height < 900.0 || layout.pill.max_y() > 0.9 * height - 1.0),
                "{at}: the field ends at {}",
                layout.pill.max_y()
            );
            // the user view starts below the time
            assert!(
                layout.avatar.y() - USER_VIEW_BORDER >= time_bottom(&layout) + USER_VIEW_GAP,
                "{at}"
            );
            // the avatar, the name and the field stack without touching
            assert!(
                layout.avatar.max_y() < layout.name_baseline - ASCENT * NAME_SIZE,
                "{at}"
            );
            assert!(
                layout.name_baseline + DESCENT * NAME_SIZE < layout.pill.y(),
                "{at}"
            );
            // two lines of message start below the field and end above the help line
            let messages_top = layout.notice_baseline - ASCENT * NOTICE_SIZE;
            let help_top = layout.help_baseline - ASCENT * HELP_SIZE;
            assert!(messages_top > layout.pill.max_y(), "{at}");
            assert!(
                messages_top + 2.0 * line <= layout.notice_bottom + 1e-9,
                "{at}"
            );
            assert!(layout.notice_bottom < help_top, "{at}");
            assert!(layout.help_baseline + DESCENT * HELP_SIZE <= height, "{at}");
        }
    }

    #[test]
    fn on_a_short_screen_the_user_view_stands_lower_and_never_over_the_time() {
        let layout = layout(1280.0, 420.0);
        assert!(
            layout.avatar.y() - USER_VIEW_BORDER >= time_bottom(&layout) + USER_VIEW_GAP - 1e-9
        );
    }

    #[test]
    fn the_avatar_is_a_twentieth_of_the_short_side_in_whole_points() {
        approx(avatar_diameter(1440.0), 72.0);
        approx(avatar_diameter(900.0), 45.0);
        approx(avatar_diameter(1079.0), 53.0);
        let layout = layout(2560.0, 1440.0);
        approx(layout.avatar.width(), 72.0);
        approx(layout.avatar.height(), 72.0);
        // 10 above the avatar, 72 of it, 30 to the name's baseline, 3.75 of descent, 10 below
        approx(layout.tile_height, 125.75);
    }

    #[test]
    fn the_field_grows_past_160_points_only_for_a_placeholder_that_needs_it() {
        approx(pill_width(90.0), 160.0);
        approx(pill_width(132.0), 160.0);
        // ceil(140.2) + 28
        approx(pill_width(140.2), 169.0);
        let layout = Layout::new(1920.0, 1080.0, ASCENT, DESCENT, 140.2);
        approx(layout.pill.width(), 169.0);
        approx(layout.pill.x(), 960.0 - 84.5);
    }

    #[test]
    fn the_submit_button_ends_the_field_and_the_help_button_follows_it() {
        let layout = layout(1920.0, 1080.0);
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
        let layout = layout(2560.0, 1440.0);
        approx(layout.date_width(), 18.0 * 0.028 * 1440.0);
        approx(layout.time_width(), 5.0 * 0.14 * 1440.0);
    }

    #[test]
    fn the_power_item_ends_twenty_points_from_the_trailing_edge_and_its_menu_hangs_under_it() {
        let layout = layout(1920.0, 1080.0);
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
    fn picker_tiles_form_a_centred_grid_whose_last_row_ends_where_the_field_does() {
        let layout = layout(1920.0, 1080.0);
        // five tiles are 3 rows of 2: the grid is 2 x 186 + 12 wide, from x 768; its last row ends 0.1 H above the
        // bottom and the rows stand 12 apart above it
        let tiles = layout.tiles(Grid::for_count(5), 5);
        let height = layout.tile_height;
        assert_eq!(tiles.len(), 5);
        approx(tiles[4].max_y(), 972.0);
        approx(tiles[0].y(), 972.0 - 3.0 * height - 2.0 * 12.0);
        approx(tiles[0].x(), 768.0);
        approx(tiles[3].x(), 768.0 + 186.0 + 12.0);
        approx(tiles[4].x(), 768.0);
        approx(tiles[0].width(), 186.0);
        // the space between two tiles is no less than the padding inside one
        approx(tiles[1].x() - tiles[0].max_x(), 12.0);
        approx(tiles[2].y() - tiles[0].max_y(), 12.0);
        const { assert!(TILE_GAP >= TILE_PADDING) };
        // one row stands at the bottom too
        let one = layout.tiles(Grid::for_count(1), 1);
        approx(one[0].max_y(), 972.0);
        approx(one[0].x() + one[0].width() / 2.0, 960.0);
    }

    #[test]
    fn a_picker_tile_pads_its_avatar_and_name_alike_above_and_below() {
        for (width, height) in [(2560.0, 1440.0), (1440.0, 900.0), (1080.0, 1920.0)] {
            let layout = layout(width, height);
            let avatar = layout.tile_avatar();
            let below = layout.tile_height - (layout.tile_name_baseline() + DESCENT * NAME_SIZE);
            approx(avatar.y(), TILE_PADDING);
            approx(below, TILE_PADDING);
            // centred across
            approx(avatar.x() + avatar.width() / 2.0, TILE_WIDTH / 2.0);
        }
    }

    #[test]
    fn a_full_picker_page_never_reaches_the_time() {
        for (width, height) in [(1440.0, 900.0), (1025.0, 769.0), (1280.0, 600.0)] {
            let layout = layout(width, height);
            let tiles = layout.tiles(Grid::for_count(25), 25);
            assert!(
                tiles[0].y() >= time_bottom(&layout) + USER_VIEW_GAP - 1e-9,
                "{width} x {height}"
            );
        }
    }

    #[test]
    fn sizes_follow_the_short_side_of_a_portrait_output() {
        let layout = layout(1080.0, 1920.0);
        approx(layout.date_size, 30.24);
        approx(layout.date_baseline, 172.8);
        approx(layout.time_size, 151.2);
    }
}
