//! How late after a vblank a display's next frame starts. A frame drawn right after the vblank sits finished for most
//! of the period before its flip, and every input that arrives meanwhile waits a whole period more. Where the display's
//! flips report real presentation timestamps (the driver's flip timestamp, not the server's own clock), the vblank's
//! phase is known, so the frame can start later and still make the next vblank: it starts the recent maximum render
//! time plus [`MARGIN`] before that vblank, and never later than half a period after the last one, because a missed
//! refresh costs more than the latency it saves.
//!
//! The render times are the last [`WINDOW`] frames' times from the start of drawing to the GPU finishing the frame,
//! the moment its render fence signalled ([`GpuDone::At`]), as the flip is queued before the GPU is done. Where the
//! fence cannot be read, a frame's render time is its time to the flip being queued plus a kept estimate of the GPU's
//! part, which a held frame that still missed its vblank raises for good ([`LateLatch::gpu_estimate`]). A frame that
//! missed its vblank also turns the delay off: for [`WINDOW`] frames, and twice as long as the time before for each
//! miss that follows within a window of the delay coming back, up to [`MAX_OFF`]. Without render times yet, or with the
//! switch off (`CUPERTINO_LATE_LATCH=0`), frames start at once, as on a display whose timestamps come from the server's
//! own clock and on the headless and nested backends.

use std::collections::VecDeque;
use std::time::Duration;

/// Turns the late start off when set to `0` or `off`; on otherwise.
pub const ENV: &str = "CUPERTINO_LATE_LATCH";

/// How many recent frames' render times the delay is taken from, and how many frames a first miss turns it off for:
/// one second of frames at 60 Hz.
pub const WINDOW: usize = 60;

/// The longest the delay stays off after misses that follow one another: 16 windows, 16 seconds at 60 Hz.
pub const MAX_OFF: u32 = WINDOW as u32 * 16;

/// Time kept between the recent maximum render time and the vblank: the commit, the flip's own latency and the loop's
/// wake-up.
pub const MARGIN: Duration = Duration::from_micros(2_000);

/// Whether the late start is on, from the variable's value.
pub fn requested(value: Option<&str>) -> bool {
    !matches!(value.map(str::trim), Some("0" | "off"))
}

/// When the GPU finished a frame, as far as the backend can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuDone {
    /// Within the CPU time: the renderer waited for the GPU, or the frame drew nothing of its own on it (a client
    /// buffer scanned out).
    WithinCpu,
    /// Its render fence signalled this long after the frame started.
    At(Duration),
    /// Not known: the fence could not be read.
    Unknown,
}

/// The times of a frame whose flip came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTimes {
    /// When it started, on the clock of the flips' timestamps.
    pub started: Duration,
    /// From its start to its flip being queued.
    pub cpu: Duration,
    /// When the GPU finished it.
    pub gpu: GpuDone,
    /// It started late after a vblank, held by the delay.
    pub held: bool,
}

/// The recent render times of one display and the delay they allow.
#[derive(Clone, Debug, Default)]
pub struct LateLatch {
    times: VecDeque<Duration>,
    /// The GPU's part of a frame whose fence cannot be read, kept from the misses of such frames.
    gpu_estimate: Duration,
    /// Frames left with the delay off.
    off: u32,
    /// How many frames the last miss turned the delay off for; 0 before any miss.
    off_len: u32,
    /// Frames since the last miss; `None` before any.
    since_miss: Option<u32>,
}

impl LateLatch {
    /// A frame took `took` from the start of drawing to the GPU finishing it.
    pub fn rendered(&mut self, took: Duration) {
        self.push(took);
        self.off = self.off.saturating_sub(1);
        self.since_miss = self.since_miss.map(|n| n.saturating_add(1));
    }

    fn push(&mut self, took: Duration) {
        if self.times.len() == WINDOW {
            self.times.pop_front();
        }
        self.times.push_back(took);
    }

    /// A frame's render time from its CPU time `cpu` and when its GPU work was done: the later of the two, or the CPU
    /// time and the kept estimate where the fence could not be read.
    pub fn render_time(&self, cpu: Duration, gpu: GpuDone) -> Duration {
        match gpu {
            GpuDone::WithinCpu => cpu,
            GpuDone::At(done) => cpu.max(done),
            GpuDone::Unknown => cpu + self.gpu_estimate,
        }
    }

    /// A frame's flip came after the vblank it was drawn for: it counts as a whole `refresh` in the window, and the
    /// delay is off for [`WINDOW`] frames, or for twice as long as the last time when the delay had been back for at
    /// most a window, up to [`MAX_OFF`].
    pub fn missed(&mut self, refresh: Duration) {
        self.push(refresh);
        let window = WINDOW as u32;
        let soon = self
            .since_miss
            .is_some_and(|n| n <= self.off_len.saturating_add(window));
        self.off_len = if soon {
            self.off_len.saturating_mul(2).clamp(window, MAX_OFF)
        } else {
            window
        };
        self.off = self.off_len;
        self.since_miss = Some(0);
    }

