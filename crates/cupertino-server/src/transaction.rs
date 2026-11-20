//! Server-side transactions, gesture-bound transactions, and window/workspace holes
//! (`cupertino_transaction_manager_v1`, `cupertino_transaction_v1`, `cupertino_hole_v1`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use cupertino_animation::{LayerCurve, LayerProperty};
use cupertino_protocols::server::transaction::cupertino_hole_v1::{
    self, CupertinoHoleV1, Error as HoleError, Source as WireSource,
};
use cupertino_protocols::server::transaction::cupertino_transaction_manager_v1::{
    self, CupertinoTransactionManagerV1, Error as ManagerError,
};
use cupertino_protocols::server::transaction::cupertino_transaction_v1::{
    self, CupertinoTransactionV1, Error as TxError, Gesture as WireGesture, Outcome as WireOutcome,
};
use cupertino_protocols::transaction::{
    AUTO_SUBMIT_MS, COMMIT_GRACE_MS, DEFAULT_DEADLINE_EXTRA_MS, ENDED_RETENTION_SECS, JOIN_WAIT_MS,
    MAX_ACTIVE_TRANSACTIONS, MAX_DEADLINE_MS, MAX_EXPECTED_COMPONENTS, MAX_HOLES_PER_CONNECTION,
    MAX_STAGED_SURFACES, SPRING_DAMPING, SPRING_RESPONSE,
};
use cupertino_wm::{OutputId, WindowId, WorkspaceId};
use smithay::output::Output;
use smithay::reexports::wayland_server::backend::{ClientId, GlobalId, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::wayland::compositor::get_role;

use crate::authorization::{authorized, ShellRole};
use crate::host::component_spec;
use crate::layers_v1::LAYER_ROLE;
use crate::state::{ClientState, Cupertino};

/// Protocol version offered for `cupertino_transaction_manager_v1`.
pub const TRANSACTION_PROTOCOL_VERSION: u32 = 1;

/// Unique key of a transaction across components and restarts (`component`, `instance`, `serial`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransactionKey {
    pub component: &'static str,
    pub instance: u32,
    pub serial: u32,
}

/// Internal numeric handle for a transaction record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TxId(u64);

/// Internal numeric handle for a hole record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct HoleId(u64);

/// User data on a `cupertino_transaction_v1` resource.
#[derive(Clone, Copy, Debug)]
pub struct TransactionObjectData {
    tx_id: TxId,
    participant_id: u64,
    is_creator: bool,
}

/// User data on a `cupertino_hole_v1` resource.
#[derive(Clone, Copy, Debug)]
pub struct HoleObjectData {
    hole_id: HoleId,
}

/// A deferred window change queued inside `begin_window_changes`..`end_window_changes`.
#[derive(Clone, Debug)]
pub enum QueuedWindowChange {
    ActivateWindow(WindowId),
    SwitchWorkspace {
        output: OutputId,
        workspace: WorkspaceId,
    },
    MoveWindowToWorkspace {
        window: WindowId,
        workspace: WorkspaceId,
    },
    CommitWindowManager(OutputId),
    RequestRedraw,
}

/// A surface staged by one participant in a transaction.
#[derive(Clone, Debug)]
struct StagedSurface {
    surface_id: ObjectId,
    committed: bool,
}

/// One participant's part of a transaction.
struct Participant {
    id: u64,
    component: &'static str,
    client_id: ClientId,
    is_creator: bool,
    resource: Option<CupertinoTransactionV1>,
    created_at: Instant,
    submitted_at: Option<Instant>,
    staged: Vec<StagedSurface>,
    window_changes: Vec<QueuedWindowChange>,
    ready: bool,
    applied: bool,
    withdrawn: bool,
}

impl Participant {
    fn all_staged_committed(&self) -> bool {
        self.staged.iter().all(|s| s.committed)
    }
}

/// Gesture binding state attached to a transaction by `bind_gesture` / `bind_reverse_gesture`.
struct GestureBinding {
    gesture: WireGesture,
    reverse_gesture: Option<WireGesture>,
    response: f64,
    damping: f64,
    armed_seq: u64,
    completed_seq: Option<u64>,
    /// Active spring settling progress (`0.0..=1.0`) after fingers lift.
    settle: Option<GestureSettle>,
    /// Live progress (`0.0..=1.0`) while fingers are on the trackpad.
    tracking_progress: Option<f64>,
    /// Whether the currently active gesture is the reverse gesture (`1.0 -> 0.0`).
    reversing: bool,
}

