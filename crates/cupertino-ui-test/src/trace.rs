use cupertino_ui_platform::{ClipboardToken, PlatformEvent, Timestamp, WindowEvent, WindowId};
use cupertino_ui_text::ImeEvent;
use std::collections::HashMap;

pub const TRACE_VERSION: u32 = 1;

/// The kind and address of input that went to a protected field. A pasted value arrives as a clipboard read, which
/// belongs to no window, so it is addressed by its token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectedInput {
    Key(WindowId),
    ImeBatch(WindowId),
    Clipboard(ClipboardToken),
}

/// What a trace keeps of one input. Input that went to a protected field is never kept: only the window and the kind
/// of input remain, and replay needs the real event from the test's own [`SecureInputs`].
#[derive(Clone, Debug, PartialEq)]
pub enum RecordedInput {
    Event(PlatformEvent),
    Protected(ProtectedInput),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TraceEvent {
    pub at: Timestamp,
    pub input: RecordedInput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceError {
    UnsupportedVersion(u32),
    /// The event at `index` is earlier than the one before it.
    TimeWentBackwards {
        index: usize,
    },
    /// The event at `index` was sent to a protected field and `SecureInputs` has none for it.
    MissingSecureInput {
        index: usize,
    },
    /// The supplied event for `index` is not the window and kind of input the trace recorded.
    SecureInputMismatch {
        index: usize,
    },
}

/// The events of one run, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    pub version: u32,
    pub events: Vec<TraceEvent>,
}

impl Default for Trace {
    fn default() -> Self {
        Self {
            version: TRACE_VERSION,
            events: Vec::new(),
        }
    }
}

/// The real events behind a trace's protected entries, by position in the trace. They live in the test, never in the
/// trace.
#[derive(Clone, Debug, Default)]
pub struct SecureInputs {
    events: HashMap<usize, PlatformEvent>,
}

impl SecureInputs {
    pub fn insert(&mut self, index: usize, event: PlatformEvent) {
        self.events.insert(index, event);
    }
}

fn protected_input(event: &PlatformEvent) -> Option<ProtectedInput> {
    match event {
        PlatformEvent::Window {
            window,
            event: WindowEvent::Key(..),
        } => Some(ProtectedInput::Key(*window)),
        PlatformEvent::Window {
            window,
            event: WindowEvent::Ime(ImeEvent::Done(_)),
        } => Some(ProtectedInput::ImeBatch(*window)),
        PlatformEvent::Clipboard {
            token,
            data: Some(_),
        } => Some(ProtectedInput::Clipboard(*token)),
        _ => None,
    }
}

impl Trace {
    /// Appends an event. With `protected_focus`, keys, input-method batches and clipboard data
    /// (a paste) are recorded as [`RecordedInput::Protected`] without their content.
    pub(crate) fn record(
        &mut self,
        at: Timestamp,
        event: PlatformEvent,
        protected_focus: bool,
    ) -> Result<(), TraceError> {
        if self.events.last().is_some_and(|last| at < last.at) {
            return Err(TraceError::TimeWentBackwards {
                index: self.events.len(),
            });
        }
        let input = match protected_input(&event) {
            Some(protected) if protected_focus => RecordedInput::Protected(protected),
            _ => RecordedInput::Event(event),
        };
        self.events.push(TraceEvent { at, input });
        Ok(())
    }

    /// The events to feed a window system, with their times. Fails before returning anything when the trace cannot
    /// be replayed whole.
    pub fn replay(
        &self,
        secure: &SecureInputs,
    ) -> Result<Vec<(Timestamp, PlatformEvent)>, TraceError> {
        if self.version != TRACE_VERSION {
            return Err(TraceError::UnsupportedVersion(self.version));
        }
        let mut out = Vec::with_capacity(self.events.len());
        let mut previous = Timestamp(0);
        for (index, recorded) in self.events.iter().enumerate() {
            if recorded.at < previous {
                return Err(TraceError::TimeWentBackwards { index });
            }
            previous = recorded.at;
            let event = match &recorded.input {
                RecordedInput::Event(event) => event.clone(),
                RecordedInput::Protected(protected) => {
                    let event = secure
                        .events
                        .get(&index)
                        .ok_or(TraceError::MissingSecureInput { index })?;
                    if protected_input(event) != Some(*protected) {
                        return Err(TraceError::SecureInputMismatch { index });
                    }
                    event.clone()
                }
            };
            out.push((recorded.at, event));
        }
        Ok(out)
    }
}
