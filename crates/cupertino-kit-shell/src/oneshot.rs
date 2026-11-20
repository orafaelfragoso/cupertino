//! A value handed from one thread to the future that waits for it, waking that future: the result of a helper thread,
//! a captured frame. The future is driven by whatever polls it (the component's loop), never by sleeping.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

/// One value, given once; [`Oneshot::wait`] resolves with it.
pub(crate) struct Oneshot<T> {
    state: Mutex<(Option<T>, Option<Waker>)>,
}

impl<T> Default for Oneshot<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new((None, None)),
        }
    }
}

impl<T> Oneshot<T> {
    /// Gives `value` unless one was given already, waking the future that waits for it; false when one was.
    pub(crate) fn fill(&self, value: T) -> bool {
        let waker = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.0.is_some() {
                return false;
            }
            state.0 = Some(value);
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        true
    }

    /// The value, once given.
    pub(crate) fn wait(&self) -> Wait<'_, T> {
        Wait { slot: self }
    }
}

/// The future of [`Oneshot::wait`].
pub(crate) struct Wait<'a, T> {
    slot: &'a Oneshot<T>,
}

impl<T> Future for Wait<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut state = self
            .slot
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match state.0.take() {
            Some(value) => Poll::Ready(value),
            None => {
                state.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}
