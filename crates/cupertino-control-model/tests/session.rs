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

fn active(scope: Scope, ops: &[Operation]) -> Session {
    let mut s = Session::new(epoch(1));
    let g = grant(scope, ops);
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
    use Operation::*;
    let caps = Capabilities::of([Observe, Keyboard]);
    for (op, allowed) in [
        (Observe, true),
        (Keyboard, true),
        (Capture, false),
        (Pointer, false),
        (Semantic, false),
    ] {
        assert_eq!(caps.allows(op), allowed, "{op:?}");
    }
    assert!(!caps.is_empty());
    assert!(Capabilities::NONE.is_empty() && Capabilities::of([]).is_empty());
}

#[test]
fn operations_are_authorised_only_after_consent_and_again_after_a_denial() {
    let g = grant(output(1), &[Operation::Observe, Operation::Pointer]);
    let mut s = Session::new(epoch(1));
    assert_eq!((s.phase(), s.revocation()), (Phase::Inactive, None));
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Observe),
        Err(Refusal::NotGranted)
    );
    s.request(epoch(1), g).unwrap();
    assert_eq!(s.phase(), Phase::AwaitingConsent);
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Observe),
        Err(Refusal::NotGranted),
        "a request is not a grant"
    );
    s.deny(epoch(1)).unwrap();
    assert_eq!(s.phase(), Phase::Inactive);
    s.request(epoch(1), g).unwrap();
    s.approve(epoch(1), g).unwrap();
    assert_eq!(s.phase(), Phase::Active);
    assert_eq!(s.authorize(epoch(1), output(1), Operation::Pointer), Ok(()));
}

#[test]
fn a_grant_is_never_empty_and_approval_may_narrow_but_never_widen_it() {
    let mut s = Session::new(epoch(1));
    assert_eq!(
        s.request(epoch(1), grant(output(1), &[])),
        Err(Refusal::EmptyCapabilities)
    );
    assert_eq!(s.phase(), Phase::Inactive);
    s.request(
        epoch(1),
        grant(output(1), &[Operation::Observe, Operation::Capture]),
    )
    .unwrap();
    for (name, approval, refusal) in [
        (
            "another operation",
            grant(output(1), &[Operation::Observe, Operation::Keyboard]),
            Refusal::GrantEscalation,
        ),
        (
            "another target",
            grant(output(2), &[Operation::Observe]),
            Refusal::GrantEscalation,
        ),
        ("nothing", grant(output(1), &[]), Refusal::EmptyCapabilities),
    ] {
        assert_eq!(s.approve(epoch(1), approval), Err(refusal), "{name}");
        assert_eq!(s.phase(), Phase::AwaitingConsent, "{name} changes nothing");
    }
    s.approve(epoch(1), grant(output(1), &[Operation::Observe]))
        .unwrap();
    assert_eq!(s.authorize(epoch(1), output(1), Operation::Observe), Ok(()));
    assert_eq!(
        s.authorize(epoch(1), output(1), Operation::Capture),
        Err(Refusal::CapabilityDenied)
    );
}

#[test]
fn a_grant_covers_only_its_own_scope() {
    let window = Scope::Window(Handle {
        id: 1,
        generation: 1,
    });
    let reused = Scope::Output(Handle {
        id: 1,
        generation: 2,
    });
    for (name, granted, asked, expected) in [
        ("the window granted", window, window, Ok(())),
        (
            "an output with the window's id",
            window,
            output(1),
            Err(Refusal::OutsideScope),
        ),
        (
            "a reused handle id of another generation",
            output(1),
            reused,
            Err(Refusal::OutsideScope),
        ),
    ] {
        let s = active(granted, &[Operation::Semantic]);
        assert_eq!(
            s.authorize(epoch(1), asked, Operation::Semantic),
            expected,
            "{name}"
        );
    }
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
fn every_call_with_another_epoch_is_stale_even_after_revocation() {
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
    s.revoke(Revocation::Withdrawn);
    assert_eq!(s.suspend(epoch(9)), Err(Refusal::StaleEpoch));
}

#[test]
fn suspension_blocks_operations_until_resumed() {
    let mut s = active(output(1), &[Operation::Keyboard]);
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
fn revocation_is_terminal_from_any_phase_and_keeps_the_first_reason() {
    let g = grant(output(1), &[Operation::Observe]);
    let mut pending = Session::new(epoch(1));
    pending.request(epoch(1), g).unwrap();
    for (name, mut s) in [
        ("active", active(output(1), &[Operation::Observe])),
        ("awaiting consent", pending),
    ] {
        s.revoke(Revocation::Locked);
        s.revoke(Revocation::Stopped);
        assert_eq!(s.phase(), Phase::Revoked, "{name}");
        assert_eq!(s.revocation(), Some(Revocation::Locked), "{name}");
        assert_eq!(s.request(epoch(1), g), Err(Refusal::Revoked), "{name}");
        assert_eq!(s.approve(epoch(1), g), Err(Refusal::Revoked), "{name}");
        assert_eq!(s.resume(epoch(1)), Err(Refusal::Revoked), "{name}");
        assert_eq!(
            s.authorize(epoch(1), output(1), Operation::Observe),
            Err(Refusal::Revoked),
            "{name}"
        );
    }
}