struct GestureSettle {
    property: LayerProperty,
    outcome: WireOutcome,
}

/// Lifecycle phase of a transaction.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TxPhase {
    /// Collecting parts; waiting for expected components or deadline.
    Pending,
    /// Armed for a touchpad gesture (waits for `gesture_begin`).
    Armed,
    /// Actively tracking a touchpad gesture (`gesture_began` sent).
    GestureTracking,
    /// Settling along the bound spring after fingers lifted (`gesture_ended` sent).
    GestureSettling,
    /// Started at a presentation clock time (`started` sent); late joiners catch up from `start_model_s`.
    Started {
        tv_sec_hi: u32,
        tv_sec_lo: u32,
        tv_nsec: u32,
        start_model_s: f64,
    },
    /// Completed and resting at `1.0` with a reverse gesture armed.
    RestingCompleted,
    /// Ended (`finished` sent); retained for [`ENDED_RETENTION_SECS`] for late joins.
    Ended {
        ended_at: Instant,
        outcome: WireOutcome,
    },
}

struct TransactionRecord {
    id: TxId,
    key: TransactionKey,
    output: OutputId,
    /// If created by a `join` that arrived before the creator's `create`, the deadline to promote to standalone.
    orphan_join_until: Option<Instant>,
    expected: Vec<&'static str>,
    /// Explicit deadline in ms requested by `set_deadline`, clamped to [`MAX_DEADLINE_MS`].
    deadline_ms: Option<u32>,
    phase: TxPhase,
    gesture: Option<GestureBinding>,
    participants: Vec<Participant>,
}

/// Content source bound to a `cupertino_hole_v1` layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HoleSource {
    #[default]
    None,
    Window(WindowId),
    Thumbnail(WindowId),
    Workspace(WorkspaceId),
}

/// State of one `cupertino_hole_v1` object.
#[derive(Clone, Debug)]
pub struct HoleRecord {
    pub id: u64,
    pub client_id: ClientId,
    pub layer_surface_id: ObjectId,
    pub resource: CupertinoHoleV1,
    pub pending_source: HoleSource,
    pub committed_source: HoleSource,
    pub pending_claim: bool,
    pub committed_claim: bool,
}

/// Server-side manager for transactions, gesture bindings, and holes.
pub struct TransactionManager {
    _global: GlobalId,
    next_tx_id: u64,
    next_participant_id: u64,
    next_hole_id: u64,
    next_seq: u64,
    transactions: HashMap<TxId, TransactionRecord>,
    by_key: HashMap<TransactionKey, TxId>,
    /// Open window-change bracket per connection (at most [`MAX_OPEN_BRACKETS`] = 1).
    open_brackets: HashMap<ClientId, (TxId, u64)>,
    /// Surfaces currently staged in an unstarted/unapplied participant part.
    staged_surface_owner: HashMap<ObjectId, (TxId, u64)>,
    /// Active holes keyed by `HoleId`.
    holes: HashMap<HoleId, HoleRecord>,
    /// Lookup from layer `WlSurface` object id to `HoleId` (at most one hole per layer).
    hole_by_surface: HashMap<ObjectId, HoleId>,
    /// Claim stacks per window in commit order (last element is the active claim).
    window_claims: HashMap<WindowId, Vec<HoleId>>,
    /// Active gesture transaction per output while tracking or settling.
    active_gesture_tx: HashMap<OutputId, TxId>,
}

impl TransactionManager {
    pub fn new(dh: &DisplayHandle) -> Self {
        let global = dh.create_global::<Cupertino, CupertinoTransactionManagerV1, ()>(
            TRANSACTION_PROTOCOL_VERSION,
            (),
        );
        Self {
            _global: global,
            next_tx_id: 1,
            next_participant_id: 1,
            next_hole_id: 1,
            next_seq: 1,
            transactions: HashMap::new(),
            by_key: HashMap::new(),
            open_brackets: HashMap::new(),
            staged_surface_owner: HashMap::new(),
            holes: HashMap::new(),
            hole_by_surface: HashMap::new(),
            window_claims: HashMap::new(),
            active_gesture_tx: HashMap::new(),
        }
    }

