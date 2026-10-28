//! Runs CupertinoKit applications without a display. A [`HeadlessWindowSystem`] stands in for the compositor, a
//! [`Trace`] records or replays the events a run received, and the [`Inspector`] dumps what the runtime built.

use cupertino_ui_platform::{PlatformEvent, Timestamp};

/// A window system with no display: windows exist in memory, presented frames are kept for inspection, and time
/// moves only when the test advances it.
#[derive(Debug, Default)]
pub struct HeadlessWindowSystem {
    _private: (),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TraceEvent {
    pub at: Timestamp,
    pub event: PlatformEvent,
}

/// The events of one run, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trace {
    pub events: Vec<TraceEvent>,
}

/// Deterministic text dumps of a runtime's layout, accessibility tree and display lists, for golden tests.
#[derive(Debug, Default)]
pub struct Inspector {
    _private: (),
}
