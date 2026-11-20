//! Server-side transactions, gesture-bound transactions, and window/workspace holes
//! (`cupertino_transaction_manager_v1`, `cupertino_transaction_v1`, `cupertino_hole_v1`).

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
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
use smithay::reexports::wayland_server::protocol::wl_callback::WlCallback;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::compositor::get_role;

use crate::authorization::{authorized, ShellRole};
use crate::host::{component_spec, HeldSurface};
use crate::layers_v1::LAYER_ROLE;
use crate::state::{ClientState, Cupertino};

/// Protocol version offered for `cupertino_transaction_manager_v1`.
pub const TRANSACTION_PROTOCOL_VERSION: u32 = 1;

/// How many holes deep the scene draws sources: a window drawn in a hole draws its own layers, and a hole among them
/// draws nothing, so no source draws itself and a frame never recurses.
pub const MAX_HOLE_NESTING: u8 = 1;

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
    /// When the part became ready: a late part waits for a frame of the transaction's output from then on.
    ready_since: Option<Instant>,
    applied: bool,
    withdrawn: bool,
}

impl Participant {
    fn all_staged_committed(&self) -> bool {
        self.staged.iter().all(|s| s.committed)
    }

    /// The part is ready at `now`, unless it already was.
    fn mark_ready(&mut self, now: Instant) {
        self.ready = true;
        self.ready_since.get_or_insert(now);
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
    /// When the creator and every expected component were ready (or the deadline passed): the transaction starts on
    /// the next frame of its output that the server prepares.
    ready_at: Option<start::ReadyMark>,
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
    /// The source and claim each commit of the layer surface carries, in commit order, taken as it is made (its
    /// pre-commit hook) and applied as it applies, so a kept commit applies the hole state it was made with; a commit
    /// merged into a kept one replaces the state that one carries, so the queue stays as short as the kept commits.
    pub queued: VecDeque<(HoleSource, bool)>,
    /// The window whose picture fades out in the hole after it closed ([`ClosedSource`]).
    pub closing: Option<ClosedSource>,
    /// The component whose connection made the hole.
    pub component: &'static str,
    /// Where the scene last drew the hole of a layer shell surface's layer: its output and bounds (output-local
    /// points), which a hold keeps ([`HeldHole`]).
    pub shown: Cell<Option<(OutputId, Rectangle<f64, Logical>)>>,
    /// The hole stays on a held surface after its connection ended ([`HeldHole`]).
    pub held: Option<HeldHole>,
}

/// A hole of a persistent surface held after its component's connection ended (D19): the surface shows its last
/// picture, and the hole goes on drawing its source live where it last showed, its claim kept, until the hold ends.
#[derive(Clone, Debug)]
pub struct HeldHole {
    /// The hold's persistence key.
    pub key: String,
    pub output: OutputId,
    /// The hole's bounds as it last showed, output-local points.
    pub bounds: Rectangle<f64, Logical>,
}

/// A hole's window that closed: the hole keeps the window's last picture and fades it out where it lies, along the
/// server's closing animation, then draws nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClosedSource {
    pub window: WindowId,
    /// The window's frame, decorations included, in logical points, as it was when it closed: its picture is fitted in
    /// the hole as the window was.
    pub size: (f64, f64),
    /// The hole held the claim in force: the window's picture fades out there alone, not at its own frame.
    pub claimed: bool,
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
    /// The kept commits of each staged surface, by the staged surface's object id ([`staging`]).
    holds: HashMap<ObjectId, staging::StageHold>,
    /// The frames of windows that are closing, until their holes hear it ([`TransactionManager::window_closing`]).
    closing_frames: HashMap<WindowId, (f64, f64)>,
    /// Frame callbacks of kept commits, answered with the next frame of their output.
    held_callbacks: Vec<(OutputId, WlCallback)>,
    /// How many hole sources the scene is drawing inside each other ([`TransactionManager::enter_hole`]).
    hole_depth: Cell<u8>,
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
            holds: HashMap::new(),
            closing_frames: HashMap::new(),
            held_callbacks: Vec::new(),
            hole_depth: Cell::new(0),
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

    /// The hole of the layer surface `layer_surface_id`, if it has one: what the scene draws in its bounds.
    pub fn hole_for_layer(&self, layer_surface_id: &ObjectId) -> Option<&HoleRecord> {
        let hid = self.hole_by_surface.get(layer_surface_id)?;
        self.holes.get(hid)
    }

    /// Whether the claim of hole `hole` (its [`HoleRecord::id`]) on `window` is the one in force: the window is drawn in
    /// that hole. A claim below it is suspended, and its hole draws nothing.
    pub fn claim_in_force(&self, window: WindowId, hole: u64) -> bool {
        self.active_claim_for(window).is_some_and(|h| h.id == hole)
    }

