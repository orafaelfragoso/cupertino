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

    /// Frames in one second of an output whose one client commits a new frame as soon as it is called back, with loop
    /// passes 100 µs apart.
    fn client_frames_in_a_second(mut s: FrameScheduler) -> u32 {
        let mut frames = 0;
        let mut now = Duration::ZERO;
        while now < Duration::from_secs(1) {
            if s.due(now, false) {
                s.cycle(now, REFRESH);
                s.called_back();
                s.damage();
                frames += 1;
            }
            now += Duration::from_micros(100);
        }
        frames
    }

    #[test]
    fn without_a_vblank_a_client_drawing_on_every_callback_runs_at_the_refresh_rate() {
        let mut paced = FrameScheduler::default();
        paced.pace_changes(true);
        assert_eq!(client_frames_in_a_second(paced), 60);
        assert_eq!(
            client_frames_in_a_second(FrameScheduler::default()),
            10_000,
            "with a vblank the flip in flight paces it, not the scheduler"
        );
    }

    #[test]
    fn without_a_vblank_a_change_is_drawn_at_the_next_grid_point() {
        let mut s = FrameScheduler::default();
        s.pace_changes(true);
        assert!(s.due(Duration::ZERO, false), "a new output draws at once");
        s.cycle(Duration::ZERO, REFRESH);
        s.called_back();
        s.damage();
        assert!(!s.due(MS, false));
        assert_eq!(s.deadline(MS, false), Some(REFRESH));
        assert!(s.due(REFRESH, false));
        s.reset();
        assert!(!s.due(MS, false), "a reset keeps the pacing");
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

    #[test]
    fn only_the_vblank_of_the_flip_in_flight_confirms_a_frame() {
        let mut s = FrameScheduler::default();
        let first = s.flip_queued(Duration::ZERO, true);
        let second = PresentationId {
            sequence: first.sequence + 1,
            ..first
        };
        assert_eq!(s.presented(second), None, "an id not in flight is stale");
        assert!(s.in_flight());
        assert_eq!(
            s.presented(first),
            Some(Presented {
                locked: true,
                late_periods: 0
            })
        );
        assert_eq!(s.presented(first), None, "a vblank confirms its flip once");
        let next = s.flip_queued(MS, false);
        assert!(next.sequence > first.sequence);
        assert_eq!(s.counts().queued, 2);
    }

    #[test]
    fn a_reset_drops_the_flip_and_makes_its_late_vblank_stale() {
        let mut s = FrameScheduler::default();
        s.called_back();
        let before = s.flip_queued(Duration::ZERO, true);
        s.reset();
        assert!(!s.in_flight());
        assert!(s.damaged(), "the next frame is drawn in full");
        let after = s.flip_queued(MS, true);
        assert_ne!(after.generation, before.generation);
        assert_eq!(s.presented(before), None);
        assert!(s.presented(after).is_some());
        let other = FrameScheduler::default();
        assert_ne!(
            other.generation, after.generation,
            "no two outputs share a generation"
        );
    }

    #[test]
    fn an_overdue_flip_times_out_calls_back_once_per_refresh_and_is_given_up_once() {
        let refresh = period(60_000.0);
        let mut s = FrameScheduler::default();
        let id = s.flip_queued(Duration::ZERO, false);
        let mut events = Vec::new();
        let mut now = Duration::ZERO;
        while now < Duration::from_millis(1500) {
            if let Some(e) = s.overdue(now, refresh) {
                events.push((now, e));
            }
            now += MS;
        }
        let timeout = (3 * refresh).max(50 * MS);
        assert_eq!(events[0].1, Overdue::TimedOut);
        assert!(
            events[0].0 >= timeout && events[0].0 < timeout + MS,
            "three refresh periods: {:?}",
            events[0].0
        );
        let callbacks: Vec<Duration> = events
            .iter()
            .filter(|(_, e)| *e != Overdue::GiveUp)
            .map(|&(t, _)| t)
            .collect();
        let gaps: Vec<Duration> = callbacks.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.iter().all(|&g| g + MS > refresh && g < refresh + MS),
            "one callback per refresh: {gaps:?}"
        );
        let given_up: Vec<Duration> = events
            .iter()
            .filter(|(_, e)| *e == Overdue::GiveUp)
            .map(|&(t, _)| t)
            .collect();
        assert_eq!(given_up, [Duration::from_secs(1)], "once, after one second");
        let late = s
            .presented(id)
            .expect("the flip is still in flight")
            .late_periods;
        assert!(late > 50, "{late} periods without a vblank");

        let fast = period(144_000.0);
        let mut s = FrameScheduler::default();
        s.flip_queued(Duration::ZERO, false);
        assert_eq!(s.overdue(49 * MS, fast), None, "the floor is 50 ms");
        assert_eq!(s.overdue(50 * MS, fast), Some(Overdue::TimedOut));
        assert_ne!(
            s.overdue(500 * MS, fast),
            Some(Overdue::GiveUp),
            "not before one second at 144 Hz"
        );
    }

    #[test]
    fn a_frame_is_presented_at_the_first_vblank_after_it_is_drawn() {
        let mut s = FrameScheduler::default();
        assert_eq!(s.presentation_at(MS, REFRESH), None, "no vblank seen yet");
        let v = Duration::from_secs(3);
        s.vblank(v, REFRESH);
        assert_eq!(s.presentation_at(v, REFRESH), Some(v + REFRESH));
        assert_eq!(s.presentation_at(v + MS, REFRESH), Some(v + REFRESH));
        assert_eq!(
            s.presentation_at(v + REFRESH + MS, REFRESH),
            Some(v + 2 * REFRESH),
            "a loop that drew past the next vblank waits for the one after"
        );
        assert_eq!(
            s.presentation_at(v + PHASE_TRUSTED_FOR + MS, REFRESH),
            None,
            "an old phase is not trusted"
        );
    }

    /// The model times a 60 Hz head samples animation at over one second, drawing each frame `delay(n)` after its vblank,
    /// as the loop gets to it: at the time the frame is drawn, or at the time it is presented.
    fn sample_times(delay: impl Fn(u32) -> Duration, at_presentation: bool) -> Vec<Duration> {
        let mut s = FrameScheduler::default();
        let mut samples = Vec::new();
        for n in 0..60 {
            let vblank = Duration::from_secs(10) + REFRESH * n;
            s.vblank(vblank, REFRESH);
            let drawn = vblank + delay(n);
            samples.push(if at_presentation {
                s.presentation_at(drawn, REFRESH)
                    .expect("a vblank was seen")
            } else {
                drawn
            });
        }
        samples
    }

    #[test]
    fn sampling_at_the_presentation_time_steps_animation_one_refresh_per_frame_however_late_the_loop_draws(
    ) {
        // the loop draws 0.2 to 9 ms after each vblank, as events, clients and the previous frame keep it busy
        let delay = |n: u32| Duration::from_micros(200 + u64::from(n * 7919 % 8800));
        let steps = |samples: Vec<Duration>| -> Vec<Duration> {
            samples.windows(2).map(|w| w[1] - w[0]).collect()
        };
        let at_draw = steps(sample_times(delay, false));
        let jitter = at_draw.iter().max().unwrap().as_micros() as i64
            - at_draw.iter().min().unwrap().as_micros() as i64;
        assert!(
            jitter > 4000,
            "sampled when drawn, animation steps vary by {jitter} us between frames shown one refresh apart"
        );
        let at_present = steps(sample_times(delay, true));
        assert!(
            at_present.iter().all(|&step| step == REFRESH),
            "sampled when presented, every step is one refresh: {at_present:?}"
        );
    }

    #[test]
    fn a_failed_submission_confirms_nothing_and_starts_the_head_over() {
        let mut s = FrameScheduler::default();
        s.called_back();
        let parked = s.flip_queued(Duration::ZERO, true);
        s.reset();
        assert_eq!(
            s.presented(parked),
            None,
            "the frame that failed to submit was never shown"
        );
        assert!(s.damaged(), "the next frame is drawn in full");
    }

    #[test]
    fn a_held_frame_waits_for_its_late_start_and_the_next_cycle_ends_the_hold() {
        let mut s = FrameScheduler::default();
        let vblank = 100 * MS;
        s.vblank(vblank, REFRESH);
        s.damage();
        s.hold_until(Some(vblank + 8 * MS));
        assert!(
            !s.due(vblank + MS, false),
            "a change waits for the late start"
        );
        assert!(!s.due(vblank + MS, true), "so does an animation frame");
        assert_eq!(s.deadline(vblank + MS, false), Some(vblank + 8 * MS));
        assert!(s.due(vblank + 8 * MS, false));
        s.cycle(vblank + 8 * MS, REFRESH);
        s.called_back();
        s.damage();
        assert!(
            s.due(vblank + 9 * MS, false),
            "the frame cycle ended the hold"
        );
        s.hold_until(Some(vblank + 30 * MS));
        s.reset();
        assert!(s.due(vblank + 10 * MS, false), "so does a reset");
        s.hold_until(None);
        assert!(s.due(vblank + 10 * MS, false), "no hold: at once");
    }
}