    /// Number of unended transactions `client_id` currently participates in.
    fn active_tx_count_for(&self, client_id: &ClientId) -> usize {
        self.transactions
            .values()
            .filter(|tx| !matches!(tx.phase, TxPhase::Ended { .. }))
            .filter(|tx| {
                tx.participants
                    .iter()
                    .any(|p| p.client_id == *client_id && !p.withdrawn)
            })
            .count()
    }

    /// Number of holes currently owned by `client_id`.
    fn hole_count_for(&self, client_id: &ClientId) -> usize {
        self.holes
            .values()
            .filter(|h| h.client_id == *client_id)
            .count()
    }

    /// Whether `window` is currently claimed by at least one hole (hidden at its own frame, taking no input there).
    pub fn is_window_claimed(&self, window: WindowId) -> bool {
        self.window_claims
            .get(&window)
            .is_some_and(|stack| !stack.is_empty())
    }

    /// The top (active) hole claiming `window`, if any.
    pub fn active_claim_for(&self, window: WindowId) -> Option<&HoleRecord> {
        let hid = self.window_claims.get(&window)?.last()?;
        self.holes.get(hid)
    }

    /// All committed holes currently targeting `window` or any source (for scene rendering).
    pub fn hole_for_layer(&self, layer_surface_id: &ObjectId) -> Option<&HoleRecord> {
        let hid = self.hole_by_surface.get(layer_surface_id)?;
        self.holes.get(hid)
    }

    /// Whether `client_id` currently has an open window-change bracket; if so, enqueue `change` and return `true`.
    pub fn enqueue_window_change(
        &mut self,
        client_id: &ClientId,
        change: QueuedWindowChange,
    ) -> bool {
        let Some(&(tx_id, pid)) = self.open_brackets.get(client_id) else {
            return false;
        };
        if let Some(tx) = self.transactions.get_mut(&tx_id) {
            if let Some(p) = tx.participants.iter_mut().find(|p| p.id == pid) {
                p.window_changes.push(change);
                return true;
            }
        }
        false
    }

    /// Current gesture progress (`0.0..=1.0`) for `output` while a bound gesture transaction is tracking or settling.
    pub fn gesture_progress_on(&self, output: OutputId, model_now: f64) -> Option<f64> {
        let tx_id = self.active_gesture_tx.get(&output)?;
        let tx = self.transactions.get(tx_id)?;
        let binding = tx.gesture.as_ref()?;
        if let Some(settle) = &binding.settle {
            return Some(settle.property.value(model_now));
        }
        binding.tracking_progress
    }

    /// Whether any armed or resting-completed transaction is bound to `gesture` on `output`.
    pub fn has_gesture_binding(&self, output: OutputId, gesture: WireGesture) -> bool {
        self.pick_gesture_binding(output, gesture).is_some()
    }

    /// Select the winning transaction for `(output, gesture)` per spec arbitration:
    /// 1. Reverse binding of a resting-completed transaction (most recently completed wins).
    /// 2. Armed transaction (most recently armed wins).
    fn pick_gesture_binding(&self, output: OutputId, gesture: WireGesture) -> Option<(TxId, bool)> {
        let reverse = self
            .transactions
            .values()
            .filter(|tx| tx.output == output && tx.phase == TxPhase::RestingCompleted)
            .filter_map(|tx| {
                let g = tx.gesture.as_ref()?;
                (g.reverse_gesture == Some(gesture))
                    .then_some((g.completed_seq.unwrap_or(0), tx.id))
            })
            .max_by_key(|(seq, _)| *seq)
            .map(|(_, id)| (id, true));
        if reverse.is_some() {
            return reverse;
        }
        self.transactions
            .values()
            .filter(|tx| tx.output == output && tx.phase == TxPhase::Armed)
            .filter_map(|tx| {
                let g = tx.gesture.as_ref()?;
                (g.gesture == gesture).then_some((g.armed_seq, tx.id))
            })
            .max_by_key(|(seq, _)| *seq)
            .map(|(_, id)| (id, false))
    }

    /// Whether a transaction keyed by `key` has started and what its start timestamp is.
    pub fn started_timestamp(&self, key: &TransactionKey) -> Option<(u32, u32, u32)> {
        let tx = self.transactions.get(self.by_key.get(key)?)?;
        match tx.phase {
            TxPhase::Started {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                ..
            } => Some((tv_sec_hi, tv_sec_lo, tv_nsec)),
            _ => None,
        }
    }

