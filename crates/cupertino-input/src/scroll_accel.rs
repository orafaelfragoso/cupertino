//! Scroll acceleration: per-axis speed estimate over a short event history, fed through the same
//! parametric or table curves as pointer motion.
//!
//! a 9-slot history of
//! `(dt_ms, |delta|) per axis, reset on a direction change or a gap above 500 ms and summed back
//! until a gap above 150 ms (counted as 150), 500 ms in total or the oldest slot; the speed
//! polynomial, the `√n` boost of table curves and the 6554/65536 output scale. Momentum events
//! are scaled by their dispatch rate over 60 before and divided by it after. A device without a
//! scroll resolution gets no scroll acceleration (no default): do not build an
//! accelerator for it.
//!
//! Units: the input is raw scroll counts of the device; the output is in lines, which the event
//! translator turns into [`POINTS_PER_LINE`](crate::pointer::POINTS_PER_LINE) points each.
//!
//! [`ScrollConsume`] is the translator's coalescing of those lines:
//! small events are held back and emitted as one in 0.1-line steps; what an emission truncates is kept for the next
//! one, so a slow scroll in one direction loses no distance.

use crate::pointer::{AccelCurve, MIN_SPEED};

/// History slots per axis.
const SLOTS: usize = 9;
/// A gap longer than this restarts the history, in ms.
const RESET_MS: f64 = 500.0;
/// Longest interval one slot contributes, in ms.
const MAX_INTERVAL_MS: f64 = 150.0;
/// Output scale applied to every accelerated delta, `6554/65536`.
pub const SCROLL_OUTPUT_SCALE: f64 = 6554.0 / 65536.0;
/// Scroll report rate assumed when a device declares none, in Hz.
pub const DEFAULT_SCROLL_REPORT_RATE_HZ: f64 = 67.0;
/// Scroll resolution of the built-in and external trackpads on every axis, counts per inch. The built-in trackpads declare no scroll report rate, so
/// [`DEFAULT_SCROLL_REPORT_RATE_HZ`] applies to them.
pub const TRACKPAD_SCROLL_RESOLUTION: f64 = 400.0;
/// Dispatch rate momentum deltas are normalised to, in Hz.
const MOMENTUM_BASE_RATE_HZ: f64 = 60.0;

/// Smallest non-zero value the translator emits, in lines: one point.
const LINE_QUANTUM: f64 = 0.1;

/// The translator's scroll coalescing for one axis (`GetScrollDelta`).
///
/// With `u = trunc(2 · resolution / 100) (8 at the trackpad's 400), accelerated lines add up while
/// the direction holds and are emitted, truncated to 0.1 line, once one event's `|raw|` exceeds `u`
/// or the raw counts since the last emission reach `3u`; the events in between emit nothing. A
/// resolution under 50 gives `u = 0`: the record is not used and lines pass through.
///
/// Deviation: the design keeps the raw sum as an integer, which suits its integer
/// counts; the fractional counts of a libinput finger scroll would never accumulate in it, so the
/// sum is kept as a float.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollConsume {
    unit: f64,
    threshold: f64,
    positive: bool,
    lines: f64,
    raw_sum: f64,
}

impl ScrollConsume {
    /// `resolution` is the device's scroll resolution for this axis, counts per inch.
    pub fn new(resolution: f64) -> Self {
        let unit = (2.0 * resolution / 100.0).trunc();
        Self {
            unit,
            threshold: 3.0 * unit,
            positive: false,
            lines: 0.0,
            raw_sum: 0.0,
        }
    }

    /// Lines to emit for an event of `raw` counts that the accelerator turned into `lines`: zero
    /// while the event is held back.
    pub fn consume(&mut self, raw: f64, lines: f64) -> f64 {
        let out = if self.threshold == 0.0 {
            lines
        } else {
            if self.positive != (raw >= 0.0) {
                self.lines = 0.0;
                self.raw_sum = 0.0;
            }
            self.positive = raw >= 0.0;
            self.lines += lines;
            self.raw_sum += raw.abs();
            if raw != 0.0 && (raw.abs() > self.unit || self.raw_sum >= self.threshold) {
                // the epsilon keeps a carried remainder from truncating a whole tenth away
                let emitted = (self.lines * 10.0 + 1e-9_f64.copysign(self.lines)).trunc() / 10.0;
                // the part under a tenth stays for the next emission in the same direction
                self.lines -= emitted;
                self.raw_sum = 0.0;
                emitted
            } else {
                0.0
            }
        };
        if out != 0.0 && out.abs() < LINE_QUANTUM {
            LINE_QUANTUM.copysign(out)
        } else {
            out
        }
    }
}

