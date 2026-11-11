use std::collections::{BTreeMap, BTreeSet};

use crate::Epoch;

/// Maximum distinct input/source pairs held at once, bounding cleanup work and the ownership table.
pub const MAX_HELD_INPUTS: usize = 1024;

/// Source identities are assigned by the server, never taken from an untrusted control request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputSource {
    Physical(u64),
    Controller(Epoch),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Input {
    Key(u32),
    Button(u32),
}

/// An aggregate transition to deliver through the normal input router.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    Press(Input),
    Release(Input),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnershipError {
    Full,
}

/// Ownership only: no device access or event delivery. Callers handle target routing and protected surfaces.
#[derive(Debug, Default)]
pub struct InputOwners {
    owners: BTreeMap<Input, BTreeSet<InputSource>>,
    held: usize,
}

impl InputOwners {
    /// Duplicate presses/releases change nothing. Only the first press and last release reach the aggregate seat.
    pub fn set(
        &mut self,
        source: InputSource,
        input: Input,
        pressed: bool,
    ) -> Result<Option<Transition>, OwnershipError> {
        if pressed {
            if self
                .owners
                .get(&input)
                .is_some_and(|set| set.contains(&source))
            {
                return Ok(None);
            }
            if self.held == MAX_HELD_INPUTS {
                return Err(OwnershipError::Full);
            }
            let owners = self.owners.entry(input).or_default();
            let first = owners.is_empty();
            owners.insert(source);
            self.held += 1;
            Ok(first.then_some(Transition::Press(input)))
        } else {
            let Some(owners) = self.owners.get_mut(&input) else {
                return Ok(None);
            };
            if !owners.remove(&source) {
                return Ok(None);
            }
            self.held -= 1;
            if owners.is_empty() {
                self.owners.remove(&input);
                Ok(Some(Transition::Release(input)))
            } else {
                Ok(None)
            }
        }
    }

    /// Disconnecting one source releases only inputs nobody else holds; releases are in deterministic input order.
    pub fn release_source(&mut self, source: InputSource) -> Vec<Transition> {
        let mut releases = Vec::new();
        self.owners.retain(|input, owners| {
            if owners.remove(&source) {
                self.held -= 1;
                if owners.is_empty() {
                    releases.push(Transition::Release(*input));
                    return false;
                }
            }
            true
        });
        releases
    }

    /// Whether `source` holds `input` down.
    pub fn holds(&self, source: InputSource, input: Input) -> bool {
        self.owners
            .get(&input)
            .is_some_and(|owners| owners.contains(&source))
    }

    /// Whether a source is in a pointer gesture that another controller must leave alone.
    pub fn has_buttons(&self, source: InputSource) -> bool {
        self.owners
            .iter()
            .any(|(input, owners)| matches!(input, Input::Button(_)) && owners.contains(&source))
    }

    pub fn held_count(&self) -> usize {
        self.held
    }
}
