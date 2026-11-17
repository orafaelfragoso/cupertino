//! The confirmation panels: log out, restart and shut down, whose countdown ends in the action, and the
//! power-button panel with four stacked buttons and no countdown. What each asks, where its icon, text and buttons sit and what
//! its keys do. Pure: time is an absolute `f64` in seconds on the caller's clock.

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
    /// Shown by a held power button: Shut Down, Restart, Sleep and Cancel; no info line, no countdown.
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

/// The words of a panel.
pub struct Text {
    pub title: &'static str,
    /// The sentence under the title up to the countdown, which [`format_countdown`] ends; `None` on the power-button
    /// panel, which has no info line.
    info_before: Option<&'static str>,
    pub confirm: &'static str,
}

pub const CANCEL_LABEL: &str = "Cancel";

/// The checkbox of every panel: whether the applications open now start again at the next
/// login ([`crate::restore`]).
pub const REOPEN_LABEL: &str = "Open my windows again after I log in";

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
            title: "Log out now?",
            info_before: Some("Logging out automatically in "),
            confirm: "Log Out",
        };
        const RESTART: Text = Text {
            title: "Restart this computer?",
            info_before: Some("Restarting automatically in "),
            confirm: "Restart",
        };
        const SHUT_DOWN: Text = Text {
            title: "Turn off this computer?",
            info_before: Some("Turning off automatically in "),
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

    /// The three confirmation panels count down to their default action; the power-button panel waits.
    fn has_countdown(self) -> bool {
        self != Kind::PowerButton
    }

    /// The picture at the top of the panel: what its default button does.
    pub fn icon(self) -> Icon {
        match self {
            Kind::LogOut => Icon::LogOut,
            Kind::Restart => Icon::Restart,
            Kind::ShutDown | Kind::PowerButton => Icon::Power,
        }
    }
}

/// The picture a panel shows above its title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// A ring open at the top with a bar through the opening.
    Power,
    /// A ring open at the top right, an arrow head at its end.
    Restart,
    /// A door frame with an arrow leaving it.
    LogOut,
}

/// The countdown in words: whole seconds up to a minute ("60 seconds", "1 second"), above it minutes and seconds
/// ("1 minute, 1 second", a zero unit dropped). Each number and its unit are held together by a no-break space, so a
/// line that wraps never leaves a number at the end of one line and its unit on the next.
pub fn format_countdown(seconds: u32) -> String {
    let unit = |count: u32, name: &str| {
        format!(
            "{count}{NO_BREAK_SPACE}{name}{}",
            if count == 1 { "" } else { "s" }
        )
    };
    if seconds <= SECONDS_ONLY_UP_TO {
        return unit(seconds, "second");
    }
    match (seconds / 60, seconds % 60) {
        (minutes, 0) => unit(minutes, "minute"),
        (minutes, seconds) => format!("{}, {}", unit(minutes, "minute"), unit(seconds, "second")),
    }
}

/// The space between a number and its unit.
pub const NO_BREAK_SPACE: char = '\u{a0}';
/// The longest countdown said in seconds alone.
const SECONDS_ONLY_UP_TO: u32 = 60;

/// The narrowest the panel is; it grows to fit its checkbox row on one line ([`width_for`]).
pub const MIN_WIDTH: f64 = 260.0;
/// The alert's padding above and around its content; the icon and the text sit [`TEXT_INSET`] further in than the
/// buttons.
const PADDING_TOP: f64 = 20.0;
const PADDING_SIDE: f64 = 16.0;
const PADDING_BOTTOM: f64 = 16.0;
const TEXT_INSET: f64 = 6.0;
/// The text column's leading edge.
const TEXT_LEFT: f64 = PADDING_SIDE + TEXT_INSET;
/// The room a checkbox's box and the gap after it take before its label: the regular checkbox's 16 pt box and the
/// 6 pt before its label.
pub const CHECKBOX_BOX: f64 = 22.0;

/// The panel's width for a checkbox label `label_width` wide on one line: the label, its box and the text column's
/// insets on both sides, at least [`MIN_WIDTH`].
pub fn width_for(label_width: f64) -> f64 {
    (label_width.ceil() + CHECKBOX_BOX + 2.0 * TEXT_LEFT).max(MIN_WIDTH)
}

/// Width at which the title and the info line wrap in a panel `width` wide: the text column.
pub fn wrap_width(width: f64) -> f64 {
    width - 2.0 * TEXT_LEFT
}

