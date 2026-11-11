//! A folder's stack as a fan: the newest files rise from the folder's tile one over the other in a gentle arc, each a
//! small icon with its name on a pill to its left, and an "open the folder" button at the top. This module holds what
//! the fan is and where its parts go: how it comes and goes, how many entries it holds, and the arc's geometry. The
//! window is [`crate::view::fan`], and the model opens and closes it ([`crate::kit`]).

use cupertino_animation::CubicBezier;
use cupertino_defaults::dock::StackStyle;
use cupertino_wm::geometry::{Point, Rect};

use crate::stack::StackEntry;

/// The most entries the fan holds, the button not counted: a folder with more is shown as the grid.
pub const MAX_ENTRIES: usize = 10;
/// The distance between the centres of neighbouring rows.
pub const PITCH: f64 = 63.5;
/// The square an entry's icon is drawn in, and the button's diameter.
pub const ICON: f64 = 56.0;
/// A pill's height, the space either side of its text, the gap between it and its icon, and the text's size.
pub const PILL_HEIGHT: f64 = 26.5;
pub const PILL_PADDING: f64 = 13.0;
pub const PILL_GAP: f64 = 8.0;
pub const PILL_FONT_PT: f64 = 13.0;
/// The widest a pill gets; a longer name is cut with an ellipsis.
pub const PILL_MAX_WIDTH: f64 = 260.0;
/// How far above the bar's top edge the lowest row's centre is.
pub const LOWEST_ABOVE_BAR: f64 = 40.5;
/// The room kept above the top row and around the arc for a row's tilt.
const MARGIN: f64 = 14.0;
/// How long the fan takes to open and to close, in seconds.
pub const OPEN_S: f64 = 0.244;
pub const CLOSE_S: f64 = 0.178;
/// The curve of opening, slow at both ends, and of closing, slow at first.
pub const OPEN_EASE: CubicBezier = CubicBezier::new(0.42, 0.0, 0.58, 1.0);
pub const CLOSE_EASE: CubicBezier = CubicBezier::new(0.42, 0.0, 1.0, 1.0);
/// How far the rows sit from the tile's middle to the trailing side, by row from the lowest: the arc bends towards the
/// trailing side as it rises, less and less.
const DRIFT: [f64; 12] = [
    0.0, 1.5, 5.5, 11.0, 17.75, 25.0, 32.5, 40.0, 47.25, 54.25, 61.0, 67.5,
];
/// How far each row is turned clockwise, in degrees, by row from the lowest.
const TILT_DEGREES: [f64; 12] = [
    0.5, 2.9, 4.1, 5.3, 6.6, 7.7, 8.7, 9.6, 10.4, 11.1, 11.7, 12.2,
];
/// Where the rows start as the fan opens: this far above the window's bottom, which is the tile's middle.
const START_RISE: f64 = 6.0;

/// How the folder's entries are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Fan,
    Grid,
}

/// Fan or grid for `count` entries: the setting's choice, and a grid for a folder with more entries than the fan holds,
/// or when the bar is not at the bottom of the output or the output has no room for the fan.
pub fn shape(style: StackStyle, count: usize, room: usize, bottom: bool) -> Shape {
    let fits = bottom && count <= MAX_ENTRIES.min(room);
    match style {
        StackStyle::Grid => Shape::Grid,
        StackStyle::Auto | StackStyle::Fan if fits => Shape::Fan,
        StackStyle::Auto | StackStyle::Fan => Shape::Grid,
    }
}

/// How many entries fit in the fan on an output `screen_height` points tall when the lowest row's centre is `lowest`
/// points above the output's bottom edge: the rows rise until the top one, the button, is under the output's top.
pub fn room(screen_height: f64, lowest: f64) -> usize {
    let rows = ((screen_height - lowest - ICON / 2.0 - MARGIN) / PITCH).floor();
    if rows < 1.0 {
        0
    } else {
        rows as usize
    }
}

/// How wide the pill of `name` is, roughly: used to find the parts a pointer is on before they are drawn. Wide
/// characters count a full em, the rest 0.56 of one.
pub fn estimated_pill_width(name: &str) -> f64 {
    let ems: f64 = name
        .chars()
        .map(|c| if c.is_ascii() { 0.56 } else { 1.0 })
        .sum();
    (ems * PILL_FONT_PT + 2.0 * PILL_PADDING).min(PILL_MAX_WIDTH)
}

