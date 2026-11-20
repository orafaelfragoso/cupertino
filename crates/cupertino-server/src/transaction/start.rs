//! When transactions start. Once the creator and every component it expects are ready, or the deadline passed, the
//! transaction starts on the first frame of its output that the server prepares ([`Cupertino::transactions_frame`]),
//! and that frame's presentation time is the start time every participant hears. A part that becomes ready later applies
//! on the next frame of the output, at the original start time. An output that is gone, off or shows no frame within
//! [`FRAME_WAIT_MS`] does not hold a transaction back: it starts on the server's clock at the moment it was ready.

use super::*;
use crate::state::OutputEntry;

/// How long a ready transaction, or a late part, waits for a frame of its output before it starts on the server's
/// clock: an output that stopped drawing (a stalled display) holds nothing back for longer.
pub const FRAME_WAIT_MS: u64 = 50;

/// The moment a transaction was ready to start: `at` on the loop's clock, `clock` on the presentation clock (its start
/// time when no frame of its output comes).
#[derive(Clone, Copy, Debug)]
pub(super) struct ReadyMark {
    at: Instant,
    clock: Duration,
}

/// Whether output `id` draws frames that a transaction can start on: it exists and is powered.
pub(super) fn frames_shown(
    outputs: &[OutputEntry],
    displays: &crate::displays::Displays,
    id: OutputId,
) -> bool {
    outputs.iter().any(|o| o.id == id) && displays.powered(id)
}

/// Ask output `id` for a frame.
fn want_frame(outputs: &mut [OutputEntry], id: OutputId) {
    if let Some(o) = outputs.iter_mut().find(|o| o.id == id) {
        o.scheduler.damage();
    }
}

/// The deadline of `tx` in milliseconds after its creator's `submit`: `set_deadline`'s, else one frame interval of its
/// output plus [`DEFAULT_DEADLINE_EXTRA_MS`], never more than [`MAX_DEADLINE_MS`].
fn deadline_ms(tx: &TransactionRecord, outputs: &[OutputEntry]) -> u32 {
    tx.deadline_ms.unwrap_or_else(|| {
        let frame_ms = outputs
            .iter()
            .find(|o| o.id == tx.output)
            .and_then(|o| o.output.current_mode())
            .map(|m| (1_000_000 / m.refresh.max(1)) as u32)
            .unwrap_or(16);
        (frame_ms + DEFAULT_DEADLINE_EXTRA_MS).min(MAX_DEADLINE_MS)
    })
}

/// Take the parts of `tx` on: a participant that has not submitted [`AUTO_SUBMIT_MS`] after it created or joined is
/// taken as submitted, and one whose staged surface has not committed [`COMMIT_GRACE_MS`] after its `submit` is ready
/// as it stands, that surface leaving the part.
fn advance_parts(
    tx: &mut TransactionRecord,
    now: Instant,
    owners: &mut HashMap<ObjectId, (TxId, u64)>,
) {
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
                p.mark_ready(now);
            }
        }
        if let Some(sub_at) = p.submitted_at {
            if !p.ready
                && now.saturating_duration_since(sub_at)
                    >= Duration::from_millis(COMMIT_GRACE_MS.into())
            {
                p.staged.retain(|s| {
                    if !s.committed {
                        owners.remove(&s.surface_id);
                    }
                    s.committed
                });
                p.mark_ready(now);
            }
        }
    }
}

/// Whether `tx` (pending) may start: its creator is ready and so is every component it expects that is waited for, or
/// the deadline after the creator's `submit` passed.
fn start_due(
    tx: &TransactionRecord,
    now: Instant,
    hosts: &crate::host::HostManager,
    outputs: &[OutputEntry],
) -> bool {
    if tx.orphan_join_until.is_some() {
        return false;
    }
    let Some(creator) = tx
        .participants
        .iter()
        .find(|p| p.is_creator && !p.withdrawn)
    else {
        return false;
    };
    if !creator.ready {
        return false;
    }
    let all_expected_ready = tx.expected.iter().all(|&comp| {
        hosts.component_not_waited_for(comp)
            || tx
                .participants
                .iter()
                .any(|p| p.component == comp && p.ready && !p.withdrawn)
    });
    let deadline = Duration::from_millis(deadline_ms(tx, outputs).into());
    let deadline_passed = creator
        .submitted_at
        .is_some_and(|s| now.saturating_duration_since(s) >= deadline);
    all_expected_ready || deadline_passed
}

