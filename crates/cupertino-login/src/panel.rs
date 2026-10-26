//! The confirmation panels: log out, restart and shut down, whose countdown ends in the action, and the
//! power-button panel with four buttons and no countdown. What each asks, where its buttons sit and what its
//! keys do. Pure: time is an absolute `f64` in seconds on the caller's clock.

use cupertino_geometry::{Point, Rect};

use crate::model::Key;

/// Seconds until the action is performed without an answer (confirmation countdown, ticking 1.0 s).
pub const COUNTDOWN_SECONDS: f64 = 60.0;

/// What the panel asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    LogOut,
    Restart,
    ShutDown,
    /// Shown by a held power button: Restart, Sleep, Cancel and Shut Down; no info line, no countdown.
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
        const LOG_OUT: Text = Text { title: "Are you sure you want to quit all applications and log out now?", info_before: Some("If you do nothing, you will be logged out automatically in "), confirm: "Log Out" };
        const RESTART: Text = Text { title: "Are you sure you want to restart your computer now?", info_before: Some("If you do nothing, the computer will restart automatically in "), confirm: "Restart" };
        const SHUT_DOWN: Text = Text { title: "Are you sure you want to shut down your computer now?", info_before: Some("If you do nothing, the computer will shut down automatically in "), confirm: "Shut Down" };
        const POWER_BUTTON: Text = Text { title: SHUT_DOWN.title, info_before: None, confirm: SHUT_DOWN.confirm };
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
        self.text().info_before.map(|before| format!("{before}{}.", format_countdown(seconds)))
    }

    /// What Return does, and the end of the countdown: the action of the default button.
    pub fn default_action(self) -> Action {
        match self {
            Kind::LogOut => Action::LogOut,
            Kind::Restart => Action::Restart,
            Kind::ShutDown | Kind::PowerButton => Action::ShutDown,
        }
    }

    /// The three confirmation panels count down to their default action; the power-button panel waits.
    fn has_countdown(self) -> bool {
        self != Kind::PowerButton
    }
}

/// The countdown as the design's date components formatter writes it (full units style, minutes and
/// seconds allowed, at most two units): "1 minute", "59 seconds", "1 minute, 1 second". approximate: the formatter's
/// dropping of a zero unit and its English ", " joiner are read from its documented behaviour, not from a capture.
pub fn format_countdown(seconds: u32) -> String {
    let unit = |count: u32, name: &str| format!("{count} {name}{}", if count == 1 { "" } else { "s" });
    match (seconds / 60, seconds % 60) {
        (0, seconds) => unit(seconds, "second"),
        (minutes, 0) => unit(minutes, "minute"),
        (minutes, seconds) => format!("{}, {}", unit(minutes, "minute"), unit(seconds, "second")),
    }
}

/// Content width in points. The height follows from the text, see [`Kind::frames`].
const fn width(kind: Kind) -> f64 {
    match kind {
        Kind::LogOut | Kind::Restart | Kind::ShutDown => 417.0,
        Kind::PowerButton => 466.0,
    }
}

/// Width at which the title and the info line wrap (the preferred layout width, the frame width minus 4).
pub const fn wrap_width(kind: Kind) -> f64 {
    match kind {
        Kind::LogOut | Kind::Restart | Kind::ShutDown => 305.0,
        Kind::PowerButton => 354.0,
    }
}

/// The buttons' x origin and width, left to right. Neighbours overlap by 1 to 2 pt in the
/// register's numbers; a click there goes to the left button.
const fn button_columns(kind: Kind) -> &'static [(f64, f64)] {
    match kind {
        Kind::LogOut => &[(239.0, 83.0), (321.0, 83.0)],
        Kind::Restart => &[(250.0, 78.0), (326.0, 78.0)],
        Kind::ShutDown => &[(206.0, 100.0), (304.0, 100.0)],
        Kind::PowerButton => &[(59.0, 100.0), (157.0, 100.0), (255.0, 100.0), (353.0, 100.0)],
    }
}

/// What each button of the power-button panel does and says, left to right.
const POWER_BUTTONS: [(Decision, &str); 4] = [
    (Decision::Do(Action::Restart), "Restart"),
    (Decision::Do(Action::Sleep), "Sleep"),
    (Decision::Cancel, CANCEL_LABEL),
    (Decision::Do(Action::ShutDown), "Shut Down"),
];