    /// Notify holes whose source window closed, removing its claims and emitting `source_closed`.
    pub fn notify_window_closed(&mut self, window: WindowId) {
        self.window_claims.remove(&window);
        for hole in self.holes.values_mut() {
            if matches!(
                hole.committed_source,
                HoleSource::Window(w) | HoleSource::Thumbnail(w) if w == window
            ) {
                hole.committed_source = HoleSource::None;
                hole.committed_claim = false;
                hole.resource.source_closed();
            }
        }
    }

    /// Notify holes whose source workspace was removed.
    pub fn notify_workspace_removed(&mut self, workspace: WorkspaceId) {
        for hole in self.holes.values_mut() {
            if hole.committed_source == HoleSource::Workspace(workspace) {
                hole.committed_source = HoleSource::None;
                hole.resource.source_closed();
            }
        }
    }

    /// Notify thumbnail holes when a minimized window unminimizes.
    pub fn notify_window_unminimized(&mut self, window: WindowId) {
        for hole in self.holes.values_mut() {
            if hole.committed_source == HoleSource::Thumbnail(window) {
                hole.committed_source = HoleSource::None;
                hole.resource.source_closed();
            }
        }
    }

    /// Clean up all transaction parts, brackets, staged surfaces and holes owned by a disconnected client.
    pub fn client_disconnected(&mut self, client_id: &ClientId) {
        self.open_brackets.remove(client_id);

        // Remove holes owned by this client and pop them from claim stacks.
        let gone_holes: Vec<HoleId> = self
            .holes
            .iter()
            .filter_map(|(&hid, h)| (h.client_id == *client_id).then_some(hid))
            .collect();
        for hid in gone_holes {
            if let Some(h) = self.holes.remove(&hid) {
                self.hole_by_surface.remove(&h.layer_surface_id);
                for stack in self.window_claims.values_mut() {
                    stack.retain(|&id| id != hid);
                }
            }
        }
        self.window_claims.retain(|_, stack| !stack.is_empty());

        // Withdraw unapplied parts and settle active gestures if their creator disconnected.
        let mut creator_gone_txs = Vec::new();
        for tx in self.transactions.values_mut() {
            for p in &mut tx.participants {
                if p.client_id == *client_id {
                    p.withdrawn = true;
                    p.resource = None;
                    for s in &p.staged {
                        self.staged_surface_owner.remove(&s.surface_id);
                    }
                    p.staged.clear();
                    p.window_changes.clear();
                    if p.is_creator && tx.gesture.is_some() {
                        creator_gone_txs.push(tx.id);
                    }
                }
            }
        }
        for tx_id in creator_gone_txs {
            if let Some(tx) = self.transactions.get_mut(&tx_id) {
                if let Some(g) = &mut tx.gesture {
                    g.reverse_gesture = None;
                }
                if matches!(tx.phase, TxPhase::Armed | TxPhase::RestingCompleted) {
                    tx.phase = TxPhase::Ended {
                        ended_at: Instant::now(),
                        outcome: WireOutcome::Cancelled,
                    };
                }
            }
        }
    }
}

/// Convert monotonic duration to `(tv_sec_hi, tv_sec_lo, tv_nsec)` matching `wp_presentation`.
fn split_timestamp(d: Duration) -> (u32, u32, u32) {
    let secs = d.as_secs();
    ((secs >> 32) as u32, secs as u32, d.subsec_nanos())
}

impl Cupertino {
    /// Called from `CompositorHandler::commit`: marks staged surfaces committed and applies pending hole state for
    /// layer surfaces owned by `surface`.
    pub fn transaction_surface_committed(&mut self, surface: &WlSurface) {
        let sid = surface.id();

        // 1. Apply double-buffered hole state if this surface itself is a hole layer (or has child hole layers).
        if let Some(&hid) = self.transactions.hole_by_surface.get(&sid) {
            self.commit_hole(hid);
        }

        // 2. Mark staged surface committed in its participant part.
        let owner = self.transactions.staged_surface_owner.get(&sid).copied();
        if let Some((tx_id, pid)) = owner {
            if let Some(tx) = self.transactions.transactions.get_mut(&tx_id) {
                if let Some(p) = tx.participants.iter_mut().find(|p| p.id == pid) {
                    for s in &mut p.staged {
                        if s.surface_id == sid {
                            s.committed = true;
                        }
                    }
                    if p.submitted_at.is_some() && p.all_staged_committed() {
                        p.ready = true;
                    }
                }
            }
        }
    }

