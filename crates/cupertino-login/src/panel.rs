//! The confirmation panels: log out, restart and shut down, whose countdown ends in the action, and the
//! power-button panel with four buttons and no countdown. What each asks, where its buttons sit and what its
//! keys do. Pure: time is an absolute `f64` in seconds on the caller's clock.

use cupertino_geometry::Rect;

use crate::model::Key;

/// Seconds until the action is performed without an answer (confirmation countdown, ticking 1.0 s).
pub const COUNTDOWN_SECONDS: f64 = 60.0;

/// What the panel asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    LogOut,
    Restart,
    ShutDown,
    /// Shown by a held power button: Shut Down, Cancel, Sleep and Restart; no info line, no countdown.
    PowerButton,
}

/// What a panel carries out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    LogOut,
    Restart,
    ShutDown,
    Sleep,
}

impl Action {
    /// The name in the log line `panel confirmed: <name>`.
    pub fn log_name(self) -> &'static str {
        match self {
            Action::LogOut => "log-out",
            Action::Restart => "restart",
            Action::ShutDown => "shut-down",
            Action::Sleep => "sleep",
        }
    }
}

/// What the user decided, or the countdown decided for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Do(Action),
    Cancel,
}

/// The words of a panel (reference strings).
pub struct Text {
    pub title: &'static str,
    /// The sentence under the title up to the countdown, which the formatter fills in (`WILL_*_INFO`, the `%@`); `None` on
    /// the power-button panel, which has no info line.
    info_before: Option<&'static str>,
    pub confirm: &'static str,
}

pub const CANCEL_LABEL: &str = "Cancel";

/// The checkbox of the log out, restart and shut down panels: whether the applications open now start again at the next
/// login ([`crate::restore`]).
pub const REOPEN_LABEL: &str = "Reopen windows when logging back in";

impl Kind {
    /// The name in the log lines `panel shown: <name>`.
    pub fn log_name(self) -> &'static str {
        match self {
            Kind::LogOut => "log-out",
            Kind::Restart => "restart",
            Kind::ShutDown => "shut-down",
            Kind::PowerButton => "power-button",
        }
    }

    pub fn text(self) -> &'static Text {
        const LOG_OUT: Text = Text {
            title: "Are you sure you want to quit all applications and log out now?",
            info_before: Some("If you do nothing, you will be logged out automatically in "),
            confirm: "Log Out",
        };
        const RESTART: Text = Text {
            title: "Are you sure you want to restart your computer now?",
            info_before: Some("If you do nothing, the computer will restart automatically in "),
            confirm: "Restart",
        };
        const SHUT_DOWN: Text = Text {
            title: "Are you sure you want to shut down your computer now?",
            info_before: Some("If you do nothing, the computer will shut down automatically in "),
            confirm: "Shut Down",
        };
        const POWER_BUTTON: Text = Text {
            title: SHUT_DOWN.title,
            info_before: None,
            confirm: SHUT_DOWN.confirm,
        };
        match self {
            Kind::LogOut => &LOG_OUT,
            Kind::Restart => &RESTART,
            Kind::ShutDown => &SHUT_DOWN,
            Kind::PowerButton => &POWER_BUTTON,
        }
    }

    /// The line under the title: what happens when the countdown ends, with `seconds` left on it; `None` on a panel
    /// without a countdown.
    pub fn countdown_message(self, seconds: u32) -> Option<String> {
        self.text()
            .info_before
            .map(|before| format!("{before}{}.", format_countdown(seconds)))
    }

    /// What Return does, and the end of the countdown: the action of the default button.
    pub fn default_action(self) -> Action {
        match self {
            Kind::LogOut => Action::LogOut,
            Kind::Restart => Action::Restart,
            Kind::ShutDown | Kind::PowerButton => Action::ShutDown,
        }
    }

    /// The button that has the focus when the panel appears, which Space presses: the default button, but Cancel on the
    /// power-button panel, whose default Shut Down has no countdown, so that one Space shuts nothing down.
    pub fn focused(self) -> Decision {
        match self {
            Kind::PowerButton => Decision::Cancel,
            _ => Decision::Do(self.default_action()),
        }
    }

    /// The three confirmation panels count down to their default action; the power-button panel waits.
    fn has_countdown(self) -> bool {
        self != Kind::PowerButton
    }

    /// The three confirmation panels show the "Reopen windows" checkbox; the power-button panel does not, and its restart
    /// and shut down follow the choice the person last confirmed.
    pub fn has_checkbox(self) -> bool {
        self != Kind::PowerButton
    }
}

