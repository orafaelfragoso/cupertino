//! The panel that interrupts a log out, restart or shut down when an application has not quit in time
//! ([`crate::logout::QUIT_TIMEOUT_S`]), with words for why: the application stopped it (it kept its windows), it did not
//! finish quitting, or it does not respond. Try Again asks the applications to quit again, Cancel stops the log out,
//! and Force Quit (when the application failed or hangs) asks a second, smaller question before it kills the
//! application and the log out goes on. Pure: frames, words and keys.
//!
//! The layout: content 413 wide and at least 128 high, a 64 pt icon at (20, 13), a bold 13 pt title at (102, 13) and the
//! info line 10 pt under it, both 293 wide and as many 16 pt lines as their words take, and the buttons 12 pt under the
//! info line (on y 83 at the least, clear of the icon), right-aligned to the text column's end, 17 pt above the bottom.

use cupertino_geometry::Rect;

use crate::model::Key;
use crate::panel::{Action, CANCEL_LABEL};

/// Content width, and the least content height: a two-line title and a one-line info line.
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
/// One line of the title or the info line.
const LINE: f64 = 16.0;
const TITLE_INFO_GAP: f64 = 10.0;
const INFO_BUTTONS_GAP: f64 = 12.0;
/// The buttons' least top, and the room under them.
const BUTTONS_TOP: f64 = 83.0;
const BOTTOM: f64 = 17.0;
/// Button height and gap: the capsules of the confirmation panels.
const BUTTON_HEIGHT: f64 = crate::panel::BUTTON_HEIGHT;
const BUTTON_GAP: f64 = crate::panel::BUTTON_GAP;

pub const TRY_AGAIN_LABEL: &str = "Try Again";
pub const FORCE_QUIT_LABEL: &str = "Force Quit";

/// Why the application is still there when its time to quit ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// It answers and keeps every window it was asked to close, with nothing else open: it stopped the log out.
    Cancelled,
    /// It answers but has not finished: some of its windows are still open, or a question of its own is.
    FailedToQuit,
    /// It does not answer the server.
    NotResponding,
}

impl Reason {
    /// The reason the server's quit status names (`QuitStatus`): `kept-windows`, `not-responding`, and anything else
    /// (`closing`, a failed call) as an application that did not finish.
    pub fn of_status(status: &str) -> Reason {
        match status {
            "kept-windows" => Reason::Cancelled,
            "not-responding" => Reason::NotResponding,
            _ => Reason::FailedToQuit,
        }
    }

    /// Force Quit is offered for an application that failed to quit or hangs; one that stopped the log out on purpose is
    /// asked again or left alone.
    pub fn offers_force_quit(self) -> bool {
        self != Reason::Cancelled
    }
}

/// Which question the panel asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Why the log out stopped, with Try Again, Cancel and (by the reason) Force Quit.
    Ask,
    /// Whether to force the application to quit, after Force Quit: Cancel goes back, Force Quit kills it.
    ConfirmForceQuit,
}

/// What the person chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Ask the applications to quit again: the log out starts over.
    TryAgain,
    /// Stop the log out; the applications that quit stay closed.
    Cancel,
    /// Ask whether to force the application to quit.
    ForceQuit,
    /// Force it: kill the application, and the log out goes on.
    ConfirmForceQuit,
    /// Leave the force quit question for the first one.
    Back,
}

/// One button of the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InterruptButton {
    pub frame: Rect,
    pub label: &'static str,
    pub choice: Choice,
    /// Return presses it.
    pub default: bool,
}

/// Where everything of the panel sits, in points from its top-left corner.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    /// The content's height: [`HEIGHT`] at the least.
    pub height: f64,
    pub icon: Rect,
    /// The title's box; its text wraps at [`WRAP_WIDTH`] from the top.
    pub title: Rect,
    pub info: Rect,
    /// Left to right, the last against the text column's end.
    pub buttons: Vec<InterruptButton>,
}

