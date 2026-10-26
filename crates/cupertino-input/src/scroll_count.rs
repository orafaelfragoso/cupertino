//! Scroll count: how many flicks of one series the user has made, so a client can scroll further for
//! repeated flicks. The state machine of the translator's `ProcessScrollCount`;
//! its constants are the `HIDScrollCountBootDefault` values the translator hard-codes.
//!
//! One count per device, advanced by every scroll event of a gesture:
//!
//! * the count is 0 until a scroll is fast enough ([`START_SPEED_SQUARED`]) and then 1;
//! * a gesture that begins within [`BETWEEN_GESTURES_MS`] of the previous one's end raises it by one
//!   (once per gesture);
//! * from the third on, every event raises it by `speed / `[`SPEED_DIVISOR`], up to [`MAX_COUNT`];
//! * the count drops to 0 when the scroll direction flips, when a gesture begins too late, or when
//!   no event is faster than [`SUSTAIN_SPEED_SQUARED`] for [`SUSTAIN_MS`].
//!
//! Speed is the length of the event's movement in points ([`POINTS_PER_LINE`](crate::pointer::POINTS_PER_LINE)
//! per line). The translator runs the count only for events that carry a phase, so a wheel,
//! which has none, gets no count.
//!
//! approximate: whether the design feeds the count the event before or after the coalescing of
//! [`ScrollConsume`](crate::scroll_accel::ScrollConsume) is not known; the server feeds it the
//! points it sends, after the coalescing, and skips the events the coalescing holds back.

/// Longest gap between the end of a gesture and the begin of the next that continues the series, in
/// ms.
pub const BETWEEN_GESTURES_MS: f64 = 600.0;
/// Longest time a series survives without an event faster than [`SUSTAIN_SPEED_SQUARED`], in ms.
pub const SUSTAIN_MS: f64 = 250.0;
/// Speed squared, in points per event squared, that starts a series: 30 points.
pub const START_SPEED_SQUARED: f64 = 900.0;
/// Speed squared that keeps a series alive: 20 points.
pub const SUSTAIN_SPEED_SQUARED: f64 = 400.0;
/// Largest count.
pub const MAX_COUNT: u32 = 2000;
/// A count above 2 grows by `speed / SPEED_DIVISOR` per event.
pub const SPEED_DIVISOR: f64 = 2.5;

/// Where an event is in its gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollPhase {
    /// The first event of a gesture.
    Began,
    Changed,
    /// The fingers lifted.
    Ended,
    Cancelled,
}

/// Scroll count of one device.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScrollCounter {
    count: u32,
    /// Vertical direction of the series, `1` down, `-1` up, `0` before the first vertical event.
    direction: i8,
    /// When the last gesture of the series ended, in ms.
    ended_ms: f64,
    /// When the series last moved fast enough, in ms.
    active_ms: f64,
    /// The current gesture already raised the count.
    raised: bool,
}