/// The countdown as the design's date components formatter writes it (full units style, minutes and
/// seconds allowed, at most two units): "1 minute", "59 seconds", "1 minute, 1 second". approximate: the formatter's
/// dropping of a zero unit and its English ", " joiner are read from its documented behaviour, not from a capture.
pub fn format_countdown(seconds: u32) -> String {
    let unit =
        |count: u32, name: &str| format!("{count} {name}{}", if count == 1 { "" } else { "s" });
    match (seconds / 60, seconds % 60) {
        (0, seconds) => unit(seconds, "second"),
        (minutes, 0) => unit(minutes, "minute"),
        (minutes, seconds) => format!("{}, {}", unit(minutes, "minute"), unit(seconds, "second")),
    }
}

/// The panel is an alert: this wide on every kind, its height following from the text, see [`Kind::frames`].
pub const WIDTH: f64 = 260.0;
/// The alert's padding above and around its content; the text sits [`TEXT_INSET`] further in than the buttons.
const PADDING_TOP: f64 = 22.0;
const PADDING_SIDE: f64 = 16.0;
const PADDING_BOTTOM: f64 = 16.0;
const TEXT_INSET: f64 = 6.0;
/// The text column's leading edge.
const TEXT_LEFT: f64 = PADDING_SIDE + TEXT_INSET;

/// Width at which the title, the info line and the checkbox row wrap: the text column.
pub const fn wrap_width(_kind: Kind) -> f64 {
    WIDTH - 2.0 * TEXT_LEFT
}

/// The vertical chain: the padding, the title (bold 13 pt on 16 pt lines), 10 pt, the info line (13 pt on 16 pt lines),
/// 12 pt, the checkbox row, 16 pt, the buttons, the padding.
pub const TITLE_LINE: f64 = 16.0;
pub const INFO_LINE: f64 = 16.0;
const TITLE_INFO_GAP: f64 = 10.0;
const INFO_CHECKBOX_GAP: f64 = 12.0;
const TEXT_BUTTONS_GAP: f64 = 16.0;
/// Height of a checkbox row whose label takes one line (the box and the line), and of each line of a label that wraps.
const CHECKBOX_HEIGHT: f64 = 18.0;
pub const CHECKBOX_LINE: f64 = 16.0;
/// The buttons: capsules this high, this far apart, side by side across the alert between its side padding, or
/// stacked full width when there are more than two.
pub const BUTTON_HEIGHT: f64 = 28.0;
pub const BUTTON_GAP: f64 = 8.0;

/// What each button of the power-button panel does and says, top to bottom: the default first, as an alert stacks its
/// buttons.
const POWER_BUTTONS: [(Decision, &str); 4] = [
    (Decision::Do(Action::ShutDown), "Shut Down"),
    (Decision::Cancel, CANCEL_LABEL),
    (Decision::Do(Action::Sleep), "Sleep"),
    (Decision::Do(Action::Restart), "Restart"),
];

/// One button of a panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelButton {
    pub frame: Rect,
    pub label: &'static str,
    /// What a click on it does.
    pub decision: Decision,
}

/// The "Reopen windows" checkbox of a panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelCheckbox {
    /// The row: the box and its label, which wraps in the text column; a click anywhere on it toggles.
    pub frame: Rect,
    pub label: &'static str,
}

/// Where everything of a panel sits, in points from its top-left corner. The panel has no icon.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    pub width: f64,
    pub height: f64,
    pub title: Rect,
    /// `None` on the power-button panel.
    pub info: Option<Rect>,
    /// `None` on the power-button panel.
    pub checkbox: Option<PanelCheckbox>,
    /// Cancel and the confirming button left to right, or the four buttons of the power-button panel top to bottom.
    pub buttons: Vec<PanelButton>,
}