    /// The windows whose closing picture fades out in the hole that claimed them, and so not at their own frame.
    pub fn closing_in_claims(&self) -> Vec<WindowId> {
        self.holes
            .values()
            .filter_map(|h| h.closing.filter(|c| c.claimed).map(|c| c.window))
            .collect()
    }

    /// The scene starts drawing a hole's source; `false` (and nothing to undo) when it is already as deep inside other
    /// holes' sources as [`MAX_HOLE_NESTING`] allows. Each `true` is followed by [`TransactionManager::leave_hole`].
    pub fn enter_hole(&self) -> bool {
        let depth = self.hole_depth.get();
        if depth >= MAX_HOLE_NESTING {
            return false;
        }
        self.hole_depth.set(depth + 1);
        true
    }

    /// The scene finished drawing a hole's source.
    pub fn leave_hole(&self) {
        self.hole_depth.set(self.hole_depth.get().saturating_sub(1));
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
            // a transaction that ended keeps no changes: the request is carried out at once, as an ordinary one
            if matches!(tx.phase, TxPhase::Ended { .. }) {
                return false;
            }
            if let Some(p) = tx.participants.iter_mut().find(|p| p.id == pid) {
                p.window_changes.push(change);
                return true;
            }
        }
        false
    }

    /// End `tx_id` as cancelled for every participant: each that has not withdrawn hears `finished(cancelled)` and its
    /// staged surfaces leave the transaction, so their kept commits apply at once, as ordinary commits
    /// ([`Cupertino::release_left_holds`]). Returns the window changes to carry out: before the start each part applies
    /// by itself; after it, the changes not yet carried out are dropped.
    fn end_cancelled(&mut self, tx_id: TxId, now: Instant) -> Vec<QueuedWindowChange> {
        let mut changes = Vec::new();
        let Some(tx) = self.transactions.get_mut(&tx_id) else {
            return changes;
        };
        let before_start = matches!(tx.phase, TxPhase::Pending | TxPhase::Armed);
        if self.active_gesture_tx.get(&tx.output) == Some(&tx_id) {
            self.active_gesture_tx.remove(&tx.output);
        }
        for p in &mut tx.participants {
            if p.withdrawn {
                continue;
            }
            for s in &p.staged {
                self.staged_surface_owner.remove(&s.surface_id);
            }
            p.staged.clear();
            if before_start {
                changes.extend(std::mem::take(&mut p.window_changes));
            } else {
                p.window_changes.clear();
            }
            if let Some(res) = &p.resource {
                res.finished(WireOutcome::Cancelled);
            }
        }
        tx.phase = TxPhase::Ended {
            ended_at: now,
            outcome: WireOutcome::Cancelled,
        };
        changes
    }

    /// The transactions not started whose creator withdrew (it destroyed its object before `submit`, or its connection
    /// ended): nothing would start them.
    fn abandoned(&self) -> Vec<TxId> {
        self.transactions
            .values()
            .filter(|tx| matches!(tx.phase, TxPhase::Pending | TxPhase::Armed))
            .filter(|tx| tx.orphan_join_until.is_none())
            .filter(|tx| !tx.participants.iter().any(|p| p.is_creator && !p.withdrawn))
            .map(|tx| tx.id)
            .collect()
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

    /// The frame of `window`, decorations included, in logical points, as it closes ([`notify_window_closed`]).
    ///
    /// [`notify_window_closed`]: TransactionManager::notify_window_closed
    pub fn window_closing(&mut self, window: WindowId, size: (f64, f64)) {
        self.closing_frames.insert(window, size);
    }

    /// Notify holes whose source window closed, removing its claims and emitting `source_closed`. A hole that drew the
    /// window fades its last picture out there, fitted as the window's frame was when it closed
    /// ([`TransactionManager::window_closing`], [`ClosedSource`]); a hole whose claim was suspended draws nothing.
    pub fn notify_window_closed(&mut self, window: WindowId) {
        let size = self.closing_frames.remove(&window);
        let in_force = self
            .window_claims
            .remove(&window)
            .and_then(|stack| stack.last().copied());
        for (&hid, hole) in &mut self.holes {
            if matches!(
                hole.committed_source,
                HoleSource::Window(w) | HoleSource::Thumbnail(w) if w == window
            ) {
                let claimed = hole.committed_claim && in_force == Some(hid);
                let suspended = hole.committed_claim && !claimed;
                hole.closing = size.filter(|_| !suspended).map(|size| ClosedSource {
                    window,
                    size,
                    claimed,
                });
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

    /// Clean up all transaction parts, brackets, staged surfaces and holes owned by a disconnected client. A hole that
    /// last showed on one of the component's surfaces the server now holds (`held`) stays there, drawing its source
    /// and keeping its claim, until the hold ends ([`TransactionManager::drop_ended_holds`]).
    pub fn client_disconnected(&mut self, client_id: &ClientId, held: &[HeldSurface]) {
        self.open_brackets.remove(client_id);

        // Remove holes owned by this client and pop them from claim stacks, but the held ones.
        let gone_holes: Vec<HoleId> = self
            .holes
            .iter()
            .filter_map(|(&hid, h)| (h.client_id == *client_id).then_some(hid))
            .collect();
        let mut dropped = Vec::new();
        for hid in gone_holes {
            let Some(h) = self.holes.get_mut(&hid) else {
                continue;
            };
            self.hole_by_surface.remove(&h.layer_surface_id);
            h.queued.clear();
            h.held = h.shown.get().and_then(|(output, bounds)| {
                let centre = (
                    bounds.loc.x + bounds.size.w / 2.0,
                    bounds.loc.y + bounds.size.h / 2.0,
                );
                let mine = || {
                    held.iter()
                        .filter(|s| s.component == h.component && s.output == output)
                };
                // the held surface the hole lay on; one whose geometry is not known is the component's only hold there
                let on = mine()
                    .find(|s| s.geometry.to_f64().contains(centre))
                    .or_else(|| {
                        let mut only = mine().filter(|s| s.geometry.is_empty());
                        only.next().filter(|_| only.next().is_none())
                    });
                on.map(|s| HeldHole {
                    key: s.key.clone(),
                    output,
                    bounds,
                })
            });
            if h.held.is_none() {
                dropped.push(hid);
            }
        }
        self.drop_holes(&dropped);

        // Withdraw unapplied parts and settle active gestures if their creator disconnected. A transaction that had not
        // started ends as cancelled with the next tick ([`Cupertino::end_abandoned_transactions`]).
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
                if tx.phase == TxPhase::RestingCompleted {
                    tx.phase = TxPhase::Ended {
                        ended_at: Instant::now(),
                        outcome: WireOutcome::Cancelled,
                    };
                }
            }
        }
    }

    /// Remove holes `gone` and their claims.
    fn drop_holes(&mut self, gone: &[HoleId]) {
        for hid in gone {
            if let Some(h) = self.holes.remove(hid) {
                self.hole_by_surface.remove(&h.layer_surface_id);
                for stack in self.window_claims.values_mut() {
                    stack.retain(|id| id != hid);
                }
            }
        }
        self.window_claims.retain(|_, stack| !stack.is_empty());
    }

    /// End the holes of holds that ended (`held` is what the server still holds): the component took its surface over
    /// again, or the hold was given up. Their claims end with them.
    pub fn drop_ended_holds(&mut self, held: &[HeldSurface]) {
        let ended: Vec<HoleId> = self
            .holes
            .iter()
            .filter(|(_, h)| {
                h.held.as_ref().is_some_and(|hh| {
                    !held.iter().any(|s| {
                        s.component == h.component && s.key == hh.key && s.output == hh.output
                    })
                })
            })
            .map(|(&hid, _)| hid)
            .collect();
        self.drop_holes(&ended);
    }

    /// The holes of held surfaces ([`HeldHole`]).
    pub fn held_holes(&self) -> impl Iterator<Item = &HoleRecord> {
        self.holes.values().filter(|h| h.held.is_some())
    }
}

/// Convert monotonic duration to `(tv_sec_hi, tv_sec_lo, tv_nsec)` matching `wp_presentation`.
fn split_timestamp(d: Duration) -> (u32, u32, u32) {
    let secs = d.as_secs();
    ((secs >> 32) as u32, secs as u32, d.subsec_nanos())
}

impl Cupertino {
    /// Called from `CompositorHandler::commit`, as a commit of `surface` applies: the hole state it carries applies with it.
    /// A staged surface's commit counts towards its part when it is made, in its pre-commit hook
    /// ([`Cupertino::transaction_pre_commit`]), since a kept commit applies only with the part.
    pub fn transaction_surface_committed(&mut self, surface: &WlSurface) {
        if let Some(&hid) = self.transactions.hole_by_surface.get(&surface.id()) {
            self.commit_hole(hid);
        }
    }

    fn commit_hole(&mut self, hid: HoleId) {
        let Some(hole) = self.transactions.holes.get_mut(&hid) else {
            return;
        };
        let old_source = hole.committed_source;
        let old_claim = hole.committed_claim;
        let (source, claim) = hole
            .queued
            .pop_front()
            .unwrap_or((hole.pending_source, hole.pending_claim));
        hole.committed_source = source;
        hole.committed_claim = claim;
        // a new source takes the hole over from the picture of a window that closed
        if source != HoleSource::None {
            hole.closing = None;
        }
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
        // the parts applied: their kept commits apply with them
        self.release_left_holds();
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
mod staging;
mod start;