impl ScrollCounter {
    /// The count after an event of `phase` that moved `x` and `y` points at `ms`.
    ///
    /// Only a vertical movement (the larger component) sets the direction, as in the design, so
    /// a horizontal scroll neither starts a new series nor ends one by direction.
    pub fn update(&mut self, phase: ScrollPhase, x: f64, y: f64, ms: f64) -> u32 {
        let speed_squared = x * x + y * y;
        if x * x < y * y {
            let direction = if y > 0.0 { 1 } else { -1 };
            if self.direction != direction {
                self.count = 0;
                self.direction = direction;
            }
        }
        match phase {
            ScrollPhase::Began => {
                if self.count != 0 {
                    if ms < self.ended_ms + BETWEEN_GESTURES_MS {
                        if !self.raised {
                            self.count += 1;
                            self.raised = true;
                        }
                        self.active_ms = ms;
                    } else {
                        self.count = 0;
                    }
                }
            }
            ScrollPhase::Changed => {
                if self.count != 0 {
                    if self.count > 2 {
                        self.count =
                            ((speed_squared.sqrt() / SPEED_DIVISOR + f64::from(self.count)) as u32)
                                .min(MAX_COUNT);
                    }
                    if speed_squared > SUSTAIN_SPEED_SQUARED {
                        self.active_ms = ms;
                    } else if self.active_ms + SUSTAIN_MS < ms {
                        self.count = 0;
                    }
                } else if speed_squared >= START_SPEED_SQUARED {
                    self.count = 1;
                    self.active_ms = ms;
                }
            }
            ScrollPhase::Ended => {
                if self.count != 0 {
                    self.ended_ms = ms;
                    self.raised = false;
                }
            }
            ScrollPhase::Cancelled => {
                self.count = 0;
                self.raised = false;
            }
        }
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gesture of `events` fast vertical events of `y` points, 8 ms apart, from `start_ms`; returns
    /// the counts after its begin, its last change and its end, and the time of the end.
    fn flick(
        counter: &mut ScrollCounter,
        start_ms: f64,
        y: f64,
        events: usize,
    ) -> (u32, u32, u32, f64) {
        let began = counter.update(ScrollPhase::Began, 0.0, y, start_ms);
        let mut changed = began;
        let mut ms = start_ms;
        for _ in 0..events {
            ms += 8.0;
            changed = counter.update(ScrollPhase::Changed, 0.0, y, ms);
        }
        ms += 8.0;
        let ended = counter.update(ScrollPhase::Ended, 0.0, 0.0, ms);
        (began, changed, ended, ms)
    }

    #[test]
    fn a_slow_scroll_starts_no_series() {
        let mut counter = ScrollCounter::default();
        let (_, changed, ended, _) = flick(&mut counter, 0.0, 29.0, 10);
        assert_eq!((changed, ended), (0, 0));
    }

    #[test]
    fn a_fast_scroll_starts_the_series_at_one() {
        let mut counter = ScrollCounter::default();
        assert_eq!(flick(&mut counter, 0.0, 30.0, 3).1, 1);
    }

    #[test]
    fn a_flick_within_the_gesture_gap_raises_the_count_once() {
        let mut counter = ScrollCounter::default();
        let (_, _, _, end) = flick(&mut counter, 0.0, 30.0, 3);
        let (began, changed, _, _) = flick(&mut counter, end + 599.0, 30.0, 3);
        assert_eq!((began, changed), (2, 2));
    }

    #[test]
    fn a_flick_after_the_gesture_gap_ends_the_series() {
        let mut counter = ScrollCounter::default();
        let (_, _, _, end) = flick(&mut counter, 0.0, 30.0, 3);
        assert_eq!(
            counter.update(ScrollPhase::Began, 0.0, 30.0, end + 600.0),
            0
        );
    }

    #[test]
    fn from_the_third_flick_the_count_grows_with_the_speed() {
        let mut counter = ScrollCounter::default();
        let (_, _, _, end) = flick(&mut counter, 0.0, 30.0, 3);
        let (_, _, _, end) = flick(&mut counter, end + 100.0, 30.0, 3);
        // the third flick begins at 3, then adds 50 / 2.5 = 20 per event
        assert_eq!(
            counter.update(ScrollPhase::Began, 0.0, 50.0, end + 100.0),
            3
        );
        assert_eq!(
            counter.update(ScrollPhase::Changed, 0.0, 50.0, end + 108.0),
            23
        );
        assert_eq!(
            counter.update(ScrollPhase::Changed, 0.0, 50.0, end + 116.0),
            43
        );
    }

    #[test]
    fn the_count_stops_at_the_maximum() {
        let mut counter = ScrollCounter::default();
        let (_, _, _, end) = flick(&mut counter, 0.0, 30.0, 3);
        let (_, _, _, end) = flick(&mut counter, end + 100.0, 30.0, 3);
        counter.update(ScrollPhase::Began, 0.0, 10000.0, end + 100.0);
        assert_eq!(
            counter.update(ScrollPhase::Changed, 0.0, 10000.0, end + 108.0),
            MAX_COUNT
        );
    }

    #[test]
    fn a_direction_flip_ends_the_series() {
        let mut counter = ScrollCounter::default();
        let (_, _, _, end) = flick(&mut counter, 0.0, 30.0, 3);
        assert_eq!(
            counter.update(ScrollPhase::Began, 0.0, -30.0, end + 100.0),
            0
        );
    }

    #[test]
    fn a_horizontal_scroll_does_not_flip_the_direction() {
        let mut counter = ScrollCounter::default();
        let (_, _, _, end) = flick(&mut counter, 0.0, 30.0, 3);
        assert_eq!(
            counter.update(ScrollPhase::Began, -40.0, 0.0, end + 100.0),
            2
        );
    }

    #[test]
    fn a_series_dies_after_the_sustain_time_without_a_fast_event() {
        let mut counter = ScrollCounter::default();
        assert_eq!(counter.update(ScrollPhase::Changed, 0.0, 30.0, 0.0), 1);
        assert_eq!(counter.update(ScrollPhase::Changed, 0.0, 20.0, 250.0), 1);
        assert_eq!(counter.update(ScrollPhase::Changed, 0.0, 20.0, 251.0), 0);
    }

    #[test]
    fn a_cancelled_gesture_ends_the_series() {
        let mut counter = ScrollCounter::default();
        counter.update(ScrollPhase::Changed, 0.0, 30.0, 0.0);
        assert_eq!(counter.update(ScrollPhase::Cancelled, 0.0, 0.0, 10.0), 0);
    }
}
