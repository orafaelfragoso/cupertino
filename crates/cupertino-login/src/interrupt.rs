//! The panel that interrupts a log out when an application has not quit in time ([`crate::logout::QUIT_TIMEOUT_S`]): Cancel
//! stops the log out, Force Quit kills the application and the log out goes on. Pure: frames, words and keys.
//!
//! The layout is the design's interrupt panel: content 413 x 128, a 64 pt icon at (20, 13), a bold
//! 13 pt title at (102, 13) and a 10 pt info line at (102, 55), both 293 wide, and the buttons on y 83.

use cupertino_geometry::{Point, Rect};

use crate::model::Key;
use crate::panel::CANCEL_LABEL;

/// Content size.
pub const WIDTH: f64 = 413.0;
pub const HEIGHT: f64 = 128.0;
/// Width at which the title and the info line wrap (text field width).
pub const WRAP_WIDTH: f64 = 293.0;
/// Text sizes (bold system font 13, system font 10).
pub const TITLE_SIZE: f64 = 13.0;
pub const INFO_SIZE: f64 = 10.0;
const ICON: Rect = Rect::new(20.0, 13.0, 64.0, 64.0);
const TEXT_LEFT: f64 = 102.0;
const TITLE_TOP: f64 = 13.0;
const INFO_TOP: f64 = 55.0;
const BUTTONS_TOP: f64 = 83.0;
/// Button height of the confirmation panels; approximate for this panel.
const BUTTON_HEIGHT: f64 = 32.0;
/// Buttons' x and width, Cancel then Force Quit, right-aligned to the text column's end (approximate).
const CANCEL_COLUMN: (f64, f64) = (TEXT_LEFT + WRAP_WIDTH - 100.0 - 83.0 + 2.0, 83.0);
const FORCE_QUIT_COLUMN: (f64, f64) = (TEXT_LEFT + WRAP_WIDTH - 100.0, 100.0);

pub const FORCE_QUIT_LABEL: &str = "Force Quit";

/// What the user chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Stop the log out; the applications that quit stay closed.
    Cancel,
    ForceQuit,
}

/// One button of the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InterruptButton {
    pub frame: Rect,
    pub label: &'static str,
    pub choice: Choice,
}

/// Where everything of the panel sits, in points from its top-left corner.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    pub icon: Rect,
    /// The title's box; its text wraps at [`WRAP_WIDTH`] from the top.
    pub title: Rect,
    pub info: Rect,
    /// Cancel, then Force Quit.
    pub buttons: [InterruptButton; 2],
}

impl Frames {
    /// The panel's fixed layout.
    pub fn new() -> Frames {
        let button = |(x, width): (f64, f64), label, choice| InterruptButton {
            frame: Rect::new(x, BUTTONS_TOP, width, BUTTON_HEIGHT),
            label,
            choice,
        };
        Frames {
            icon: ICON,
            title: Rect::new(TEXT_LEFT, TITLE_TOP, WRAP_WIDTH, INFO_TOP - TITLE_TOP),
            info: Rect::new(TEXT_LEFT, INFO_TOP, WRAP_WIDTH, BUTTONS_TOP - INFO_TOP),
            buttons: [
                button(CANCEL_COLUMN, CANCEL_LABEL, Choice::Cancel),
                button(FORCE_QUIT_COLUMN, FORCE_QUIT_LABEL, Choice::ForceQuit),
            ],
        }
    }

    /// The choice a click at `point` (panel-local points) makes, if it is on a button.
    pub fn button_at(&self, point: Point) -> Option<Choice> {
        self.buttons
            .iter()
            .find(|b| b.frame.contains(point))
            .map(|b| b.choice)
    }
}

impl Default for Frames {
    fn default() -> Self {
        Frames::new()
    }
}

/// The title for the application `app` (approximate words).
pub fn title(app: &str) -> String {
    format!("The computer was not logged out because \u{201c}{app}\u{201d} did not quit.")
}

/// The info line for the application `app` (approximate words).
pub fn info(app: &str) -> String {
    format!("To quit \u{201c}{app}\u{201d} without saving and go on logging out, click Force Quit.")
}

/// Escape cancels. Return does nothing: Force Quit loses unsaved work, so it is not the default button (approximate).
pub fn press(key: Key) -> Option<Choice> {
    match key {
        Key::Escape => Some(Choice::Cancel),
        Key::Return | Key::Char(_) | Key::Backspace => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frames_are_the_references_and_stay_inside_the_panel() {
        let f = Frames::new();
        assert_eq!((f.icon.x(), f.icon.y(), f.icon.width()), (20.0, 13.0, 64.0));
        assert_eq!(
            (f.title.x(), f.title.y(), f.title.width()),
            (102.0, 13.0, 293.0)
        );
        assert_eq!((f.info.x(), f.info.y()), (102.0, 55.0));
        let [cancel, force] = f.buttons;
        assert_eq!((cancel.frame.y(), force.frame.y()), (83.0, 83.0));
        assert!(
            cancel.frame.max_x() <= force.frame.x() + 2.0 && cancel.frame.x() > f.icon.max_x(),
            "Cancel left of Force Quit, clear of the icon"
        );
        assert_eq!(
            force.frame.max_x(),
            f.title.max_x(),
            "the buttons end with the text column"
        );
        assert!(force.frame.max_x() <= WIDTH && force.frame.max_y() <= HEIGHT);
    }

    #[test]
    fn clicks_hit_a_button_and_escape_cancels() {
        let f = Frames::new();
        assert_eq!(
            f.button_at(f.buttons[1].frame.center()),
            Some(Choice::ForceQuit)
        );
        assert_eq!(
            f.button_at(f.buttons[0].frame.center()),
            Some(Choice::Cancel)
        );
        assert_eq!(f.button_at(f.icon.center()), None);
        assert_eq!(press(Key::Escape), Some(Choice::Cancel));
        assert_eq!(press(Key::Return), None, "Force Quit is not the default");
    }

    #[test]
    fn the_words_name_the_application() {
        assert!(title("Editor").contains("\u{201c}Editor\u{201d}"));
        assert!(info("Editor").contains("Force Quit"));
    }
}
