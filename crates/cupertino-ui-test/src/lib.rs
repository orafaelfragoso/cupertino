//! Runs CupertinoKit applications without a display. A [`HeadlessWindowSystem`] stands in for the compositor, a
//! [`Trace`] records or replays the events a run received, and the [`Inspector`] dumps what the runtime built. Time
//! comes from a [`ManualClock`] and moves only when the test moves it.

mod clock;
mod headless;
mod inspector;
mod trace;

pub use clock::{ManualClock, TimeWentBackwards};
pub use headless::{HeadlessWindowSystem, PresentedFrame};
pub use inspector::Inspector;
pub use trace::{
    ProtectedKind, RecordedInput, SecureInputs, Trace, TraceError, TraceEvent, TRACE_VERSION,
};
