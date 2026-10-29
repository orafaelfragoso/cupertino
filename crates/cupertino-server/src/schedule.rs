//! When each output draws its next frame. One [`FrameScheduler`] per output holds what decides that: whether something
//! on the output changed, the grid of due times animation frames follow, and the retry time of frames that failed to
//! draw. A change is drawn at once; an animation frame waits for the output's next grid point, so each output keeps
//! its own refresh rate and an idle output beside an animating one does not compose on every loop pass. The scheduler
//! also counts the frame work the output did, so pacing is checked by counts instead of wall time.

use std::time::Duration;

/// A run of frames of one output that failed to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailedFrames {
    pub count: u32,
    /// Not drawn again before this time, so a failing output is retried at the display rate.
    pub retry_at: Duration,
}

/// The frame work of one output since it was added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCounts {
    /// Scenes composed for presentation.
    pub composes: u64,
    /// Render calls for presentation, failed ones included.
    pub renders: u64,
    /// Renders that found nothing different from the screen.
    pub empty: u64,
    /// Frames handed to the display: a page flip queued or a buffer submitted.
    pub queued: u64,
    /// Rounds of frame callbacks sent to the output's clients.
    pub callbacks: u64,
}

#[derive(Debug)]
pub struct FrameScheduler {
    damaged: bool,
    /// The next grid point: previous due time plus one refresh, moved to a vblank when one is reported.
    next_due: Option<Duration>,
    failed: Option<FailedFrames>,
    counts: FrameCounts,
}

impl Default for FrameScheduler {
    /// A new output draws its first frame.
    fn default() -> Self {
        Self {
            damaged: true,
            next_due: None,
            failed: None,
            counts: FrameCounts::default(),
        }
    }
}

impl FrameScheduler {
    /// Something on the output changed: it wants a frame.
    pub fn damage(&mut self) {
        self.damaged = true;
    }

    pub fn damaged(&self) -> bool {
        self.damaged
    }

    /// The output's clients were called back: what they asked for is on its way.
    pub fn called_back(&mut self) {
        self.damaged = false;
        self.counts.callbacks += 1;
    }

    /// Whether the output draws a frame at `now`: a change at once, an animation frame from its next grid point on, and
    /// neither before the retry time of a failed frame.
    pub fn due(&self, now: Duration, animating: bool) -> bool {
        self.retry_due(now)
            && (self.damaged || animating && self.next_due.is_none_or(|due| now >= due))
    }

    /// When the output is next due, if anything wants a frame: what the loop may sleep until.
    pub fn deadline(&self, now: Duration, animating: bool) -> Option<Duration> {
        let due = if self.damaged {
            now
        } else if animating {
            self.next_due.map_or(now, |due| due.max(now))
        } else {
            return None;
        };
        Some(self.failed.map_or(due, |f| due.max(f.retry_at)))
    }

    /// A frame cycle began at `now`. On or after the grid point the grid moves one `refresh` on, or restarts from `now`
    /// when that point has passed too (an output that sat idle, or a pass that came late), so nothing catches up in a
    /// burst. A change drawn before the grid point leaves the grid as it is.
    pub fn cycle(&mut self, now: Duration, refresh: Duration) {
        if self.next_due.is_some_and(|due| now < due) {
            return;
        }
        let next = self.next_due.unwrap_or(now) + refresh;
        self.next_due = Some(if next <= now { now + refresh } else { next });
    }

    /// The display showed a frame at `time`: the grid follows its vblank.
    pub fn vblank(&mut self, time: Duration, refresh: Duration) {
        self.next_due = Some(time + refresh);
    }

    /// A frame failed to draw at `time`: the output stays damaged, and is not drawn again before one `refresh` later.
    /// Returns how many frames failed in a row.
    pub fn failed(&mut self, time: Duration, refresh: Duration) -> u32 {
        let count = self.failed.map_or(0, |f| f.count).saturating_add(1);
        self.failed = Some(FailedFrames {
            count,
            retry_at: time + refresh,
        });
        self.damaged = true;
        count
    }

    /// Whether the output may draw at `time`: not before the retry time of a frame that failed.
    pub fn retry_due(&self, time: Duration) -> bool {
        self.failed.is_none_or(|f| time >= f.retry_at)
    }

    /// A frame was drawn, which ends a run of failed frames; returns how many failed before it.
    pub fn drawn(&mut self) -> Option<u32> {
        self.failed.take().map(|f| f.count)
    }

    pub fn composed(&mut self) {
        self.counts.composes += 1;
    }

    pub fn rendered(&mut self) {
        self.counts.renders += 1;
    }

    pub fn empty(&mut self) {
        self.counts.empty += 1;
    }

    pub fn queued(&mut self) {
        self.counts.queued += 1;
    }

