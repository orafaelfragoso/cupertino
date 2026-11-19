//! Active turns of the compositor, from a poll's return to the next poll. A dispatch includes the
//! time spent asleep, so measuring around `EventLoop::dispatch` would report an idle desktop as stuck.

use std::convert::Infallible;
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::profile;
use smithay::reexports::calloop::{
    self, EventIterator, EventSource, LoopHandle, Poll, PostAction, Readiness, RegistrationToken,
    Token, TokenFactory,
};
use tracing::span::EnteredSpan;

/// How long a turn may be busy before the timeline marks it as a stall.
const STALL: Duration = Duration::from_millis(4);

struct Turn(Option<(Instant, EnteredSpan)>);

impl Turn {
    fn begin(&mut self) {
        self.0 = Some((
            Instant::now(),
            tracing::trace_span!("server.turn", stall = tracing::field::Empty).entered(),
        ));
    }

    fn finish(&mut self) {
        if let Some((start, span)) = self.0.take() {
            if start.elapsed() > STALL {
                span.record("stall", true);
            }
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.finish();
    }
}

// This source observes the poll boundaries; it has no descriptor, timer or events of its own.
impl EventSource for Turn {
    type Event = Infallible;
    type Metadata = ();
    type Ret = ();
    type Error = Infallible;

    const NEEDS_EXTRA_LIFECYCLE_EVENTS: bool = true;

    fn before_sleep(&mut self) -> calloop::Result<Option<(Readiness, Token)>> {
        self.finish();
        Ok(None)
    }

    fn before_handle_events(&mut self, _: EventIterator<'_>) {
        self.begin();
    }

    fn process_events<F>(&mut self, _: Readiness, _: Token, _: F) -> Result<PostAction, Infallible>
    where
        F: FnMut(Infallible, &mut ()),
    {
        Ok(PostAction::Continue)
    }

    fn register(&mut self, _: &mut Poll, _: &mut TokenFactory) -> calloop::Result<()> {
        Ok(())
    }

    fn reregister(&mut self, _: &mut Poll, _: &mut TokenFactory) -> calloop::Result<()> {
        Ok(())
    }

    fn unregister(&mut self, _: &mut Poll) -> calloop::Result<()> {
        Ok(())
    }
}

/// Records active compositor turns while held; removes the loop source on drop.
pub struct Recorder<D: 'static> {
    handle: LoopHandle<'static, D>,
    token: RegistrationToken,
}

impl<D: 'static> Recorder<D> {
    /// Record only the running loop, including the work before its first wait and after its last.
    pub fn start(handle: LoopHandle<'static, D>) -> Option<Self> {
        if !std::env::var_os(profile::ENV).is_some_and(|v| !v.is_empty()) {
            return None;
        }
        let mut turn = Turn(None);
        turn.begin();
        let token = handle
            .insert_source(turn, |never, _, _| match never {})
            .map_err(|e| tracing::warn!(error = %e, "cannot record active compositor turns"))
            .ok()?;
        Some(Self { handle, token })
    }
}

impl<D: 'static> Drop for Recorder<D> {
    fn drop(&mut self) {
        self.handle.remove(self.token);
    }
}
