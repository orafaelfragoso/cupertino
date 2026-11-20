//! What the host reports of a component's loop that has been out of its wait (D17,
//! `shell-host.md#hangs`). The host reads each loop's record every `hosting::LIVENESS_CHECK_INTERVAL`; the times
//! are `hosting`'s constants, named once there.

use std::time::Duration;

use cupertino_kit_shell::hosting::{
    Liveness, BUSY_THRESHOLD, LOST_THRESHOLD, STARTUP_GRACE, UNRESPONSIVE_THRESHOLD,
};

/// How a component whose loop has been out of its wait for `busy` stands, `running` after its start. Within
/// [`STARTUP_GRACE`] of its start a component is responsive whatever its loop does, so the work of its start is never
/// reported.
pub fn liveness(busy: Option<Duration>, running: Duration) -> Liveness {
    let busy = busy.unwrap_or_default();
    if running < STARTUP_GRACE {
        Liveness::Responsive
    } else if busy >= LOST_THRESHOLD {
        Liveness::Lost
    } else if busy >= UNRESPONSIVE_THRESHOLD {
        Liveness::Unresponsive
    } else if busy >= BUSY_THRESHOLD {
        Liveness::Busy
    } else {
        Liveness::Responsive
    }
}

/// Whether the process's render thread, out of its wait for `busy`, hangs the whole host: every component draws
/// through it, so the host exits with failure and the server starts it again.
pub fn render_hung(busy: Option<Duration>, running: Duration) -> bool {
    running >= STARTUP_GRACE && busy.is_some_and(|busy| busy >= LOST_THRESHOLD)
}
