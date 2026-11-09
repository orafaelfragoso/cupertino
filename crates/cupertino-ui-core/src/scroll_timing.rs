//! The time a scroll takes to reach the screen, opt-in: [`Runtime::with_scroll_timing`](crate::Runtime) gives the
//! runtime a sink, and each pointer scroll event is followed from the window system's timestamp through its handlers,
//! the model update and view it caused, the frame that drew it and the compositor showing that frame. Without a sink
//! the runtime keeps nothing and does nothing more than look whether it has one.

use cupertino_ui_platform::{Timestamp, WindowId};
use cupertino_ui_render::RenderFrameId;

/// One scroll event on its way to the screen. `input` is the window system's timestamp of the event and `presented`
/// the compositor's time of the frame; the others are read on the runtime's clock. On the desktop all of them are
/// `CLOCK_MONOTONIC`, so they compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollTiming {
    pub window: WindowId,
    pub input: Timestamp,
    /// The event's handlers ran.
    pub handled: Timestamp,
    /// The model took what they sent and the views were built again; `None` when the event changed no model.
    pub updated: Option<Timestamp>,
    /// The first frame drawn after it went to the window system.
    pub submitted: Timestamp,
    /// The compositor showed that frame; `None` when it never did (a later frame replaced it first).
    pub presented: Option<Timestamp>,
}

/// The stages' durations in nanoseconds, each from the input: handled, updated, submitted and presented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollLatency {
    pub handled: u64,
    pub updated: Option<u64>,
    pub submitted: u64,
    pub presented: Option<u64>,
}

impl ScrollTiming {
    pub fn latency(&self) -> ScrollLatency {
        let since = |t: Timestamp| t.0.saturating_sub(self.input.0);
        ScrollLatency {
            handled: since(self.handled),
            updated: self.updated.map(since),
            submitted: since(self.submitted),
            presented: self.presented.map(since),
        }
    }
}

/// Hears every [`ScrollTiming`] once its frame was shown or dropped.
pub type ScrollTimingSink = Box<dyn FnMut(ScrollTiming)>;

/// A scroll event not yet in a frame.
struct Pending {
    window: WindowId,
    input: Timestamp,
    handled: Timestamp,
    updated: Option<Timestamp>,
}

/// The events in flight: handled and waiting for a frame, then submitted and waiting for the compositor.
pub(crate) struct ScrollTimings {
    sink: ScrollTimingSink,
    pending: Vec<Pending>,
    submitted: Vec<(RenderFrameId, ScrollTiming)>,
}

/// At most this many events wait at each stage; a window that never draws or is never shown does not grow them.
const MAX_IN_FLIGHT: usize = 256;

impl ScrollTimings {
    pub(crate) fn new(sink: ScrollTimingSink) -> ScrollTimings {
        ScrollTimings {
            sink,
            pending: Vec::new(),
            submitted: Vec::new(),
        }
    }

    /// A scroll event of `window` stamped `input` was handled at `now`.
    pub(crate) fn handled(&mut self, window: WindowId, input: Timestamp, now: Timestamp) {
        if self.pending.len() == MAX_IN_FLIGHT {
            self.pending.remove(0);
        }
        self.pending.push(Pending {
            window,
            input,
            handled: now,
            updated: None,
        });
    }

    /// The model was updated and the views built again at `now`.
    pub(crate) fn updated(&mut self, now: Timestamp) {
        for p in self.pending.iter_mut().filter(|p| p.updated.is_none()) {
            p.updated = Some(now);
        }
    }

    /// `window` handed `frame` to the window system at `now`: it carries the window's pending events.
    pub(crate) fn submitted(&mut self, window: WindowId, frame: RenderFrameId, now: Timestamp) {
        let (mine, rest): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| p.window == window);
        self.pending = rest;
        for p in mine {
            if self.submitted.len() == MAX_IN_FLIGHT {
                self.submitted.remove(0);
            }
            self.submitted.push((
                frame,
                ScrollTiming {
                    window,
                    input: p.input,
                    handled: p.handled,
                    updated: p.updated,
                    submitted: now,
                    presented: None,
                },
            ));
        }
    }

    /// The compositor showed `frame` of `window` at `time`, or never will (`None`).
    pub(crate) fn presented(
        &mut self,
        window: WindowId,
        frame: RenderFrameId,
        time: Option<Timestamp>,
    ) {
        let (done, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.submitted)
            .into_iter()
            .partition(|(f, t)| *f == frame && t.window == window);
        self.submitted = rest;
        for (_, timing) in done {
            (self.sink)(ScrollTiming {
                presented: time,
                ..timing
            });
        }
    }
}
