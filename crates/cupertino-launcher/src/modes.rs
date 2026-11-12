//! The mode buttons: round buttons to the right of the search field that switch the launcher to a scope (the
//! application browser, the files scope). This module is their model: which scopes there are, the geometry of the
//! group the field and the buttons make, the shortcut each shows while the pointer is over it, and what the keys do
//! while they are on the buttons. It is pure; the buttons' presentation is [`crate::motion`]'s and their nodes are
//! `view::modes`.
//!
//! The group is [`GROUP_WIDTH`] wide: the search field, a gap, then the buttons a pitch apart. A button is a circle;
//! while the group is not yet settled the buttons are inside the field's end and come out of it as one shape.

use crate::motion::GROUP_WIDTH;
use crate::nav::View;
use cupertino_kit::geometry::Rect;

/// A button's diameter, the distance between the starts of two, and so the gap between two buttons and between the
/// field and the first.
pub const BUTTON_SIZE: f64 = 55.0;
pub const BUTTON_PITCH: f64 = 64.0;
pub const BUTTON_GAP: f64 = BUTTON_PITCH - BUTTON_SIZE;
/// The symbol in a button.
pub const GLYPH_SIZE: f64 = 24.0;
/// The shortcut chips shown in the field's trailing end while the pointer is over a button: their height, the
/// width of the digit's, the gap between them, their corner radius, their text size and the room from the field's
/// edge to the last one.
pub const CHIP_HEIGHT: f64 = 18.0;
pub const CHIP_DIGIT_WIDTH: f64 = 17.0;
pub const CHIP_GAP: f64 = 5.0;
pub const CHIP_RADIUS: f64 = 4.5;
pub const CHIP_TEXT_SIZE: f64 = 11.0;
pub const CHIP_END_INSET: f64 = 21.0;
/// What the first chip says: the platform's primary modifier.
pub const MODIFIER_NAME: &str = "Super";

/// A scope a button switches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Every application, by category.
    Applications,
    /// Recent files, and the files the query finds.
    Files,
}

impl Mode {
    /// What the button is called.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Applications => "Applications",
            Mode::Files => "Files",
        }
    }

    /// The button's symbol.
    pub fn symbol(self) -> &'static str {
        match self {
            Mode::Applications => "symbol.applications",
            Mode::Files => "symbol.folder",
        }
    }

    /// The view it opens.
    pub fn view(self) -> View {
        match self {
            Mode::Applications => View::Browse,
            Mode::Files => View::Files,
        }
    }

    /// The scope of a view, if a button leads to it.
    pub fn of_view(view: View) -> Option<Mode> {
        match view {
            View::Browse => Some(Mode::Applications),
            View::Files => Some(Mode::Files),
            _ => None,
        }
    }
}

/// The digit of the shortcut of the button at `index` (the first is 1).
pub fn digit(index: usize) -> char {
    char::from_digit((index + 1) as u32 % 10, 10).unwrap_or('?')
}

/// The index of the button the shortcut digit `c` stands for.
pub fn index_of_digit(c: char) -> Option<usize> {
    c.to_digit(10)
        .filter(|d| (1..=9).contains(d))
        .map(|d| d as usize - 1)
}

/// The search field's width while `count` buttons stand beside it.
pub fn field_width(count: usize) -> f64 {
    GROUP_WIDTH - count as f64 * BUTTON_PITCH
}

/// A button budding out of the field starts as a small blob, this share of its diameter, and is whole once it is
/// [`BUD_GROWN`] of the way to its place; each button further out lags the one before it by [`BUD_LAG`] of the way, so
/// while they bud the nearest is the largest and they shrink along the row.
pub const BUD_START: f64 = 0.4;
pub const BUD_GROWN: f64 = 0.6;
pub const BUD_LAG: f64 = 0.08;

/// The share of its diameter button `index` has when `spread` of the way out of the field: [`BUD_START`] inside the
/// field, easing to whole by [`BUD_GROWN`] (later the further out it is), and whole past its place.
pub fn bud_size(index: usize, spread: f64) -> f64 {
    let t = ((spread - index as f64 * BUD_LAG) / BUD_GROWN).clamp(0.0, 1.0);
    // ease out: it swells fast as it leaves the field and settles into its size
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    BUD_START + (1.0 - BUD_START) * eased
}

