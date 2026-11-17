//! Mouse acceleration on physical speed.
//!
//! The curves of [`crate::pointer`] and [`crate::table`] are defined on the speed of the hand in inches per second.
//! A mouse reports counts, so the speed is counts over the device's resolution (dots per inch) over real time:
//! the same hand movement gives the same speed, and the same cursor travel, at 400 or 3200 dpi and at 125 or
//! 8000 reports a second. The time comes from the event timestamps, over a short window so that a mouse whose
//! reports carry one or two counts still gives a steady speed.
//!
//! The design's curves take every count as one of a [`DEFAULT_RESOLUTION_DPI`] mouse. A mouse of [`DEFAULT_MOUSE_DPI`],
//! a typical one, moves as the design moves it: the curve gets the hand's speed times [`DESIGN_SPEED_PER_HAND_SPEED`],
//! the inches the design reads from the counts of one real inch of that mouse.
//!
//! The resolution of a device is, in this order, the one the user stored for it, the default entry of the hardware
//! database's `MOUSE_DPI` property, and [`DEFAULT_MOUSE_DPI`]. A mouse cannot tell the host that its own dpi button
//! was pressed, so the stored value is how the user tells it.

use std::collections::VecDeque;

use crate::pointer::{
    AccelCurve, DEFAULT_RESOLUTION_DPI, LINEAR_MINIMUM, MIN_SPEED, NOMINAL_RATE, PIXELS_PER_INCH,
};

/// Resolution assumed for a mouse nothing is known about.
pub const DEFAULT_MOUSE_DPI: f64 = 1000.0;
/// Speed the curve gets per inch per second of the hand: the design reads one real inch of a [`DEFAULT_MOUSE_DPI`] mouse
/// as this many inches of a [`DEFAULT_RESOLUTION_DPI`] one.
pub const DESIGN_SPEED_PER_HAND_SPEED: f64 = DEFAULT_MOUSE_DPI / DEFAULT_RESOLUTION_DPI;
/// Reports are summed over this long, in µs: one report of the slowest mice (125 Hz).
const WINDOW_US: u64 = 8_000;
/// After this long without motion the next report starts a new movement, in µs.
const PAUSE_US: u64 = 250_000;
/// Shortest span a speed is measured over, in µs: one report of the fastest mice (8000 Hz).
const MIN_SPAN_US: f64 = 125.0;

/// The dots per inch of the `MOUSE_DPI` property of the hardware database: entries such as `400@125 *1600@500`
/// separated by spaces, an optional report rate after `@`, the default marked with `*`. The marked entry wins, else
/// the first one; `None` when no entry is a positive number.
pub fn parse_mouse_dpi(property: &str) -> Option<f64> {
    let entries: Vec<(bool, f64)> = property
        .split_whitespace()
        .filter_map(|entry| {
            let (marked, entry) = match entry.strip_prefix('*') {
                Some(rest) => (true, rest),
                None => (false, entry),
            };
            let dpi: f64 = entry.split('@').next()?.parse().ok()?;
            (dpi > 0.0 && dpi.is_finite()).then_some((marked, dpi))
        })
        .collect();
    entries
        .iter()
        .find(|(marked, _)| *marked)
        .or(entries.first())
        .map(|(_, dpi)| *dpi)
}

/// The resolution of a device: `stored` by the user, else the hardware database's `MOUSE_DPI` text, else
/// [`DEFAULT_MOUSE_DPI`].
pub fn resolve_dpi(stored: Option<u32>, hwdb: Option<&str>) -> f64 {
    stored
        .filter(|dpi| *dpi > 0)
        .map(f64::from)
        .or_else(|| hwdb.and_then(parse_mouse_dpi))
        .unwrap_or(DEFAULT_MOUSE_DPI)
}

#[derive(Clone, Debug, PartialEq)]
enum Mode {
    /// Counts pass through unchanged.
    Off,
    /// `pixels = counts × gain`, scaled to the device's resolution.
    Linear(f64),
    Curve(AccelCurve),
}