/// Apply part `p` with start time `ts`: its kept commits leave the hold ([`Cupertino::release_left_holds`]), its window
/// changes go to `changes` and it hears `started`.
fn apply_part(
    p: &mut Participant,
    owners: &mut HashMap<ObjectId, (TxId, u64)>,
    changes: &mut Vec<QueuedWindowChange>,
    (tv_sec_hi, tv_sec_lo, tv_nsec): (u32, u32, u32),
) {
    p.applied = true;
    for s in &p.staged {
        owners.remove(&s.surface_id);
    }
    changes.extend(std::mem::take(&mut p.window_changes));
    if let Some(res) = &p.resource {
        res.started(tv_sec_hi, tv_sec_lo, tv_nsec);
    }
}

impl Cupertino {
    /// Advance all pending, started, late-joining, and gesture-settling transactions: parts become ready, a
    /// transaction whose parts are in is due to start on its output's next frame, and one whose output shows no frame
    /// starts on the server's clock.
    pub fn tick_transactions(&mut self, now: Instant) {
        let model_now = self.now();
        let clock_now = Duration::from(self.clock.now());
        let wait = Duration::from_millis(FRAME_WAIT_MS);
        let mut to_start: Vec<(TxId, Duration)> = Vec::new();
        let mut late_apply: Vec<(TxId, u64)> = Vec::new();
        let mut settled_gestures: Vec<(TxId, OutputId, WireOutcome, bool)> = Vec::new();
        let mut expired_ended: Vec<TxId> = Vec::new();
        // the holes of a hold that ended go with it
        self.transactions
            .drop_ended_holds(self.hosts.held_surfaces());
        self.end_abandoned_transactions(now);

        for (&tx_id, tx) in &mut self.transactions.transactions {
            // Prune retained ended transactions older than 10s.
            if let TxPhase::Ended { ended_at, .. } = tx.phase {
                // an ended transaction keeps no commits: what was staged and not applied applies as it stands
                let owners = &mut self.transactions.staged_surface_owner;
                for p in &mut tx.participants {
                    for s in p.staged.drain(..) {
                        if owners.get(&s.surface_id) == Some(&(tx_id, p.id)) {
                            owners.remove(&s.surface_id);
                        }
                    }
                }
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
                    // the first participant still in it; with none the transaction is abandoned
                    if let Some(first) = tx.participants.iter_mut().find(|p| !p.withdrawn) {
                        first.is_creator = true;
                    }
                }
            }
            advance_parts(tx, now, &mut self.transactions.staged_surface_owner);
            let shown = frames_shown(&self.outputs, &self.displays, tx.output);
            if matches!(tx.phase, TxPhase::Started { .. }) {
                for p in &tx.participants {
                    if !p.ready || p.applied || p.withdrawn {
                        continue;
                    }
                    let waited = p
                        .ready_since
                        .is_some_and(|t| now.saturating_duration_since(t) >= wait);
                    if !shown || waited {
                        late_apply.push((tx_id, p.id));
                    } else {
                        want_frame(&mut self.outputs, tx.output);
                    }
                }
            }
            match tx.phase {
                TxPhase::Pending => {
                    if tx.ready_at.is_none() && start_due(tx, now, &self.hosts, &self.outputs) {
                        if tx.gesture.is_some() {
                            tx.phase = TxPhase::Armed;
                            continue;
                        }
                        tx.ready_at = Some(ReadyMark {
                            at: now,
                            clock: clock_now,
                        });
                        want_frame(&mut self.outputs, tx.output);
                    }
                    if let Some(mark) = tx.ready_at {
                        if !shown || now.saturating_duration_since(mark.at) >= wait {
                            to_start.push((tx_id, mark.clock));
                        }
                    }
                }
                TxPhase::GestureSettling => {
                    if let Some(settle) = tx.gesture.as_ref().and_then(|g| g.settle.as_ref()) {
                        if !settle.property.is_animating(model_now) {
                            let at_one = (settle.property.target() - 1.0).abs() < 0.5;
                            settled_gestures.push((tx_id, tx.output, settle.outcome, at_one));
                        }
                    }
                }
                _ => {}
            }
        }

