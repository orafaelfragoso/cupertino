//! When transactions no gesture drives end. The server applies every part and runs every animation a part starts, so it
//! knows when the motion of a transaction has run: the layer animations its parts' kept commits started
//! ([`crate::layers_v1::Motion`], marked once they applied: [`Cupertino::mark_started_motion`]) and the window motion
//! of the window changes it carried out. Once every part that did not withdraw applied and all of that motion has
//! finished or been replaced, the transaction ends and every participant hears `finished(completed)`
//! ([`Cupertino::end_transactions_at_rest`]); it no longer counts towards its connections' limit and is forgotten after
//! the retention. A gesture's transaction ends where its progress comes to rest ([`super::start`]).

use super::*;
use crate::state::OutputEntry;

/// Until when `tx`, started and bound to no gesture, waits for a component it expects that is waited for and has not
/// joined: as long after its start as its start waited for it, the deadline, so a part a little late joins the
/// transaction rather than one that ended. None when it waits for none.
pub(super) fn waits_for_expected(
    tx: &TransactionRecord,
    hosts: &crate::host::HostManager,
    outputs: &[OutputEntry],
) -> Option<Instant> {
    let started_at = tx.started_at.filter(|_| tx.gesture.is_none())?;
    if !matches!(tx.phase, TxPhase::Started { .. }) {
        return None;
    }
    let missing = tx.expected.iter().any(|&comp| {
        !hosts.component_not_waited_for(comp)
            && !tx.participants.iter().any(|p| p.component == comp)
    });
    missing.then(|| started_at + Duration::from_millis(start::deadline_ms(tx, outputs).into()))
}

impl Cupertino {
    /// The animations waiting on the surfaces of the parts that just applied, whose kept commits applied with them, are
    /// their transaction's.
    pub(super) fn mark_started_motion(&mut self) {
        for tx in self.transactions.transactions.values_mut() {
            if tx.unmarked.is_empty() {
                continue;
            }
            let motion = tx.motion();
            for surface in std::mem::take(&mut tx.unmarked) {
                crate::layers_v1::mark_motion(&surface, motion);
                if !tx.moving.contains(&surface) {
                    tx.moving.push(surface);
                }
            }
        }
    }

    /// End every started transaction no gesture drives whose motion came to rest at `now`: every part that did not
    /// withdraw applied, none of its layer animations waits or runs, and the windows its window changes moved are
    /// still. Each participant still holding its object hears `finished(completed)`. Called once the tick sent the
    /// layers' `finished`, so a participant hears them first.
    pub fn end_transactions_at_rest(&mut self, now: Instant) {
        let now_s = self.now();
        let at_rest: Vec<TxId> = self
            .transactions
            .transactions
            .values()
            .filter(|tx| tx.gesture.is_none() && matches!(tx.phase, TxPhase::Started { .. }))
            .filter(|tx| tx.participants.iter().all(|p| p.withdrawn || p.applied))
            .filter(|tx| tx.unmarked.is_empty())
            .filter(|tx| {
                waits_for_expected(tx, &self.hosts, &self.outputs).is_none_or(|until| now >= until)
            })
            .filter(|tx| {
                let motion = tx.motion();
                !tx.moving.iter().any(|s| crate::layers_v1::moves(s, motion))
            })
            .filter(|tx| {
                !tx.carried_out
                    .iter()
                    .any(|c| self.window_change_moves(c, now_s))
            })
            .map(|tx| tx.id)
            .collect();
        for tx_id in at_rest {
            let Some(tx) = self.transactions.transactions.get_mut(&tx_id) else {
                continue;
            };
            tx.phase = TxPhase::Ended {
                ended_at: now,
                outcome: WireOutcome::Completed,
            };
            tx.moving.clear();
            tx.carried_out.clear();
            for p in tx.participants.iter().filter(|p| !p.withdrawn) {
                if let Some(res) = &p.resource {
                    res.finished(WireOutcome::Completed);
                }
            }
        }
    }

    /// Whether the window motion that `change`, carried out, runs at `now` (model seconds) still moves: the window
    /// activated (restored from the Dock, say) or moved to another desktop, the desktops of a switch, the windows a
    /// window manager's placements fade.
    fn window_change_moves(&self, change: &QueuedWindowChange, now: f64) -> bool {
        match *change {
            QueuedWindowChange::ActivateWindow(window)
            | QueuedWindowChange::MoveWindowToWorkspace { window, .. } => {
                self.wm
                    .window(window)
                    .is_some_and(|w| w.lifecycle.is_animating(now))
                    || self.window_managers.moves(window, now)
            }
            QueuedWindowChange::SwitchWorkspace { output, .. } => {
                self.wm.workspaces_sliding(output, now)
            }
            QueuedWindowChange::CommitWindowManager(output) => {
                self.window_managers.moves_on(output, now)
            }
            QueuedWindowChange::RequestRedraw => false,
        }
    }
}