/// Scroll accelerator for one axis of one device.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollAxisAccel {
    curve: AccelCurve,
    consume: ScrollConsume,
    resolution: f64,
    rate: f64,
    /// Ring of `(dt_ms, |delta|)`.
    history: [(f64, f64); SLOTS],
    head: usize,
    tail: usize,
    /// Direction of the current run (`delta > 0`).
    positive: bool,
    last_ms: Option<f64>,
}

impl ScrollAxisAccel {
    /// `curve` is selected for the scrolling-speed setting with the device's scroll curves or
    /// table (a negative setting means no accelerator); `resolution` is the device's scroll
    /// resolution for this axis; `report_rate_hz` its scroll report rate, or
    /// [`DEFAULT_SCROLL_REPORT_RATE_HZ`] when it declares none.
    pub fn new(curve: AccelCurve, resolution: f64, report_rate_hz: f64) -> Self {
        Self {
            curve,
            consume: ScrollConsume::new(resolution),
            resolution,
            rate: report_rate_hz,
            history: [(0.0, 0.0); SLOTS],
            head: 0,
            tail: 0,
            positive: false,
            last_ms: None,
        }
    }

    /// Accelerate one raw `delta` that arrived at `time_ms`. `momentum_dispatch_rate` is the
    /// dispatch rate (Hz) of a momentum event, `None` for a direct one. A zero delta is
    /// returned unchanged and does not enter the history. Panics on a dispatch rate ≤ 0.
    pub fn accelerate(
        &mut self,
        delta: f64,
        time_ms: f64,
        momentum_dispatch_rate: Option<f64>,
    ) -> f64 {
        if delta == 0.0 {
            return 0.0;
        }
        assert!(
            momentum_dispatch_rate.is_none_or(|r| r > 0.0),
            "momentum dispatch rate must be positive"
        );
        let m = momentum_dispatch_rate.map_or(1.0, |r| r / MOMENTUM_BASE_RATE_HZ);
        self.accelerate_raw(delta * m, time_ms) / m
    }

    /// Accelerate one raw `delta` and coalesce it as the translator does: the lines to emit now,
    /// zero while the event is held back.
    pub fn scroll_lines(&mut self, delta: f64, time_ms: f64) -> f64 {
        let lines = self.accelerate(delta, time_ms, None);
        self.consume.consume(delta, lines)
    }

    fn accelerate_raw(&mut self, delta: f64, time_ms: f64) -> f64 {
        // The design's first event measures from timestamp 0, a gap far above the reset.
        let dt = self.last_ms.map_or(f64::INFINITY, |last| time_ms - last);
        self.last_ms = Some(time_ms);
        let cur = self.tail;
        self.history[cur] = (dt, delta.abs());
        self.tail = (cur + 1) % SLOTS;
        if self.tail == self.head {
            self.head = (self.tail + 1) % SLOTS;
        }
        if self.positive != (delta > 0.0) || dt > RESET_MS {
            self.head = cur;
            self.positive = delta > 0.0;
        }

        let (mut idx, mut n, mut t_sum, mut d_sum) = (self.tail, 0.0, 0.0, 0.0);
        loop {
            idx = (idx + SLOTS - 1) % SLOTS;
            let (slot_dt, slot_d) = self.history[idx];
            d_sum += slot_d;
            n += 1.0;
            if slot_dt > MAX_INTERVAL_MS {
                t_sum += MAX_INTERVAL_MS;
                break;
            }
            t_sum += slot_dt;
            if t_sum >= RESET_MS || idx == self.head {
                break;
            }
        }

        let q = self.rate / 67.0;
        let t = (t_sum / n * q).clamp(1.0, MAX_INTERVAL_MS);
        let poly = t * (-955.0 / 65536.0) + t * t * (2.0 / 65536.0) + 98369.0 / 65536.0;
        let v = (q * (d_sum / n) * poly).max(MIN_SPEED);
        match self.curve {
            AccelCurve::Parametric(_) => self.pixels(v) / v * delta * SCROLL_OUTPUT_SCALE,
            AccelCurve::Table(_) => {
                let v = if n >= 3.0 { v * n.sqrt() } else { v };
                self.pixels(v) / delta.abs() * delta * SCROLL_OUTPUT_SCALE
            }
        }
    }