/// Button `index` of `count`, `height` points tall in the row its field fills, relative to the group's left edge and the
/// row's top, when `spread` of the way from inside the field's end (0) to its place (1). Its centre moves along the
/// row while it grows from a bud ([`bud_size`]).
pub fn button_rect(index: usize, count: usize, spread: f64, height: f64) -> Rect {
    let field = field_width(count);
    let start = field - BUTTON_SIZE;
    let end = field + BUTTON_GAP + index as f64 * BUTTON_PITCH;
    let side = BUTTON_SIZE * bud_size(index, spread);
    let centre = start + (end - start) * spread + BUTTON_SIZE / 2.0;
    Rect::new(centre - side / 2.0, (height - side) / 2.0, side, side)
}

/// Where the keys are while they are on the buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Focus {
    /// The button the keys are on, none while they are in the field.
    pub on: Option<usize>,
}

/// What a key does while the buttons take it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// The keys go to this button.
    To(usize),
    /// The keys go back to the field.
    Field,
    /// Press the button the keys are on.
    Press(usize),
}

impl Focus {
    /// Tab (`back`: Shift+Tab) from where the keys are among `count` buttons: from the field to the first (the last)
    /// button, along the buttons, and from the last (first) back to the field.
    pub fn tab(self, back: bool, count: usize) -> Move {
        match (self.on, back) {
            _ if count == 0 => Move::Field,
            (None, false) => Move::To(0),
            (None, true) => Move::To(count - 1),
            (Some(i), false) if i + 1 < count => Move::To(i + 1),
            (Some(i), true) if i > 0 => Move::To(i - 1),
            _ => Move::Field,
        }
    }

    /// Left or Right (`forward`) while on a button: the next one, stopping at the ends.
    pub fn step(self, forward: bool, count: usize) -> Option<Move> {
        let i = self.on?;
        let next = if forward {
            (i + 1).min(count.saturating_sub(1))
        } else {
            i.saturating_sub(1)
        };
        Some(Move::To(next))
    }

    /// Return or Space while on a button.
    pub fn press(self) -> Option<Move> {
        self.on.map(Move::Press)
    }
}

#[cfg(test)]
mod bud_tests {
    use super::*;

    #[test]
    fn a_button_buds_small_and_grows_whole_before_it_reaches_its_place() {
        assert_eq!(bud_size(0, 0.0), BUD_START);
        assert!(bud_size(0, BUD_GROWN / 2.0) > BUD_START && bud_size(0, BUD_GROWN / 2.0) < 1.0);
        assert_eq!(bud_size(0, BUD_GROWN), 1.0);
        // settled, and overshooting past its place, it is whole
        for spread in [1.0, 1.05] {
            for index in 0..4 {
                assert_eq!(bud_size(index, spread), 1.0, "{index} at {spread}");
                let r = button_rect(index, 4, spread, 57.0);
                assert_eq!((r.width(), r.height()), (BUTTON_SIZE, BUTTON_SIZE));
            }
        }
    }

    #[test]
    fn while_they_bud_the_nearest_button_is_the_largest() {
        for spread in [0.1, 0.3, 0.5] {
            let sizes: Vec<f64> = (0..4)
                .map(|i| button_rect(i, 4, spread, 57.0).width())
                .collect();
            assert!(
                sizes.windows(2).all(|w| w[0] >= w[1]),
                "{sizes:?} at {spread}"
            );
            assert!(sizes[3] < sizes[0], "{sizes:?} at {spread}");
        }
    }

    #[test]
    fn a_budding_button_stays_centred_on_the_row_and_on_its_path() {
        let r = button_rect(1, 4, 0.2, 57.0);
        assert!((r.y() + r.height() / 2.0 - 28.5).abs() < 1e-9);
        let whole = button_rect(1, 4, 1.0, 57.0);
        assert!((whole.x() - (field_width(4) + BUTTON_GAP + BUTTON_PITCH)).abs() < 1e-9);
    }
}
