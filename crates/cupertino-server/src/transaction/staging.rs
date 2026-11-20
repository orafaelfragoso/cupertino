//! The kept commits of staged surfaces (`cupertino_transaction_v1.stage`): from `stage` until the participant's part
//! applies, every commit of the surface, and every commit of its layer surfaces (`cupertino_layers_v1`), is held back
//! whole by a commit blocker (buffer, damage, layer and presentation state, the hole state its layers carry, its
//! synchronized subsurfaces' cached state). Later commits queue behind earlier ones, so the surface's state never goes
//! backwards. The part applies on the transaction's start frame, on the frame after a late part became ready, or at once
//! when it leaves the transaction (withdrawn, cancelled, its surface left the part). A part never waits without end
//! ([`super::start`]), so neither does a kept commit. The frame callbacks of a kept commit are answered with the next
//! frame of the transaction's output, once for each frame, so a component that draws keeps the output's pace while its
//! commits are kept.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use smithay::wayland::compositor::{
    add_blocker, with_states, Blocker, BlockerState, CompositorHandler, SurfaceAttributes,
};

use super::*;

/// The kept commits of one staged surface for one participant's part: one flag releases the blockers of all of them,
/// the surface's own and its layer surfaces'.
pub(super) struct StageHold {
    /// The part the commits are kept for.
    owner: (TxId, u64),
    /// The staged surface.
    surface: WlSurface,
    released: Arc<AtomicBool>,
}

impl StageHold {
    fn release(&self) {
        self.released.store(true, Ordering::Release);
    }
}

/// The blocker of one kept commit: pending until its hold is released.
struct StageBlocker(Arc<AtomicBool>);

impl Blocker for StageBlocker {
    fn state(&self) -> BlockerState {
        if self.0.load(Ordering::Acquire) {
            BlockerState::Released
        } else {
            BlockerState::Pending
        }
    }
}

impl Cupertino {
    /// The pre-commit hook of every surface. A hole layer's commit takes the hole state it carries along
    /// ([`HoleRecord::queued`]). A commit of a staged surface counts towards its part (the participant is ready once it
    /// submitted and each surface it staged committed) and, like a commit of one of the surface's layer surfaces, is
    /// kept until the part applies.
    pub fn transaction_pre_commit(&mut self, surface: &WlSurface) {
        let sid = surface.id();
        if let Some(hole) = self
            .transactions
            .hole_by_surface
            .get(&sid)
            .and_then(|hid| self.transactions.holes.get_mut(hid))
        {
            hole.queued
                .push_back((hole.pending_source, hole.pending_claim));
        }
        if self.transactions.staged_surface_owner.is_empty() {
            return;
        }
        let owners = &self.transactions.staged_surface_owner;
        let (staged, own) = if owners.contains_key(&sid) {
            (surface.clone(), true)
        } else {
            match Self::layer_parent(surface) {
                Some(parent) if owners.contains_key(&parent.id()) => (parent, false),
                _ => return,
            }
        };
        let staged_id = staged.id();
        let Some(&owner) = owners.get(&staged_id) else {
            return;
        };
        let mut output = None;
        if let Some(tx) = self.transactions.transactions.get_mut(&owner.0) {
            output = Some(tx.output);
            if let Some(p) = tx.participants.iter_mut().find(|p| p.id == owner.1) {
                if own {
                    for s in p.staged.iter_mut().filter(|s| s.surface_id == sid) {
                        s.committed = true;
                    }
                }
                if p.submitted_at.is_some() && p.all_staged_committed() {
                    p.mark_ready(Instant::now());
                }
            }
        }
        // the commits kept for a part that applied, and not yet released, go first: they queue ahead of this one
        if let Some(old) = self
            .transactions
            .holds
            .get(&staged_id)
            .filter(|h| h.owner != owner)
        {
            old.release();
            self.transactions.holds.remove(&staged_id);
        }
        let hold = self
            .transactions
            .holds
            .entry(staged_id)
            .or_insert_with(|| StageHold {
                owner,
                surface: staged,
                released: Arc::new(AtomicBool::new(false)),
            });
        add_blocker(surface, StageBlocker(hold.released.clone()));
        let callbacks = with_states(surface, |states| {
            std::mem::take(
                &mut states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .frame_callbacks,
            )
        });
        if let Some(output) = output {
            if !callbacks.is_empty() {
                self.transactions
                    .held_callbacks
                    .extend(callbacks.into_iter().map(|c| (output, c)));
                if let Some(o) = self.outputs.iter_mut().find(|o| o.id == output) {
                    o.scheduler.damage();
                }
            }
        }
    }

    /// Release the kept commits of every staged surface that is no longer in an unapplied part (its part applied, was
    /// withdrawn or cancelled, or the surface left it) and apply them, in the order they were made.
    pub fn release_left_holds(&mut self) {
        if self.transactions.holds.is_empty() {
            return;
        }
        let owners = &self.transactions.staged_surface_owner;
        let left: Vec<ObjectId> = self
            .transactions
            .holds
            .iter()
            .filter(|(sid, h)| !h.surface.is_alive() || owners.get(*sid) != Some(&h.owner))
            .map(|(sid, _)| sid.clone())
            .collect();
        let mut clients: Vec<Client> = Vec::new();
        for sid in left {
            let Some(hold) = self.transactions.holds.remove(&sid) else {
                continue;
            };
            hold.release();
            if let Some(client) = hold.surface.client() {
                if !clients.iter().any(|c| c.id() == client.id()) {
                    clients.push(client);
                }
            }
        }
        let dh = self.display_handle.clone();
        for client in clients {
            self.client_compositor_state(&client)
                .blocker_cleared(self, &dh);
        }
    }

    /// Answer the frame callbacks of kept commits on `output` with a frame of it at `time` (the presentation clock).
    pub(super) fn answer_held_callbacks(&mut self, output: OutputId, time: Duration) {
        let ms = time.as_millis() as u32;
        self.transactions.held_callbacks.retain(|(o, callback)| {
            if *o != output {
                return true;
            }
            callback.done(ms);
            false
        });
    }

    /// Answer the frame callbacks of kept commits whose output shows no frames (it is gone or off) at once, on the
    /// server's clock, as a component on no output is called back.
    pub(super) fn answer_unshown_callbacks(&mut self) {
        if self.transactions.held_callbacks.is_empty() {
            return;
        }
        let ms = Duration::from(self.clock.now()).as_millis() as u32;
        let (outputs, displays) = (&self.outputs, &self.displays);
        self.transactions.held_callbacks.retain(|(o, callback)| {
            if start::frames_shown(outputs, displays, *o) {
                return true;
            }
            callback.done(ms);
            false
        });
    }
}
