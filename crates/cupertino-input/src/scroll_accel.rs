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
