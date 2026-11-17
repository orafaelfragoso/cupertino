//! A control run moves through consent, suspension and revocation, and every operation is checked against its grant.

use cupertino_control_model::{
    Capabilities, Epoch, Grant, Handle, Operation, Phase, Refusal, Revocation, Scope, Session,
};

fn epoch(n: u64) -> Epoch {
    Epoch::new(n).expect("a non-zero epoch")
}

fn output(id: u64) -> Scope {
    Scope::Output(Handle { id, generation: 1 })
}

fn grant(scope: Scope, ops: &[Operation]) -> Grant {
    Grant {
        scope,
        capabilities: Capabilities::of(ops.iter().copied()),
    }
}

fn active(ops: &[Operation]) -> Session {
    let mut s = Session::new(epoch(1));
    let g = grant(output(1), ops);
    s.request(epoch(1), g).unwrap();
    s.approve(epoch(1), g).unwrap();
    s
}

#[test]
fn epoch_zero_is_not_an_identity() {
    assert!(Epoch::new(0).is_none());
    assert_eq!(epoch(7).get(), 7);
}

#[test]
fn capabilities_hold_exactly_the_listed_operations() {
    let caps = Capabilities::of([Operation::Observe, Operation::Keyboard]);
    assert!(caps.allows(Operation::Observe));
    assert!(caps.allows(Operation::Keyboard));
    assert!(!caps.allows(Operation::Capture));
    assert!(!caps.allows(Operation::Pointer));
    assert!(!caps.allows(Operation::Semantic));
    assert!(!caps.is_empty());
    assert!(Capabilities::NONE.is_empty());
    assert!(Capabilities::of([]).is_empty());
}

#[test]
fn a_new_session_is_inactive_and_authorises_nothing() {
    let s = Session::new(epoch(1));
    assert_eq!(s.phase(), Phase::Inactive);
    assert_eq!(s.revocation(), None);
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Observe),
        Err(Refusal::NotGranted)
    );
}

#[test]
fn consent_then_authorisation() {
    let mut s = Session::new(epoch(1));
    let g = grant(output(1), &[Operation::Observe, Operation::Pointer]);
    s.request(epoch(1), g).unwrap();
    assert_eq!(s.phase(), Phase::AwaitingConsent);
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Observe),
        Err(Refusal::NotGranted),
        "a request is not a grant"
    );
    s.approve(epoch(1), g).unwrap();
    assert_eq!(s.phase(), Phase::Active);
    assert_eq!(s.authorize(epoch(1), output(1), Operation::Pointer), Ok(()));
}

#[test]
fn an_empty_request_is_refused_and_leaves_the_session_inactive() {
    let mut s = Session::new(epoch(1));
    assert_eq!(
        s.request(epoch(1), grant(output(1), &[])),
        Err(Refusal::EmptyCapabilities)
    );
    assert_eq!(s.phase(), Phase::Inactive);
}

#[test]
fn approval_may_narrow_but_never_widen() {
    let mut s = Session::new(epoch(1));
    s.request(
        epoch(1),
        grant(output(1), &[Operation::Observe, Operation::Capture]),
    )
    .unwrap();
    assert_eq!(
        s.approve(
            epoch(1),
            grant(output(1), &[Operation::Observe, Operation::Keyboard])
        ),
        Err(Refusal::GrantEscalation)
    );
    assert_eq!(
        s.approve(epoch(1), grant(output(2), &[Operation::Observe])),
        Err(Refusal::GrantEscalation),
        "a different target is an escalation"
    );
    assert_eq!(
        s.approve(epoch(1), grant(output(1), &[])),
        Err(Refusal::EmptyCapabilities)
    );
    assert_eq!(s.phase(), Phase::AwaitingConsent, "refusals change nothing");
    s.approve(epoch(1), grant(output(1), &[Operation::Observe]))
        .unwrap();
    assert_eq!(s.authorize(epoch(1), output(1), Operation::Observe), Ok(()));
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Capture),
        Err(Refusal::CapabilityDenied)
    );
}

#[test]
fn a_window_grant_does_not_cover_an_output_of_the_same_id() {
    let window = Scope::Window(Handle {
        id: 1,
        generation: 1,
    });
    let mut s = Session::new(epoch(1));
    let g = grant(window, &[Operation::Semantic]);
    s.request(epoch(1), g).unwrap();
    s.approve(epoch(1), g).unwrap();
    assert_eq!(s.authorize(epoch(1), window, Operation::Semantic), Ok(()));
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Semantic),
        Err(Refusal::OutsideScope)
    );
}