/// How far the fan has come out at `now`, from 0 (in the tile) to 1: it opened at `opened`, and, if it is closing, began
/// at `closing.0` from the progress `closing.1`. Without motion it is out or gone.
pub fn progress(now: f64, opened: f64, closing: Option<(f64, f64)>, reduce_motion: bool) -> f64 {
    if reduce_motion {
        return if closing.is_some() { 0.0 } else { 1.0 };
    }
    match closing {
        Some((start, from)) => {
            let u = ((now - start) / CLOSE_S).clamp(0.0, 1.0);
            from * (1.0 - CLOSE_EASE.eval(u))
        }
        None => OPEN_EASE.eval(((now - opened) / OPEN_S).clamp(0.0, 1.0)),
    }
}

/// Whether the fan still moves at `now`.
pub fn moving(now: f64, opened: f64, closing: Option<f64>) -> bool {
    match closing {
        Some(start) => now < start + CLOSE_S,
        None => now < opened + OPEN_S,
    }
}

/// Where the parts of a fan of `entries` entries and a button go in its window, in points. The window is as wide on one
/// side of the column of icons as on the other, so it stands centred over the tile; its bottom edge is the tile's middle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FanLayout {
    pub entries: usize,
    pub width: f64,
    pub height: f64,
    /// How far above the window's bottom the lowest row's centre is.
    lowest: f64,
}

impl FanLayout {
    /// The fan of `names`' entries (and the button, labelled `button`), whose lowest row's centre is `lowest` points
    /// above the window's bottom edge.
    pub fn new(names: &[&str], button: &str, lowest: f64) -> Self {
        let widest = names
            .iter()
            .copied()
            .chain(std::iter::once(button))
            .map(estimated_pill_width)
            .fold(0.0, f64::max);
        let left = ICON / 2.0 + PILL_GAP + widest * 1.25 + MARGIN;
        let right = ICON / 2.0 + MARGIN + drift(names.len());
        Self {
            entries: names.len(),
            width: 2.0 * left.max(right).ceil(),
            height: (lowest + names.len() as f64 * PITCH + ICON / 2.0 + MARGIN).ceil(),
            lowest,
        }
    }

    /// The number of rows: the entries and the button.
    pub fn rows(&self) -> usize {
        self.entries + 1
    }

    /// The centre of row `row`'s icon at progress `p`, in the window.
    pub fn centre(&self, row: usize, p: f64) -> Point {
        let p = p.clamp(0.0, 1.0);
        let rest_up = self.lowest + row as f64 * PITCH;
        let up = START_RISE + (rest_up - START_RISE) * p;
        Point::new(self.width / 2.0 + drift(row) * p, self.height - up)
    }

    /// How far row `row` is turned clockwise at progress `p`, in radians.
    pub fn tilt(&self, row: usize, p: f64) -> f64 {
        tilt_degrees(row).to_radians() * p.clamp(0.0, 1.0)
    }

    /// The band of the window row `row` stands in, full width, at rest.
    pub fn band(&self, row: usize) -> Rect {
        let c = self.centre(row, 1.0);
        Rect::new(0.0, c.y - PITCH / 2.0, self.width, PITCH)
    }

    /// The part of the window a press on row `row` (named `name`, `None` for the button's pill) takes at rest: its icon
    /// and its pill, with room for the difference between the pill's real width and its estimate.
    pub fn hit_rect(&self, row: usize, name: &str) -> Rect {
        let c = self.centre(row, 1.0);
        let pill = estimated_pill_width(name) * 1.25;
        let left = c.x - ICON / 2.0 - PILL_GAP - pill;
        Rect::new(
            left,
            c.y - PITCH / 2.0,
            c.x + ICON / 2.0 + 4.0 - left,
            PITCH,
        )
    }

    /// The row a press at `p` is on, at rest: an entry's index, or `entries` for the button.
    pub fn hit(&self, p: Point, names: &[&str], button: &str) -> Option<usize> {
        (0..self.rows()).find(|&row| {
            let name = names.get(row).copied().unwrap_or(button);
            self.hit_rect(row, name).contains(p)
        })
    }
}

fn drift(row: usize) -> f64 {
    DRIFT[row.min(DRIFT.len() - 1)]
}

fn tilt_degrees(row: usize) -> f64 {
    TILT_DEGREES[row.min(TILT_DEGREES.len() - 1)]
}

/// The names of `entries`, for [`FanLayout`].
pub fn names(entries: &[StackEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}
