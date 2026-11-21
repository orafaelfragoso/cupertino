//! The keyboard's two panels as the server describes them (`cupertino_kit_shell::keyboard_panels`): the input-source
//! switcher's list, its selection and its fade, and the accent picker's candidates, focus and place. Pure logic; time is
//! seconds on any monotonic clock, and the width of a label is measured by the caller.
//!
//! The look is the one the server drew before W6.12: the switcher on the regular glass of an alert, rows of 40 points
//! with a 24-point source icon and its two-letter code, centred on the output and faded over 0.3 s once the switch ends;
//! the picker on the menu material next to the text cursor with one numbered cell for each candidate (the cells are
//! `cupertino_keyboard::accent::layout_popup`'s).

use cupertino_keyboard::accent::{layout_popup, PopupMetrics, Rect as KRect};
use cupertino_keyboard::switcher;
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::motion::Timing;
use cupertino_kit_shell::keyboard_panels::{Source, Update};
use cupertino_kit_shell::outputs::OutputId;

/// The switcher's fade once the switch ends: the catalogue's `Timing::INPUT_SOURCES_FADE`, constant speed over 0.3 s.
pub fn fade() -> Timing {
    Timing::INPUT_SOURCES_FADE
}

/// The size of the switcher's row labels, in points (approximate).
pub const LABEL_SIZE: f64 = 13.0;
/// The edge of a row's source icon, in points (approximate: the icon artwork is unknown).
pub const ICON_SIZE: f64 = 24.0;
/// The corner radius of a source icon, in points (approximate).
pub const ICON_RADIUS: f64 = 6.0;
/// The space between a row's edge and its icon, and between the icon and the label, in points (approximate).
pub const ROW_INSET: f64 = 10.0;
/// The size and the weight of the two-letter code on a source icon (approximate).
pub const CODE_SIZE: f64 = 10.0;
pub const CODE_WEIGHT: u16 = 600;
/// Space a row adds to its label: the icon, the gap after it and the inset at both row edges.
pub const LABEL_EXTRA: f64 = ICON_SIZE + ROW_INSET + 2.0 * ROW_INSET;

/// The corner radius of the picker's material, in points (approximate).
pub const PICKER_RADIUS: f64 = 12.0;
/// The corner radius of the focused candidate's highlight, in points (approximate).
pub const CELL_RADIUS: f64 = 8.0;
/// The size of a candidate, in points (approximate).
pub const CANDIDATE_SIZE: f64 = 18.0;
/// Where a candidate's middle lies, as a fraction of its cell's height from the top (approximate).
pub const CANDIDATE_MIDDLE: f64 = 0.42;
/// The size of the digit under a candidate that picks it, in points, and the distance of its middle from the cell's
/// bottom (approximate).
pub const NUMBER_SIZE: f64 = 9.0;
pub const NUMBER_FROM_BOTTOM: f64 = 8.0;
/// The digit's opacity relative to the candidate's colour (approximate).
pub const NUMBER_ALPHA: f32 = 0.6;
/// The most candidates the picker shows: one for each digit that picks.
pub const MAX_CANDIDATES: usize = 9;

/// The two-letter code of a source id on its icon (`fr(nodeadkeys)` is `FR`).
pub fn source_code(id: &str) -> String {
    id.split('(')
        .next()
        .unwrap_or(id)
        .chars()
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// The switcher's panel, from the switch's start to the end of its fade.
#[derive(Clone, Debug, PartialEq)]
pub struct SourcesPanel {
    /// The output it shows on; `None` for the first one.
    pub output: Option<OutputId>,
    pub sources: Vec<Source>,
    pub selected: usize,
    /// The panel's window, counted: a new switch, or one on another output, opens a new window.
    pub window: u64,
    /// The window showed its first frame.
    pub shown: bool,
    /// The switch ended: the panel fades from `fading_since`, or from its first frame when it has shown none.
    pub closed: bool,
    pub fading_since: Option<f64>,
}

impl SourcesPanel {
    /// The panel's size in points for the widest label: rows of 40 points and 10 points of padding, at least 167 wide.
    pub fn size(&self, widest_label: f64) -> Size {
        let (w, h) = switcher::hud_content_size(self.sources.len(), widest_label, LABEL_EXTRA);
        Size::new(w, h)
    }

    /// The opacity at `now`: 1 until the fade starts, then down to 0.
    pub fn alpha(&self, now: f64) -> f64 {
        self.fading_since
            .map_or(1.0, |since| 1.0 - fade().value(now - since).clamp(0.0, 1.0))
    }

    /// Whether it still needs the display's frames: for its first frame, and while it fades.
    pub fn wants_frames(&self) -> bool {
        !self.shown || self.closed
    }
}

/// The picker's panel, from its open to its close.
#[derive(Clone, Debug, PartialEq)]
pub struct AccentsPanel {
    pub serial: u32,
    /// The output it shows on; `None` for the first one.
    pub output: Option<OutputId>,
    /// The text cursor, in the output's points from its top left corner.
    pub anchor: Rect,
    /// The height at the output's top the picker stays below.
    pub top: f64,
    pub candidates: Vec<String>,
    pub focused: Option<usize>,
}

/// Where the picker lies on an output: its rectangle in the output's points, and each cell inside that rectangle.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerLayout {
    pub rect: Rect,
    pub cells: Vec<Rect>,
}