        let mut window_changes = Vec::new();
        for (tx_id, clock) in to_start {
            window_changes.extend(self.start_transaction(tx_id, clock, model_now));
        }
        for (tx_id, pid) in late_apply {
            window_changes.extend(self.apply_late_part(tx_id, pid));
        }
        window_changes.extend(self.finish_settled_gestures(settled_gestures, now));
        for change in window_changes {
            self.apply_queued_window_change(change);
        }
        for tx_id in expired_ended {
            if let Some(tx) = self.transactions.transactions.remove(&tx_id) {
                self.transactions.by_key.remove(&tx.key);
            }
        }
        self.release_left_holds();
        self.answer_unshown_callbacks();
    }

    /// A frame of output `idx` is being prepared: the transactions of the output that are due start on it, at its
    /// presentation time, and the late parts of its started transactions apply on it at their original start time; their
    /// kept commits apply before the scene is composed. The frame callbacks of kept commits on the output are answered.
    pub fn transactions_frame(&mut self, idx: usize) {
        let Some(out) = self.outputs.get(idx) else {
            return;
        };
        let output = out.id;
        let frame_now = self.frame_now();
        let presentation = out
            .scheduler
            .presentation_at(frame_now, self.refresh_of(idx))
            .unwrap_or(frame_now);
        let model_s = self.scene_time(idx);
        let mut due: Vec<TxId> = Vec::new();
        let mut late: Vec<(TxId, u64)> = Vec::new();
        for (&tx_id, tx) in &self.transactions.transactions {
            if tx.output != output {
                continue;
            }
            match tx.phase {
                TxPhase::Pending if tx.ready_at.is_some() => due.push(tx_id),
                TxPhase::Started { .. } => late.extend(
                    tx.participants
                        .iter()
                        .filter(|p| p.ready && !p.applied && !p.withdrawn)
                        .map(|p| (tx_id, p.id)),
                ),
                _ => {}
            }
        }
        let mut window_changes = Vec::new();
        for tx_id in due {
            window_changes.extend(self.start_transaction(tx_id, presentation, model_s));
        }
        for (tx_id, pid) in late {
            window_changes.extend(self.apply_late_part(tx_id, pid));
        }
        for change in window_changes {
            self.apply_queued_window_change(change);
        }
        self.release_left_holds();
        self.answer_held_callbacks(output, frame_now);
    }

    /// End as cancelled every transaction not started whose creator withdrew ([`TransactionManager::abandoned`]):
    /// every other participant hears `finished(cancelled)`, its part applies at once by itself, kept commits and window
    /// changes, and the transaction no longer counts towards a connection's [`MAX_ACTIVE_TRANSACTIONS`].
    pub(super) fn end_abandoned_transactions(&mut self, now: Instant) {
        let abandoned = self.transactions.abandoned();
        if abandoned.is_empty() {
            return;
        }
        let mut changes = Vec::new();
        for tx_id in abandoned {
            changes.extend(self.transactions.end_cancelled(tx_id, now));
        }
        for change in changes {
            self.apply_queued_window_change(change);
        }
        self.release_left_holds();
    }

    /// Start `tx_id` at `at` (the presentation clock; `model_s` on the model clock): every ready participant's part
    /// applies. Returns the window changes to carry out.
    fn start_transaction(
        &mut self,
        tx_id: TxId,
        at: Duration,
        model_s: f64,
    ) -> Vec<QueuedWindowChange> {
        let mut changes = Vec::new();
        let Some(tx) = self.transactions.transactions.get_mut(&tx_id) else {
            return changes;
        };
        let (tv_sec_hi, tv_sec_lo, tv_nsec) = split_timestamp(at);
        tx.phase = TxPhase::Started {
            tv_sec_hi,
            tv_sec_lo,
            tv_nsec,
            start_model_s: model_s,
        };
        tx.ready_at = None;
        for p in &mut tx.participants {
            if p.ready && !p.applied && !p.withdrawn {
                apply_part(
                    p,
                    &mut self.transactions.staged_surface_owner,
                    &mut changes,
                    (tv_sec_hi, tv_sec_lo, tv_nsec),
                );
            }
        }
        self.request_redraw();
        changes
    }

    /// Apply the late part `pid` of the started `tx_id` at the transaction's original start time.
    fn apply_late_part(&mut self, tx_id: TxId, pid: u64) -> Vec<QueuedWindowChange> {
        let mut changes = Vec::new();
        let Some(tx) = self.transactions.transactions.get_mut(&tx_id) else {
            return changes;
        };
        let TxPhase::Started {
            tv_sec_hi,
            tv_sec_lo,
            tv_nsec,
            ..
        } = tx.phase
        else {
            return changes;
        };
        if let Some(p) = tx.participants.iter_mut().find(|p| p.id == pid) {
            if p.ready && !p.applied && !p.withdrawn {
                apply_part(
                    p,
                    &mut self.transactions.staged_surface_owner,
                    &mut changes,
                    (tv_sec_hi, tv_sec_lo, tv_nsec),
                );
            }
        }
        self.request_redraw();
        changes
    }

    /// Finish the gesture-bound transactions whose settle spring came to rest: completed ones carry out their window
    /// changes (returned) and keep a reverse binding at rest, reverted ones drop them.
    fn finish_settled_gestures(
        &mut self,
        settled: Vec<(TxId, OutputId, WireOutcome, bool)>,
        now: Instant,
    ) -> Vec<QueuedWindowChange> {
        let mut window_changes = Vec::new();
        for (tx_id, output, outcome, at_one) in settled {
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
        window_changes
    }

    /// The earliest model time at which a transaction has time-driven work: a participant taken as submitted, a part
    /// ready as it stands, a deadline, an orphan join promoted, a transaction or late part that stops waiting for a
    /// frame, an ended transaction forgotten. So the loop never waits on a transaction without a deadline.
    pub fn transaction_deadline(&self) -> Option<f64> {
        let ms = |m: u32| Duration::from_millis(m.into());
        let wait = Duration::from_millis(FRAME_WAIT_MS);
        let mut next: Option<Instant> = None;
        let mut at = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
        for tx in self.transactions.transactions.values() {
            if let TxPhase::Ended { ended_at, .. } = tx.phase {
                at(ended_at + Duration::from_secs(ENDED_RETENTION_SECS));
                continue;
            }
            if let Some(until) = tx.orphan_join_until {
                at(until);
            }
            let started = matches!(tx.phase, TxPhase::Started { .. });
            for p in tx.participants.iter().filter(|p| !p.withdrawn) {
                match (p.submitted_at, p.ready) {
                    (None, _) => at(p.created_at + ms(AUTO_SUBMIT_MS)),
                    (Some(s), false) => at(s + ms(COMMIT_GRACE_MS)),
                    (Some(_), true) if started && !p.applied => {
                        if let Some(t) = p.ready_since {
                            at(t + wait);
                        }
                    }
                    _ => {}
                }
            }
            if tx.phase == TxPhase::Pending {
                match tx.ready_at {
                    Some(mark) => at(mark.at + wait),
                    // the deadline counts once the creator is ready: until then its own parts' times come first
                    None if tx.orphan_join_until.is_none() => {
                        let creator = tx
                            .participants
                            .iter()
                            .find(|p| p.is_creator && !p.withdrawn && p.ready);
                        if let Some(s) = creator.and_then(|c| c.submitted_at) {
                            at(s + ms(deadline_ms(tx, &self.outputs)));
                        }
                    }
                    None => {}
                }
            }
        }
        next.map(|t| {
            self.now() + t.saturating_duration_since(Instant::now()).as_secs_f64() * self.time_scale
        })
    }
}
