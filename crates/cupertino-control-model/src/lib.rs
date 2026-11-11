//! Desktop-control policy without transport, device I/O or a display.
//!
//! The server validates the requesting connection and genuine-user consent before calling the grant transitions.
//! This model checks epochs, state, scope and capability narrowing; it cannot establish a caller's identity.
//! [`InputOwners`] tracks aggregate input transitions so removing a controller preserves another source's press.
//! [`identity`] finds the app a request comes from and [`permission`] decides a request from what the person
//! remembered for that app.

pub mod identity;
mod input;
pub mod permission;

pub use input::{Input, InputOwners, InputSource, OwnershipError, Transition, MAX_HELD_INPUTS};

use std::num::NonZeroU64;

/// How many runs one controller serves at once, one per connected agent: the controller turns away a further agent
/// with an error, and the compositor allows this many live sessions on the controller's connection.
pub const MAX_RUNS_PER_CONTROLLER: usize = 8;

/// One server-assigned run identity. A new connection or run must receive a fresh epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Epoch(NonZeroU64);

impl Epoch {
    pub fn new(value: u64) -> Option<Self> {
        NonZeroU64::new(value).map(Self)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

/// A compositor-owned handle. Reusing an identifier requires a different generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    pub id: u64,
    pub generation: u64,
}

/// The one output or window named in consent. A grant never implicitly covers another target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Output(Handle),
    Window(Handle),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Observe,
    Capture,
    Pointer,
    Keyboard,
    Semantic,
}

impl Operation {
    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// Capabilities explicitly requested or approved for a run. These bits are model values, not a wire encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities(u8);

impl Capabilities {
    pub const NONE: Self = Self(0);

    pub fn of(operations: impl IntoIterator<Item = Operation>) -> Self {
        Self(operations.into_iter().fold(0, |bits, op| bits | op.bit()))
    }

    pub fn allows(self, operation: Operation) -> bool {
        self.0 & operation.bit() != 0
    }

    pub fn is_empty(self) -> bool {
        self == Self::NONE
    }

    fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub scope: Scope,
    pub capabilities: Capabilities,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Inactive,
    AwaitingConsent,
    Active,
    Suspended,
    Revoked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revocation {
    Locked,
    SessionDeactivated,
    ControllerDisconnected,
    AuthorityDisconnected,
    Stopped,
    /// The person turned off or removed a service the run uses.
    Withdrawn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    StaleEpoch,
    InvalidPhase,
    EmptyCapabilities,
    GrantEscalation,
    NotGranted,
    Suspended,
    Revoked,
    OutsideScope,
    CapabilityDenied,
}

/// A single run, terminal once revoked. Only the trusted server may approve or resume it.
#[derive(Debug)]
pub struct Session {
    epoch: Epoch,
    phase: Phase,
    requested: Option<Grant>,
    granted: Option<Grant>,
    revocation: Option<Revocation>,
}

impl Session {
    pub fn new(epoch: Epoch) -> Self {
        Self {
            epoch,
            phase: Phase::Inactive,
            requested: None,
            granted: None,
            revocation: None,
        }
    }

    pub fn epoch(&self) -> Epoch {
        self.epoch
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn revocation(&self) -> Option<Revocation> {
        self.revocation
    }

    pub fn request(&mut self, epoch: Epoch, requested: Grant) -> Result<(), Refusal> {
        self.expect_phase(epoch, Phase::Inactive)?;
        if requested.capabilities.is_empty() {
            return Err(Refusal::EmptyCapabilities);
        }
        self.requested = Some(requested);
        self.phase = Phase::AwaitingConsent;
        Ok(())
    }

    /// Called only after the server validates fresh genuine-user consent on the trusted session UI.
    pub fn approve(&mut self, epoch: Epoch, approved: Grant) -> Result<(), Refusal> {
        self.expect_phase(epoch, Phase::AwaitingConsent)?;
        let requested = self.requested.expect("awaiting consent holds a request");
        if approved.capabilities.is_empty() {
            return Err(Refusal::EmptyCapabilities);
        }
        if approved.scope != requested.scope
            || !requested.capabilities.contains(approved.capabilities)
        {
            return Err(Refusal::GrantEscalation);
        }
        self.requested = None;
        self.granted = Some(approved);
        self.phase = Phase::Active;
        Ok(())
    }

    pub fn deny(&mut self, epoch: Epoch) -> Result<(), Refusal> {
        self.expect_phase(epoch, Phase::AwaitingConsent)?;
        self.requested = None;
        self.phase = Phase::Inactive;
        Ok(())
    }

    pub fn suspend(&mut self, epoch: Epoch) -> Result<(), Refusal> {
        self.expect_phase(epoch, Phase::Active)?;
        self.phase = Phase::Suspended;
        Ok(())
    }

    /// Called only after the server validates an explicit genuine-user resume.
    pub fn resume(&mut self, epoch: Epoch) -> Result<(), Refusal> {
        self.expect_phase(epoch, Phase::Suspended)?;
        self.phase = Phase::Active;
        Ok(())
    }

    /// Revocation also cancels consent that was pending when the session locked or its authority disappeared.
    pub fn revoke(&mut self, reason: Revocation) {
        if self.phase != Phase::Revoked {
            self.requested = None;
            self.granted = None;
            self.revocation = Some(reason);
            self.phase = Phase::Revoked;
        }
    }

    pub fn authorize(
        &self,
        epoch: Epoch,
        scope: Scope,
        operation: Operation,
    ) -> Result<(), Refusal> {
        self.check_epoch(epoch)?;
        match self.phase {
            Phase::Inactive | Phase::AwaitingConsent => return Err(Refusal::NotGranted),
            Phase::Suspended => return Err(Refusal::Suspended),
            Phase::Revoked => return Err(Refusal::Revoked),
            Phase::Active => {}
        }
        let grant = self.granted.expect("an active session holds a grant");
        if grant.scope != scope {
            return Err(Refusal::OutsideScope);
        }
        if !grant.capabilities.allows(operation) {
            return Err(Refusal::CapabilityDenied);
        }
        Ok(())
    }

    fn check_epoch(&self, epoch: Epoch) -> Result<(), Refusal> {
        if epoch != self.epoch {
            Err(Refusal::StaleEpoch)
        } else {
            Ok(())
        }
    }

    fn expect_phase(&self, epoch: Epoch, expected: Phase) -> Result<(), Refusal> {
        self.check_epoch(epoch)?;
        if self.phase == Phase::Revoked {
            Err(Refusal::Revoked)
        } else if self.phase != expected {
            Err(Refusal::InvalidPhase)
        } else {
            Ok(())
        }
    }
}