/// Icon edge and its inset from the left and top edges (64 x 64 at (20, 20) on all four panels).
const ICON_SIZE: f64 = 64.0;
const ICON_INSET: f64 = 20.0;
/// The vertical chain (constraints): top margin 20, title on 16 pt lines, 8 pt, info on 14 pt lines,
/// buttons 32 pt high, bottom margin 13. The text column starts at x 90.
const TOP_MARGIN: f64 = 20.0;
const TEXT_LEFT: f64 = 90.0;
pub const TITLE_LINE: f64 = 16.0;
pub const INFO_LINE: f64 = 14.0;
const TITLE_INFO_GAP: f64 = 8.0;
const BUTTON_HEIGHT: f64 = 32.0;
const BOTTOM_MARGIN: f64 = 13.0;
/// Gap from the info line's bottom edge (power-button panel: the title's) to the buttons. approximate: the design chain
/// is `info, 7, checkbox, 14, buttons` (`title, 9, checkbox, 21, buttons`) and the "Reopen windows" checkbox is not built,
/// so only the last link of the chain, the checkbox-to-buttons gap, is kept as the gap from the text.
const INFO_BUTTONS_GAP: f64 = 14.0;
const TITLE_BUTTONS_GAP: f64 = 21.0;
/// Least gap from the icon's bottom edge to the buttons. approximate: without the checkbox the power-button panel's buttons
/// (from x 59) would end up under the icon; 14 pt is the design's checkbox-to-buttons gap.
const ICON_BUTTONS_GAP: f64 = 14.0;

/// One button of a panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelButton {
    pub frame: Rect,
    pub label: &'static str,
    /// What a click on it does.
    pub decision: Decision,
}

/// Where everything of a panel sits, in points from its top-left corner.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    pub width: f64,
    pub height: f64,
    pub icon: Rect,
    pub title: Rect,
    /// `None` on the power-button panel.
    pub info: Option<Rect>,
    /// Left to right: Cancel and the confirming button, or the four buttons of the power-button panel.
    pub buttons: Vec<PanelButton>,
}

impl Frames {
    /// The decision a click at `point` (panel-local points) makes, if it is on a button.
    pub fn button_at(&self, point: Point) -> Option<Decision> {
        self.buttons.iter().find(|button| button.frame.contains(point)).map(|button| button.decision)
    }
}

impl Kind {
    /// The panel laid out for a title wrapped to `title_lines` lines and an info line wrapped to `info_lines` (ignored by
    /// the power-button panel, which has none). Each element sits at the previous one's bottom edge and the height is the
    /// end of the chain; the width is fixed.: no code sets a size on the design windows, so their
    /// height follows the constraint chain in the same way.
    pub fn frames(self, title_lines: u32, info_lines: u32) -> Frames {
        let wrap = wrap_width(self);
        let title = Rect::new(TEXT_LEFT, TOP_MARGIN, wrap, TITLE_LINE * f64::from(title_lines));
        let info = self.text().info_before.map(|_| Rect::new(TEXT_LEFT, title.max_y() + TITLE_INFO_GAP, wrap, INFO_LINE * f64::from(info_lines)));
        let after_text = info.map_or(title.max_y() + TITLE_BUTTONS_GAP, |info| info.max_y() + INFO_BUTTONS_GAP);
        let buttons_top = after_text.max(ICON_INSET + ICON_SIZE + ICON_BUTTONS_GAP);
        let frame = |&(x, width): &(f64, f64)| Rect::new(x, buttons_top, width, BUTTON_HEIGHT);
        let columns = button_columns(self);
        let buttons = match self {
            Kind::PowerButton => columns.iter().zip(POWER_BUTTONS).map(|(column, (decision, label))| PanelButton { frame: frame(column), label, decision }).collect(),
            Kind::LogOut | Kind::Restart | Kind::ShutDown => vec![
                PanelButton { frame: frame(&columns[0]), label: CANCEL_LABEL, decision: Decision::Cancel },
                PanelButton { frame: frame(&columns[1]), label: self.text().confirm, decision: Decision::Do(self.default_action()) },
            ],
        };
        Frames { width: width(self), height: buttons_top + BUTTON_HEIGHT + BOTTOM_MARGIN, icon: Rect::new(ICON_INSET, ICON_INSET, ICON_SIZE, ICON_SIZE), title, info, buttons }
    }
}

/// A panel on screen since `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    kind: Kind,
    start: f64,
}