    /// The longest render time in the window.
    pub fn max_render(&self) -> Option<Duration> {
        self.times.iter().max().copied()
    }

    /// The kept estimate of the GPU's part of a frame whose fence cannot be read.
    pub fn gpu_estimate(&self) -> Duration {
        self.gpu_estimate
    }

    /// How many more frames the delay stays off for.
    pub fn off_for(&self) -> u32 {
        self.off
    }

    /// How long after a vblank the next frame starts on a display of period `refresh`: the period less the recent
    /// maximum render time and [`MARGIN`], at most half the period; nothing without render times or while a miss keeps
    /// the delay off.
    pub fn delay(&self, refresh: Duration) -> Duration {
        let Some(max) = self.max_render().filter(|_| self.off == 0) else {
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
    /// driver's timestamp `stamp` (`None`: the server's own clock stood in for it); `frame` holds the frame's times,
    /// where the backend kept them, and its render time joins the window. How many vblanks late the flip was, which
    /// counts as a miss here, and when the next frame starts: `None`, at once, without a real timestamp or with the
    /// late start off (`on`). A held frame whose GPU time is not known and which still missed needed more than the
    /// time from its start to that vblank, so the kept estimate of the GPU's part rises to at least that less its CPU
    /// time.
    pub fn flipped(
        &mut self,
        frame: Option<FrameTimes>,
        stamp: Option<Duration>,
        target: Option<Duration>,
        refresh: Duration,
        on: bool,
    ) -> Flipped {
        if let Some(f) = frame {
            self.rendered(self.render_time(f.cpu, f.gpu));
        }
        let late = match (stamp, target) {
            (Some(shown), Some(target)) => periods_late(target, shown, refresh),
            _ => 0,
        };
        if late > 0 {
            if let (Some(f), Some(target)) = (frame, target) {
                if f.held && f.gpu == GpuDone::Unknown {
                    let needed = target.saturating_sub(f.started).saturating_sub(f.cpu);
                    self.gpu_estimate = self.gpu_estimate.max(needed).min(refresh);
                }
            }
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

    fn frame(cpu: f64, gpu: GpuDone) -> FrameTimes {
        FrameTimes {
            started: ms(90.0),
            cpu: ms(cpu),
            gpu,
            held: false,
        }
    }

    #[test]
    fn a_frame_cheap_on_the_cpu_but_heavy_on_the_gpu_gets_no_late_start() {
        let target = Some(ms(100.0));
        let mut latch = LateLatch::default();
        let flipped = latch.flipped(
            Some(frame(1.0, GpuDone::At(ms(15.0)))),
            Some(ms(100.0)),
            target,
            PERIOD,
            true,
        );
        assert_eq!(
            latch.max_render(),
            Some(ms(15.0)),
            "the fence's time counts"
        );
        assert_eq!(flipped.start_at, None);
    }

    #[test]
    fn a_gpu_heavy_frame_starts_its_gpu_time_and_the_margin_before_the_vblank() {
        let mut latch = LateLatch::default();
        let vblank = ms(100.0);
        let flipped = latch.flipped(
            Some(frame(1.0, GpuDone::At(ms(12.0)))),
            Some(vblank),
            Some(vblank),
            PERIOD,
            true,
        );
        let start = flipped.start_at.expect("a short late start");
        assert_eq!(start + ms(12.0) + MARGIN, vblank + PERIOD);
        // the CPU time counts where it is the longer
        assert_eq!(latch.render_time(ms(5.0), GpuDone::At(ms(3.0))), ms(5.0));
        assert_eq!(latch.render_time(ms(5.0), GpuDone::WithinCpu), ms(5.0));
    }

    #[test]
    fn a_held_frame_whose_gpu_time_is_unknown_and_missed_raises_the_kept_estimate_for_good() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(1.0));
        let vblank = ms(100.0);
        let target = vblank + PERIOD;
        let started = latch.start_at(vblank, PERIOD).unwrap();
        let missed = FrameTimes {
            started,
            cpu: ms(1.0),
            gpu: GpuDone::Unknown,
            held: true,
        };
        let flipped = latch.flipped(
            Some(missed),
            Some(target + PERIOD),
            Some(target),
            PERIOD,
            true,
        );
        assert_eq!(flipped.late, 1);
        // it had half a period from its start, and the CPU used 1 ms of it
        assert_eq!(latch.gpu_estimate(), PERIOD - PERIOD / 2 - ms(1.0));
        assert_eq!(
            latch.render_time(ms(1.0), GpuDone::Unknown),
            PERIOD - PERIOD / 2,
            "an unknown GPU time counts as the estimate"
        );
        // the estimate outlasts the window
        for _ in 0..WINDOW * 3 {
            latch.rendered(ms(1.0));
        }
        assert_eq!(latch.gpu_estimate(), PERIOD - PERIOD / 2 - ms(1.0));
        // a frame not held, or one whose fence was read, does not raise it
        let mut other = LateLatch::default();
        for f in [
            FrameTimes {
                held: false,
                ..missed
            },
            FrameTimes {
                gpu: GpuDone::At(ms(14.0)),
                ..missed
            },
        ] {
            other.flipped(Some(f), Some(target + PERIOD), Some(target), PERIOD, true);
        }
        assert_eq!(other.gpu_estimate(), Duration::ZERO);
    }

    /// A display whose frames cost `cpu` on the CPU and `gpu` more on the GPU, which the backend cannot see: how many of
    /// `frames` frames miss their vblank.
    fn misses_with_unseen_gpu_time(cpu: Duration, gpu: Duration, frames: usize) -> usize {
        let mut latch = LateLatch::default();
        let mut vblank = ms(1000.0);
        let mut next = None;
        let mut misses = 0;
        for _ in 0..frames {
            let started = next.unwrap_or(vblank);
            let target = vblank + PERIOD;
            let done = started + cpu + gpu;
            // the flip makes its vblank when the GPU is done by then
            let shown = if done <= target {
                target
            } else {
                target + PERIOD
            };
            let times = FrameTimes {
                started,
                cpu,
                gpu: GpuDone::Unknown,
                held: next.is_some(),
            };
            let flipped = latch.flipped(Some(times), Some(shown), Some(target), PERIOD, true);
            misses += usize::from(flipped.late > 0);
            next = flipped.start_at;
            vblank = shown;
        }
        misses
    }

    #[test]
    fn a_frame_whose_gpu_time_cannot_be_read_stops_missing_after_a_miss() {
        // 1 ms on the CPU and 9 ms on the GPU: a half-period start misses, an earlier one does not
        let misses = misses_with_unseen_gpu_time(ms(1.0), ms(9.0), 600);
        assert_eq!(
            misses, 1,
            "ten seconds at 60 Hz miss once, not once a second"
        );
        // a cheap frame never misses
        assert_eq!(misses_with_unseen_gpu_time(ms(1.0), ms(1.0), 600), 0);
    }

    #[test]
    fn misses_that_follow_one_another_keep_the_delay_off_for_longer() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(1.0));
        let mut off = Vec::new();
        for _ in 0..6 {
            latch.missed(PERIOD);
            let len = latch.off_for();
            off.push(len);
            // the delay comes back after the off period, and the next miss comes at once
            for _ in 0..len {
                assert_eq!(latch.delay(PERIOD), Duration::ZERO);
                latch.rendered(ms(1.0));
            }
            assert_eq!(latch.delay(PERIOD), PERIOD / 2);
        }
        assert_eq!(
            off,
            [60, 120, 240, 480, 960, 960],
            "doubling up to the bound"
        );
        assert_eq!(MAX_OFF, 960);
        // a miss a window after the delay came back starts over from one window
        for _ in 0..=WINDOW {
            latch.rendered(ms(1.0));
        }
        latch.missed(PERIOD);
        assert_eq!(latch.off_for(), WINDOW as u32);
    }

