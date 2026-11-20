//! The kept commits of staged surfaces (`cupertino_transaction_v1.stage`): from `stage` until the participant's part
//! applies, every commit of the surface, and every commit of its layer surfaces (`cupertino_layers_v1`), is held back
//! whole by a commit blocker (buffer, damage, layer and presentation state, the hole state its layers carry, its
//! synchronized subsurfaces' cached state). Later commits queue behind earlier ones, so the surface's state never goes
//! backwards. The part applies on the transaction's start frame, on the frame after a late part became ready, or at once
//! when it leaves the transaction (withdrawn, cancelled, its surface left the part). A part never waits without end
//! ([`super::start`]), so neither does a kept commit. The frame callbacks of a kept commit are answered with the next
//! frame of the transaction's output, once for each frame, so a component that draws keeps the output's pace while its
//! commits are kept.
//!
//! A gesture keeps an armed transaction's commits for as long as it waits, so what a surface keeps is bounded: the
//! first kept commit of each surface carries the hold's blocker (Smithay applies a client's later commits of the same
//! surface only after it), the callbacks of a surface's commits past [`PACED_KEPT_COMMITS`] wait for the part to
//! apply, so a component that draws on its frame callbacks pauses, and a commit past [`MAX_KEPT_COMMITS`] takes the
//! staged surface out of the part.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use smithay::wayland::compositor::{
    add_blocker, with_states, Blocker, BlockerState, CompositorHandler, SurfaceAttributes,
};

use super::*;

/// How many kept commits of one surface have their frame callbacks answered with the output's frames. The callbacks of
/// later ones wait for the part to apply: a component that draws on its frame callbacks, with two or three buffers,
/// goes on drawing that far and then pauses, so its kept commits (and the buffers they hold) stop piling up while a
/// gesture keeps a transaction armed.
pub const PACED_KEPT_COMMITS: usize = 3;

/// How many commits of one surface (the staged surface or one of its layer surfaces) a part keeps at most. A commit
/// past them takes the staged surface out of the part, its kept commits applying at once and in order, as ordinary
/// commits, as a staged surface that does not commit in time leaves it ([`COMMIT_GRACE_MS`]): a component that
/// commits without waiting for its frame callbacks never has the server keep its commits without end.
pub const MAX_KEPT_COMMITS: usize = 64;

/// The kept commits of one staged surface for one participant's part: one flag releases the blockers of all of them,
/// the surface's own and its layer surfaces'.
pub(super) struct StageHold {
    /// The part the commits are kept for.
    owner: (TxId, u64),
    /// The staged surface.
    surface: WlSurface,
    released: Arc<AtomicBool>,
    /// The transaction's output, whose frames answer the kept commits' frame callbacks.
    output: Option<OutputId>,
    /// How many commits of each surface of the hold (the staged surface, its layer surfaces) are kept, by object id.
    kept: HashMap<ObjectId, usize>,
    /// The frame callbacks of the commits kept past [`PACED_KEPT_COMMITS`], answered once the hold ends.
    waiting: Vec<WlCallback>,
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
        if self
            .transactions
            .holds
            .get(&staged_id)
            .is_some_and(|h| h.owner != owner)
        {
            if let Some(old) = self.transactions.holds.remove(&staged_id) {
                self.end_hold(old);
            }
        }
        let hold = self
            .transactions
            .holds
            .entry(staged_id.clone())
            .or_insert_with(|| StageHold {
                owner,
                surface: staged,
                released: Arc::new(AtomicBool::new(false)),
                output,
                kept: HashMap::new(),
                waiting: Vec::new(),
            });
        let kept = hold.kept.entry(sid).or_insert(0);
        if *kept >= MAX_KEPT_COMMITS {
            // the staged surface leaves the part: with its hold released, the commits kept apply ahead of this one
            self.transactions.unstage(owner, &staged_id);
            if let Some(hold) = self.transactions.holds.remove(&staged_id) {
                self.end_hold(hold);
            }
            return;
        }
        // later commits of the surface queue behind its first kept one
        if *kept == 0 {
            add_blocker(surface, StageBlocker(hold.released.clone()));
        }
        *kept += 1;
        let paced = *kept <= PACED_KEPT_COMMITS;
        let callbacks = with_states(surface, |states| {
            std::mem::take(
                &mut states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .frame_callbacks,
            )
        });
        if callbacks.is_empty() {
            return;
        }
        if !paced {
            hold.waiting.extend(callbacks);
            return;
        }
        self.hold_callbacks(output, callbacks);
    }

    /// Answer `callbacks` of kept commits with the next frame of `output`, or at once with no output to wait for.
    fn hold_callbacks(&mut self, output: Option<OutputId>, callbacks: Vec<WlCallback>) {
        let Some(output) = output else {
            let ms = Duration::from(self.clock.now()).as_millis() as u32;
            for callback in callbacks {
                callback.done(ms);
            }
            return;
        };
        self.transactions
            .held_callbacks
            .extend(callbacks.into_iter().map(|c| (output, c)));
        if let Some(o) = self.outputs.iter_mut().find(|o| o.id == output) {
            o.scheduler.damage();
        }
    }

    /// Release `hold`'s kept commits; the frame callbacks it kept waiting are answered with the next frame.
    fn end_hold(&mut self, mut hold: StageHold) {
        hold.release();
        let waiting = std::mem::take(&mut hold.waiting);
        if !waiting.is_empty() {
            self.hold_callbacks(hold.output, waiting);
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
            if let Some(client) = hold.surface.client() {
                if !clients.iter().any(|c| c.id() == client.id()) {
                    clients.push(client);
                }
            }
            self.end_hold(hold);
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

impl TransactionManager {
    /// The staged surface `staged` leaves the part `owner`, which is ready without it once it submitted and its other
    /// staged surfaces committed.
    fn unstage(&mut self, owner: (TxId, u64), staged: &ObjectId) {
        if self.staged_surface_owner.get(staged) == Some(&owner) {
            self.staged_surface_owner.remove(staged);
        }
        let Some(p) = self
            .transactions
            .get_mut(&owner.0)
            .and_then(|tx| tx.participants.iter_mut().find(|p| p.id == owner.1))
        else {
            return;
        };
        p.staged.retain(|s| &s.surface_id != staged);
        if p.submitted_at.is_some() && p.all_staged_committed() {
            p.mark_ready(Instant::now());
        }
    }
}