impl Kind {
    /// The panel laid out for a title wrapped to `title_lines` lines, an info line wrapped to `info_lines` and a checkbox
    /// label wrapped to `checkbox_lines` (both ignored by the power-button panel, which has neither). Each element sits
    /// at the previous one's bottom edge and the height is the end of the chain; the width is fixed.
    pub fn frames(self, title_lines: u32, info_lines: u32, checkbox_lines: u32) -> Frames {
        let wrap = wrap_width(self);
        let title = Rect::new(
            TEXT_LEFT,
            PADDING_TOP,
            wrap,
            TITLE_LINE * f64::from(title_lines),
        );
        let info = self.text().info_before.map(|_| {
            Rect::new(
                TEXT_LEFT,
                title.max_y() + TITLE_INFO_GAP,
                wrap,
                INFO_LINE * f64::from(info_lines),
            )
        });
        let checkbox = info
            .filter(|_| self.has_checkbox())
            .map(|info| PanelCheckbox {
                frame: Rect::new(
                    TEXT_LEFT,
                    info.max_y() + INFO_CHECKBOX_GAP,
                    wrap,
                    CHECKBOX_HEIGHT.max(CHECKBOX_LINE * f64::from(checkbox_lines)),
                ),
                label: REOPEN_LABEL,
            });
        let text_bottom = checkbox
            .map(|c| c.frame.max_y())
            .or(info.map(|i| i.max_y()))
            .unwrap_or(title.max_y());
        let top = text_bottom + TEXT_BUTTONS_GAP;
        let across = WIDTH - 2.0 * PADDING_SIDE;
        let buttons: Vec<PanelButton> = match self {
            Kind::PowerButton => POWER_BUTTONS
                .iter()
                .enumerate()
                .map(|(row, &(decision, label))| PanelButton {
                    frame: Rect::new(
                        PADDING_SIDE,
                        top + row as f64 * (BUTTON_HEIGHT + BUTTON_GAP),
                        across,
                        BUTTON_HEIGHT,
                    ),
                    label,
                    decision,
                })
                .collect(),
            Kind::LogOut | Kind::Restart | Kind::ShutDown => {
                let width = (across - BUTTON_GAP) / 2.0;
                [
                    (CANCEL_LABEL, Decision::Cancel),
                    (self.text().confirm, Decision::Do(self.default_action())),
                ]
                .into_iter()
                .enumerate()
                .map(|(column, (label, decision))| PanelButton {
                    frame: Rect::new(
                        PADDING_SIDE + column as f64 * (width + BUTTON_GAP),
                        top,
                        width,
                        BUTTON_HEIGHT,
                    ),
                    label,
                    decision,
                })
                .collect()
            }
        };
        let bottom = buttons.iter().map(|b| b.frame.max_y()).fold(top, f64::max);
        Frames {
            width: WIDTH,
            height: bottom + PADDING_BOTTOM,
            title,
            info,
            checkbox,
            buttons,
        }
    }
}

/// A panel on screen since `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    kind: Kind,
    start: f64,
    /// The "Reopen windows" choice: the checkbox's state, or on the power-button panel the choice last confirmed.
    reopen: bool,
}

