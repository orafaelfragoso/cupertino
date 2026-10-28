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
    pub fn record(
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

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_ui_platform::{ClipboardData, KeyEvent, Keysym, Mods};
    use cupertino_ui_text::ImeBatch;

    fn key(window: u32, text: char) -> PlatformEvent {
        PlatformEvent::Window {
            window: WindowId(window),
            event: WindowEvent::Key(
                KeyEvent {
                    code: 30,
                    keysym: Keysym(text as u32),
                    text: Some(text),
                    pressed: true,
                    repeat: false,
                    mods: Mods::default(),
                },
                Timestamp(0),
            ),
        }
    }

    fn paste(token: u64, text: &str) -> PlatformEvent {
        PlatformEvent::Clipboard {
            token: ClipboardToken(token),
            data: Some(ClipboardData {
                mime: "text/plain".into(),
                bytes: text.as_bytes().to_vec(),
            }),
        }
    }

    fn commit(window: u32, text: &str) -> PlatformEvent {
        PlatformEvent::Window {
            window: WindowId(window),
            event: WindowEvent::Ime(ImeEvent::Done(ImeBatch {
                commit: Some(text.into()),
                current: true,
                ..ImeBatch::default()
            })),
        }
    }

    #[test]
    fn plain_events_replay_in_order_with_their_times() {
        let mut trace = Trace::default();
        trace.record(Timestamp(10), key(1, 'a'), false).unwrap();
        trace
            .record(Timestamp(10), PlatformEvent::Wake, false)
            .unwrap();
        trace.record(Timestamp(25), key(1, 'b'), false).unwrap();
        assert_eq!(
            trace.replay(&SecureInputs::default()).unwrap(),
            vec![
                (Timestamp(10), key(1, 'a')),
                (Timestamp(10), PlatformEvent::Wake),
                (Timestamp(25), key(1, 'b')),
            ]
        );
    }

    #[test]
    fn recording_refuses_an_event_earlier_than_the_last() {
        let mut trace = Trace::default();
        trace
            .record(Timestamp(10), PlatformEvent::Wake, false)
            .unwrap();
        assert_eq!(
            trace.record(Timestamp(9), PlatformEvent::Wake, false),
            Err(TraceError::TimeWentBackwards { index: 1 })
        );
        assert_eq!(trace.events.len(), 1);
    }

    #[test]
    fn replay_rejects_an_unknown_version_and_a_backwards_trace() {
        let trace = Trace {
            version: TRACE_VERSION + 1,
            ..Trace::default()
        };
        assert_eq!(
            trace.replay(&SecureInputs::default()),
            Err(TraceError::UnsupportedVersion(TRACE_VERSION + 1))
        );
        let backwards = Trace {
            version: TRACE_VERSION,
            events: vec![
                TraceEvent {
                    at: Timestamp(5),
                    input: RecordedInput::Event(PlatformEvent::Wake),
                },
                TraceEvent {
                    at: Timestamp(4),
                    input: RecordedInput::Event(PlatformEvent::Wake),
                },
            ],
        };
        assert_eq!(
            backwards.replay(&SecureInputs::default()),
            Err(TraceError::TimeWentBackwards { index: 1 })
        );
    }

    #[test]
    fn input_to_a_protected_field_is_not_kept_and_needs_its_own_fixture() {
        let mut trace = Trace::default();
        trace.record(Timestamp(1), key(1, 'h'), true).unwrap();
        trace
            .record(Timestamp(2), commit(1, "hunter2"), true)
            .unwrap();
        trace
            .record(Timestamp(3), paste(7, "s3cret-paste"), true)
            .unwrap();
        trace
            .record(Timestamp(4), PlatformEvent::Wake, true)
            .unwrap();

        assert_eq!(
            trace.events[0].input,
            RecordedInput::Protected(ProtectedInput::Key(WindowId(1)))
        );
        assert_eq!(
            trace.events[1].input,
            RecordedInput::Protected(ProtectedInput::ImeBatch(WindowId(1)))
        );
        assert_eq!(
            trace.events[2].input,
            RecordedInput::Protected(ProtectedInput::Clipboard(ClipboardToken(7)))
        );
        assert_eq!(
            trace.events[3].input,
            RecordedInput::Event(PlatformEvent::Wake)
        );
        let dump = format!("{trace:?}");
        assert!(!dump.contains("hunter2"), "{dump}");
        assert!(!dump.contains("'h'"), "{dump}");
        assert!(!dump.contains("s3cret"), "{dump}");
        assert!(!dump.contains("115, 51"), "{dump}");

        assert_eq!(
            trace.replay(&SecureInputs::default()),
            Err(TraceError::MissingSecureInput { index: 0 })
        );

        let mut wrong_kind = SecureInputs::default();
        wrong_kind.insert(0, commit(1, "h"));
        assert_eq!(
            trace.replay(&wrong_kind),
            Err(TraceError::SecureInputMismatch { index: 0 })
        );
        let mut wrong_window = SecureInputs::default();
        wrong_window.insert(0, key(2, 'h'));
        assert_eq!(
            trace.replay(&wrong_window),
            Err(TraceError::SecureInputMismatch { index: 0 })
        );

        let mut wrong_token = SecureInputs::default();
        wrong_token.insert(0, key(1, 'h'));
        wrong_token.insert(1, commit(1, "hunter2"));
        wrong_token.insert(2, paste(8, "s3cret-paste"));
        assert_eq!(
            trace.replay(&wrong_token),
            Err(TraceError::SecureInputMismatch { index: 2 })
        );

        let mut secure = SecureInputs::default();
        secure.insert(0, key(1, 'h'));
        secure.insert(1, commit(1, "hunter2"));
        secure.insert(2, paste(7, "s3cret-paste"));
        assert_eq!(
            trace.replay(&secure).unwrap(),
            vec![
                (Timestamp(1), key(1, 'h')),
                (Timestamp(2), commit(1, "hunter2")),
                (Timestamp(3), paste(7, "s3cret-paste")),
                (Timestamp(4), PlatformEvent::Wake),
            ]
        );
    }

    #[test]
    fn a_clipboard_read_with_no_data_is_kept_under_protected_focus() {
        let miss = PlatformEvent::Clipboard {
            token: ClipboardToken(3),
            data: None,
        };
        let mut trace = Trace::default();
        trace.record(Timestamp(1), miss.clone(), true).unwrap();
        assert_eq!(trace.events[0].input, RecordedInput::Event(miss));
    }
}