/// Acceleration of one mouse.
#[derive(Clone, Debug, PartialEq)]
pub struct MouseAccel {
    mode: Mode,
    dpi: f64,
    /// Reports of the window: time in µs and counts moved; the first is the newest one at or before the window's
    /// start, which only marks where the span begins.
    history: VecDeque<(u64, f64)>,
    /// Speed of the last report, inches per second; kept when a report brings no usable elapsed time.
    last_speed: Option<f64>,
}

impl MouseAccel {
    /// Accelerated by `curve` for a device of `dpi`; `None` passes counts through unchanged.
    pub fn new(curve: Option<AccelCurve>, dpi: f64) -> Self {
        Self::with_mode(curve.map_or(Mode::Off, Mode::Curve), dpi)
    }

    /// Linear scaling by `setting` (acceleration off): a setting of 0 uses [`LINEAR_MINIMUM`], a negative one passes
    /// counts through. A gain of 1 is one pixel per count of a [`DEFAULT_MOUSE_DPI`] device.
    pub fn linear(setting: f64, dpi: f64) -> Self {
        let mode = match setting {
            s if s < 0.0 => Mode::Off,
            0.0 => Mode::Linear(LINEAR_MINIMUM),
            s => Mode::Linear(s),
        };
        Self::with_mode(mode, dpi)
    }

    fn with_mode(mode: Mode, dpi: f64) -> Self {
        Self {
            mode,
            dpi: if dpi > 0.0 { dpi } else { DEFAULT_MOUSE_DPI },
            history: VecDeque::new(),
            last_speed: None,
        }
    }

    /// Pixels for `(dx, dy)` counts reported at `time_us`.
    pub fn accelerate(&mut self, dx: f64, dy: f64, time_us: u64) -> (f64, f64) {
        let curve = match &self.mode {
            Mode::Off => return (dx, dy),
            Mode::Linear(gain) => {
                let scale = gain * DEFAULT_MOUSE_DPI / self.dpi;
                return (dx * scale, dy * scale);
            }
            Mode::Curve(curve) => curve,
        };
        debug_assert!(dx.is_finite() && dy.is_finite(), "motion ({dx}, {dy})");
        let counts = dx.hypot(dy);
        // Time that runs backwards counts as no time passing.
        let time_us = time_us.max(self.history.back().map_or(0, |(last, _)| *last));
        if self
            .history
            .back()
            .is_some_and(|(last, _)| time_us.saturating_sub(*last) > PAUSE_US)
        {
            self.history.clear();
            self.last_speed = None;
        }
        self.history.push_back((time_us, counts));
        while self.history.len() > 2 && time_us.saturating_sub(self.history[1].0) >= WINDOW_US {
            self.history.pop_front();
        }
        // The counts of the reports after the first one were moved since the first one's time. With no earlier report
        // the movement has no known duration: it takes one period of the nominal rate. Reports that all carry the
        // same timestamp (a batch) have no duration either: the speed of the last report stays.
        let (moved, span_us) = if self.history.len() > 1 {
            let moved: f64 = self.history.iter().skip(1).map(|(_, c)| c).sum();
            (moved, time_us.saturating_sub(self.history[0].0) as f64)
        } else {
            (counts, 1e6 / NOMINAL_RATE)
        };
        let inches_per_second = match (span_us > 0.0, self.last_speed) {
            (false, Some(last)) => last,
            _ => (moved / self.dpi / (span_us.max(MIN_SPAN_US) / 1e6)).max(MIN_SPEED),
        };
        self.last_speed = Some(inches_per_second);
        let output = curve.output_speed(inches_per_second * DESIGN_SPEED_PER_HAND_SPEED);
        let gain = output * PIXELS_PER_INCH / (inches_per_second * self.dpi);
        (dx * gain, dy * gain)
    }
}