impl Panel {
    pub fn new(kind: Kind, now: f64) -> Self {
        Panel { kind, start: now }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Whole seconds left on the countdown, rounded up so that it reads 60 when the panel appears and 0 only at the end;
    /// `None` on a panel without a countdown.
    pub fn seconds_left(&self, now: f64) -> Option<u32> {
        self.kind.has_countdown().then(|| (COUNTDOWN_SECONDS - (now - self.start)).clamp(0.0, COUNTDOWN_SECONDS).ceil() as u32)
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
        (self.kind.has_countdown() && now - self.start >= COUNTDOWN_SECONDS).then_some(Decision::Do(self.kind.default_action()))
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
        let panel = Panel::new(Kind::ShutDown, 10.0);
        let left: Vec<Option<u32>> = [10.0, 10.5, 11.0, 69.5, 70.0].into_iter().map(|now| panel.seconds_left(now)).collect();
        assert_eq!(left, [Some(60), Some(60), Some(59), Some(1), Some(0)]);
        assert_eq!(panel.tick(69.9), None);
        assert_eq!(panel.tick(70.0), Some(Decision::Do(Action::ShutDown)));
        assert_eq!(Panel::new(Kind::LogOut, 0.0).tick(60.0), Some(Decision::Do(Action::LogOut)));
    }

    #[test]
    fn return_does_the_default_action_escape_cancels_and_typing_does_nothing() {
        let panel = Panel::new(Kind::Restart, 0.0);
        assert_eq!(panel.press(Key::Return), Some(Decision::Do(Action::Restart)));
        assert_eq!(panel.press(Key::Escape), Some(Decision::Cancel));
        assert_eq!((panel.press(Key::Char('s')), panel.press(Key::Char('r')), panel.press(Key::Backspace)), (None, None, None));
    }

    #[test]
    fn the_power_button_panel_has_no_countdown_and_takes_r_s_escape_and_return() {
        let panel = Panel::new(Kind::PowerButton, 0.0);
        assert_eq!((panel.seconds_left(0.0), panel.seconds_left(500.0), panel.tick(500.0), panel.next_wakeup(0.0)), (None, None, None, None));
        assert_eq!(Kind::PowerButton.countdown_message(60), None);
        assert_eq!(panel.press(Key::Return), Some(Decision::Do(Action::ShutDown)));
        assert_eq!(panel.press(Key::Escape), Some(Decision::Cancel));
        assert_eq!((panel.press(Key::Char('r')), panel.press(Key::Char('S')), panel.press(Key::Char('y'))), (Some(Decision::Do(Action::Restart)), Some(Decision::Do(Action::Sleep)), None));
    }

    #[test]
    fn the_number_changes_every_second() {
        let panel = Panel::new(Kind::LogOut, 0.0);
        assert_eq!(panel.next_wakeup(0.0), Some(1.0));
        assert!((panel.next_wakeup(0.25).unwrap() - 0.75).abs() < 1e-9);
        assert!((panel.next_wakeup(59.5).unwrap() - 0.5).abs() < 1e-9);
    }

    const KINDS: [Kind; 4] = [Kind::LogOut, Kind::Restart, Kind::ShutDown, Kind::PowerButton];

    #[test]
    fn with_two_title_lines_and_two_info_lines_no_element_overlaps_another() {
        for kind in KINDS {
            let frames = kind.frames(2, 2);
            let mut elements = vec![("icon", frames.icon), ("title", frames.title)];
            elements.extend(frames.info.map(|info| ("info", info)));
            elements.extend(frames.buttons.iter().map(|button| (button.label, button.frame)));
            for (i, (name, rect)) in elements.iter().enumerate() {
                for (other_name, other) in &elements[i + 1..] {
                    match rect.intersection(other) {
                        None => {}
                        // the register's own 1 to 2 pt overlap of neighbouring buttons
                        Some(shared) => assert!(frames.buttons.iter().any(|b| b.label == *name) && frames.buttons.iter().any(|b| b.label == *other_name) && shared.width() <= 2.0, "{kind:?}: {name} overlaps {other_name}"),
                    }
                }
            }
        }
    }

    #[test]
    fn the_buttons_have_the_registers_x_and_width_and_a_height_of_32() {
        let shown = |kind: Kind| -> Vec<(&'static str, f64, f64, f64)> { kind.frames(2, 2).buttons.iter().map(|b| (b.label, b.frame.x(), b.frame.width(), b.frame.height())).collect() };
        assert_eq!(shown(Kind::LogOut), [("Cancel", 239.0, 83.0, 32.0), ("Log Out", 321.0, 83.0, 32.0)]);
        assert_eq!(shown(Kind::Restart), [("Cancel", 250.0, 78.0, 32.0), ("Restart", 326.0, 78.0, 32.0)]);
        assert_eq!(shown(Kind::ShutDown), [("Cancel", 206.0, 100.0, 32.0), ("Shut Down", 304.0, 100.0, 32.0)]);
        assert_eq!(shown(Kind::PowerButton), [("Restart", 59.0, 100.0, 32.0), ("Sleep", 157.0, 100.0, 32.0), ("Cancel", 255.0, 100.0, 32.0), ("Shut Down", 353.0, 100.0, 32.0)]);
        let decisions: Vec<Decision> = Kind::PowerButton.frames(2, 0).buttons.iter().map(|b| b.decision).collect();
        assert_eq!(decisions, [Decision::Do(Action::Restart), Decision::Do(Action::Sleep), Decision::Cancel, Decision::Do(Action::ShutDown)]);
    }

    #[test]
    fn the_height_is_the_chain_of_the_wrapped_text() {
        // top 20, title 2 x 16, gap 8, info 2 x 14, gap 14, buttons 32, bottom 13
        for kind in [Kind::LogOut, Kind::Restart, Kind::ShutDown] {
            let frames = kind.frames(2, 2);
            assert_eq!((frames.width, frames.height), (417.0, 20.0 + 32.0 + 8.0 + 28.0 + 14.0 + 32.0 + 13.0));
            assert_eq!((frames.title, frames.info.unwrap(), frames.buttons[0].frame.y()), (Rect::new(90.0, 20.0, 305.0, 32.0), Rect::new(90.0, 60.0, 305.0, 28.0), 102.0));
        }
        // the icon (bottom edge 84) plus 14 holds the buttons at 98, below the title chain's 20 + 32 + 21 = 73
        let power = Kind::PowerButton.frames(2, 0);
        assert_eq!((power.width, power.height, power.info, power.title, power.buttons[0].frame.y()), (466.0, 98.0 + 32.0 + 13.0, None, Rect::new(90.0, 20.0, 354.0, 32.0), 98.0));
    }

    #[test]
    fn each_extra_text_line_moves_what_follows_down_by_its_line_height() {
        let two = Kind::LogOut.frames(2, 2);
        let more = Kind::LogOut.frames(3, 3);
        assert_eq!((more.info.unwrap().y() - two.info.unwrap().y(), more.buttons[0].frame.y() - two.buttons[0].frame.y(), more.height - two.height), (16.0, 30.0, 30.0));
    }

    #[test]
    fn a_click_hits_a_button_or_nothing() {
        let frames = Kind::ShutDown.frames(2, 2);
        assert_eq!(frames.button_at(frames.buttons[1].frame.center()), Some(Decision::Do(Action::ShutDown)));
        assert_eq!(frames.button_at(frames.buttons[0].frame.center()), Some(Decision::Cancel));
        assert_eq!(frames.button_at(Point::new(5.0, 5.0)), None);
        let power = Kind::PowerButton.frames(2, 0);
        assert_eq!(power.button_at(power.buttons[1].frame.center()), Some(Decision::Do(Action::Sleep)));
        assert_eq!(power.button_at(power.buttons[2].frame.center()), Some(Decision::Cancel));
    }

    #[test]
    fn the_message_is_the_reference_sentence_with_the_formatted_countdown() {
        assert_eq!(Kind::ShutDown.countdown_message(60).as_deref(), Some("If you do nothing, the computer will shut down automatically in 1 minute."));
        assert_eq!(Kind::LogOut.countdown_message(1).as_deref(), Some("If you do nothing, you will be logged out automatically in 1 second."));
        assert_eq!(Kind::Restart.countdown_message(59).as_deref(), Some("If you do nothing, the computer will restart automatically in 59 seconds."));
    }

    #[test]
    fn the_countdown_shows_minutes_and_seconds_and_drops_a_zero_unit() {
        let shown: Vec<String> = [0, 1, 59, 60, 61, 120].into_iter().map(format_countdown).collect();
        assert_eq!(shown, ["0 seconds", "1 second", "59 seconds", "1 minute", "1 minute, 1 second", "2 minutes"]);
    }
}
