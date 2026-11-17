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
/// A picker row: the avatar of diameter `D` at its leading side and the name after it. The row is `D` plus this wide
/// and `D` plus this tall; the avatar's centre is this far from the row's leading edge past `D / 2`, and this far below
/// its top past `D / 2`; the name starts this far after the avatar.
const ROW_EXTRA_WIDTH: f64 = 196.0;
const ROW_EXTRA_HEIGHT: f64 = 14.0;
const ROW_AVATAR_LEADING: f64 = 18.0;
const ROW_AVATAR_TOP: f64 = 7.0;
const ROW_NAME_GAP: f64 = 10.0;
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
/// The submit, help and back buttons: circles as tall as the field, the submit button inside the field's row at its
/// end after a gap, the help button this far past the field's trailing edge and the back button this far before its
/// leading edge; and the glyph they hold.
pub const BUTTON_SIZE: f64 = 28.0;
pub const SUBMIT_GAP: f64 = 2.0;
const HELP_BUTTON_GAP: f64 = 6.0;
const BACK_BUTTON_GAP: f64 = 6.0;
pub const BUTTON_GLYPH: f64 = 22.0;
/// The hints under the user name: the main hint's first baseline this share of `H` below the name's, at most this
/// wide, in the callout size (approximate, our callout); the sub hint this share of `H` below the main hint's first
/// baseline (the larger share when the main hint wraps), at most this wide, in the subheadline size (approximate, our
/// subheadline). Both stay clear of the field: at least this far below it (approximate).
const HINT_SHARE: f64 = 0.06;
pub const HINT_WIDTH: f64 = 210.0;
pub const HINT_SIZE: f64 = 12.0;
const SUB_HINT_SHARE: f64 = 0.03;
const SUB_HINT_WRAPPED_SHARE: f64 = 0.035;
pub const SUB_HINT_WIDTH: f64 = 450.0;
pub const SUB_HINT_SIZE: f64 = 11.0;
const HINT_FIELD_GAP: f64 = 8.0;
/// Lines the main hint may take.
pub const HINT_LINES: f64 = 2.0;
/// The lock message: its bottom edge this far above the screen's bottom (approximate), its size and its widest; and the
/// least space between the sub hint and it (approximate).
const LOCK_MESSAGE_INSET: f64 = 32.0;
pub const LOCK_MESSAGE_SIZE: f64 = 13.0;
pub const LOCK_MESSAGE_WIDTH: f64 = 400.0;
const LOCK_MESSAGE_GAP: f64 = 2.0;

