//! How late after a vblank a display's next frame starts. A frame drawn right after the vblank sits finished for most
//! of the period before its flip, and every input that arrives meanwhile waits a whole period more. Where the display's
//! flips report real presentation timestamps (the driver's flip timestamp, not the server's own clock), the vblank's
//! phase is known, so the frame can start later and still make the next vblank: it starts the recent maximum render
//! time plus [`MARGIN`] before that vblank, and never later than half a period after the last one, because a missed
//! refresh costs more than the latency it saves.
//!
//! The render times are the last [`WINDOW`] frames' times from the start of drawing to the flip being queued. A frame
//! that still missed its vblank counts as a whole period, which turns the delay off until it leaves the window. Without
//! render times yet, or with the switch off (`CUPERTINO_LATE_LATCH=0`), frames start at once, as on a display whose
//! timestamps come from the server's own clock and on the headless and nested backends.

use std::collections::VecDeque;
use std::time::Duration;

/// Turns the late start off when set to `0` or `off`; on otherwise.
pub const ENV: &str = "CUPERTINO_LATE_LATCH";

/// How many recent frames' render times the delay is taken from: one second of frames at 60 Hz.
pub const WINDOW: usize = 60;

/// Time kept between the recent maximum render time and the vblank: the GPU's work after the flip is queued, the
/// commit, and the loop's wake-up.
pub const MARGIN: Duration = Duration::from_micros(2_000);

/// Whether the late start is on, from the variable's value.
pub fn requested(value: Option<&str>) -> bool {
    !matches!(value.map(str::trim), Some("0" | "off"))
}

/// The recent render times of one display and the delay they allow.
#[derive(Clone, Debug, Default)]
pub struct LateLatch {
    times: VecDeque<Duration>,
}

impl LateLatch {
    /// A frame took `took` from the start of drawing to its flip being queued.
    pub fn rendered(&mut self, took: Duration) {
        if self.times.len() == WINDOW {
            self.times.pop_front();
        }
        self.times.push_back(took);
    }

    /// A frame's flip came after the vblank it was drawn for: it counts as a whole `refresh`, so frames start at once
    /// until it leaves the window.
    pub fn missed(&mut self, refresh: Duration) {
        self.rendered(refresh);
    }

    /// The longest render time in the window.
    pub fn max_render(&self) -> Option<Duration> {
        self.times.iter().max().copied()
    }

    /// How long after a vblank the next frame starts on a display of period `refresh`: the period less the recent
    /// maximum render time and [`MARGIN`], at most half the period, nothing without render times.
    pub fn delay(&self, refresh: Duration) -> Duration {
        let Some(max) = self.max_render() else {
            return Duration::ZERO;
        };
        refresh
            .saturating_sub(max)
            .saturating_sub(MARGIN)
            .min(refresh / 2)
    }

    /// When the frame after the vblank at `vblank` starts: `None` (at once) when the delay is nothing.
    pub fn start_at(&self, vblank: Duration, refresh: Duration) -> Option<Duration> {
        let delay = self.delay(refresh);
        (!delay.is_zero()).then_some(vblank + delay)
    }

    /// The flip of a frame drawn for the vblank at `target` (`None`: drawn while the phase was unknown) came, with the
    /// driver's timestamp `stamp` (`None`: the server's own clock stood in for it). How many vblanks late it was,
    /// which counts as a miss here, and when the next frame starts: `None`, at once, without a real timestamp or with
    /// the late start off (`on`).
    pub fn flipped(
        &mut self,
        stamp: Option<Duration>,
        target: Option<Duration>,
        refresh: Duration,
        on: bool,
    ) -> Flipped {
        let late = match (stamp, target) {
            (Some(shown), Some(target)) => periods_late(target, shown, refresh),
            _ => 0,
        };
        if late > 0 {
            self.missed(refresh);
        }
        let start_at = stamp
            .filter(|_| on)
            .and_then(|vblank| self.start_at(vblank, refresh));
        Flipped { late, start_at }
    }
}

/// What a flip means for the next frame ([`LateLatch::flipped`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flipped {
    /// Vblanks after the one the frame was drawn for.
    pub late: u32,
    /// When the next frame starts; `None`: at once.
    pub start_at: Option<Duration>,
}