    fn pixels(&self, v: f64) -> f64 {
        self.curve.pixels(v, self.resolution, self.rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointer::{ParametricCurve, MOUSE_CURVES, TRACKPAD_SCROLL_CURVES};
    use crate::table::{AccelTable, TableCurve};

    fn table(setting: f64) -> AccelCurve {
        AccelCurve::Table(TableCurve::new(&AccelTable::default_table(), setting).unwrap())
    }

    /// Feed `events` of `(delta, time_ms)` and compare against `expected`.
    fn check(
        mut a: ScrollAxisAccel,
        events: &[(f64, f64)],
        momentum: Option<f64>,
        expected: &[f64],
    ) {
        for (&(d, t), want) in events.iter().zip(expected) {
            let got = a.accelerate(d, t, momentum);
            assert!(
                (got - want).abs() < 1e-12 * want.abs().max(1.0),
                "t {t}: {got} vs {want}"
            );
        }
    }

    fn steady(delta: f64, interval: f64) -> Vec<(f64, f64)> {
        (0..5).map(|i| (delta, f64::from(i) * interval)).collect()
    }

    // Expected outputs below come from `ScrollAxis`
    // (scroll resolution 400; the trackpad scroll curves are set `6e28d9c1` of its `accel_curves.json`).

    #[test]
    fn table_curve_matches_reference_implementation() {
        let rate = DEFAULT_SCROLL_REPORT_RATE_HZ;
        check(
            ScrollAxisAccel::new(table(0.3125), 400.0, rate),
            &steady(1.0, 25.0),
            None,
            &[
                3.902232390069384e-05,
                0.009876582971134433,
                0.024758586204527443,
                0.03343352371732829,
                0.04078254709786619,
            ],
        );
        check(
            ScrollAxisAccel::new(table(1.0), 400.0, rate),
            &steady(8.0, 12.0),
            None,
            &[
                0.0002955988184088988,
                0.20656173430958039,
                0.8416231804546546,
                1.2141229943101304,
                1.529913891932944,
            ],
        );
    }

    #[test]
    fn parametric_curve_matches_reference_implementation() {
        let curve = |s| AccelCurve::Parametric(ParametricCurve::new(&MOUSE_CURVES, s).unwrap());
        check(
            ScrollAxisAccel::new(curve(1.0), 400.0, 67.0),
            &steady(4.0, 16.0),
            None,
            &[
                0.09229750528657668,
                0.12849227114475326,
                0.1450069675456642,
                0.15410403213877347,
                0.15983178999178574,
            ],
        );
        check(
            ScrollAxisAccel::new(curve(0.3125), 400.0, 125.0),
            &steady(2.0, 25.0),
            None,
            &[
                0.08288324996594171,
                0.08288324996594171,
                0.0866276853973835,
                0.09010181197555317,
                0.09244963007720403,
            ],
        );
    }

    #[test]
    fn trackpad_scroll_curves_match_reference_implementation() {
        let curve =
            |s| AccelCurve::Parametric(ParametricCurve::new(&TRACKPAD_SCROLL_CURVES, s).unwrap());
        let trackpad = |s| {
            ScrollAxisAccel::new(
                curve(s),
                TRACKPAD_SCROLL_RESOLUTION,
                DEFAULT_SCROLL_REPORT_RATE_HZ,
            )
        };
        check(
            trackpad(0.3125),
            &steady(4.0, 10.0),
            None,
            &[
                0.08887067994726819,
                0.10800083003165198,
                0.11678223450360818,
                0.12162381569780409,
                0.1246730456809513,
            ],
        );
        check(
            trackpad(0.3125),
            &steady(30.0, 8.0),
            None,
            &[
                0.6697329504898516,
                1.7655862070282453,
                2.270026594958617,
                2.548338533127098,
                2.7236750541732406,
            ],
        );
        check(
            trackpad(1.0),
            &steady(12.0, 16.0),
            None,
            &[
                0.21870425586362358,
                0.9596084640810058,
                1.2968116290570155,
                1.482332291964435,
                1.5990587954431006,
            ],
        );
    }

    #[test]
    fn direction_change_and_long_gap_reset_the_history() {
        check(
            ScrollAxisAccel::new(table(1.0), 400.0, DEFAULT_SCROLL_REPORT_RATE_HZ),
            &[
                (3.0, 0.0),
                (3.0, 10.0),
                (3.0, 20.0),
                (-3.0, 30.0),
                (-3.0, 40.0),
                (-3.0, 600.0),
            ],
            None,
            &[
                0.00011084955690333707,
                0.03238763230228678,
                0.19162712010165064,
                -0.19734761372048198,
                -0.19734761372048198,
                -0.00011084955690333707,
            ],
        );
    }

    #[test]
    fn momentum_scales_by_dispatch_rate() {
        // 120 Hz momentum: the delta is doubled before and halved after.
        check(
            ScrollAxisAccel::new(table(1.0), 400.0, DEFAULT_SCROLL_REPORT_RATE_HZ),
            &[(5.0, 0.0), (5.0, 8.0), (5.0, 16.0), (5.0, 24.0)],
            Some(120.0),
            &[
                0.00018474926150556176,
                0.16513510130671144,
                0.5780447101986634,
                0.8206805933263328,
            ],
        );
    }

    #[test]
    fn zero_delta_is_skipped() {
        let mut a = ScrollAxisAccel::new(table(1.0), 400.0, DEFAULT_SCROLL_REPORT_RATE_HZ);
        assert_eq!(a.accelerate(0.0, 5.0, None), 0.0);
        assert_eq!(a.last_ms, None);
    }

    #[test]
    fn consume_holds_small_events_until_the_raw_sum_reaches_three_units() {
        // Resolution 400: u = 8, threshold 24. Twelve events of 2 counts reach 24.
        let mut c = ScrollConsume::new(400.0);
        let out: Vec<f64> = (0..12).map(|_| c.consume(2.0, 0.0625)).collect();
        assert_eq!(out[..11], [0.0; 11]);
        assert_eq!(out[11], 0.7); // trunc(0.75 · 10) / 10
    }

    #[test]
    fn consume_keeps_what_it_truncates_for_the_next_emission() {
        // 24 events of 1/16 line: 1.5 lines in all, emitted as 0.7 and then the 0.05 left over plus the next 0.75
        let mut c = ScrollConsume::new(400.0);
        let out: Vec<f64> = (0..24).map(|_| c.consume(2.0, 0.0625)).collect();
        assert_eq!((out[11], out[23]), (0.7, 0.8));
        assert!((out.iter().sum::<f64>() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn consume_emits_at_once_when_one_event_exceeds_the_unit() {
        let mut c = ScrollConsume::new(400.0);
        assert_eq!(c.consume(2.0, 0.25), 0.0);
        assert_eq!(c.consume(8.0, 0.25), 0.0); // 8 is not above u
        assert_eq!(c.consume(9.0, 0.5), 1.0);
        assert_eq!(c.consume(1.0, 0.25), 0.0); // the record restarted
    }

    #[test]
    fn consume_drops_the_held_lines_on_a_direction_change() {
        let mut c = ScrollConsume::new(400.0);
        assert_eq!(c.consume(2.0, 0.5), 0.0);
        assert_eq!(c.consume(-9.0, -0.5), -0.5);
    }

    #[test]
    fn consume_passes_a_device_without_unit_through() {
        // Resolution 43: u = 0, so no record; a non-zero value under 0.1 is raised to 0.1.
        let mut c = ScrollConsume::new(43.0);
        assert_eq!(c.consume(1.0, 0.75), 0.75);
        assert_eq!(c.consume(-1.0, -0.03), -0.1);
        assert_eq!(c.consume(0.0, 0.0), 0.0);
    }

    #[test]
    fn scroll_lines_emits_the_truncated_sum_of_the_accelerated_events() {
        let mut a = ScrollAxisAccel::new(
            table(1.0),
            TRACKPAD_SCROLL_RESOLUTION,
            DEFAULT_SCROLL_REPORT_RATE_HZ,
        );
        let mut twin = a.clone();
        let sum = twin.accelerate(1.0, 0.0, None) + twin.accelerate(9.0, 15.0, None);
        assert_eq!(a.scroll_lines(1.0, 0.0), 0.0);
        assert_eq!(a.scroll_lines(9.0, 15.0), (sum * 10.0).trunc() / 10.0);
    }
}