/// The vertical chain: the padding, the icon, 16 pt, the title (bold 13 pt on 16 pt lines), 10 pt, the info line (13 pt
/// on 16 pt lines), 16 pt, the checkbox row, 16 pt, the buttons, the padding.
pub const ICON_SIZE: f64 = 64.0;
const ICON_TITLE_GAP: f64 = 16.0;
pub const TITLE_LINE: f64 = 16.0;
pub const INFO_LINE: f64 = 16.0;
const TITLE_INFO_GAP: f64 = 10.0;
const TEXT_CHECKBOX_GAP: f64 = 16.0;
const CHECKBOX_BUTTONS_GAP: f64 = 16.0;
/// Height of the checkbox row: the box and its one line of label.
pub const CHECKBOX_HEIGHT: f64 = 16.0;
/// The buttons: capsules this high, side by side across the alert between its side padding [`BUTTON_GAP`] apart, or
/// stacked full width [`STACKED_BUTTON_GAP`] apart when there are more than two.
pub const BUTTON_HEIGHT: f64 = 28.0;
pub const BUTTON_GAP: f64 = 8.0;
pub const STACKED_BUTTON_GAP: f64 = 6.0;

/// What each button of the power-button panel does and says, top to bottom: the default first, Cancel last.
const POWER_BUTTONS: [(Decision, &str); 4] = [
    (Decision::Do(Action::ShutDown), "Shut Down"),
    (Decision::Do(Action::Restart), "Restart"),
    (Decision::Do(Action::Sleep), "Sleep"),
    (Decision::Cancel, CANCEL_LABEL),
];

/// One button of a panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelButton {
    pub frame: Rect,
    pub label: &'static str,
    /// What a click on it does.
    pub decision: Decision,
}

/// The "Open my windows again" checkbox of a panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelCheckbox {
    /// The row: the box and its label across the text column; a click anywhere on it toggles.
    pub frame: Rect,
    pub label: &'static str,
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
    pub checkbox: PanelCheckbox,
    /// Cancel and the confirming button left to right, or the four buttons of the power-button panel top to bottom.
    pub buttons: Vec<PanelButton>,
}