    fn commit_hole(&mut self, hid: HoleId) {
        let Some(hole) = self.transactions.holes.get_mut(&hid) else {
            return;
        };
        let old_source = hole.committed_source;
        let old_claim = hole.committed_claim;
        hole.committed_source = hole.pending_source;
        hole.committed_claim = hole.pending_claim;
        let new_source = hole.committed_source;
        let new_claim = hole.committed_claim;

        if old_claim {
            if let HoleSource::Window(w) = old_source {
                if let Some(stack) = self.transactions.window_claims.get_mut(&w) {
                    stack.retain(|&id| id != hid);
                    if stack.is_empty() {
                        self.transactions.window_claims.remove(&w);
                    }
                }
            }
        }
        if new_claim {
            if let HoleSource::Window(w) = new_source {
                let stack = self.transactions.window_claims.entry(w).or_default();
                stack.retain(|&id| id != hid);
                stack.push(hid);
            }
        }
        self.request_redraw();
    }

    /// Begin a touchpad gesture bound to a transaction on `output`. Returns `true` if a transaction claimed it.
    pub fn transaction_gesture_begin(&mut self, output: OutputId, gesture: WireGesture) -> bool {
        let Some((tx_id, reversing)) = self.transactions.pick_gesture_binding(output, gesture)
        else {
            return false;
        };
        if let Some(tx) = self.transactions.transactions.get_mut(&tx_id) {
            tx.phase = TxPhase::GestureTracking;
            if let Some(binding) = &mut tx.gesture {
                binding.reversing = reversing;
                binding.settle = None;
                binding.tracking_progress = Some(if reversing { 1.0 } else { 0.0 });
            }
            for p in &mut tx.participants {
                if !p.withdrawn {
                    if let Some(res) = &p.resource {
                        res.gesture_began(gesture);
                    }
                    if p.ready && !p.applied {
                        p.applied = true;
                        for s in &p.staged {
                            self.transactions.staged_surface_owner.remove(&s.surface_id);
                        }
                    }
                }
            }
        }
        self.transactions.active_gesture_tx.insert(output, tx_id);
        self.request_redraw();
        true
    }

    /// Update the progress (`0.0..=1.0`) of an active gesture-bound transaction on `output`.
    pub fn transaction_gesture_update(&mut self, output: OutputId, progress: f64) -> bool {
        let Some(&tx_id) = self.transactions.active_gesture_tx.get(&output) else {
            return false;
        };
        let Some(tx) = self.transactions.transactions.get_mut(&tx_id) else {
            return false;
        };
        if tx.phase != TxPhase::GestureTracking {
            return false;
        }
        if let Some(binding) = &mut tx.gesture {
            let p = progress.clamp(0.0, 1.0);
            binding.tracking_progress = Some(if binding.reversing { 1.0 - p } else { p });
            self.request_redraw();
            return true;
        }
        false
    }

    /// End an active gesture-bound transaction on `output`, settling along its spring from `velocity`.
    pub fn transaction_gesture_end(
        &mut self,
        output: OutputId,
        completed: bool,
        velocity: f64,
    ) -> bool {
        let now_s = self.now();
        let Some(&tx_id) = self.transactions.active_gesture_tx.get(&output) else {
            return false;
        };
        let Some(tx) = self.transactions.transactions.get_mut(&tx_id) else {
            return false;
        };
        if tx.phase != TxPhase::GestureTracking {
            return false;
        }
        let outcome = if completed {
            WireOutcome::Completed
        } else {
            WireOutcome::Reverted
        };
        tx.phase = TxPhase::GestureSettling;
        if let Some(binding) = &mut tx.gesture {
            let cur = binding.tracking_progress.unwrap_or(0.0);
            let target = match (binding.reversing, completed) {
                (false, true) | (true, false) => 1.0,
                (false, false) | (true, true) => 0.0,
            };
            let v = if binding.reversing {
                -velocity
            } else {
                velocity
            };
            let mut prop = LayerProperty::new(cur);
            prop.animate(
                target,
                LayerCurve::Spring {
                    response: binding.response,
                    damping: binding.damping,
                },
                now_s,
                v,
            );
            binding.settle = Some(GestureSettle {
                property: prop,
                outcome,
            });
        }
        for p in &tx.participants {
            if !p.withdrawn {
                if let Some(res) = &p.resource {
                    res.gesture_ended(outcome);
                }
            }
        }
        self.request_redraw();
        true
    }

