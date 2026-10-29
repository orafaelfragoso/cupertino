//! When each output draws its next frame. One [`FrameScheduler`] per output holds what decides that: whether something
//! on the output changed, and the retry time of frames that failed to draw. It also counts the frame work the output
//! did, so pacing is checked by counts instead of wall time.

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
    failed: Option<FailedFrames>,
    counts: FrameCounts,
}

impl Default for FrameScheduler {
    /// A new output draws its first frame.
    fn default() -> Self {
        Self {
            damaged: true,
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

    /// A frame failed to draw at `time`: the output stays damaged, and is not drawn again before one `refresh` later.
    /// Returns how many frames failed in a row.
    pub fn failed(&mut self, time: Duration, refresh: Duration) -> u32 {
        let count = self.failed.map_or(0, |f| f.count) + 1;
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
