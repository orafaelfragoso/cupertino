//! The pairing state machine for one device, with time passed in.
//!
//! A pairing starts when this side sends Pair Device (`local`) or when the remote side starts one
//! and the kernel asks us to confirm it. The kernel then asks, through events, for a yes or no
//! (`Confirming`), for a passkey the user reads off the remote device (`Passkey`), or tells us a
//! passkey to show (`Displaying`). The user's answer comes back from the agent. The pairing ends
//! with the Pair Device reply (local), the new key (remote), an authentication failure, or the
//! deadline.
//!
//! The machine returns [`Action`]s and does no I/O; the model turns them into commands and calls
//! to the agent.

use std::time::Duration;

use cupertino_bluetooth::{Command, DeviceId, Status};

/// How long a pairing may take before it is cancelled. approximate: the user has to read a code
/// off one screen and confirm it on another; the specification bounds only the link-layer steps
/// (the pairing procedure timeout of the Security Manager is 30 s), not the person.
pub const PAIRING_TIMEOUT: Duration = Duration::from_secs(60);

/// Names one pairing, so that an answer that arrives after the pairing ended is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PairingId(pub u64);

/// Why a pairing did not succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairError {
    /// The user, through the agent, refused.
    Rejected,
    /// The remote device or the codes did not check out.
    AuthenticationFailed(Status),
    /// The deadline passed.
    Timeout,
    /// Something cancelled it (the controller powered off, the adapter went away).
    Cancelled,
    /// The agent could not be asked, or failed.
    Agent(String),
    /// The remote device wants a legacy PIN, which this daemon does not support.
    PinUnsupported,
    /// The controller reported another failure.
    Controller(Status),
}

/// What the agent is asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// Confirm. With `just_works` a plain yes or no; otherwise show `passkey` so the user can
    /// compare it with the other device (numeric comparison).
    Confirm { passkey: u32, just_works: bool },
    /// Ask for the passkey the remote device displays.
    RequestPasskey,
    /// Show a passkey the user types on the remote device; `entered` digits were typed so far.
    DisplayPasskey { passkey: u32, entered: u8 },
    /// The pairing is over; close any dialog.
    Cancel,
}

/// An answer from the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentReply {
    Confirm(bool),
    /// `None` is a refusal.
    Passkey(Option<u32>),
    /// The agent could not answer (no agent, the call failed).
    Failed(String),
}

/// What the machine wants done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Send(Command),
    Prompt(Prompt),
    /// The pairing is over; the machine does nothing more.
    Finish(Result<(), PairError>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Waiting for the kernel.
    Waiting,
    Confirming,
    Passkey,
    Displaying,
}

#[derive(Clone, Debug)]
pub struct Pairing {
    pub id: PairingId,
    pub device: DeviceId,
    /// True when this side sent Pair Device.
    pub local: bool,
    phase: Phase,
    deadline: Duration,
    /// The user refused.
    rejected: bool,
    /// The first failure seen before the pairing ended.
    failure: Option<PairError>,
}

impl Pairing {
    pub fn new(id: PairingId, device: DeviceId, local: bool, now: Duration) -> Self {
        Self {
            id,
            device,
            local,
            phase: Phase::Waiting,
            deadline: now + PAIRING_TIMEOUT,
            rejected: false,
            failure: None,
        }
    }

    pub fn deadline(&self) -> Duration {
        self.deadline
    }

    /// User Confirmation Request.
    pub fn on_confirm_request(&mut self, just_works: bool, value: u32) -> Vec<Action> {
        self.phase = Phase::Confirming;
        vec![Action::Prompt(Prompt::Confirm {
            passkey: value,
            just_works,
        })]
    }

    /// User Passkey Request.
    pub fn on_passkey_request(&mut self) -> Vec<Action> {
        self.phase = Phase::Passkey;
        vec![Action::Prompt(Prompt::RequestPasskey)]
    }

    /// Passkey Notify.
    pub fn on_passkey_notify(&mut self, passkey: u32, entered: u8) -> Vec<Action> {
        self.phase = Phase::Displaying;
        vec![Action::Prompt(Prompt::DisplayPasskey { passkey, entered })]
    }

