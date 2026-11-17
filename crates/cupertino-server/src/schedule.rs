//! When each output draws its next frame. One [`FrameScheduler`] per output holds what decides that: whether something
//! on the output changed, the grid of due times animation frames follow, and the retry time of frames that failed to
//! draw. A change is drawn at once; an animation frame waits for the output's next grid point, so each output keeps
//! its own refresh rate and an idle output beside an animating one does not compose on every loop pass. The scheduler
//! also counts the frame work the output did, so pacing is checked by counts instead of wall time.
//!
//! On a display with page flips (udev) every shown vblank marks its output, so an animating head draws right after
//! each vblank and the flip is what paces it; there the grid paces heads that queue no flips, such as an idle head
//! beside an animating one. Headless and nested outputs have no flips, and the grid paces all their animation.
//!
//! A flip queued to the display gets a [`PresentationId`]. Only the vblank of the flip in flight confirms a frame; an
//! event of an earlier generation, after a reset that dropped its flip, changes nothing. A flip whose vblank does not
//! come times out: the output's clients are then called back once per refresh, so none waits on a dead head, until the
//! event arrives or the backend gives the flip up.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Generations of every output's presentations: a reset takes the next one, so no id is ever used twice.
static GENERATIONS: AtomicU64 = AtomicU64::new(0);

/// One flip queued to the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PresentationId {
    pub generation: u64,
    pub sequence: u64,
}

/// The flip in flight, once its vblank came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presented {
    /// The frame showed the locked scene.
    pub locked: bool,
    /// Refresh periods it was overdue by.
    pub late_periods: u32,
}

/// What a flip that outlived its vblank asks of the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overdue {
    /// The flip just timed out: call the clients back, and say so once.
    TimedOut,
    /// Another refresh passed without the vblank: call the clients back.
    CallBack,
    /// No vblank for long enough that the head is dead: drop the flip.
    GiveUp,
}

#[derive(Clone, Copy, Debug)]
struct InFlight {
    id: PresentationId,
    queued_at: Duration,
    locked: bool,
    /// The next paced callback once the flip timed out.
    callback_at: Option<Duration>,
    late_periods: u32,
    given_up: bool,
}

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
    /// A change waits for the grid point too ([`FrameScheduler::pace_changes`]).
    paced: bool,
    /// The time of the last vblank the display reported: the phase of its refresh grid.
    last_vblank: Option<Duration>,
    /// No frame starts before this time: the late start after a vblank ([`FrameScheduler::hold_until`]).
    held_until: Option<Duration>,
    failed: Option<FailedFrames>,
    in_flight: Option<InFlight>,
    generation: u64,
    next_sequence: u64,
    counts: FrameCounts,
}

impl Default for FrameScheduler {
    /// A new output draws its first frame.
    fn default() -> Self {
        Self {
            damaged: true,
            next_due: None,
            paced: false,
            last_vblank: None,
            held_until: None,
            failed: None,
            in_flight: None,
            generation: GENERATIONS.fetch_add(1, Ordering::Relaxed),
            next_sequence: 0,
            counts: FrameCounts::default(),
        }
    }
}

impl FrameScheduler {
    /// Whether a change waits for the grid point too, for an output that no vblank paces (an offscreen one): a client
    /// that draws on every frame callback then runs at the refresh rate, not as fast as the loop turns.
    pub fn pace_changes(&mut self, paced: bool) {
        self.paced = paced;
    }

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

    /// Whether the output draws a frame at `now`: a change at once (at the next grid point without a vblank), an
    /// animation frame from its next grid point on, and neither before the retry time of a failed frame.
    pub fn due(&self, now: Duration, animating: bool) -> bool {
        let on_grid = self.next_due.is_none_or(|due| now >= due);
        let held = self.held_until.is_some_and(|until| now < until);
        self.retry_due(now)
            && !held
            && (self.damaged && (on_grid || !self.paced) || animating && on_grid)
    }

    /// When the output is next due, if anything wants a frame: what the loop may sleep until.
    pub fn deadline(&self, now: Duration, animating: bool) -> Option<Duration> {
        let due = if self.damaged && !self.paced {
            now
        } else if self.damaged || animating {
            self.next_due.map_or(now, |due| due.max(now))
        } else {
            return None;
        };
        let due = self.held_until.map_or(due, |until| due.max(until));
        Some(self.failed.map_or(due, |f| due.max(f.retry_at)))
    }

    /// The next frame starts no earlier than `until` (`None`: at once): a display whose vblank phase is known starts
    /// its frame late, so it shows the latest input ([`crate::latch`]). The hold ends with the next frame cycle and with a
    /// reset.
    /// When the display last reported a vblank ([`FrameScheduler::vblank`]).
    pub fn last_vblank(&self) -> Option<Duration> {
        self.last_vblank
    }

    pub fn hold_until(&mut self, until: Option<Duration>) {
        self.held_until = until;
    }