/// How many vblanks after `target` (the vblank a frame was drawn for) a flip shown at `shown` came, on a display of
/// period `refresh`: 0 for one shown on time, a flip half a period off still counting as on time (timestamps jitter).
pub fn periods_late(target: Duration, shown: Duration, refresh: Duration) -> u32 {
    if refresh.is_zero() {
        return 0;
    }
    let late = shown.saturating_sub(target);
    let periods = (late + refresh / 2).as_nanos() / refresh.as_nanos();
    u32::try_from(periods).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD: Duration = Duration::from_nanos(16_666_667);

    fn ms(n: f64) -> Duration {
        Duration::from_secs_f64(n / 1000.0)
    }

    #[test]
    fn the_late_start_is_on_unless_turned_off() {
        assert!(requested(None));
        assert!(requested(Some("1")));
        assert!(requested(Some("")));
        assert!(!requested(Some("0")));
        assert!(!requested(Some("off")));
    }

    #[test]
    fn without_render_times_a_frame_starts_at_once() {
        let latch = LateLatch::default();
        assert_eq!(latch.delay(PERIOD), Duration::ZERO);
        assert_eq!(latch.start_at(ms(100.0), PERIOD), None);
    }

    #[test]
    fn a_cheap_frame_starts_half_a_period_after_the_vblank_and_no_later() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(1.0));
        assert_eq!(latch.delay(PERIOD), PERIOD / 2);
        assert_eq!(
            latch.start_at(ms(100.0), PERIOD),
            Some(ms(100.0) + PERIOD / 2)
        );
    }

    #[test]
    fn a_costly_frame_starts_its_render_time_and_the_margin_before_the_next_vblank() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(1.0));
        latch.rendered(ms(10.0));
        latch.rendered(ms(3.0));
        // the window's maximum counts, not the last frame's time
        let delay = PERIOD - ms(10.0) - MARGIN;
        assert_eq!(latch.delay(PERIOD), delay);
        let start = latch.start_at(ms(200.0), PERIOD).unwrap();
        assert_eq!(start + ms(10.0) + MARGIN, ms(200.0) + PERIOD);
    }

    #[test]
    fn a_frame_that_takes_the_period_starts_at_once() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(15.0));
        assert_eq!(latch.delay(PERIOD), Duration::ZERO);
        assert_eq!(latch.start_at(ms(100.0), PERIOD), None);
    }

    #[test]
    fn only_a_real_timestamp_with_the_switch_on_delays_the_next_frame() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(1.0));
        let target = Some(ms(100.0));
        let on_time = latch.flipped(Some(ms(100.0)), target, PERIOD, true);
        assert_eq!(
            on_time,
            Flipped {
                late: 0,
                start_at: Some(ms(100.0) + PERIOD / 2)
            }
        );
        assert_eq!(
            latch.flipped(None, target, PERIOD, true),
            Flipped {
                late: 0,
                start_at: None
            },
            "the server's own clock: at once, and no miss is told"
        );
        assert_eq!(
            latch
                .flipped(Some(ms(100.0)), target, PERIOD, false)
                .start_at,
            None,
            "switched off: at once"
        );
        assert_eq!(
            latch.flipped(Some(ms(100.0)), None, PERIOD, true),
            on_time,
            "a frame drawn with the phase unknown has no vblank to miss"
        );
    }

    #[test]
    fn a_flip_after_the_vblank_it_was_drawn_for_is_a_miss_and_the_next_frame_starts_at_once() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(1.0));
        let target = ms(100.0);
        let flipped = latch.flipped(Some(target + PERIOD), Some(target), PERIOD, true);
        assert_eq!(
            flipped,
            Flipped {
                late: 1,
                start_at: None
            }
        );
        assert_eq!(latch.max_render(), Some(PERIOD));
        // the delay comes back once the miss has left the window
        for _ in 0..WINDOW {
            latch.rendered(ms(1.0));
        }
        let next = target + PERIOD * 2;
        assert_eq!(
            latch.flipped(Some(next), Some(next), PERIOD, true).start_at,
            Some(next + PERIOD / 2)
        );
    }

    #[test]
    fn a_slow_frame_counts_until_the_window_has_moved_past_it() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(12.0));
        for _ in 1..WINDOW {
            latch.rendered(ms(1.0));
        }
        assert_eq!(latch.max_render(), Some(ms(12.0)), "still in the window");
        latch.rendered(ms(1.0));
        assert_eq!(latch.max_render(), Some(ms(1.0)), "it left the window");
        assert_eq!(latch.delay(PERIOD), PERIOD / 2);
    }

    #[test]
    fn a_missed_vblank_turns_the_delay_off_for_the_window() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(1.0));
        latch.missed(PERIOD);
        assert_eq!(latch.delay(PERIOD), Duration::ZERO);
        for _ in 1..WINDOW {
            latch.rendered(ms(1.0));
            assert_eq!(latch.delay(PERIOD), Duration::ZERO);
        }
        latch.rendered(ms(1.0));
        assert_eq!(latch.delay(PERIOD), PERIOD / 2);
    }

    #[test]
    fn the_delay_follows_the_period_of_the_mode() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(4.0));
        let p144 = Duration::from_nanos(6_944_444);
        // 6.94 - 4 - 2 = 0.94 ms, under half the period
        assert_eq!(latch.delay(p144), p144 - ms(4.0) - MARGIN);
        latch.rendered(ms(5.0));
        assert_eq!(latch.delay(p144), Duration::ZERO);
        assert_eq!(latch.delay(PERIOD), PERIOD / 2);
    }

    #[test]
    fn a_flip_is_late_by_the_whole_periods_after_its_vblank() {
        let target = ms(1000.0);
        assert_eq!(periods_late(target, target, PERIOD), 0);
        assert_eq!(periods_late(target, target + ms(0.3), PERIOD), 0, "jitter");
        assert_eq!(periods_late(target, target - ms(0.3), PERIOD), 0, "jitter");
        assert_eq!(periods_late(target, target + PERIOD, PERIOD), 1);
        assert_eq!(
            periods_late(target, target + PERIOD * 3 + ms(0.2), PERIOD),
            3
        );
        assert_eq!(periods_late(target, target, Duration::ZERO), 0);
    }
}