impl Kind {
    /// The panel `width` wide ([`width_for`]) laid out for a title wrapped to `title_lines` lines and an info line
    /// wrapped to `info_lines` (ignored by the power-button panel, which has none). Each element sits under the
    /// previous one and the height is the end of the chain.
    pub fn frames(self, width: f64, title_lines: u32, info_lines: u32) -> Frames {
        let wrap = wrap_width(width);
        let icon = Rect::new(TEXT_LEFT, PADDING_TOP, ICON_SIZE, ICON_SIZE);
        let title = Rect::new(
            TEXT_LEFT,
            icon.max_y() + ICON_TITLE_GAP,
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
        let text_bottom = info.map_or(title.max_y(), |i| i.max_y());
        // every panel has the checkbox, which has the keyboard focus when the panel appears
        let checkbox = PanelCheckbox {
            frame: Rect::new(
                TEXT_LEFT,
                text_bottom + TEXT_CHECKBOX_GAP,
                wrap,
                CHECKBOX_HEIGHT,
            ),
            label: REOPEN_LABEL,
        };
        let top = checkbox.frame.max_y() + CHECKBOX_BUTTONS_GAP;
        let across = width - 2.0 * PADDING_SIDE;
        let buttons: Vec<PanelButton> = match self {
            Kind::PowerButton => POWER_BUTTONS
                .iter()
                .enumerate()
                .map(|(row, &(decision, label))| PanelButton {
                    frame: Rect::new(
                        PADDING_SIDE,
                        top + row as f64 * (BUTTON_HEIGHT + STACKED_BUTTON_GAP),
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
            width,
            height: bottom + PADDING_BOTTOM,
            icon,
            title,
            info,
            checkbox,
            buttons,
        }
    }
}

/// Where a session panel stands on its output, in whole points from the output's top-left corner: centred when it
/// appears, then moved by drags of its background, and kept on the output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    output: (f64, f64),
    size: (f64, f64),
    origin: (f64, f64),
}

impl Placement {
    /// A panel of `size` centred on an output of `output` points.
    pub fn centred(output: (f64, f64), size: (f64, f64)) -> Placement {
        let mut placement = Placement {
            output,
            size,
            origin: (
                ((output.0 - size.0) / 2.0).round(),
                ((output.1 - size.1) / 2.0).round(),
            ),
        };
        placement.keep_on_output();
        placement
    }

    /// Move by `by` points, as far as the output's edges allow.
    pub fn moved(&mut self, by: (f64, f64)) {
        self.origin = (
            (self.origin.0 + by.0).round(),
            (self.origin.1 + by.1).round(),
        );
        self.keep_on_output();
    }

    /// The top-left corner: the margins of a surface anchored to the output's top and left edges.
    pub fn origin(&self) -> (i32, i32) {
        (self.origin.0 as i32, self.origin.1 as i32)
    }

    fn keep_on_output(&mut self) {
        let keep = |at: f64, room: f64, size: f64| at.min(room - size).max(0.0);
        self.origin = (
            keep(self.origin.0, self.output.0, self.size.0),
            keep(self.origin.1, self.output.1, self.size.1),
        );
    }
}

/// A panel on screen since `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    kind: Kind,
    start: f64,
    /// The "Open my windows again" choice: the checkbox's state.
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

    /// Flip the checkbox. The countdown goes on.
    pub fn toggle_reopen(&mut self) {
        self.reopen = !self.reopen;
    }

    /// Space flips the checkbox, which has the focus when the panel appears (the buttons answer Return and Escape).
    pub fn toggles(&self, key: Key) -> bool {
        key == Key::Char(' ')
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

    /// The width of a panel whose checkbox label is 201 pt wide: a 223 pt checkbox row.
    const WIDTH: f64 = 267.0;

    #[test]
    fn the_panel_grows_to_fit_its_checkbox_row_and_never_narrows_below_260() {
        // the label, the box and the gap after it (21), and the text column's insets on both sides (2 x 22)
        assert_eq!(width_for(201.0), 267.0);
        assert_eq!(
            width_for(200.3),
            267.0,
            "a fraction of a point takes a whole one"
        );
        assert_eq!(width_for(150.0), 260.0);
        assert_eq!(wrap_width(267.0), 223.0);
        for kind in KINDS {
            let frames = kind.frames(WIDTH, 2, 2);
            assert_eq!(frames.width, WIDTH, "{kind:?}");
            assert_eq!(
                (frames.checkbox.frame.x(), frames.checkbox.frame.width()),
                (22.0, 223.0),
                "{kind:?}: the row spans the text column, so the label is part of the target"
            );
        }
    }

    #[test]
    fn with_two_lines_of_each_text_no_element_overlaps_another() {
        for kind in KINDS {
            let frames = kind.frames(WIDTH, 2, 2);
            let mut elements = vec![("icon", frames.icon), ("title", frames.title)];
            elements.extend(frames.info.map(|info| ("info", info)));
            elements.push(("checkbox", frames.checkbox.frame));
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
    fn two_buttons_share_the_row_at_113_5_by_28_8_pt_apart() {
        let shown = |kind: Kind| -> Vec<(&'static str, Rect)> {
            kind.frames(WIDTH, 2, 2)
                .buttons
                .iter()
                .map(|b| (b.label, b.frame))
                .collect()
        };
        // two side by side, the confirming one trailing, sharing the 235 pt between the 16 pt padding
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
                    ("Cancel", Rect::new(16.0, y, 113.5, 28.0)),
                    (confirm, Rect::new(137.5, y, 113.5, 28.0))
                ],
                "{kind:?}"
            );
        }
    }

    #[test]
    fn the_power_button_panel_stacks_shut_down_restart_sleep_then_cancel_6_pt_apart() {
        let power = Kind::PowerButton.frames(WIDTH, 2, 0);
        let shown: Vec<(&str, Decision)> = power
            .buttons
            .iter()
            .map(|b| (b.label, b.decision))
            .collect();
        assert_eq!(
            shown,
            [
                ("Shut Down", Decision::Do(Action::ShutDown)),
                ("Restart", Decision::Do(Action::Restart)),
                ("Sleep", Decision::Do(Action::Sleep)),
                ("Cancel", Decision::Cancel)
            ]
        );
        for pair in power.buttons.windows(2) {
            assert_eq!(pair[1].frame.min_y() - pair[0].frame.max_y(), 6.0);
        }
        for button in &power.buttons {
            assert_eq!(
                (
                    button.frame.x(),
                    button.frame.width(),
                    button.frame.height()
                ),
                (16.0, 235.0, 28.0)
            );
        }
        assert_eq!(
            power.checkbox.label, REOPEN_LABEL,
            "the checkbox shows there too"
        );
    }

    #[test]
    fn the_height_is_the_chain_of_icon_text_checkbox_and_buttons() {
        // padding 20, icon 64, gap 16, title 2 x 16, gap 10, info 2 x 16, gap 16, checkbox 16, gap 16, buttons 28,
        // padding 16
        for kind in [Kind::LogOut, Kind::Restart, Kind::ShutDown] {
            let frames = kind.frames(WIDTH, 2, 2);
            assert_eq!(
                frames.height,
                20.0 + 64.0 + 16.0 + 32.0 + 10.0 + 32.0 + 16.0 + 16.0 + 16.0 + 28.0 + 16.0,
                "{kind:?}"
            );
            assert_eq!(
                (
                    frames.icon,
                    frames.title,
                    frames.info.unwrap(),
                    frames.checkbox.frame,
                    frames.buttons[0].frame.y()
                ),
                (
                    Rect::new(22.0, 20.0, 64.0, 64.0),
                    Rect::new(22.0, 100.0, 223.0, 32.0),
                    Rect::new(22.0, 142.0, 223.0, 32.0),
                    Rect::new(22.0, 190.0, 223.0, 16.0),
                    222.0
                ),
                "{kind:?}"
            );
        }
        // no info line: the checkbox 16 pt under the title, then four rows of 28 pt 6 pt apart
        let power = Kind::PowerButton.frames(WIDTH, 2, 0);
        assert_eq!(power.info, None);
        assert_eq!(power.checkbox.frame.y(), 100.0 + 32.0 + 16.0);
        assert_eq!(
            power.height,
            20.0 + 64.0 + 16.0 + 32.0 + 16.0 + 16.0 + 16.0 + 4.0 * 28.0 + 3.0 * 6.0 + 16.0
        );
    }

    #[test]
    fn a_panel_appears_centred_moves_by_whole_points_and_stays_on_its_output() {
        let mut p = Placement::centred((1920.0, 1080.0), (267.0, 314.0));
        assert_eq!(p.origin(), (827, 383));
        p.moved((-30.4, 12.6));
        assert_eq!(p.origin(), (797, 396));
        p.moved((-5000.0, -5000.0));
        assert_eq!(p.origin(), (0, 0), "no further than the top-left corner");
        p.moved((5000.0, 5000.0));
        assert_eq!(
            p.origin(),
            (1920 - 267, 1080 - 314),
            "nor the bottom-right one"
        );
        assert_eq!(
            Placement::centred((200.0, 100.0), (267.0, 314.0)).origin(),
            (0, 0),
            "a panel larger than its output keeps its top-left corner on it"
        );
    }

    #[test]
    fn each_panel_shows_the_icon_of_its_default_action() {
        assert_eq!(
            KINDS.map(Kind::icon),
            [Icon::LogOut, Icon::Restart, Icon::Power, Icon::Power]
        );
    }

    #[test]
    fn the_checkbox_starts_at_the_saved_choice_and_space_flips_it_on_every_panel() {
        for kind in KINDS {
            let mut panel = Panel::new(kind, 0.0, false);
            assert!(!panel.reopen());
            assert!(panel.toggles(Key::Char(' ')));
            assert!(!panel.toggles(Key::Char('x')));
            assert_eq!(panel.press(Key::Char(' ')), None, "space decides nothing");
            panel.toggle_reopen();
            assert!(panel.reopen());
            if kind != Kind::PowerButton {
                assert_eq!(panel.seconds_left(1.0), Some(59), "the countdown goes on");
            }
            panel.toggle_reopen();
            assert!(!panel.reopen());
        }
    }

    #[test]
    fn each_extra_text_line_moves_what_follows_down_by_its_line_height() {
        let two = Kind::LogOut.frames(WIDTH, 2, 2);
        let more = Kind::LogOut.frames(WIDTH, 3, 3);
        assert_eq!(
            (
                more.info.unwrap().y() - two.info.unwrap().y(),
                more.buttons[0].frame.y() - two.buttons[0].frame.y(),
                more.height - two.height
            ),
            (16.0, 32.0, 32.0)
        );
    }

    #[test]
    fn the_message_is_the_sentence_with_the_countdown_in_seconds() {
        assert_eq!(
            Kind::ShutDown.countdown_message(60).as_deref(),
            Some("Turning off automatically in 60\u{a0}seconds.")
        );
        assert_eq!(
            Kind::LogOut.countdown_message(1).as_deref(),
            Some("Logging out automatically in 1\u{a0}second.")
        );
        assert_eq!(
            Kind::Restart.countdown_message(59).as_deref(),
            Some("Restarting automatically in 59\u{a0}seconds.")
        );
    }

    #[test]
    fn the_countdown_says_seconds_up_to_a_minute_and_holds_number_and_unit_together() {
        let shown: Vec<String> = [0, 1, 59, 60, 61, 120]
            .into_iter()
            .map(format_countdown)
            .collect();
        assert_eq!(
            shown,
            [
                "0\u{a0}seconds",
                "1\u{a0}second",
                "59\u{a0}seconds",
                "60\u{a0}seconds",
                "1\u{a0}minute, 1\u{a0}second",
                "2\u{a0}minutes"
            ]
        );
        assert!(
            shown
                .iter()
                .all(|s| !s.contains("60 ") && !s.contains("1 s")),
            "no breaking space between a number and its unit"
        );
    }
}
