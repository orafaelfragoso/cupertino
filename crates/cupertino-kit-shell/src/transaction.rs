//! Coordinated shell transitions across components (`cupertino_transaction_manager_v1`,
//! `docs/architecture/shell-host.md#transactions`).

use std::time::Duration;

use cupertino_kit::core::WindowKey;
use cupertino_kit::motion::SpringPreset;
use cupertino_kit::platform::Timestamp;
use cupertino_protocols::transaction::{MAX_DEADLINE_MS, SPRING_DAMPING, SPRING_RESPONSE};
use serde::{Deserialize, Serialize};

use crate::context::{self, ShellRequest};
use crate::outputs::OutputId;
use crate::ComponentName;

/// Maximum explicit transaction deadline accepted by the server (`cupertino_protocols::transaction::MAX_DEADLINE_MS`).
pub const MAX_DEADLINE: Duration = Duration::from_millis(MAX_DEADLINE_MS as u64);

/// A transaction named by its creator, the server's number for that instance, and the serial chosen by the creator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Id {
    pub component: ComponentName,
    pub instance: u32,
    pub serial: u32,
}

/// Touchpad gesture that may drive a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gesture {
    SwipeUp,
    SwipeDown,
    Pinch,
    Spread,
}

/// Final outcome of a gesture or transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    Completed,
    Reverted,
    Cancelled,
}

/// Transaction lifecycle events delivered as [`crate::Event::Transaction`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The part was applied; every animation of the transaction counts from `at`.
    Started {
        id: Id,
        at: Timestamp,
    },
    GestureBegan {
        id: Id,
        gesture: Gesture,
    },
    GestureEnded {
        id: Id,
        outcome: Outcome,
    },
    Finished {
        id: Id,
        outcome: Outcome,
    },
}

/// A transaction part being assembled. Dropping without calling [`Transaction::submit`] withdraws it.
#[derive(Debug)]
pub struct Transaction {
    id: Id,
    is_creator: bool,
    submitted: bool,
}

impl Transaction {
    /// Transaction identity (`(component, instance, serial)`).
    pub fn id(&self) -> Id {
        self.id
    }

    /// Wait for `component`'s part until the deadline (creator only).
    pub fn expect(&mut self, component: ComponentName) {
        if !self.is_creator {
            tracing::warn!("transaction::expect called on a non-creator part; ignored");
            return;
        }
        context::push_request(ShellRequest::TxExpect {
            id: self.id,
            component,
        });
    }

    /// Set the creator's deadline, clamped to [`MAX_DEADLINE`].
    pub fn deadline(&mut self, deadline: Duration) {
        if !self.is_creator {
            tracing::warn!("transaction::deadline called on a non-creator part; ignored");
            return;
        }
        context::push_request(ShellRequest::TxDeadline {
            id: self.id,
            deadline: deadline.min(MAX_DEADLINE),
        });
    }

    /// Stage `window`'s next commit (and its layer/presentation commands) in this transaction part.
    pub fn stage(&mut self, window: &WindowKey) {
        context::push_request(ShellRequest::TxStage {
            id: self.id,
            window: window.clone(),
        });
    }

    /// Bracket the window-change requests made by `changes` so they apply at transaction start.
    pub fn window_changes(&mut self, changes: impl FnOnce()) {
        context::push_request(ShellRequest::TxBeginWindowChanges { id: self.id });
        changes();
        context::push_request(ShellRequest::TxEndWindowChanges { id: self.id });
    }

    /// Arm the transaction for `gesture`, settling along `settle` (creator only).
    pub fn bind(&mut self, gesture: Gesture, settle: SpringPreset) {
        if !self.is_creator {
            tracing::warn!("transaction::bind called on a non-creator part; ignored");
            return;
        }
        let p = settle.params();
        let response = p
            .perceptual_duration()
            .clamp(SPRING_RESPONSE.0, SPRING_RESPONSE.1);
        let damping = (p.damping / (2.0 * (p.stiffness * p.mass).sqrt()))
            .clamp(SPRING_DAMPING.0, SPRING_DAMPING.1);
        context::push_request(ShellRequest::TxBindGesture {
            id: self.id,
            gesture,
            response,
            damping,
        });
    }

    /// Bind a reverse gesture to return a completed gesture-bound transaction back to 0 (creator only).
    pub fn bind_reverse(&mut self, gesture: Gesture) {
        if !self.is_creator {
            tracing::warn!("transaction::bind_reverse called on a non-creator part; ignored");
            return;
        }
        context::push_request(ShellRequest::TxBindReverseGesture {
            id: self.id,
            gesture,
        });
    }

    /// Submit this part and return a [`Handle`] to track or cancel it.
    pub fn submit(mut self) -> Handle {
        self.submitted = true;
        context::push_request(ShellRequest::TxSubmit { id: self.id });
        Handle {
            id: self.id,
            cancelled: false,
        }
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !self.submitted {
            context::push_request(ShellRequest::TxDestroy { id: self.id });
        }
    }
}

/// A submitted transaction part. Dropping lets go of events and releases a creator's gesture bindings.
#[derive(Debug)]
pub struct Handle {
    id: Id,
    cancelled: bool,
}

impl Handle {
    pub fn id(&self) -> Id {
        self.id
    }

    /// Cancel the transaction (creator cancels for all participants; non-creator withdraws only itself).
    pub fn cancel(mut self) {
        self.cancelled = true;
        context::push_request(ShellRequest::TxCancel { id: self.id });
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.cancelled {
            context::push_request(ShellRequest::TxDestroy { id: self.id });
        }
    }
}

/// Create a transaction on `output`'s clock for the calling thread's component.
pub fn create(output: OutputId) -> Transaction {
    let id = context::with_ctx_mut(|ctx| {
        let component = ctx.component.unwrap_or(ComponentName::OVERVIEW);
        let instance = ctx.instance;
        let serial = ctx.next_tx_serial;
        ctx.next_tx_serial = ctx.next_tx_serial.wrapping_add(1).max(1);
        let id = Id {
            component,
            instance,
            serial,
        };
        ctx.outgoing
            .push_back(ShellRequest::TxCreate { id, output });
        id
    })
    .unwrap_or(Id {
        component: ComponentName::OVERVIEW,
        instance: 0,
        serial: 0,
    });
    Transaction {
        id,
        is_creator: true,
        submitted: false,
    }
}

/// Join a transaction created by another component on `output`'s clock.
pub fn join(id: Id, output: OutputId) -> Transaction {
    context::push_request(ShellRequest::TxJoin { id, output });
    Transaction {
        id,
        is_creator: false,
        submitted: false,
    }
}