    /// A frame cycle began at `now`. On or after the grid point the grid moves one `refresh` on, or restarts from `now`
    /// when that point has passed too (an output that sat idle, or a pass that came late), so nothing catches up in a
    /// burst. A change drawn before the grid point leaves the grid as it is.
    pub fn cycle(&mut self, now: Duration, refresh: Duration) {
        self.held_until = None;
        if self.next_due.is_some_and(|due| now < due) {
            return;
        }
        let next = self.next_due.unwrap_or(now) + refresh;
        self.next_due = Some(if next <= now { now + refresh } else { next });
    }

    /// The display showed a frame at `time`: the grid follows its vblank.
    pub fn vblank(&mut self, time: Duration, refresh: Duration) {
        self.next_due = Some(time + refresh);
        self.last_vblank = Some(time);
    }

    /// When a frame drawn at `now` reaches the screen: the first vblank after `now` on the grid of the last one the
    /// display reported. Animation is sampled at this time, so each frame shows where the motion is when it is seen,
    /// however long after the vblank the loop drew it. `None` without a reported vblank (an output no flip paces) or
    /// when the last one is more than [`PHASE_TRUSTED_FOR`] old, as the refresh period is rounded and the phase drifts.
    pub fn presentation_at(&self, now: Duration, refresh: Duration) -> Option<Duration> {
        let vblank = self.last_vblank?;
        if refresh.is_zero() {
            return None;
        }
        let Some(since) = now.checked_sub(vblank) else {
            return Some(vblank + refresh);
        };
        if since > PHASE_TRUSTED_FOR {
            return None;
        }
        let periods = since.as_nanos() / refresh.as_nanos() + 1;
        Some(vblank + refresh * u32::try_from(periods).ok()?)
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

    /// The id the next queued flip gets.
    pub fn next_presentation(&self) -> PresentationId {
        PresentationId {
            generation: self.generation,
            sequence: self.next_sequence,
        }
    }

    /// A flip was queued at `now`, of a frame that showed the locked scene when `locked`.
    pub fn flip_queued(&mut self, now: Duration, locked: bool) -> PresentationId {
        let id = self.next_presentation();
        self.next_sequence += 1;
        self.in_flight = Some(InFlight {
            id,
            queued_at: now,
            locked,
            callback_at: None,
            late_periods: 0,
            given_up: false,
        });
        self.queued();
        id
    }

    /// The vblank of flip `id` came: the flip in flight is shown. Any other id is stale and confirms nothing.
    pub fn presented(&mut self, id: PresentationId) -> Option<Presented> {
        let flight = self.in_flight.filter(|f| f.id == id)?;
        self.in_flight = None;
        Some(Presented {
            locked: flight.locked,
            late_periods: flight.late_periods,
        })
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    /// The backend ended the flip in flight without its vblank (cleared CRTC, sleep, resume, mode change): it will
    /// never be shown, and a late event for it is stale. The output draws its next frame in full.
    pub fn reset(&mut self) {
        self.in_flight = None;
        self.held_until = None;
        self.generation = GENERATIONS.fetch_add(1, Ordering::Relaxed);
        self.damaged = true;
    }

    /// The flip in flight at `now`, if its vblank is overdue: it times out after three refresh periods (at least 50 ms),
    /// calls the clients back once per refresh after that, and is given up once, after ten periods (at least one
    /// second) without the vblank.
    pub fn overdue(&mut self, now: Duration, refresh: Duration) -> Option<Overdue> {
        let flight = self.in_flight.as_mut()?;
        let waited = now.saturating_sub(flight.queued_at);
        if waited < timeout_after(refresh) {
            return None;
        }
        if !flight.given_up && waited >= give_up_after(refresh) {
            flight.given_up = true;
            return Some(Overdue::GiveUp);
        }
        match flight.callback_at {
            None => {
                flight.callback_at = Some(now + refresh);
                flight.late_periods = 1;
                Some(Overdue::TimedOut)
            }
            Some(at) if now >= at => {
                let next = at + refresh;
                flight.callback_at = Some(if next <= now { now + refresh } else { next });
                flight.late_periods += 1;
                Some(Overdue::CallBack)
            }
            Some(_) => None,
        }
    }

    /// When [`FrameScheduler::overdue`] next has something to report for the flip in flight: its timeout, its next
    /// paced callback, or the moment it is given up.
    pub fn overdue_deadline(&self, refresh: Duration) -> Option<Duration> {
        let flight = self.in_flight?;
        let next = flight
            .callback_at
            .unwrap_or(flight.queued_at + timeout_after(refresh));
        Some(if flight.given_up {
            next
        } else {
            next.min(flight.queued_at + give_up_after(refresh))
        })
    }

    pub fn counts(&self) -> FrameCounts {
        self.counts
    }
}

/// How long after a vblank the refresh grid's phase is trusted to predict the next one.
pub const PHASE_TRUSTED_FOR: Duration = Duration::from_secs(1);

/// How long a flip waits for its vblank before its clients are called back without it.
fn timeout_after(refresh: Duration) -> Duration {
    (3 * refresh).max(Duration::from_millis(50))
}

/// How long a flip waits for its vblank before it is given up.
fn give_up_after(refresh: Duration) -> Duration {
    (10 * refresh).max(Duration::from_secs(1))
}