/// The buttons of a stage, left to right with their widths: on the question Force Quit apart on the left, then Cancel,
/// then Try Again, the default, at the end; on the force quit question Cancel (back), then Force Quit. Neither button of
/// the second question is the default, so a Return held from typing kills nothing.
fn buttons(reason: Reason, stage: Stage) -> Vec<(&'static str, Choice, f64, bool)> {
    match stage {
        Stage::Ask => {
            let mut row = Vec::new();
            if reason.offers_force_quit() {
                row.push((FORCE_QUIT_LABEL, Choice::ForceQuit, FORCE_QUIT_WIDTH, false));
            }
            row.push((CANCEL_LABEL, Choice::Cancel, CANCEL_WIDTH, false));
            row.push((TRY_AGAIN_LABEL, Choice::TryAgain, TRY_AGAIN_WIDTH, true));
            row
        }
        Stage::ConfirmForceQuit => vec![
            (CANCEL_LABEL, Choice::Back, CANCEL_WIDTH, false),
            (
                FORCE_QUIT_LABEL,
                Choice::ConfirmForceQuit,
                FORCE_QUIT_WIDTH,
                false,
            ),
        ],
    }
}

/// The buttons' widths: room for their labels at 13 pt in a capsule (approximate).
const CANCEL_WIDTH: f64 = 83.0;
const TRY_AGAIN_WIDTH: f64 = 90.0;
const FORCE_QUIT_WIDTH: f64 = 100.0;

impl Frames {
    /// The panel's layout for `reason` at `stage`, its title wrapped to `title_lines` lines and its info line to
    /// `info_lines` (each at least one).
    pub fn new(reason: Reason, stage: Stage, title_lines: u32, info_lines: u32) -> Frames {
        let title_height = LINE * f64::from(title_lines.max(1));
        let info_top = TITLE_TOP + title_height + TITLE_INFO_GAP;
        let info_height = LINE * f64::from(info_lines.max(1));
        let buttons_top = (info_top + info_height + INFO_BUTTONS_GAP).max(BUTTONS_TOP);
        let row = buttons(reason, stage);
        let total: f64 =
            row.iter().map(|b| b.2).sum::<f64>() + BUTTON_GAP * (row.len() as f64 - 1.0);
        let mut x = TEXT_LEFT + WRAP_WIDTH - total;
        let buttons = row
            .into_iter()
            .map(|(label, choice, width, default)| {
                let frame = Rect::new(x, buttons_top, width, BUTTON_HEIGHT);
                x += width + BUTTON_GAP;
                InterruptButton {
                    frame,
                    label,
                    choice,
                    default,
                }
            })
            .collect();
        Frames {
            height: buttons_top + BUTTON_HEIGHT + BOTTOM,
            icon: ICON,
            title: Rect::new(TEXT_LEFT, TITLE_TOP, WRAP_WIDTH, title_height),
            info: Rect::new(TEXT_LEFT, info_top, WRAP_WIDTH, info_height),
            buttons,
        }
    }
}

/// What is stopped, in words: the log out, the restart or the shut down.
fn what(action: Action) -> &'static str {
    match action {
        Action::LogOut | Action::Sleep => "log out",
        Action::Restart => "restart",
        Action::ShutDown => "shut down",
    }
}

/// The title for the application `app` that stopped `action` for `reason`, at `stage`.
pub fn title(reason: Reason, stage: Stage, action: Action, app: &str) -> String {
    let what = what(action);
    match (stage, reason) {
        (Stage::ConfirmForceQuit, _) => format!("Force \u{201c}{app}\u{201d} to quit?"),
        (Stage::Ask, Reason::Cancelled) => format!("\u{201c}{app}\u{201d} cancelled the {what}."),
        (Stage::Ask, Reason::FailedToQuit) => {
            format!("The {what} stopped because \u{201c}{app}\u{201d} did not finish quitting.")
        }
        (Stage::Ask, Reason::NotResponding) => {
            format!("The {what} stopped because \u{201c}{app}\u{201d} is not responding.")
        }
    }
}

