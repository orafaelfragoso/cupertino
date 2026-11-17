//! What the user does on the overlay, as a state machine without a display: choosing a mode, spanning, moving and
//! resizing the selection, picking a window, the timer, and the outcome (capture this, or cancel).
//!
//! Two styles exist, as in the design: the direct selection shortcut (`--mode selection`, the
//! reference's `-i`) spans a tinted rectangle and captures on release; the toolbar (`--mode toolbar`, `-U`) keeps an
//! adjustable selection under a dim until Capture or Return. In both, Space switches between selection and window
//! pick, Escape cancels, and Control held when the capture is committed sends it to the clipboard
//! (usage text).
//!
//! From the keyboard alone: an arrow key spans a selection in the middle of the output when there is none, then moves
//! it a point at a time (Shift: [`KEYBOARD_STEP_LARGE`] points), and with Alt it grows or shrinks the selection from
//! its bottom-right corner; in the window pick the arrows go through the windows front to back; Return captures what
//! the keys chose. These keys are this tool's own.
//!
//! Positions are global logical points; `outputs` are the outputs' rectangles in the same space.

use cupertino_geometry::{Point, Rect};

use crate::pick::{self, PickWindow};
use crate::selection::{self, Handle};
use crate::settings::Mode;
use crate::values::{KEYBOARD_SELECTION, KEYBOARD_STEP_LARGE};

/// Which overlay the session runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// The direct selection shortcut: the design's simple rectangle.
    Simple,
    /// With the toolbar: the design's interactive rectangle.
    Interactive,
}

/// What a capture covers.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Every output, one image each.
    Screens,
    /// One output, by index into the session's outputs.
    Output(usize),
    /// A rectangle (global points) of one output.
    Area { output: usize, rect: Rect },
    /// A window, by its foreign-toplevel identifier.
    Window(String),
}

/// How a session ends.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Capture { target: Target, clipboard: bool },
    Cancel,
}

/// A key the overlay reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Escape,
    Space,
    Return,
    Left,
    Right,
    Up,
    Down,
}

/// The modifiers held with a key, a click or a release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyMods {
    /// Sends the capture to the clipboard.
    pub control: bool,
    /// Moves or resizes in larger steps.
    pub shift: bool,
    /// Resizes instead of moving.
    pub alt: bool,
}

impl KeyMods {
    pub const NONE: KeyMods = KeyMods {
        control: false,
        shift: false,
        alt: false,
    };
}

/// The pointer the overlay shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    /// The overlay draws the crosshair and hides the system pointer.
    Crosshair,
    /// The system arrow: over the selection, in window and screen mode (approximate: the design's window-mode
    /// pointer is unknown).
    Arrow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    Spanning { anchor: Point, output: usize },
    Moving { last: Point },
    Resizing { handle: Handle },
}

#[derive(Clone, Debug, PartialEq)]
struct Countdown {
    end: f64,
    target: Target,
    clipboard: bool,
}

/// One run of the overlay.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    style: Style,
    mode: Mode,
    outputs: Vec<Rect>,
    windows: Vec<PickWindow>,
    /// The window the arrow keys chose, by index into `windows`, until the pointer moves.
    keyboard_pick: Option<usize>,
    pointer: Option<Point>,
    /// The selection and the output it is on.
    selection: Option<(usize, Rect)>,
    gesture: Option<Gesture>,
    /// Seconds the toolbar's timer waits before capturing.
    timer: u32,
    countdown: Option<Countdown>,
}

impl Session {
    /// A session over `outputs` starting in `mode`. The toolbar starts with `remembered` as its selection when that is
    /// still whole on an output (the design restores the last selection).
    pub fn new(
        style: Style,
        mode: Mode,
        outputs: Vec<Rect>,
        remembered: Option<Rect>,
        timer: u32,
    ) -> Self {
        let selection = match style {
            Style::Interactive => remembered.and_then(|r| {
                outputs
                    .iter()
                    .position(|o| o.intersection(&r) == Some(r))
                    .map(|i| (i, r))
            }),
            Style::Simple => None,
        };
        Self {
            style,
            mode,
            outputs,
            windows: Vec::new(),
            keyboard_pick: None,
            pointer: None,
            selection,
            gesture: None,
            timer,
            countdown: None,
        }
    }