impl Panel {
    /// A panel whose checkbox starts at `reopen`, the choice the person last confirmed.
    pub fn new(kind: Kind, now: f64, reopen: bool) -> Self {
        Panel {
            kind,
            start: now,
            reopen,
        }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Whether the applications open now start again at the next login.
    pub fn reopen(&self) -> bool {
        self.reopen
    }

    /// Flip the checkbox; nothing on the power-button panel, which has none. The countdown goes on.
    pub fn toggle_reopen(&mut self) {
        if self.kind.has_checkbox() {
            self.reopen = !self.reopen;
        }
    }

    /// Space flips the checkbox (the panel's only control besides the buttons, which Return and Escape answer).
    pub fn toggles(&self, key: Key) -> bool {
        self.kind.has_checkbox() && key == Key::Char(' ')
    }

    /// Whole seconds left on the countdown, rounded up so that it reads 60 when the panel appears and 0 only at the end;
    /// `None` on a panel without a countdown.
    pub fn seconds_left(&self, now: f64) -> Option<u32> {
        self.kind.has_countdown().then(|| {
            (COUNTDOWN_SECONDS - (now - self.start))
                .clamp(0.0, COUNTDOWN_SECONDS)
                .ceil() as u32
        })
    }

    /// Return does the default button's action, Escape cancels. On the power-button panel `r` restarts and `s` sleeps;
    /// every other key does nothing.
    pub fn press(&self, key: Key) -> Option<Decision> {
        match key {
            Key::Return => Some(Decision::Do(self.kind.default_action())),
            Key::Escape => Some(Decision::Cancel),
            Key::Char(c) if self.kind == Kind::PowerButton => match c.to_ascii_lowercase() {
                'r' => Some(Decision::Do(Action::Restart)),
                's' => Some(Decision::Do(Action::Sleep)),
                _ => None,
            },
            Key::Char(_) | Key::Backspace => None,
        }
    }

    /// The end of the countdown does the default action.
    pub fn tick(&self, now: f64) -> Option<Decision> {
        (self.kind.has_countdown() && now - self.start >= COUNTDOWN_SECONDS)
            .then_some(Decision::Do(self.kind.default_action()))
    }

    /// Seconds until the countdown shows another number; `None` on a panel without a countdown.
    pub fn next_wakeup(&self, now: f64) -> Option<f64> {
        self.kind.has_countdown().then(|| {
            let left = COUNTDOWN_SECONDS - (now - self.start);
            (left - (left.ceil() - 1.0).max(0.0)).max(0.0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_countdown_reads_sixty_when_it_appears_and_does_the_default_action_at_zero() {
        let panel = Panel::new(Kind::ShutDown, 10.0, true);
        let left: Vec<Option<u32>> = [10.0, 10.5, 11.0, 69.5, 70.0]
            .into_iter()
            .map(|now| panel.seconds_left(now))
            .collect();
        assert_eq!(left, [Some(60), Some(60), Some(59), Some(1), Some(0)]);
        assert_eq!(panel.tick(69.9), None);
        assert_eq!(panel.tick(70.0), Some(Decision::Do(Action::ShutDown)));
        assert_eq!(
            Panel::new(Kind::LogOut, 0.0, true).tick(60.0),
            Some(Decision::Do(Action::LogOut))
        );
    }

    #[test]
    fn return_does_the_default_action_escape_cancels_and_typing_does_nothing() {
        let panel = Panel::new(Kind::Restart, 0.0, true);
        assert_eq!(
            panel.press(Key::Return),
            Some(Decision::Do(Action::Restart))
        );
        assert_eq!(panel.press(Key::Escape), Some(Decision::Cancel));
        assert_eq!(
            (
                panel.press(Key::Char('s')),
                panel.press(Key::Char('r')),
                panel.press(Key::Backspace)
            ),
            (None, None, None)
        );
    }

    #[test]
    fn the_power_button_panel_has_no_countdown_and_takes_r_s_escape_and_return() {
        let panel = Panel::new(Kind::PowerButton, 0.0, true);
        assert_eq!(
            (
                panel.seconds_left(0.0),
                panel.seconds_left(500.0),
                panel.tick(500.0),
                panel.next_wakeup(0.0)
            ),
            (None, None, None, None)
        );
        assert_eq!(Kind::PowerButton.countdown_message(60), None);
        assert_eq!(
            panel.press(Key::Return),
            Some(Decision::Do(Action::ShutDown))
        );
        assert_eq!(panel.press(Key::Escape), Some(Decision::Cancel));
        assert_eq!(
            (
                panel.press(Key::Char('r')),
                panel.press(Key::Char('S')),
                panel.press(Key::Char('y'))
            ),
            (
                Some(Decision::Do(Action::Restart)),
                Some(Decision::Do(Action::Sleep)),
                None
            )
        );
    }

    #[test]
    fn the_number_changes_every_second() {
        let panel = Panel::new(Kind::LogOut, 0.0, true);
        assert_eq!(panel.next_wakeup(0.0), Some(1.0));
        assert!((panel.next_wakeup(0.25).unwrap() - 0.75).abs() < 1e-9);
        assert!((panel.next_wakeup(59.5).unwrap() - 0.5).abs() < 1e-9);
    }

    const KINDS: [Kind; 4] = [
        Kind::LogOut,
        Kind::Restart,
        Kind::ShutDown,
        Kind::PowerButton,
    ];

    #[test]
    fn with_two_lines_of_each_text_no_element_overlaps_another() {
        for kind in KINDS {
            let frames = kind.frames(2, 2, 2);
            let mut elements = vec![("title", frames.title)];
            elements.extend(frames.info.map(|info| ("info", info)));
            elements.extend(frames.checkbox.map(|c| ("checkbox", c.frame)));
            elements.extend(
                frames
                    .buttons
                    .iter()
                    .map(|button| (button.label, button.frame)),
            );
            for (i, (name, rect)) in elements.iter().enumerate() {
                assert!(
                    rect.min_x() >= 0.0
                        && rect.max_x() <= frames.width
                        && rect.min_y() >= 0.0
                        && rect.max_y() <= frames.height,
                    "{kind:?}: {name} leaves the panel"
                );
                for (other_name, other) in &elements[i + 1..] {
                    assert_eq!(
                        rect.intersection(other),
                        None,
                        "{kind:?}: {name} overlaps {other_name}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_buttons_are_28_pt_capsules_8_pt_apart_across_the_alert() {
        let shown = |kind: Kind| -> Vec<(&'static str, Rect)> {
            kind.frames(2, 2, 2)
                .buttons
                .iter()
                .map(|b| (b.label, b.frame))
                .collect()
        };
        // two side by side, the confirming one trailing, sharing the 228 pt between the 16 pt padding
        for (kind, confirm) in [
            (Kind::LogOut, "Log Out"),
            (Kind::Restart, "Restart"),
            (Kind::ShutDown, "Shut Down"),
        ] {
            let buttons = shown(kind);
            let y = buttons[0].1.y();
            assert_eq!(
                buttons,
                [
                    ("Cancel", Rect::new(16.0, y, 110.0, 28.0)),
                    (confirm, Rect::new(134.0, y, 110.0, 28.0))
                ],
                "{kind:?}"
            );
            assert_eq!(
                buttons[1].1.min_x() - buttons[0].1.max_x(),
                8.0,
                "{kind:?}: a gap between the buttons"
            );
        }
        // four stacked full width, the default on top
        let power = shown(Kind::PowerButton);
        let labels: Vec<&str> = power.iter().map(|(label, _)| *label).collect();
        assert_eq!(labels, ["Shut Down", "Cancel", "Sleep", "Restart"]);
        for pair in power.windows(2) {
            assert_eq!(pair[1].1.min_y() - pair[0].1.max_y(), 8.0);
        }
        for (_, frame) in &power {
            assert_eq!(
                (frame.x(), frame.width(), frame.height()),
                (16.0, 228.0, 28.0)
            );
        }
    }

    #[test]
    fn the_height_is_the_chain_of_the_wrapped_text() {
        // padding 22, title 2 x 16, gap 10, info 2 x 16, gap 12, checkbox 2 x 16, gap 16, buttons 28, padding 16
        for kind in [Kind::LogOut, Kind::Restart, Kind::ShutDown] {
            let frames = kind.frames(2, 2, 2);
            assert_eq!(
                (frames.width, frames.height),
                (
                    260.0,
                    22.0 + 32.0 + 10.0 + 32.0 + 12.0 + 32.0 + 16.0 + 28.0 + 16.0
                )
            );
            let checkbox = frames.checkbox.unwrap();
            assert_eq!(
                (
                    frames.title,
                    frames.info.unwrap(),
                    checkbox.frame,
                    frames.buttons[0].frame.y()
                ),
                (
                    Rect::new(22.0, 22.0, 216.0, 32.0),
                    Rect::new(22.0, 64.0, 216.0, 32.0),
                    Rect::new(22.0, 108.0, 216.0, 32.0),
                    156.0
                )
            );
            // a label on one line keeps the box's row at 18 pt
            assert_eq!(kind.frames(2, 2, 1).checkbox.unwrap().frame.height(), 18.0);
        }
        // no info line and no checkbox: the buttons 16 pt under the title, four rows of 28 pt 8 pt apart
        let power = Kind::PowerButton.frames(2, 0, 0);
        assert_eq!(
            (
                power.width,
                power.height,
                power.info,
                power.title,
                power.buttons[0].frame.y()
            ),
            (
                260.0,
                22.0 + 32.0 + 16.0 + 4.0 * 28.0 + 3.0 * 8.0 + 16.0,
                None,
                Rect::new(22.0, 22.0, 216.0, 32.0),
                70.0
            )
        );
        assert_eq!(
            power.checkbox, None,
            "the power-button panel has no checkbox"
        );
    }

    #[test]
    fn the_checkbox_starts_at_the_saved_choice_and_a_click_or_space_flips_it() {
        let frames = Kind::LogOut.frames(2, 2, 2);
        let checkbox = frames.checkbox.unwrap();
        assert_eq!(checkbox.label, "Reopen windows when logging back in");
        assert_eq!(
            checkbox.frame.width(),
            wrap_width(Kind::LogOut),
            "the row spans the text column, so the label is part of the target"
        );
        assert!(frames
            .buttons
            .iter()
            .all(|b| b.frame.intersection(&checkbox.frame).is_none()));

        let mut panel = Panel::new(Kind::Restart, 0.0, false);
        assert!(!panel.reopen());
        assert!(panel.toggles(Key::Char(' ')));
        assert!(!panel.toggles(Key::Char('x')));
        assert_eq!(panel.press(Key::Char(' ')), None, "space decides nothing");
        panel.toggle_reopen();
        assert!(panel.reopen());
        assert_eq!(panel.seconds_left(1.0), Some(59), "the countdown goes on");
        panel.toggle_reopen();
        assert!(!panel.reopen());
    }

    #[test]
    fn the_power_button_panel_keeps_the_saved_choice_and_cannot_flip_it() {
        let mut panel = Panel::new(Kind::PowerButton, 0.0, true);
        assert!(!panel.toggles(Key::Char(' ')));
        panel.toggle_reopen();
        assert!(panel.reopen());
        assert_eq!(Kind::PowerButton.frames(2, 0, 0).checkbox, None);
    }

    #[test]
    fn each_extra_text_line_moves_what_follows_down_by_its_line_height() {
        let two = Kind::LogOut.frames(2, 2, 2);
        let more = Kind::LogOut.frames(3, 3, 3);
        assert_eq!(
            (
                more.info.unwrap().y() - two.info.unwrap().y(),
                more.buttons[0].frame.y() - two.buttons[0].frame.y(),
                more.height - two.height
            ),
            (16.0, 48.0, 48.0)
        );
    }

    #[test]
    fn the_buttons_decide_cancel_then_the_action_and_the_power_panel_offers_four() {
        let frames = Kind::ShutDown.frames(2, 2, 2);
        let decisions: Vec<Decision> = frames.buttons.iter().map(|b| b.decision).collect();
        assert_eq!(
            decisions,
            [Decision::Cancel, Decision::Do(Action::ShutDown)]
        );
        let power = Kind::PowerButton.frames(2, 0, 0);
        let decisions: Vec<Decision> = power.buttons.iter().map(|b| b.decision).collect();
        assert_eq!(
            decisions,
            [
                Decision::Do(Action::ShutDown),
                Decision::Cancel,
                Decision::Do(Action::Sleep),
                Decision::Do(Action::Restart)
            ]
        );
    }

    #[test]
    fn the_message_is_the_reference_sentence_with_the_formatted_countdown() {
        assert_eq!(
            Kind::ShutDown.countdown_message(60).as_deref(),
            Some("If you do nothing, the computer will shut down automatically in 1 minute.")
        );
        assert_eq!(
            Kind::LogOut.countdown_message(1).as_deref(),
            Some("If you do nothing, you will be logged out automatically in 1 second.")
        );
        assert_eq!(
            Kind::Restart.countdown_message(59).as_deref(),
            Some("If you do nothing, the computer will restart automatically in 59 seconds.")
        );
    }

    #[test]
    fn the_countdown_shows_minutes_and_seconds_and_drops_a_zero_unit() {
        let shown: Vec<String> = [0, 1, 59, 60, 61, 120]
            .into_iter()
            .map(format_countdown)
            .collect();
        assert_eq!(
            shown,
            [
                "0 seconds",
                "1 second",
                "59 seconds",
                "1 minute",
                "1 minute, 1 second",
                "2 minutes"
            ]
        );
    }
}