    #[test]
    fn only_a_real_timestamp_with_the_switch_on_delays_the_next_frame() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(1.0));
        let target = Some(ms(100.0));
        let on_time = latch.flipped(None, Some(ms(100.0)), target, PERIOD, true);
        assert_eq!(
            on_time,
            Flipped {
                late: 0,
                start_at: Some(ms(100.0) + PERIOD / 2)
            }
        );
        assert_eq!(
            latch.flipped(None, None, target, PERIOD, true),
            Flipped {
                late: 0,
                start_at: None
            },
            "the server's own clock: at once, and no miss is told"
        );
        assert_eq!(
            latch
                .flipped(None, Some(ms(100.0)), target, PERIOD, false)
                .start_at,
            None,
            "switched off: at once"
        );
        assert_eq!(
            latch.flipped(None, Some(ms(100.0)), None, PERIOD, true),
            on_time,
            "a frame drawn with the phase unknown has no vblank to miss"
        );
    }

    #[test]
    fn a_flip_after_the_vblank_it_was_drawn_for_is_a_miss_and_the_next_frame_starts_at_once() {
        let mut latch = LateLatch::default();
        latch.rendered(ms(1.0));
        let target = ms(100.0);
        let flipped = latch.flipped(None, Some(target + PERIOD), Some(target), PERIOD, true);
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
            latch
                .flipped(None, Some(next), Some(next), PERIOD, true)
                .start_at,
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