    /// PIN Code Request: refused, legacy PIN pairing is not supported.
    pub fn on_pin_request(&mut self) -> Vec<Action> {
        self.failure.get_or_insert(PairError::PinUnsupported);
        let mut actions = vec![Action::Send(Command::PinCodeNegativeReply(self.device))];
        if !self.local {
            actions.push(Action::Finish(Err(PairError::PinUnsupported)));
        }
        actions
    }

    /// The agent's answer. An answer to a question that is no longer open does nothing.
    pub fn on_agent_reply(&mut self, reply: AgentReply) -> Vec<Action> {
        let device = self.device;
        match (self.phase, reply) {
            (Phase::Confirming, AgentReply::Confirm(accept)) => {
                self.phase = Phase::Waiting;
                self.rejected |= !accept;
                vec![Action::Send(Command::UserConfirmationReply {
                    device,
                    accept,
                })]
            }
            (Phase::Passkey, AgentReply::Passkey(passkey)) => {
                self.phase = Phase::Waiting;
                self.rejected |= passkey.is_none();
                vec![Action::Send(Command::UserPasskeyReply { device, passkey })]
            }
            (Phase::Confirming, AgentReply::Failed(message)) => {
                self.phase = Phase::Waiting;
                self.failure.get_or_insert(PairError::Agent(message));
                vec![Action::Send(Command::UserConfirmationReply {
                    device,
                    accept: false,
                })]
            }
            (Phase::Passkey, AgentReply::Failed(message)) => {
                self.phase = Phase::Waiting;
                self.failure.get_or_insert(PairError::Agent(message));
                vec![Action::Send(Command::UserPasskeyReply {
                    device,
                    passkey: None,
                })]
            }
            _ => Vec::new(),
        }
    }

    /// Authentication Failed. A pairing the remote side started ends here; for ours the Pair
    /// Device reply follows.
    pub fn on_authentication_failed(&mut self, status: Status) -> Vec<Action> {
        let error = self.rejected_or(PairError::AuthenticationFailed(status));
        self.failure.get_or_insert(error.clone());
        if self.local {
            Vec::new()
        } else {
            vec![Action::Finish(Err(error))]
        }
    }

    /// A key for the device arrived. For a pairing the remote side started this is the end.
    pub fn on_key(&mut self) -> Vec<Action> {
        if self.local {
            Vec::new()
        } else {
            vec![Action::Finish(Ok(()))]
        }
    }

    /// The Pair Device reply.
    pub fn on_pair_reply(&mut self, status: Status) -> Vec<Action> {
        if status.is_success() {
            return vec![Action::Finish(Ok(()))];
        }
        // What the user or the remote side did explains the failure better than the status.
        let error = if self.rejected {
            PairError::Rejected
        } else if let Some(failure) = &self.failure {
            failure.clone()
        } else {
            match status {
                Status::Timeout => PairError::Timeout,
                Status::Cancelled => PairError::Cancelled,
                Status::AuthenticationFailed => PairError::AuthenticationFailed(status),
                other => PairError::Controller(other),
            }
        };
        vec![Action::Finish(Err(error))]
    }

    /// The controller went away or powered off.
    pub fn on_aborted(&mut self) -> Vec<Action> {
        self.closing(PairError::Cancelled)
    }

    /// Time passed. At the deadline the pairing is cancelled.
    pub fn on_tick(&mut self, now: Duration) -> Vec<Action> {
        if now < self.deadline {
            return Vec::new();
        }
        let mut actions = Vec::new();
        if self.local {
            actions.push(Action::Send(Command::CancelPairDevice(self.device)));
        }
        actions.extend(self.closing(PairError::Timeout));
        actions
    }

    fn closing(&mut self, error: PairError) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.phase != Phase::Waiting {
            actions.push(Action::Prompt(Prompt::Cancel));
        }
        actions.push(Action::Finish(Err(error)));
        actions
    }

    fn rejected_or(&self, error: PairError) -> PairError {
        if self.rejected {
            PairError::Rejected
        } else {
            error
        }
    }
}