impl AccentsPanel {
    /// The layout on an output of `output` points: `layout_popup` of the candidates around the anchor, on the output
    /// less `top` at its top.
    pub fn layout(&self, output: Size) -> PickerLayout {
        let top = self.top.clamp(0.0, output.height);
        let anchor = KRect {
            x: self.anchor.x(),
            y: self.anchor.y(),
            w: self.anchor.width(),
            h: self.anchor.height(),
        };
        let screen = KRect {
            x: 0.0,
            y: top,
            w: output.width,
            h: (output.height - top).max(1.0),
        };
        let count = self.candidates.len().min(MAX_CANDIDATES);
        let layout = layout_popup(count, anchor, screen, &PopupMetrics::default());
        let r = layout.rect;
        PickerLayout {
            rect: Rect::new(r.x, r.y, r.w, r.h),
            cells: layout
                .cells
                .iter()
                .take(count)
                .map(|c| Rect::new(c.x - r.x, c.y - r.y, c.w, c.h))
                .collect(),
        }
    }
}

/// Both panels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Panels {
    pub sources: Option<SourcesPanel>,
    pub accents: Option<AccentsPanel>,
    /// The switcher's windows so far.
    windows: u64,
}

impl Panels {
    /// What the server said, at `now`.
    pub fn apply(&mut self, update: Update, now: f64) {
        match update {
            Update::Sources {
                output,
                sources,
                selected,
            } => {
                let selected = selected.min(sources.len().saturating_sub(1));
                match self.sources.as_mut() {
                    // a step of the switch on screen
                    Some(panel) if !panel.closed && panel.output == output => {
                        panel.sources = sources;
                        panel.selected = selected;
                    }
                    _ => {
                        self.windows += 1;
                        self.sources = Some(SourcesPanel {
                            output,
                            sources,
                            selected,
                            window: self.windows,
                            shown: false,
                            closed: false,
                            fading_since: None,
                        });
                    }
                }
            }
            Update::SourcesClosed => {
                if let Some(panel) = self.sources.as_mut().filter(|p| !p.closed) {
                    panel.closed = true;
                    if panel.shown {
                        panel.fading_since = Some(now);
                    }
                }
            }
            Update::Accents {
                serial,
                output,
                anchor,
                top,
                candidates,
                focused,
            } => {
                let focused = focused.filter(|&f| f < candidates.len().min(MAX_CANDIDATES));
                self.accents = Some(AccentsPanel {
                    serial,
                    output,
                    anchor,
                    top,
                    candidates,
                    focused,
                });
            }
            Update::AccentsClosed { serial } => {
                if self.accents.as_ref().is_some_and(|a| a.serial == serial) {
                    self.accents = None;
                }
            }
        }
    }

    /// A frame of the switcher's window at `now`: the first one starts a fade that waited for it, and a finished fade
    /// takes the panel away.
    pub fn sources_frame(&mut self, now: f64) {
        if let Some(panel) = self.sources.as_mut() {
            panel.shown = true;
            if panel.closed && panel.fading_since.is_none() {
                panel.fading_since = Some(now);
            }
        }
        self.tick(now);
    }

    /// Drop a switcher's panel that faded out by `now`.
    pub fn tick(&mut self, now: f64) {
        if self
            .sources
            .as_ref()
            .is_some_and(|p| p.fading_since.is_some() && p.alpha(now) <= 0.0)
        {
            self.sources = None;
        }
    }
}