    pub fn counts(&self) -> FrameCounts {
        self.counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFRESH: Duration = Duration::from_micros(16_667);
    const MS: Duration = Duration::from_millis(1);

    /// The refresh period of a mode, as the backends derive it.
    fn period(mhz: f64) -> Duration {
        Duration::from_secs_f64(1000.0 / mhz)
    }

    /// An output animating with nothing else changing for one second, with loop passes `pass` apart: its frames.
    fn animate_for_a_second(refresh: Duration, pass: Duration) -> u32 {
        let mut s = FrameScheduler::default();
        s.called_back();
        let mut frames = 0;
        let mut now = Duration::ZERO;
        while now < Duration::from_secs(1) {
            if s.due(now, true) {
                s.cycle(now, refresh);
                s.called_back();
                frames += 1;
            }
            now += pass;
        }
        frames
    }

    #[test]
    fn each_output_animates_at_its_own_refresh_whatever_the_loop_rate() {
        for pass in [MS, 4 * MS] {
            assert_eq!(animate_for_a_second(period(60_000.0), pass), 60, "{pass:?}");
            assert_eq!(
                animate_for_a_second(period(144_000.0), pass),
                144,
                "{pass:?}"
            );
        }
    }

    #[test]
    fn a_change_is_due_at_once_and_an_animation_frame_at_the_grid_point() {
        let mut s = FrameScheduler::default();
        s.cycle(Duration::ZERO, REFRESH);
        s.called_back();
        assert!(
            !s.due(MS, true),
            "the next animation frame waits for the grid"
        );
        assert!(s.due(REFRESH, true));
        assert!(!s.due(REFRESH, false), "nothing wants a frame");
        s.damage();
        assert!(s.due(MS, false), "a change is drawn at once");
        s.cycle(MS, REFRESH);
        s.called_back();
        assert!(
            s.due(REFRESH, true),
            "a change drawn before the grid point leaves the grid"
        );
    }

    #[test]
    fn a_late_pass_skips_forward_without_a_burst_and_a_vblank_moves_the_grid() {
        let mut s = FrameScheduler::default();
        s.cycle(Duration::ZERO, REFRESH);
        s.called_back();
        s.cycle(40 * MS, REFRESH);
        assert!(
            !s.due(41 * MS, true),
            "no catching up on missed grid points"
        );
        assert!(s.due(40 * MS + REFRESH, true));
        s.vblank(45 * MS, REFRESH);
        assert!(!s.due(40 * MS + REFRESH, true));
        assert!(s.due(45 * MS + REFRESH, true));
    }

    #[test]
    fn the_first_animation_frame_after_idle_is_due_at_once() {
        let mut s = FrameScheduler::default();
        s.cycle(Duration::ZERO, REFRESH);
        s.called_back();
        let start = Duration::from_secs(5) + 3 * MS;
        assert!(s.due(start, true));
        s.cycle(start, REFRESH);
        assert!(!s.due(start + REFRESH - MS, true));
        assert!(s.due(start + REFRESH, true));
    }

    #[test]
    fn the_deadline_is_now_for_a_change_the_grid_point_for_animation_and_none_when_idle() {
        let mut s = FrameScheduler::default();
        assert_eq!(
            s.deadline(MS, false),
            Some(MS),
            "a new output draws at once"
        );
        s.cycle(Duration::ZERO, REFRESH);
        s.called_back();
        assert_eq!(s.deadline(MS, false), None);
        assert_eq!(s.deadline(MS, true), Some(REFRESH));
        assert_eq!(s.deadline(2 * REFRESH, true), Some(2 * REFRESH));
        s.failed(MS, REFRESH);
        assert_eq!(
            s.deadline(2 * MS, false),
            Some(MS + REFRESH),
            "a failed frame waits for its retry"
        );
    }

    #[test]
    fn a_new_output_wants_its_first_frame_and_a_callback_round_settles_it() {
        let mut s = FrameScheduler::default();
        assert!(s.damaged());
        s.called_back();
        assert!(!s.damaged());
        s.damage();
        assert!(s.damaged());
        assert_eq!(s.counts().callbacks, 1);
    }

    #[test]
    fn a_failed_frame_is_retried_one_refresh_later_until_one_is_drawn() {
        let mut s = FrameScheduler::default();
        let t = Duration::from_secs(1);
        s.called_back();
        assert_eq!(s.failed(t, REFRESH), 1);
        assert!(s.damaged(), "a failed frame is drawn again");
        assert!(!s.retry_due(t + REFRESH - Duration::from_micros(1)));
        assert!(s.retry_due(t + REFRESH));
        assert_eq!(s.failed(t + REFRESH, REFRESH), 2);
        assert_eq!(s.drawn(), Some(2));
        assert!(
            s.retry_due(t),
            "a drawn frame ends the wait between retries"
        );
        assert_eq!(s.drawn(), None);
    }
}
