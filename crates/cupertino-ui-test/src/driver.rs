use crate::{HeadlessWindowSystem, ManualClock, TimeWentBackwards, Trace, TraceError};
use cupertino_ui_core::{
    Application, Clock, Pending, Runtime, RuntimeOptions, StepStatus, UiError,
};
use cupertino_ui_platform::{
    Capabilities, ClipboardKind, ClipboardToken, PlatformEvent, Timestamp, WindowId, WindowSystem,
};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverError {
    Runtime(UiError),
    Trace(TraceError),
    Time(TimeWentBackwards),
    UnknownClipboardToken(ClipboardToken),
    UnknownWindow(WindowId),
}

/// Drives the production runtime at explicit times and records input before it is routed. Each platform event
/// gets its own runtime turn, so focus changes affect the next event even when inputs share a timestamp.
pub struct TestDriver<A: Application> {
    pub(crate) runtime: Runtime<A, HeadlessWindowSystem>,
    pub(crate) clock: ManualClock,
    trace: Trace,
    clipboard_reads: HashMap<ClipboardToken, bool>,
}

impl<A: Application> TestDriver<A> {
    /// Uses `clock` for both runtime work and trace timestamps, replacing the clock in `options`.
    pub fn new(
        capabilities: Capabilities,
        mut options: RuntimeOptions,
        clock: ManualClock,
    ) -> Self {
        options.clock = Arc::new(clock.clone());
        let mut system = HeadlessWindowSystem::new(capabilities);
        system.dispatch_one = true;
        Self {
            runtime: Runtime::new(system, options),
            clock,
            trace: Trace::default(),
            clipboard_reads: HashMap::new(),
        }
    }

    pub fn runtime(&self) -> &Runtime<A, HeadlessWindowSystem> {
        &self.runtime
    }

    pub fn window_system_mut(&mut self) -> &mut HeadlessWindowSystem {
        self.runtime.window_system_mut()
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn advance_to(&mut self, at: Timestamp) -> Result<(), DriverError> {
        self.clock.advance_to(at).map_err(DriverError::Time)
    }

    /// Starts a clipboard read on behalf of the window's current focus. Its protection is retained by token
    /// until delivery, even if the focus moves or the requesting window closes in the meantime.
    pub fn read_clipboard(
        &mut self,
        window: WindowId,
        kind: ClipboardKind,
        mime: &str,
    ) -> Result<ClipboardToken, DriverError> {
        let protected = self
            .runtime
            .protected_focus(window)
            .ok_or(DriverError::UnknownWindow(window))?;
        let token = self.runtime.window_system_mut().read_clipboard(kind, mime);
        self.clipboard_reads.insert(token, protected);
        Ok(token)
    }

    /// Drains ready work without advancing time, at most `max_turns` runtime turns. Waiting work (tasks,
    /// compositor frames, animations) remains explicit; queued input consumes the same turn budget.
    pub fn run_until_idle(&mut self, max_turns: NonZeroU32) -> Result<StepStatus, DriverError> {
        let one = NonZeroU32::new(1).unwrap();
        for turn in 0..max_turns.get() {
            if self.runtime.exited() {
                return Ok(StepStatus::Idle);
            }
            if let Some(event) = self.runtime.window_system().next_event().cloned() {
                let protected = match &event {
                    PlatformEvent::Window { window, .. } => {
                        self.runtime
                            .protected_focus(*window)
                            .ok_or(DriverError::UnknownWindow(*window))?
                    }
                    // a drop is read for a drop target, not for the focused field
                    PlatformEvent::Clipboard { token, .. }
                        if self.runtime.window_system().reads_drop(*token) =>
                    {
                        false
                    }
                    PlatformEvent::Clipboard { token, .. } => *self
                        .clipboard_reads
                        .get(token)
                        .ok_or(DriverError::UnknownClipboardToken(*token))?,
                    _ => false,
                };
                self.trace
                    .record(self.clock.now(), event.clone(), protected)
                    .map_err(DriverError::Trace)?;
                if let PlatformEvent::Clipboard { token, .. } = event {
                    self.clipboard_reads.remove(&token);
                }
            }
            let status = self.runtime.step(one).map_err(DriverError::Runtime)?;
            if self.runtime.window_system().queued() != 0 {
                if turn + 1 == max_turns.get() {
                    let pending = match status {
                        StepStatus::Idle => Pending::default(),
                        StepStatus::Waiting(p) | StepStatus::BudgetExhausted(p) => p,
                    };
                    return Ok(StepStatus::BudgetExhausted(pending));
                }
                continue;
            }
            if status == StepStatus::Waiting(Pending::default()) {
                return Ok(StepStatus::Idle);
            }
            if !matches!(status, StepStatus::BudgetExhausted(_)) {
                return Ok(status);
            }
            if turn + 1 == max_turns.get() {
                return Ok(status);
            }
        }
        unreachable!("a non-zero turn budget executes at least one turn")
    }
}

#[cfg(test)]
#[path = "driver-tests.rs"]
mod tests;
