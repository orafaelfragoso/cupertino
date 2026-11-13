//! What each frame costs, phase by phase, opt-in: [`Runtime::with_frame_timing`](crate::Runtime) gives the runtime a
//! sink, and every turn that presents a frame is reported once its last window was handed to the window system.
//!
//! The phases of a turn: `update` (window system events, handlers, timers, tasks and the model's update), `view` (the
//! views built again and reconciled with the tree), `layout` (layout, placements, semantics and focus of the windows
//! laid out again), `paint` (display lists and damage), `raster` (pixels drawn into the frame), `copy` (pixels copied
//! into the window system's buffers) and `commit` (the rest of handing the frame over). `total` is the whole turn, so
//! what no phase covers (accessibility and semantics updates, popups following their anchors) is `total` minus their
//! sum. Without a sink the runtime keeps nothing and reads no clock: it only looks whether it has one.

use std::time::{Duration, Instant};

use cupertino_ui_platform::Timestamp;

/// The phases of one turn that presented at least one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameTiming {
    /// When the turn began, on the runtime's clock (`CLOCK_MONOTONIC` on the desktop).
    pub start: Timestamp,
    pub update: Duration,
    pub view: Duration,
    pub layout: Duration,
    pub paint: Duration,
    pub raster: Duration,
    pub copy: Duration,
    pub commit: Duration,
    /// The whole turn.
    pub total: Duration,
    /// Frames presented in the turn (a window and its raised layer count once).
    pub frames: u32,
}

/// Hears every turn that presented a frame.
pub type FrameTimingSink = Box<dyn FnMut(&FrameTiming)>;

/// A phase of a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Update,
    View,
    Layout,
    Paint,
    Raster,
    Copy,
    Commit,
}

/// The turn being timed and where its timings go.
pub(crate) struct FrameTimings {
    sink: FrameTimingSink,
    turn: FrameTiming,
    began: Option<Instant>,
}

impl FrameTimings {
    pub(crate) fn new(sink: FrameTimingSink) -> FrameTimings {
        FrameTimings {
            sink,
            turn: FrameTiming::default(),
            began: None,
        }
    }

    /// A turn begins at `start` on the runtime's clock.
    pub(crate) fn begin(&mut self, start: Timestamp) -> Instant {
        let now = Instant::now();
        self.turn = FrameTiming {
            start,
            ..FrameTiming::default()
        };
        self.began = Some(now);
        now
    }

    /// `phase` took `took`.
    pub(crate) fn add(&mut self, phase: Phase, took: Duration) {
        let t = &mut self.turn;
        let slot = match phase {
            Phase::Update => &mut t.update,
            Phase::View => &mut t.view,
            Phase::Layout => &mut t.layout,
            Phase::Paint => &mut t.paint,
            Phase::Raster => &mut t.raster,
            Phase::Copy => &mut t.copy,
            Phase::Commit => &mut t.commit,
        };
        *slot += took;
    }

    /// A frame was presented.
    pub(crate) fn presented(&mut self) {
        self.turn.frames += 1;
    }

    /// The turn ended: the sink hears it when it presented a frame.
    pub(crate) fn end(&mut self) {
        let Some(began) = self.began.take() else {
            return;
        };
        if self.turn.frames == 0 {
            return;
        }
        self.turn.total = began.elapsed();
        (self.sink)(&self.turn);
    }
}

/// The time of `phase` from `since` to now, when timing (`since` is `None` otherwise).
pub(crate) fn lap(timings: &mut Option<FrameTimings>, phase: Phase, since: Option<Instant>) {
    if let (Some(t), Some(since)) = (timings.as_mut(), since) {
        t.add(phase, since.elapsed());
    }
}

/// A frame was handed to `system` since `since` (when timing): its copy into a buffer, as the window system timed it,
/// and the rest of the hand-over, the commit.
pub(crate) fn handed_over<W: cupertino_ui_platform::WindowSystem>(
    timings: &mut Option<FrameTimings>,
    system: &mut W,
    since: Option<Instant>,
) {
    let (Some(t), Some(since)) = (timings.as_mut(), since) else {
        return;
    };
    let took = since.elapsed();
    let copy = system.take_copy_time().min(took);
    t.add(Phase::Copy, copy);
    t.add(Phase::Commit, took - copy);
}

/// Now, when timing.
pub(crate) fn mark(timings: &Option<FrameTimings>) -> Option<Instant> {
    timings.as_ref().map(|_| Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn recorder() -> (FrameTimings, Rc<RefCell<Vec<FrameTiming>>>) {
        let heard = Rc::new(RefCell::new(Vec::new()));
        let h = Rc::clone(&heard);
        (
            FrameTimings::new(Box::new(move |t| h.borrow_mut().push(*t))),
            heard,
        )
    }

    #[test]
    fn a_turn_is_heard_only_when_it_presented_a_frame_with_each_phase_added_up() {
        let (mut t, heard) = recorder();
        t.begin(Timestamp(5));
        t.add(Phase::Layout, Duration::from_micros(30));
        t.end();
        assert!(heard.borrow().is_empty(), "nothing was presented");
        t.begin(Timestamp(9));
        t.add(Phase::Paint, Duration::from_micros(10));
        t.add(Phase::Paint, Duration::from_micros(5));
        t.add(Phase::Copy, Duration::from_micros(7));
        t.presented();
        t.presented();
        t.end();
        let heard = heard.borrow();
        assert_eq!(heard.len(), 1);
        assert_eq!(heard[0].start, Timestamp(9));
        assert_eq!(heard[0].paint, Duration::from_micros(15));
        assert_eq!(heard[0].copy, Duration::from_micros(7));
        assert_eq!(
            heard[0].layout,
            Duration::ZERO,
            "the turn before is forgotten"
        );
        assert_eq!(heard[0].frames, 2);
        assert!(heard[0].total >= Duration::ZERO);
    }

    #[test]
    fn without_timings_no_clock_is_read_and_nothing_is_kept() {
        let mut off: Option<FrameTimings> = None;
        assert_eq!(mark(&off), None);
        lap(&mut off, Phase::Raster, None);
        assert!(off.is_none());
    }
}
