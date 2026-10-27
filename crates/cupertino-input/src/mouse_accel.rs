//! Mouse acceleration on physical speed.
//!
//! The curves of [`crate::pointer`] and [`crate::table`] are defined on the speed of the hand in inches per second.
//! A mouse reports counts, so the speed is counts over the device's resolution (dots per inch) over real time:
//! the same hand movement gives the same speed, and the same cursor travel, at 400 or 3200 dpi and at 125 or
//! 8000 reports a second. The time comes from the event timestamps, over a short window so that a mouse whose
//! reports carry one or two counts still gives a steady speed.
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
    /// counts through. A gain of 1 is one pixel per count of a [`DEFAULT_RESOLUTION_DPI`] device.
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
                let scale = gain * DEFAULT_RESOLUTION_DPI / self.dpi;
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
        let output = curve.output_speed(inches_per_second);
        let gain = output * PIXELS_PER_INCH / (inches_per_second * self.dpi);
        (dx * gain, dy * gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{AccelTable, TableCurve};

    fn default_curve(setting: f64) -> AccelCurve {
        AccelCurve::Table(TableCurve::new(&AccelTable::default_table(), setting).unwrap())
    }

    /// Pixels moved in one second by a hand at `inches_per_second`, reported `hz` times a second by a mouse of `dpi`.
    fn travel(inches_per_second: f64, dpi: f64, hz: u64) -> f64 {
        let mut mouse = MouseAccel::new(Some(default_curve(0.6875)), dpi);
        let counts = inches_per_second * dpi / hz as f64;
        assert_eq!(counts.fract(), 0.0, "pick a speed that gives whole counts");
        let period_us = 1_000_000 / hz;
        let mut x = 0.0;
        for i in 0..2 * hz {
            let moved = mouse.accelerate(counts, 0.0, i * period_us).0;
            if i >= hz {
                x += moved;
            }
        }
        x
    }

    #[test]
    fn the_same_hand_speed_travels_the_same_at_any_dpi_and_report_rate() {
        let reference = travel(5.0, 400.0, 125);
        assert!(reference > 400.0, "{reference}");
        for (dpi, hz) in [(400.0, 1000), (1600.0, 125), (1600.0, 1000), (3200.0, 1000)] {
            let got = travel(5.0, dpi, hz);
            assert!(
                (got - reference).abs() / reference < 1e-9,
                "{dpi} dpi at {hz} Hz: {got} != {reference}"
            );
        }
    }

    #[test]
    fn a_faster_hand_travels_further_per_inch_than_a_slower_one() {
        let slow = travel(1.0, 1000.0, 1000) / 1.0;
        let fast = travel(20.0, 1000.0, 1000) / 20.0;
        assert!(fast > 2.0 * slow, "{fast} {slow}");
    }

    #[test]
    fn the_first_report_takes_one_period_of_the_nominal_rate_like_the_design() {
        // 60 counts at 400 dpi in one 1/67 s period: gain 1.30798… px per count at the default speed
        let mut mouse = MouseAccel::new(Some(default_curve(0.6875)), 400.0);
        let (x, y) = mouse.accelerate(60.0, 0.0, 1_000);
        assert!((x - 60.0 * 1.3079891292521268).abs() < 1e-9, "{x}");
        assert_eq!(y, 0.0);
    }

    #[test]
    fn a_pause_starts_a_new_movement() {
        let mut mouse = MouseAccel::new(Some(default_curve(0.6875)), 400.0);
        let first = mouse.accelerate(60.0, 0.0, 0).0;
        let after_pause = mouse.accelerate(60.0, 0.0, 5_000_000).0;
        assert_eq!(first, after_pause);
    }

    /// Pixels per count of each report of a steady 1 count/ms movement at 1000 dpi, then of `report`.
    fn after_steady_motion(report: impl FnOnce(&mut MouseAccel) -> (f64, f64)) -> (f64, f64) {
        let mut mouse = MouseAccel::new(Some(default_curve(0.6875)), 1000.0);
        for i in 0..20 {
            mouse.accelerate(1.0, 0.0, i * 1_000);
        }
        report(&mut mouse)
    }

    #[test]
    fn a_batched_report_with_a_duplicate_timestamp_does_not_jump_the_speed() {
        let steady = after_steady_motion(|m| m.accelerate(1.0, 0.0, 20_000)).0;
        let batched = after_steady_motion(|m| {
            m.accelerate(1.0, 0.0, 20_000);
            m.accelerate(1.0, 0.0, 20_000)
        })
        .0;
        // pixels per count of a steady 1 count/ms at 1000 dpi, then with a duplicated timestamp: a batch moves the
        // speed by a few per cent, not by the factor a lone short gap would give
        assert!((steady - 0.2083).abs() < 1e-4, "{steady}");
        assert!((batched - 0.2191).abs() < 1e-4, "{batched}");
    }

    #[test]
    fn time_running_backwards_keeps_the_last_speed() {
        let steady = after_steady_motion(|m| m.accelerate(1.0, 0.0, 20_000)).0;
        let backwards = after_steady_motion(|m| m.accelerate(1.0, 0.0, 15_000)).0;
        assert!(
            (backwards / steady - 1.0).abs() < 0.12,
            "{backwards} vs {steady}"
        );
        // a first report with the time of the previous one, however old, does not panic or blow up
        let mut mouse = MouseAccel::new(Some(default_curve(0.6875)), 1000.0);
        mouse.accelerate(3.0, 0.0, 9_000);
        let same = mouse.accelerate(3.0, 0.0, 9_000).0;
        let earlier = mouse.accelerate(3.0, 0.0, 1_000).0;
        assert!(same.is_finite() && earlier.is_finite());
    }

    #[test]
    fn linear_scaling_follows_the_resolution() {
        let at = |dpi| MouseAccel::linear(2.0, dpi).accelerate(5.0, -3.0, 0);
        assert_eq!(at(400.0), (10.0, -6.0));
        assert_eq!(at(800.0), (5.0, -3.0));
        assert_eq!(
            MouseAccel::linear(-1.0, 800.0).accelerate(5.0, -3.0, 0),
            (5.0, -3.0)
        );
        assert_eq!(
            MouseAccel::linear(0.0, 400.0).accelerate(10.0, 0.0, 0),
            (1.0, 0.0)
        );
    }

    #[test]
    fn the_database_marks_the_default_resolution() {
        assert_eq!(parse_mouse_dpi("400@125 800@125 *1600@500"), Some(1600.0));
        assert_eq!(parse_mouse_dpi("*800 1600"), Some(800.0));
        assert_eq!(parse_mouse_dpi("400 800"), Some(400.0));
        assert_eq!(parse_mouse_dpi("1200@1000"), Some(1200.0));
        assert_eq!(parse_mouse_dpi(""), None);
        assert_eq!(parse_mouse_dpi("fast 0 -5"), None);
    }

    #[test]
    fn the_resolution_is_the_stored_one_then_the_database_then_1000() {
        assert_eq!(resolve_dpi(Some(3200), Some("*800")), 3200.0);
        assert_eq!(resolve_dpi(None, Some("400 *800")), 800.0);
        assert_eq!(resolve_dpi(None, Some("garbage")), 1000.0);
        assert_eq!(resolve_dpi(None, None), 1000.0);
        assert_eq!(resolve_dpi(Some(0), None), 1000.0);
    }
}