/// The status bar: as tall as the menu bar, its items in 22 pt frames centred in it, the last one ending 20 pt from the
/// trailing edge, the items 14 pt apart; text in the menu bar's size (approximate).
pub const STATUS_BAR_HEIGHT: f64 = 33.0;
const STATUS_HEIGHT: f64 = 22.0;
const STATUS_TRAILING: f64 = 20.0;
const STATUS_SPACING: f64 = 14.0;
pub const STATUS_TEXT_SIZE: f64 = 13.0;
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
    pub height: f64,
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
    /// The greeter's back button, before the capsule's leading edge.
    pub back_button: Rect,
    /// The main hint's first baseline under the field.
    pub hint_baseline: f64,
    /// The lock message's first baseline, near the bottom.
    pub lock_message_baseline: f64,
    /// The highest the user view and the picker may start: clear of the time.
    pub block_top: f64,
    /// The bottom of the field and of the picker's last row.
    pub block_bottom: f64,
    /// A picker row's width and height.
    pub row_width: f64,
    pub row_height: f64,
    /// The font's ascent and descent in em.
    ascent: f64,
    descent: f64,
    /// The power item in the status bar.
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
    /// [`BLOCK_BOTTOM`] of the height above the screen's, the name and the avatar stacked above it, the hints under
    /// it and the lock message under them near the bottom; when the hints would not fit above the lock message the field
    /// stands higher, and on a screen too short for that the view stands lower, never over the time.
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
        let lock_message_baseline =
            height - LOCK_MESSAGE_INSET - descent_per_em * LOCK_MESSAGE_SIZE;
        let lock_message_top = lock_message_baseline - ascent_per_em * LOCK_MESSAGE_SIZE;
        // from the field's bottom down to the sub hint's bottom, with the main hint on its lines: the field's bottom
        // is 46 pt below the name's baseline, from which the main hint's place is measured
        let to_name = PILL_GAP + PILL_HEIGHT;
        let to_hint =
            (HINT_SHARE * height - to_name).max(HINT_FIELD_GAP + ascent_per_em * HINT_SIZE);
        let below_field = to_hint
            + sub_hint_offset(height, HINT_LINES, ascent_per_em, descent_per_em)
            + descent_per_em * SUB_HINT_SIZE;
        // from the field's bottom up to the user view's top
        let above_field =
            PILL_HEIGHT + PILL_GAP + NAME_BASELINE_GAP + 2.0 * USER_VIEW_BORDER + diameter;
        let block_top = time_bottom + USER_VIEW_GAP;
        let field_bottom = ((1.0 - BLOCK_BOTTOM) * height)
            .min(lock_message_top - LOCK_MESSAGE_GAP - below_field)
            .max(block_top + above_field);
        let pill_top = field_bottom - PILL_HEIGHT;
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
            height,
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
            back_button: Rect::new(
                pill.x() - BACK_BUTTON_GAP - BUTTON_SIZE,
                pill_top,
                BUTTON_SIZE,
                BUTTON_SIZE,
            ),
            hint_baseline: field_bottom + to_hint,
            lock_message_baseline,
            block_top,
            block_bottom: (1.0 - BLOCK_BOTTOM) * height,
            row_width: diameter + ROW_EXTRA_WIDTH,
            row_height: diameter + ROW_EXTRA_HEIGHT,
            ascent: ascent_per_em,
            descent: descent_per_em,
            power_item: Layout::status_item(width, 0),
        }
    }

    /// The sub hint's first baseline when the main hint takes `main_lines` lines.
    pub fn sub_hint_baseline(&self, main_lines: f64) -> f64 {
        self.hint_baseline + sub_hint_offset(self.height, main_lines, self.ascent, self.descent)
    }

    /// The `index`th item of the status bar from its trailing end: frames the height of an item, centred in the bar,
    /// 14 pt apart.
    pub fn status_item(width: f64, index: usize) -> Rect {
        Rect::new(
            width
                - STATUS_TRAILING
                - STATUS_HEIGHT
                - index as f64 * (STATUS_HEIGHT + STATUS_SPACING),
            (STATUS_BAR_HEIGHT - STATUS_HEIGHT) / 2.0,
            STATUS_HEIGHT,
            STATUS_HEIGHT,
        )
    }

    /// The status items of the given widths, from the trailing end leftward, 14 pt apart: their frames in that order.
    pub fn status_items(width: f64, widths: &[f64]) -> Vec<Rect> {
        let mut right = width - STATUS_TRAILING;
        widths
            .iter()
            .map(|&w| {
                let frame = Rect::new(
                    right - w,
                    (STATUS_BAR_HEIGHT - STATUS_HEIGHT) / 2.0,
                    w,
                    STATUS_HEIGHT,
                );
                right -= w + STATUS_SPACING;
                frame
            })
            .collect()
    }

    /// The widest the date and the time draw, centred on the screen.
    pub fn date_width(&self) -> f64 {
        DATE_WIDTH_IN_SIZES * self.date_size
    }

    pub fn time_width(&self) -> f64 {
        TIME_WIDTH_IN_SIZES * self.time_size
    }

    /// The avatar of a picker row, in the row's points: its centre `D / 2 + 18` from the leading edge (mirrored right
    /// to left) and `D / 2 + 7` below the top.
    pub fn row_avatar(&self, rtl: bool) -> Rect {
        let diameter = self.avatar.width();
        let leading = ROW_AVATAR_LEADING;
        let x = if rtl {
            self.row_width - leading - diameter
        } else {
            leading
        };
        Rect::new(x, ROW_AVATAR_TOP, diameter, diameter)
    }

    /// The name of a picker row, in the row's points: its box's left edge and width, and its first baseline, centred
    /// on the avatar's middle; it starts 10 pt after the avatar and takes up to [`NAME_MAX_WIDTH`].
    pub fn row_name(&self, rtl: bool) -> (f64, f64, f64) {
        let avatar = self.row_avatar(rtl);
        let x = if rtl {
            avatar.x() - ROW_NAME_GAP - NAME_MAX_WIDTH
        } else {
            avatar.max_x() + ROW_NAME_GAP
        };
        let middle = avatar.y() + avatar.height() / 2.0;
        let baseline = middle + (self.ascent - self.descent) * NAME_SIZE / 2.0;
        (x, NAME_MAX_WIDTH, baseline)
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

    /// The frames of the `count` rows of a picker page of `grid`: whole multiples of the row's size from the grid's
    /// corner with no gap, column by column from the leading side (right to left: from the right); the grid is centred
    /// across, its last row's bottom where the password field's is, and it grows upward with its rows, never over the
    /// time.
    pub fn tiles(&self, grid: Grid, count: usize, rtl: bool) -> Vec<Rect> {
        let columns = grid.columns.max(1);
        let rows = count.div_ceil(columns);
        let tall = rows as f64 * self.row_height;
        let top = (self.block_bottom - tall).max(self.block_top);
        let wide = columns as f64 * self.row_width;
        let left = self.center_x - wide / 2.0;
        (0..count)
            .map(|i| {
                let column = (i % columns) as f64;
                let x = if rtl {
                    left + wide - (column + 1.0) * self.row_width
                } else {
                    left + column * self.row_width
                };
                Rect::new(
                    x,
                    top + (i / columns) as f64 * self.row_height,
                    self.row_width,
                    self.row_height,
                )
            })
            .collect()
    }
}