#[test]
fn a_reused_handle_id_with_another_generation_is_outside_the_scope() {
    let s = active(&[Operation::Observe]);
    let reused = Scope::Output(Handle {
        id: 1,
        generation: 2,
    });
    assert_eq!(
        s.authorize(epoch(1), reused, Operation::Observe),
        Err(Refusal::OutsideScope)
    );
}

#[test]
fn denial_returns_to_inactive_and_allows_a_new_request() {
    let mut s = Session::new(epoch(1));
    let g = grant(output(1), &[Operation::Observe]);
    s.request(epoch(1), g).unwrap();
    s.deny(epoch(1)).unwrap();
    assert_eq!(s.phase(), Phase::Inactive);
    s.request(epoch(1), g).unwrap();
    assert_eq!(s.phase(), Phase::AwaitingConsent);
}

#[test]
fn transitions_out_of_order_are_invalid() {
    let g = grant(output(1), &[Operation::Observe]);
    let mut s = Session::new(epoch(1));
    assert_eq!(s.approve(epoch(1), g), Err(Refusal::InvalidPhase));
    assert_eq!(s.deny(epoch(1)), Err(Refusal::InvalidPhase));
    assert_eq!(s.suspend(epoch(1)), Err(Refusal::InvalidPhase));
    assert_eq!(s.resume(epoch(1)), Err(Refusal::InvalidPhase));
    s.request(epoch(1), g).unwrap();
    assert_eq!(s.request(epoch(1), g), Err(Refusal::InvalidPhase));
    assert_eq!(s.suspend(epoch(1)), Err(Refusal::InvalidPhase));
    s.approve(epoch(1), g).unwrap();
    assert_eq!(s.request(epoch(1), g), Err(Refusal::InvalidPhase));
    assert_eq!(s.resume(epoch(1)), Err(Refusal::InvalidPhase));
}

#[test]
fn every_call_with_another_epoch_is_stale() {
    let g = grant(output(1), &[Operation::Observe]);
    let mut s = Session::new(epoch(1));
    assert_eq!(s.request(epoch(2), g), Err(Refusal::StaleEpoch));
    s.request(epoch(1), g).unwrap();
    assert_eq!(s.approve(epoch(2), g), Err(Refusal::StaleEpoch));
    assert_eq!(s.deny(epoch(2)), Err(Refusal::StaleEpoch));
    s.approve(epoch(1), g).unwrap();
    assert_eq!(s.suspend(epoch(2)), Err(Refusal::StaleEpoch));
    assert_eq!(
        s.authorize(epoch(2), output(1), Operation::Observe),
        Err(Refusal::StaleEpoch)
    );
    assert_eq!(s.phase(), Phase::Active);
}

#[test]
fn suspension_blocks_operations_until_resumed() {
    let mut s = active(&[Operation::Keyboard]);
    s.suspend(epoch(1)).unwrap();
    assert_eq!(s.phase(), Phase::Suspended);
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Keyboard),
        Err(Refusal::Suspended)
    );
    s.resume(epoch(1)).unwrap();
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Keyboard),
        Ok(())
    );
}

#[test]
fn revocation_is_terminal_and_keeps_the_first_reason() {
    let mut s = active(&[Operation::Observe]);
    s.revoke(Revocation::Locked);
    assert_eq!(s.phase(), Phase::Revoked);
    assert_eq!(s.revocation(), Some(Revocation::Locked));
    s.revoke(Revocation::Stopped);
    assert_eq!(s.revocation(), Some(Revocation::Locked));
    let g = grant(output(1), &[Operation::Observe]);
    assert_eq!(s.request(epoch(1), g), Err(Refusal::Revoked));
    assert_eq!(s.approve(epoch(1), g), Err(Refusal::Revoked));
    assert_eq!(s.resume(epoch(1)), Err(Refusal::Revoked));
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Observe),
        Err(Refusal::Revoked)
    );
}

#[test]
fn revoking_while_consent_is_pending_cancels_it() {
    let mut s = Session::new(epoch(1));
    let g = grant(output(1), &[Operation::Observe]);
    s.request(epoch(1), g).unwrap();
    s.revoke(Revocation::AuthorityDisconnected);
    assert_eq!(s.phase(), Phase::Revoked);
    assert_eq!(s.approve(epoch(1), g), Err(Refusal::Revoked));
}

#[test]
fn a_stale_epoch_is_reported_before_revocation() {
    let mut s = active(&[Operation::Observe]);
    s.revoke(Revocation::Withdrawn);
    assert_eq!(s.suspend(epoch(9)), Err(Refusal::StaleEpoch));
}