    pub fn style(&self) -> Style {
        self.style
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn outputs(&self) -> &[Rect] {
        &self.outputs
    }

    /// The selection, once there is one.
    pub fn selection(&self) -> Option<Rect> {
        self.selection.map(|(_, r)| r)
    }

    pub fn pointer(&self) -> Option<Point> {
        self.pointer
    }

    /// Whether a drag is spanning or resizing the selection, when the size readout is shown.
    pub fn sizing(&self) -> bool {
        matches!(
            self.gesture,
            Some(Gesture::Spanning { .. } | Gesture::Resizing { .. })
        )
    }

    /// The windows on screen, front to back (the server's feed).
    pub fn set_windows(&mut self, windows: Vec<PickWindow>) {
        let chosen = self
            .keyboard_pick
            .and_then(|i| self.windows.get(i))
            .map(|w| w.identifier.clone());
        self.keyboard_pick = chosen.and_then(|id| windows.iter().position(|w| w.identifier == id));
        self.windows = windows;
    }

    /// The windows on screen, front to back.
    pub fn windows(&self) -> &[PickWindow] {
        &self.windows
    }

    /// The window the pick would take, in window mode: the one the arrow keys chose, else the front-most one under the
    /// pointer.
    pub fn picked(&self) -> Option<&PickWindow> {
        if self.mode != Mode::Window {
            return None;
        }
        if let Some(i) = self.keyboard_pick {
            return self.windows.get(i);
        }
        pick::window_at(&self.windows, self.pointer?).map(|i| &self.windows[i])
    }

    /// The Options menu's timer: seconds the next capture waits. A running countdown keeps its end.
    pub fn set_timer(&mut self, seconds: u32) {
        self.timer = seconds;
    }

    /// Whether a timer runs before the capture.
    pub fn counting_down(&self) -> bool {
        self.countdown.is_some()
    }

    /// The pointer to show now.
    pub fn pointer_look(&self) -> Pointer {
        match self.mode {
            Mode::Selection => match (self.style, self.selection(), self.pointer, self.gesture) {
                (_, _, _, Some(Gesture::Spanning { .. } | Gesture::Resizing { .. })) => {
                    Pointer::Crosshair
                }
                (Style::Interactive, Some(s), Some(p), _)
                    if s.contains(p) || selection::handle_at(&s, p).is_some() =>
                {
                    Pointer::Arrow
                }
                _ => Pointer::Crosshair,
            },
            Mode::Window | Mode::Screen => Pointer::Arrow,
        }
    }

    /// Seconds left on the timer, rounded up, while it runs.
    pub fn countdown(&self, now: f64) -> Option<u32> {
        self.countdown
            .as_ref()
            .map(|c| (c.end - now).max(0.0).ceil() as u32)
    }

    /// When [`Session::tick`] next has something to do: the next whole second of the countdown.
    pub fn deadline(&self, now: f64) -> Option<f64> {
        self.countdown
            .as_ref()
            .map(|c| (c.end - (c.end - now).max(0.0).floor()).max(now))
    }

    /// Advance to `now`: the capture a countdown waited for, once it ran out.
    pub fn tick(&mut self, now: f64) -> Option<Outcome> {
        if self.countdown.as_ref().is_some_and(|c| now >= c.end) {
            let c = self.countdown.take().expect("checked above");
            return Some(Outcome::Capture {
                target: c.target,
                clipboard: c.clipboard,
            });
        }
        None
    }

    /// The toolbar's mode buttons. A running countdown ignores them.
    pub fn set_mode(&mut self, mode: Mode) {
        if self.countdown.is_none() {
            self.mode = mode;
            self.gesture = None;
            self.keyboard_pick = None;
        }
    }

    fn output_at(&self, p: Point) -> Option<usize> {
        self.outputs.iter().position(|o| o.contains(p))
    }

    /// Commit `target`: at once, or after the timer when the toolbar has one.
    fn commit(&mut self, target: Target, clipboard: bool, now: f64) -> Option<Outcome> {
        if self.style == Style::Interactive && self.timer > 0 {
            self.countdown = Some(Countdown {
                end: now + f64::from(self.timer),
                target,
                clipboard,
            });
            self.gesture = None;
            return None;
        }
        Some(Outcome::Capture { target, clipboard })
    }

    /// The toolbar's Capture button (and Return): what the mode covers. Window mode has nothing to capture until a
    /// window is clicked; selection mode needs a selection.
    pub fn capture(&mut self, clipboard: bool, now: f64) -> Option<Outcome> {
        if self.countdown.is_some() {
            return None;
        }
        let target = match self.mode {
            Mode::Screen => Target::Screens,
            // the window the arrow keys chose; a click takes one under the pointer
            Mode::Window => {
                let i = self.keyboard_pick?;
                Target::Window(self.windows.get(i)?.identifier.clone())
            }
            Mode::Selection => {
                let (output, rect) = self.selection.filter(|(_, r)| selection::capturable(r))?;
                Target::Area { output, rect }
            }
        };
        self.commit(target, clipboard, now)
    }

    pub fn pointer_moved(&mut self, p: Point) {
        if self.pointer != Some(p) {
            self.keyboard_pick = None;
        }
        self.pointer = Some(p);
        match self.gesture {
            Some(Gesture::Spanning { anchor, output }) => {
                self.selection = Some((output, selection::span(anchor, p, &self.outputs[output])))
            }
            Some(Gesture::Moving { last }) => {
                if let Some((output, rect)) = self.selection {
                    self.selection = Some((
                        output,
                        selection::moved(&rect, p.x - last.x, p.y - last.y, &self.outputs[output]),
                    ));
                }
                self.gesture = Some(Gesture::Moving { last: p });
            }
            Some(Gesture::Resizing { handle }) => {
                if let Some((output, rect)) = self.selection {
                    self.selection = Some((
                        output,
                        selection::resized(&rect, handle, p, &self.outputs[output]),
                    ));
                }
            }
            None => {}
        }
    }

    /// The primary button changed state at `p`; `control` tells whether Control is held.
    pub fn button(&mut self, pressed: bool, p: Point, control: bool, now: f64) -> Option<Outcome> {
        self.pointer_moved(p);
        if self.countdown.is_some() {
            return None;
        }
        match (self.mode, pressed) {
            (Mode::Selection, true) => {
                // the toolbar's selection is adjusted where it is grabbed; anywhere else a new one is spanned
                let held = self
                    .selection
                    .filter(|_| self.style == Style::Interactive)
                    .map(|(_, r)| r);
                let adjust = held.and_then(|s| match selection::handle_at(&s, p) {
                    Some(handle) => Some(Gesture::Resizing { handle }),
                    None => s.contains(p).then_some(Gesture::Moving { last: p }),
                });
                self.gesture = match adjust {
                    Some(gesture) => Some(gesture),
                    None => {
                        let output = self.output_at(p)?;
                        self.selection =
                            Some((output, selection::span(p, p, &self.outputs[output])));
                        Some(Gesture::Spanning { anchor: p, output })
                    }
                };
                None
            }
            (Mode::Selection, false) => {
                self.gesture.take()?;
                let (output, rect) = self.selection?;
                if !selection::capturable(&rect) {
                    self.selection = None;
                    return None;
                }
                match self.style {
                    Style::Simple => Some(Outcome::Capture {
                        target: Target::Area { output, rect },
                        clipboard: control,
                    }),
                    Style::Interactive => None,
                }
            }
            (Mode::Window, true) => {
                let identifier = self.picked()?.identifier.clone();
                self.commit(Target::Window(identifier), control, now)
            }
            (Mode::Screen, true) => {
                let output = self.output_at(p)?;
                self.commit(Target::Output(output), control, now)
            }
            (Mode::Window | Mode::Screen, false) => None,
        }
    }

    pub fn key(&mut self, key: Key, mods: KeyMods, now: f64) -> Option<Outcome> {
        let idle = self.gesture.is_none() && self.countdown.is_none();
        match key {
            Key::Escape => Some(Outcome::Cancel),
            Key::Space if idle => {
                self.mode = match self.mode {
                    Mode::Selection => Mode::Window,
                    Mode::Window => Mode::Selection,
                    Mode::Screen => Mode::Screen,
                };
                self.keyboard_pick = None;
                None
            }
            Key::Return if idle => match self.style {
                Style::Interactive => self.capture(mods.control, now),
                // the direct shortcut captures what the keys chose; the pointer captures on release
                Style::Simple => match self.mode {
                    Mode::Selection => {
                        let (output, rect) =
                            self.selection.filter(|(_, r)| selection::capturable(r))?;
                        Some(Outcome::Capture {
                            target: Target::Area { output, rect },
                            clipboard: mods.control,
                        })
                    }
                    Mode::Window | Mode::Screen => self.capture(mods.control, now),
                },
            },
            Key::Left | Key::Right | Key::Up | Key::Down if idle => {
                self.arrow(key, mods);
                None
            }
            Key::Space | Key::Return | Key::Left | Key::Right | Key::Up | Key::Down => None,
        }
    }

    /// An arrow key: the selection spanned, moved or resized, or the next window chosen.
    fn arrow(&mut self, key: Key, mods: KeyMods) {
        let step = if mods.shift { KEYBOARD_STEP_LARGE } else { 1.0 };
        let (dx, dy) = match key {
            Key::Left => (-step, 0.0),
            Key::Right => (step, 0.0),
            Key::Up => (0.0, -step),
            _ => (0.0, step),
        };
        match self.mode {
            Mode::Selection => {
                let Some((output, rect)) = self.selection else {
                    let output = self.pointer.and_then(|p| self.output_at(p)).unwrap_or(0);
                    let Some(bounds) = self.outputs.get(output) else {
                        return;
                    };
                    let (w, h) = (
                        bounds.width() * KEYBOARD_SELECTION,
                        bounds.height() * KEYBOARD_SELECTION,
                    );
                    let c = bounds.center();
                    let from = Point::new(c.x - w / 2.0, c.y - h / 2.0);
                    let to = Point::new(c.x + w / 2.0, c.y + h / 2.0);
                    self.selection = Some((output, selection::span(from, to, bounds)));
                    return;
                };
                let bounds = self.outputs[output];
                let rect = if mods.alt {
                    let corner = Point::new(
                        (rect.max_x() + dx).max(rect.min_x() + 1.0),
                        (rect.max_y() + dy).max(rect.min_y() + 1.0),
                    );
                    selection::resized(&rect, Handle::BottomRight, corner, &bounds)
                } else {
                    selection::moved(&rect, dx, dy, &bounds)
                };
                self.selection = Some((output, rect));
            }
            Mode::Window if !self.windows.is_empty() => {
                let n = self.windows.len();
                let forward = matches!(key, Key::Right | Key::Down);
                let current = self
                    .keyboard_pick
                    .or_else(|| self.pointer.and_then(|p| pick::window_at(&self.windows, p)));
                self.keyboard_pick = Some(match (current, forward) {
                    (None, true) => 0,
                    (None, false) => n - 1,
                    (Some(i), true) => (i + 1) % n,
                    (Some(i), false) => (i + n - 1) % n,
                });
            }
            Mode::Window | Mode::Screen => {}
        }
    }
}
