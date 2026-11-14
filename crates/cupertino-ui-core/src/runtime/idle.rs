//! A window's idle time handed to the widgets that ask for it ([`Widget::idle`](crate::Widget::idle)): the time
//! between a presented frame and the next, when nothing is ready and the run loop would wait on the window system.
//! It goes in slices of at most [`IDLE_SLICE_NS`], each ending [`FRAME_MARGIN_NS`] before a window's next frame is
//! due by its frame clock (the shortest period its last frames came at) and before any wake asked for; between slices
//! the window system is polled, and the first event that arrives ends the idle time. A node is called once for each
//! time it asked. With no request left the loop waits as it did before, so idle work adds no wakeup once it is done.

use super::{Runtime, WindowState};
use crate::cx::IdleCx;
use crate::{Application, UiError};
use cupertino_ui_platform::{Timestamp, WindowSystem};
use std::collections::BTreeSet;
use std::time::Duration;

/// The longest slice of idle time, in nanoseconds: an event that arrives while one runs waits at most this long and
/// one piece of a widget's work.
pub(super) const IDLE_SLICE_NS: u64 = 1_000_000;
/// How long before a window's next frame is due its idle time ends, in nanoseconds.
pub(super) const FRAME_MARGIN_NS: u64 = 2_000_000;
/// The shortest time between two frames taken for the display's period, in nanoseconds: frames closer together came
/// for something other than the display's refresh.
const MIN_PERIOD_NS: u64 = 2_000_000;
/// The longest, in nanoseconds: frames further apart came after a pause.
const MAX_PERIOD_NS: u64 = 100_000_000;
/// How many of the last periods between a window's frames its frame clock keeps; it takes the shortest.
const PERIODS: usize = 8;

/// When a window's frames came: the last, and the periods between the last few.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FrameClock {
    last: Option<Timestamp>,
    /// Zero where none was seen yet.
    periods: [u64; PERIODS],
    next: usize,
}

impl FrameClock {
    /// The compositor was ready for a frame at `at`.
    pub(super) fn frame(&mut self, at: Timestamp) {
        if let Some(last) = self.last {
            let period = at.0.saturating_sub(last.0);
            if (MIN_PERIOD_NS..=MAX_PERIOD_NS).contains(&period) {
                self.periods[self.next] = period;
                self.next = (self.next + 1) % PERIODS;
            }
        }
        self.last = Some(at);
    }

    /// The display's period: the shortest of the last periods between frames, once one is known.
    pub(super) fn period(&self) -> Option<u64> {
        self.periods.iter().copied().filter(|&p| p > 0).min()
    }

    /// When the next frame is due, seen at `now`: a period after the last one, until it is a whole period late (a
    /// compositor holds back the frames of a window it does not show).
    pub(super) fn due(&self, now: Timestamp) -> Option<Timestamp> {
        let due = self.last?.0.saturating_add(self.period()?);
        (now.0 < self.late(due)).then_some(Timestamp(due))
    }

    /// When a frame due at `due` is a whole period late.
    fn late(&self, due: u64) -> u64 {
        due.saturating_add(self.period().unwrap_or(0))
    }

    /// When the next frame counts as held back: a whole period after it was due.
    pub(super) fn held_back(&self) -> Option<Timestamp> {
        let due = self.last?.0.saturating_add(self.period()?);
        Some(Timestamp(self.late(due)))
    }
}

/// Whether the window waits for the frame its last commit asked for.
fn waits_for_frame<M>(w: &WindowState<M>) -> bool {
    w.committed && !w.frame_ready
}

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    /// Whether a node asked for idle time and has not been called since.
    pub fn has_idle_work(&self) -> bool {
        self.windows.iter().any(|w| !w.idle.is_empty())
    }

    /// Hands the windows' idle time to the nodes that asked for it, in slices, while nothing is ready: it returns once
    /// every node was called and none asked again, an event arrived or something else is ready (a wake that came due,
    /// a message, a frame to draw), or a window's next frame is about to be due. [`Runtime::run`] calls it before it
    /// waits on the window system; it never runs inside a frame and does not move time.
    pub fn run_idle(&mut self) -> Result<(), UiError> {
        while !self.exit && self.has_idle_work() {
            // what arrived since the last look goes first
            self.system
                .dispatch(Some(Duration::ZERO), &mut self.events)
                .map_err(UiError::Platform)?;
            if self.ready() {
                break;
            }
            let Some(until) = self.idle_until(self.options.clock.now()) else {
                break;
            };
            self.idle_slice(until);
        }
        Ok(())
    }

    /// When a slice started at `now` ends: a slice's length on, before the next wake and before any window's next
    /// frame is due; `None` when that leaves no time.
    pub(super) fn idle_until(&self, now: Timestamp) -> Option<Timestamp> {
        let mut until = now.0.saturating_add(IDLE_SLICE_NS);
        if let Some(wake) = self.next_wake() {
            until = until.min(wake.0);
        }
        for w in self.windows.iter().filter(|w| waits_for_frame(w)) {
            if let Some(due) = w.frame_clock.due(now) {
                until = until.min(due.0.saturating_sub(FRAME_MARGIN_NS));
            }
        }
        (until > now.0).then_some(Timestamp(until))
    }

    /// When the loop, waiting with idle work left because a frame was due, gives up on the frame and goes on with the
    /// work: once the frame is a whole period late. `None` with no idle work.
    pub(super) fn idle_resume(&self) -> Option<Timestamp> {
        if !self.has_idle_work() {
            return None;
        }
        self.windows
            .iter()
            .filter(|w| waits_for_frame(w))
            .filter_map(|w| w.frame_clock.held_back())
            .min()
    }

    /// One slice: each node that asked is called once, in the order of the windows and of the nodes' ids, until the
    /// slice ends; those not reached and those that asked again wait for the next.
    fn idle_slice(&mut self, until: Timestamp) {
        let clock = &*self.options.clock;
        for w in &mut self.windows {
            if w.idle.is_empty() {
                continue;
            }
            let mut left = BTreeSet::new();
            for id in std::mem::take(&mut w.idle) {
                if clock.now() >= until {
                    left.insert(id);
                    continue;
                }
                // a node that went away asks for nothing
                let Some(node) = self.tree.get(id) else {
                    continue;
                };
                let mut cx = IdleCx {
                    node: id,
                    env: &w.env,
                    clock,
                    until,
                    again: false,
                };
                node.widget.idle(&*node.state, &mut cx);
                if cx.again {
                    left.insert(id);
                }
            }
            w.idle.extend(left);
        }
    }
}