/// From the main hint's first baseline to the sub hint's on a screen `height` tall, the main hint on `main_lines`
/// lines; never less than clears the main hint's last line.
fn sub_hint_offset(height: f64, main_lines: f64, ascent: f64, descent: f64) -> f64 {
    let share = if main_lines > 1.0 {
        SUB_HINT_WRAPPED_SHARE
    } else {
        SUB_HINT_SHARE
    };
    let line = (ascent + descent) * HINT_SIZE;
    let clear = (main_lines.max(1.0) - 1.0) * line + descent * HINT_SIZE + ascent * SUB_HINT_SIZE;
    (share * height).max(clear)
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
        // 2560 x 1440 points, a placeholder narrower than the field
        let layout = layout(2560.0, 1440.0);
        approx(layout.center_x, 1280.0);
        // 0.028 S, first baseline at 0.09 H
        approx(layout.date_size, 0.028 * 1440.0);
        approx(layout.date_baseline, 0.09 * 1440.0);
        // 0.14 S, first baseline 4.3 date sizes below the date's
        approx(layout.time_size, 0.14 * 1440.0);
        approx(layout.time_baseline, 0.09 * 1440.0 + 4.3 * 0.028 * 1440.0);
        // the field's bottom edge 0.1 H above the screen's bottom, 28 tall, its top 18 below the name's baseline
        approx(layout.pill.max_y(), 1296.0);
        approx(layout.pill.y(), 1268.0);
        approx(layout.name_baseline, 1268.0 - 18.0);
        approx(layout.pill.x(), 1200.0);
        approx(layout.pill.width(), 160.0);
        // the name's baseline 23 below the user view, the avatar floor(0.05 S) = 72 points across in its 7 pt borders
        approx(layout.avatar.max_y(), 1250.0 - 23.0 - 7.0);
        approx(layout.avatar.width(), 72.0);
        approx(layout.avatar.x(), 1280.0 - 36.0);
        // the main hint's first baseline 0.06 H below the name's
        approx(layout.hint_baseline, 1250.0 + 0.06 * 1440.0);
        // the sub hint 0.03 H below it, 0.035 H when the main hint wraps
        approx(
            layout.sub_hint_baseline(1.0),
            layout.hint_baseline + 0.03 * 1440.0,
        );
        approx(
            layout.sub_hint_baseline(2.0),
            layout.hint_baseline + 0.035 * 1440.0,
        );
        // the lock message's bottom edge 32 above the screen's bottom, 13 pt text descending 3.25
        approx(layout.lock_message_baseline, 1440.0 - 32.0 - 3.25);
    }

    #[test]
    fn nothing_of_the_password_screen_overlaps_on_common_and_portrait_outputs() {
        let callout = (ASCENT + DESCENT) * HINT_SIZE;
        for (width, height) in [
            (2560.0, 1440.0),
            (1440.0, 900.0),
            (1920.0, 1080.0),
            (1080.0, 1920.0),
            (1024.0, 768.0),
        ] {
            let layout = layout(width, height);
            let at = format!("{width} x {height}");
            // the field stands at its share of the bottom or higher, by no more than the hints need
            assert!(layout.pill.max_y() <= 0.9 * height + 1e-9, "{at}");
            assert!(layout.pill.max_y() > 0.9 * height - 32.0, "{at}");
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
            // the main hint starts below the field, the sub hint below the main hint on two lines, and both end above
            // the lock message
            let main_top = layout.hint_baseline - ASCENT * HINT_SIZE;
            assert!(main_top >= layout.pill.max_y() + 8.0 - 1e-9, "{at}");
            let main_bottom = layout.hint_baseline + callout + DESCENT * HINT_SIZE;
            let sub = layout.sub_hint_baseline(2.0);
            assert!(sub - ASCENT * SUB_HINT_SIZE >= main_bottom - 1e-9, "{at}");
            let lock_top = layout.lock_message_baseline - ASCENT * LOCK_MESSAGE_SIZE;
            assert!(sub + DESCENT * SUB_HINT_SIZE < lock_top, "{at}");
            assert!(
                layout.lock_message_baseline + DESCENT * LOCK_MESSAGE_SIZE <= height,
                "{at}"
            );
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
    fn the_buttons_stand_beside_the_field_the_submit_one_inside_its_end() {
        let layout = layout(1920.0, 1080.0);
        let (pill, submit, help, back) = (
            layout.pill,
            layout.submit,
            layout.help_button,
            layout.back_button,
        );
        // inside the 28 pt row at its end
        approx(submit.max_x(), pill.max_x());
        approx(submit.y(), pill.y());
        approx(submit.width(), 28.0);
        approx(submit.height(), 28.0);
        // 6 pt past the field's trailing edge, and 6 pt before its leading edge
        approx(help.x(), pill.max_x() + 6.0);
        approx(help.y(), pill.y());
        approx(help.width(), 28.0);
        approx(back.max_x(), pill.x() - 6.0);
        approx(back.y(), pill.y());
        approx(back.width(), 28.0);
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
        // a 22 pt frame centred in the 33 pt bar
        assert_eq!(
            (
                layout.power_item.x(),
                layout.power_item.y(),
                layout.power_item.width(),
                layout.power_item.height()
            ),
            (1878.0, 5.5, 22.0, 22.0)
        );
        let (body, rows) = layout.power_menu(3);
        // right edges line up; 4 pt gap below the item; 4 pt padding and 3 rows of 24
        assert_eq!(
            (body.x() + body.width(), body.y(), body.height()),
            (1900.0, 31.5, 80.0)
        );
        assert_eq!(
            (rows[0].y(), rows[2].y(), rows[1].x(), rows[1].width()),
            (35.5, 83.5, 1750.0, 150.0)
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
        // items of their own widths, from the trailing end
        let items = Layout::status_items(1920.0, &[22.0, 40.0, 30.0]);
        approx(items[0].max_x(), 1900.0);
        approx(items[0].x() - items[1].max_x(), 14.0);
        approx(items[1].width(), 40.0);
        approx(items[1].x() - items[2].max_x(), 14.0);
        approx(items[2].y(), (33.0 - 22.0) / 2.0);
    }

    #[test]
    fn a_picker_row_holds_its_avatar_at_the_leading_side_and_its_name_after_it() {
        let layout = layout(2560.0, 1440.0);
        // D = 72: the row D + 196 wide and D + 14 tall
        approx(layout.row_width, 72.0 + 196.0);
        approx(layout.row_height, 72.0 + 14.0);
        // the avatar's centre D / 2 + 18 from the leading edge and D / 2 + 7 below the top
        let avatar = layout.row_avatar(false);
        approx(avatar.x() + avatar.width() / 2.0, 36.0 + 18.0);
        approx(avatar.y() + avatar.height() / 2.0, 36.0 + 7.0);
        // the padding above and below the avatar is the same
        approx(avatar.y(), layout.row_height - avatar.max_y());
        // the name 10 pt after the avatar, up to 150 wide, its line centred on the avatar's middle
        let (x, width, baseline) = layout.row_name(false);
        approx(x, avatar.max_x() + 10.0);
        approx(width, 150.0);
        let middle = (baseline - ASCENT * NAME_SIZE + baseline + DESCENT * NAME_SIZE) / 2.0;
        approx(middle, avatar.y() + avatar.height() / 2.0);
        assert!(x + width <= layout.row_width);
        // right to left: mirrored
        let mirrored = layout.row_avatar(true);
        approx(mirrored.max_x(), layout.row_width - avatar.x());
        let (rx, rwidth, _) = layout.row_name(true);
        approx(rx + rwidth, mirrored.x() - 10.0);
        assert!(rx >= 0.0);
    }

    #[test]
    fn picker_rows_form_a_centred_grid_of_whole_cells_whose_last_row_ends_where_the_field_should() {
        let layout = layout(1920.0, 1080.0);
        // five rows are 3 rows of 2 columns: the grid is 2 row widths, centred; its last row ends 0.1 H above the
        // bottom; the cells touch
        let (w, h) = (layout.row_width, layout.row_height);
        let tiles = layout.tiles(Grid::for_count(5), 5, false);
        assert_eq!(tiles.len(), 5);
        approx(tiles[4].max_y(), 972.0);
        approx(tiles[0].y(), 972.0 - 3.0 * h);
        approx(tiles[0].x(), 960.0 - w);
        approx(tiles[1].x(), tiles[0].max_x());
        approx(tiles[2].y(), tiles[0].max_y());
        approx(tiles[4].x(), 960.0 - w);
        // right to left: the first column on the right
        let rtl = layout.tiles(Grid::for_count(5), 5, true);
        approx(rtl[0].x(), 960.0);
        approx(rtl[1].max_x(), 960.0);
        // one row stands at the bottom too
        let one = layout.tiles(Grid::for_count(1), 1, false);
        approx(one[0].max_y(), 972.0);
        approx(one[0].x() + one[0].width() / 2.0, 960.0);
    }

    #[test]
    fn a_full_picker_page_never_reaches_the_time() {
        for (width, height) in [(1440.0, 900.0), (1025.0, 769.0), (1280.0, 600.0)] {
            let layout = layout(width, height);
            let tiles = layout.tiles(Grid::for_count(25), 25, false);
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
