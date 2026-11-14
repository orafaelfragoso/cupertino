//! Runs CupertinoKit applications without a display. A [`HeadlessWindowSystem`] stands in for the compositor, a
//! [`Trace`] records or replays the events a run received, and the [`Inspector`] dumps what the runtime built. Time
//! comes from a [`ManualClock`] and moves only when the test moves it.

mod assistive;
mod audit;
mod clock;
mod driver;
mod headless;
mod inspector;
mod look;
pub mod repaint;
mod semantics;
mod trace;

pub use assistive::{Announcement, AssistiveTechnology};
pub use audit::{accessibility_audit, reading_order_breaks, secret_leaks};
pub use clock::{ManualClock, TimeWentBackwards};
pub use driver::{DriverError, TestDriver};
pub use headless::{
    HeadlessWindowSystem, IncomingDrag, PresentedFrame, PresentedLayer, PresentedSurfaceLayer,
    SolidPresent, StartedDrag,
};
pub use inspector::Inspector;
pub use look::{appearance_name, LookError, LOOK_APPEARANCES, LOOK_DIR, LOOK_SCALE};
pub use semantics::{describe, semantic_gaps, semantic_outline, unannounced, whole_tree};
pub use trace::{
    ProtectedInput, RecordedInput, SecureInputs, Trace, TraceError, TraceEvent, TRACE_VERSION,
};