    /// Advance all pending, started, late-joining, and gesture-settling transactions.
    pub fn tick_transactions(&mut self, now: Instant) {
        let model_now = self.now();
        let clock_dur = Duration::from(self.clock.now());
        let (tv_sec_hi, tv_sec_lo, tv_nsec) = split_timestamp(clock_dur);

        // 1. Auto-submit after 100 ms, commit grace after 100 ms, and orphan join promotion after 100 ms.
        let mut to_start: Vec<TxId> = Vec::new();
        let mut late_apply: Vec<(TxId, u64)> = Vec::new();
        let mut settled_gestures: Vec<(TxId, OutputId, WireOutcome, bool)> = Vec::new();
        let mut expired_ended: Vec<TxId> = Vec::new();

        for (&tx_id, tx) in &mut self.transactions.transactions {
            // Prune retained ended transactions older than 10s.
            if let TxPhase::Ended { ended_at, .. } = tx.phase {
                if now.saturating_duration_since(ended_at)
                    >= Duration::from_secs(ENDED_RETENTION_SECS)
                {
                    expired_ended.push(tx_id);
                }
                continue;
            }

            // Promote orphan join if creator never arrived within JOIN_WAIT_MS.
            if let Some(until) = tx.orphan_join_until {
                if now >= until {
                    tx.orphan_join_until = None;
                    if let Some(first) = tx.participants.first_mut() {
                        first.is_creator = true;
                    }
                }
            }

            // Participant auto-submit & commit grace.
            for p in &mut tx.participants {
                if p.withdrawn {
                    continue;
                }
                if p.submitted_at.is_none()
                    && now.saturating_duration_since(p.created_at)
                        >= Duration::from_millis(AUTO_SUBMIT_MS.into())
                {
                    p.submitted_at = Some(now);
                    if p.all_staged_committed() {
                        p.ready = true;
                    }
                }
                if let Some(sub_at) = p.submitted_at {
                    if !p.ready
                        && now.saturating_duration_since(sub_at)
                            >= Duration::from_millis(COMMIT_GRACE_MS.into())
                    {
                        // Drop uncommitted staged surfaces from this part; rest becomes ready.
                        p.staged.retain(|s| {
                            if !s.committed {
                                self.transactions.staged_surface_owner.remove(&s.surface_id);
                                false
                            } else {
                                true
                            }
                        });
                        p.ready = true;
                    }
                }
                if p.ready && !p.applied && matches!(tx.phase, TxPhase::Started { .. }) {
                    late_apply.push((tx_id, p.id));
                }
            }

            match tx.phase {
                TxPhase::Pending => {
                    if tx.orphan_join_until.is_some() {
                        continue;
                    }
                    let Some(creator) = tx
                        .participants
                        .iter()
                        .find(|p| p.is_creator && !p.withdrawn)
                    else {
                        continue;
                    };
                    if !creator.ready {
                        continue;
                    }
                    let all_expected_ready = tx.expected.iter().all(|&comp| {
                        self.hosts.component_not_waited_for(comp)
                            || tx
                                .participants
                                .iter()
                                .any(|p| p.component == comp && p.ready && !p.withdrawn)
                    });
                    let deadline_ms = tx.deadline_ms.unwrap_or_else(|| {
                        let frame_ms = self
                            .outputs
                            .iter()
                            .find(|o| o.id == tx.output)
                            .and_then(|o| o.output.current_mode())
                            .map(|m| (1_000_000 / m.refresh.max(1)) as u32)
                            .unwrap_or(16);
                        (frame_ms + DEFAULT_DEADLINE_EXTRA_MS).min(MAX_DEADLINE_MS)
                    });
                    let deadline_passed = creator.submitted_at.is_some_and(|s| {
                        now.saturating_duration_since(s)
                            >= Duration::from_millis(deadline_ms.into())
                    });
                    if all_expected_ready || deadline_passed {
                        if tx.gesture.is_some() {
                            tx.phase = TxPhase::Armed;
                        } else {
                            to_start.push(tx_id);
                        }
                    }
                }
                TxPhase::GestureSettling => {
                    if let Some(binding) = &tx.gesture {
                        if let Some(settle) = &binding.settle {
                            if !settle.property.is_animating(model_now) {
                                let at_one = (settle.property.target() - 1.0).abs() < 0.5;
                                settled_gestures.push((tx_id, tx.output, settle.outcome, at_one));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // 2. Start ready transactions on one shared presentation timestamp.
        let mut window_changes = Vec::new();
        for tx_id in to_start {
            if let Some(tx) = self.transactions.transactions.get_mut(&tx_id) {
                tx.phase = TxPhase::Started {
                    tv_sec_hi,
                    tv_sec_lo,
                    tv_nsec,
                    start_model_s: model_now,
                };
                for p in &mut tx.participants {
                    if p.ready && !p.applied && !p.withdrawn {
                        p.applied = true;
                        for s in &p.staged {
                            self.transactions.staged_surface_owner.remove(&s.surface_id);
                        }
                        window_changes.extend(std::mem::take(&mut p.window_changes));
                        if let Some(res) = &p.resource {
                            res.started(tv_sec_hi, tv_sec_lo, tv_nsec);
                        }
                    }
                }
            }
            self.request_redraw();
        }

        // 3. Apply late joiners in step using the transaction's original start timestamp.
        for (tx_id, pid) in late_apply {
            if let Some(tx) = self.transactions.transactions.get_mut(&tx_id) {
                let TxPhase::Started {
                    tv_sec_hi,
                    tv_sec_lo,
                    tv_nsec,
                    ..
                } = tx.phase
                else {
                    continue;
                };
                if let Some(p) = tx.participants.iter_mut().find(|p| p.id == pid) {
                    if p.ready && !p.applied && !p.withdrawn {
                        p.applied = true;
                        for s in &p.staged {
                            self.transactions.staged_surface_owner.remove(&s.surface_id);
                        }
                        window_changes.extend(std::mem::take(&mut p.window_changes));
                        if let Some(res) = &p.resource {
                            res.started(tv_sec_hi, tv_sec_lo, tv_nsec);
                        }
                    }
                }
            }
            self.request_redraw();
        }

        // 4. Finish settled gestures.
        for (tx_id, output, outcome, at_one) in settled_gestures {
            self.transactions.active_gesture_tx.remove(&output);
            self.transactions.next_seq += 1;
            let seq = self.transactions.next_seq;
            if let Some(tx) = self.transactions.transactions.get_mut(&tx_id) {
                let keep_reverse = at_one
                    && tx
                        .gesture
                        .as_ref()
                        .is_some_and(|g| g.reverse_gesture.is_some());
                if let Some(g) = &mut tx.gesture {
                    g.settle = None;
                    g.tracking_progress = None;
                    if keep_reverse {
                        g.completed_seq = Some(seq);
                    }
                }
                for p in &mut tx.participants {
                    if !p.withdrawn {
                        if at_one {
                            window_changes.extend(std::mem::take(&mut p.window_changes));
                        } else {
                            p.window_changes.clear();
                        }
                        if let Some(res) = &p.resource {
                            res.finished(outcome);
                        }
                    }
                }
                tx.phase = if keep_reverse {
                    TxPhase::RestingCompleted
                } else {
                    TxPhase::Ended {
                        ended_at: now,
                        outcome,
                    }
                };
            }
            self.request_redraw();
        }

        // 5. Execute queued window changes.
        for change in window_changes {
            self.apply_queued_window_change(change);
        }

        // 6. Drop expired ended transactions.
        for tx_id in expired_ended {
            if let Some(tx) = self.transactions.transactions.remove(&tx_id) {
                self.transactions.by_key.remove(&tx.key);
            }
        }
    }

    fn apply_queued_window_change(&mut self, change: QueuedWindowChange) {
        match change {
            QueuedWindowChange::ActivateWindow(id) => {
                if self.wm.window(id).is_some() {
                    self.focus_window_by_input(id);
                }
            }
            QueuedWindowChange::SwitchWorkspace { output, workspace } => {
                self.command(cupertino_wm::Command::SwitchWorkspace { output, workspace });
            }
            QueuedWindowChange::MoveWindowToWorkspace { window, workspace } => {
                self.command(cupertino_wm::Command::MoveWindowToWorkspace { window, workspace });
            }
            QueuedWindowChange::CommitWindowManager(output) => {
                self.commit_placements(output);
            }
            QueuedWindowChange::RequestRedraw => self.request_redraw(),
        }
    }
}

mod dispatch;