/// The info line under the title.
pub fn info(reason: Reason, stage: Stage, action: Action, app: &str) -> String {
    let what = what(action);
    match (stage, reason) {
        (Stage::ConfirmForceQuit, _) => {
            format!(
                "Changes \u{201c}{app}\u{201d} has not saved are lost, then the {what} goes on."
            )
        }
        (Stage::Ask, Reason::Cancelled) => {
            format!("Deal with what it asks, then try again, or cancel the {what}.")
        }
        (Stage::Ask, Reason::FailedToQuit) => {
            format!("Try again, or force it to quit and go on with the {what}.")
        }
        (Stage::Ask, Reason::NotResponding) => {
            format!("Wait and try again, or force it to quit and go on with the {what}.")
        }
    }
}

/// Escape cancels the question (on the force quit question it goes back); Return presses the default button, Try
/// Again on the first question and nothing on the force quit question.
pub fn press(stage: Stage, key: Key) -> Option<Choice> {
    match (stage, key) {
        (Stage::Ask, Key::Escape) => Some(Choice::Cancel),
        (Stage::ConfirmForceQuit, Key::Escape) => Some(Choice::Back),
        (Stage::Ask, Key::Return) => Some(Choice::TryAgain),
        (Stage::ConfirmForceQuit, Key::Return) | (_, Key::Char(_) | Key::Backspace) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REASONS: [Reason; 3] = [
        Reason::Cancelled,
        Reason::FailedToQuit,
        Reason::NotResponding,
    ];

    fn choices(reason: Reason, stage: Stage) -> Vec<Choice> {
        Frames::new(reason, stage, 2, 1)
            .buttons
            .iter()
            .map(|b| b.choice)
            .collect()
    }

    #[test]
    fn force_quit_is_offered_only_when_the_application_failed_or_hangs() {
        assert_eq!(
            choices(Reason::Cancelled, Stage::Ask),
            [Choice::Cancel, Choice::TryAgain]
        );
        for reason in [Reason::FailedToQuit, Reason::NotResponding] {
            assert_eq!(
                choices(reason, Stage::Ask),
                [Choice::ForceQuit, Choice::Cancel, Choice::TryAgain],
                "{reason:?}"
            );
            assert_eq!(
                choices(reason, Stage::ConfirmForceQuit),
                [Choice::Back, Choice::ConfirmForceQuit],
                "{reason:?}: a second question before the kill"
            );
        }
    }

    #[test]
    fn the_buttons_end_with_the_text_column_8_pt_apart_clear_of_the_icon() {
        for reason in REASONS {
            for stage in [Stage::Ask, Stage::ConfirmForceQuit] {
                let f = Frames::new(reason, stage, 2, 1);
                let last = f.buttons.last().unwrap().frame;
                assert_eq!(last.max_x(), f.title.max_x(), "{reason:?} {stage:?}");
                for pair in f.buttons.windows(2) {
                    assert_eq!(pair[1].frame.x() - pair[0].frame.max_x(), 8.0);
                }
                for b in &f.buttons {
                    assert_eq!((b.frame.y(), b.frame.height()), (83.0, 28.0));
                    assert!(b.frame.x() > f.icon.max_x() && b.frame.max_y() <= HEIGHT);
                }
            }
        }
        let f = Frames::new(Reason::NotResponding, Stage::Ask, 2, 1);
        assert_eq!(
            (f.icon, f.title.origin, f.info.origin),
            (
                Rect::new(20.0, 13.0, 64.0, 64.0),
                cupertino_geometry::Point::new(102.0, 13.0),
                cupertino_geometry::Point::new(102.0, 55.0)
            )
        );
        assert_eq!(f.height, HEIGHT);
    }

    /// The info line stands 10 pt under the title and the buttons 12 pt under the info line, however many lines they
    /// take; a short text keeps the buttons clear of the icon and the panel at its least height.
    #[test]
    fn the_panel_grows_with_its_wrapped_title_and_info_line() {
        let f = Frames::new(Reason::NotResponding, Stage::Ask, 2, 2);
        assert_eq!(f.info, Rect::new(102.0, 55.0, WRAP_WIDTH, 32.0));
        assert_eq!(f.buttons[0].frame.y(), 99.0);
        assert_eq!(f.height, 144.0);
        let f = Frames::new(Reason::NotResponding, Stage::Ask, 3, 2);
        assert_eq!(
            (f.title.max_y() + 10.0, f.info.max_y() + 12.0),
            (f.info.y(), f.buttons[0].frame.y())
        );
        assert_eq!(f.height - f.buttons[0].frame.max_y(), 17.0);
        let f = Frames::new(Reason::Cancelled, Stage::ConfirmForceQuit, 1, 1);
        assert_eq!(f.info.y(), 39.0);
        assert_eq!((f.buttons[0].frame.y(), f.height), (83.0, HEIGHT));
    }

    #[test]
    fn try_again_is_the_default_and_escape_cancels_or_goes_back() {
        assert_eq!(press(Stage::Ask, Key::Return), Some(Choice::TryAgain));
        assert_eq!(press(Stage::Ask, Key::Escape), Some(Choice::Cancel));
        assert_eq!(
            press(Stage::ConfirmForceQuit, Key::Return),
            None,
            "Return does not kill"
        );
        assert_eq!(
            press(Stage::ConfirmForceQuit, Key::Escape),
            Some(Choice::Back)
        );
        assert_eq!(press(Stage::Ask, Key::Char('f')), None);
        let defaults: Vec<&str> = Frames::new(Reason::FailedToQuit, Stage::Ask, 2, 1)
            .buttons
            .iter()
            .filter(|b| b.default)
            .map(|b| b.label)
            .collect();
        assert_eq!(defaults, [TRY_AGAIN_LABEL]);
        assert!(
            Frames::new(Reason::FailedToQuit, Stage::ConfirmForceQuit, 2, 1)
                .buttons
                .iter()
                .all(|b| !b.default)
        );
    }

    #[test]
    fn each_reason_has_its_own_words_naming_the_application_and_what_stopped() {
        let titles: Vec<String> = REASONS
            .iter()
            .map(|&r| title(r, Stage::Ask, Action::LogOut, "Editor"))
            .collect();
        for (i, t) in titles.iter().enumerate() {
            assert!(
                t.contains("\u{201c}Editor\u{201d}") && t.contains("log out"),
                "{t}"
            );
            assert!(!titles[i + 1..].contains(t), "{t} is said twice");
        }
        assert!(
            title(Reason::NotResponding, Stage::Ask, Action::Restart, "Editor").contains("restart")
        );
        assert!(
            info(Reason::FailedToQuit, Stage::Ask, Action::ShutDown, "Editor").contains("force")
        );
        assert!(!info(Reason::Cancelled, Stage::Ask, Action::LogOut, "Editor").contains("force"));
        assert_eq!(
            title(
                Reason::FailedToQuit,
                Stage::ConfirmForceQuit,
                Action::LogOut,
                "Editor"
            ),
            "Force \u{201c}Editor\u{201d} to quit?"
        );
        assert!(info(
            Reason::NotResponding,
            Stage::ConfirmForceQuit,
            Action::LogOut,
            "Editor"
        )
        .contains("not saved"));
    }

    #[test]
    fn the_server_status_names_the_reason() {
        assert_eq!(Reason::of_status("kept-windows"), Reason::Cancelled);
        assert_eq!(Reason::of_status("not-responding"), Reason::NotResponding);
        assert_eq!(Reason::of_status("closing"), Reason::FailedToQuit);
        assert_eq!(Reason::of_status(""), Reason::FailedToQuit);
    }
}
