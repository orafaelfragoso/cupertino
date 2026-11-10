use cupertino_control_model::{
    Capabilities, Epoch, Grant, Handle, Input, InputOwners, InputSource, Operation, OwnershipError,
    Phase, Refusal, Revocation, Scope, Session, Transition, MAX_HELD_INPUTS,
};

fn epoch(value: u64) -> Epoch {
    Epoch::new(value).unwrap()
}

fn scope() -> Scope {
    Scope::Window(Handle {
        id: 4,
        generation: 2,
    })
}

fn grant() -> Grant {
    Grant {
        scope: scope(),
        capabilities: Capabilities::of([Operation::Observe, Operation::Pointer]),
    }
}

fn active() -> Session {
    let mut session = Session::new(epoch(1));
    session.request(epoch(1), grant()).unwrap();
    session.approve(epoch(1), grant()).unwrap();
    session
}

#[test]
fn discovery_and_a_pending_consent_request_grant_nothing() {
    assert_eq!(Epoch::new(0), None);
    let mut session = Session::new(epoch(1));
    assert_eq!(session.phase(), Phase::Inactive);
    assert_eq!(
        session.authorize(epoch(1), scope(), Operation::Observe),
        Err(Refusal::NotGranted)
    );
    assert_eq!(
        session.approve(epoch(1), grant()),
        Err(Refusal::InvalidPhase)
    );
    session.request(epoch(1), grant()).unwrap();
    for operation in [Operation::Observe, Operation::Capture, Operation::Pointer] {
        assert_eq!(
            session.authorize(epoch(1), scope(), operation),
            Err(Refusal::NotGranted)
        );
    }
    session.deny(epoch(1)).unwrap();
    assert_eq!(session.phase(), Phase::Inactive);
    assert_eq!(
        session.approve(epoch(1), grant()),
        Err(Refusal::InvalidPhase)
    );
}

#[test]
fn consent_can_narrow_but_cannot_expand_capabilities_or_change_the_target() {
    let mut session = Session::new(epoch(1));
    session.request(epoch(1), grant()).unwrap();
    let expanded = Grant {
        capabilities: Capabilities::of([Operation::Observe, Operation::Keyboard]),
        ..grant()
    };
    assert_eq!(
        session.approve(epoch(1), expanded),
        Err(Refusal::GrantEscalation)
    );
    let another = Grant {
        scope: Scope::Output(Handle {
            id: 4,
            generation: 2,
        }),
        ..grant()
    };
    assert_eq!(
        session.approve(epoch(1), another),
        Err(Refusal::GrantEscalation)
    );
    assert_eq!(session.phase(), Phase::AwaitingConsent);
    let narrowed = Grant {
        capabilities: Capabilities::of([Operation::Observe]),
        ..grant()
    };
    session.approve(epoch(1), narrowed).unwrap();
    assert_eq!(
        session.authorize(epoch(1), scope(), Operation::Observe),
        Ok(())
    );
    assert_eq!(
        session.authorize(epoch(1), scope(), Operation::Pointer),
        Err(Refusal::CapabilityDenied)
    );
    assert_eq!(
        session.request(epoch(1), expanded),
        Err(Refusal::InvalidPhase)
    );
}

#[test]
fn an_empty_request_or_approval_does_not_advance_consent() {
    let empty = Grant {
        capabilities: Capabilities::NONE,
        ..grant()
    };
    let mut session = Session::new(epoch(1));
    assert_eq!(
        session.request(epoch(1), empty),
        Err(Refusal::EmptyCapabilities)
    );
    assert_eq!(session.phase(), Phase::Inactive);
    session.request(epoch(1), grant()).unwrap();
    assert_eq!(
        session.approve(epoch(1), empty),
        Err(Refusal::EmptyCapabilities)
    );
    assert_eq!(session.phase(), Phase::AwaitingConsent);
}

#[test]
fn stale_epochs_and_reused_window_identifiers_cannot_act() {
    let session = active();
    assert_eq!(
        session.authorize(epoch(2), scope(), Operation::Observe),
        Err(Refusal::StaleEpoch)
    );
    for target in [
        Scope::Window(Handle {
            id: 4,
            generation: 3,
        }),
        Scope::Window(Handle {
            id: 5,
            generation: 2,
        }),
        Scope::Output(Handle {
            id: 4,
            generation: 2,
        }),
    ] {
        assert_eq!(
            session.authorize(epoch(1), target, Operation::Observe),
            Err(Refusal::OutsideScope)
        );
    }
}

#[test]
fn suspension_stops_reads_and_input_until_an_explicit_resume() {
    let mut session = active();
    session.suspend(epoch(1)).unwrap();
    for operation in [Operation::Observe, Operation::Pointer] {
        assert_eq!(
            session.authorize(epoch(1), scope(), operation),
            Err(Refusal::Suspended)
        );
    }
    assert_eq!(session.resume(epoch(2)), Err(Refusal::StaleEpoch));
    assert_eq!(session.phase(), Phase::Suspended);
    session.resume(epoch(1)).unwrap();
    assert_eq!(
        session.authorize(epoch(1), scope(), Operation::Pointer),
        Ok(())
    );
}

#[test]
fn lock_disconnect_and_stop_are_terminal_in_every_phase() {
    for reason in [
        Revocation::Locked,
        Revocation::SessionDeactivated,
        Revocation::ControllerDisconnected,
        Revocation::AuthorityDisconnected,
        Revocation::Stopped,
    ] {
        for phase in [
            Phase::Inactive,
            Phase::AwaitingConsent,
            Phase::Active,
            Phase::Suspended,
        ] {
            let mut session = Session::new(epoch(1));
            if phase != Phase::Inactive {
                session.request(epoch(1), grant()).unwrap();
            }
            if matches!(phase, Phase::Active | Phase::Suspended) {
                session.approve(epoch(1), grant()).unwrap();
            }
            if phase == Phase::Suspended {
                session.suspend(epoch(1)).unwrap();
            }
            session.revoke(reason);
            assert_eq!(session.revocation(), Some(reason));
            session.revoke(Revocation::Stopped);
            assert_eq!(
                session.revocation(),
                Some(reason),
                "keep the original cause"
            );
            assert_eq!(session.phase(), Phase::Revoked);
            assert_eq!(session.request(epoch(1), grant()), Err(Refusal::Revoked));
            assert_eq!(session.approve(epoch(1), grant()), Err(Refusal::Revoked));
            assert_eq!(session.resume(epoch(1)), Err(Refusal::Revoked));
            assert_eq!(
                session.authorize(epoch(1), scope(), Operation::Observe),
                Err(Refusal::Revoked)
            );
        }
    }
}

#[test]
fn controller_disconnect_preserves_physical_keys_and_buttons() {
    let physical = InputSource::Physical(1);
    let controller = InputSource::Controller(epoch(1));
    let mut owners = InputOwners::default();
    for input in [Input::Key(42), Input::Button(272)] {
        assert_eq!(
            owners.set(physical, input, true),
            Ok(Some(Transition::Press(input)))
        );
        assert_eq!(owners.set(controller, input, true), Ok(None));
    }
    assert_eq!(owners.release_source(controller), vec![]);
    assert_eq!(owners.held_count(), 2);
    for input in [Input::Key(42), Input::Button(272)] {
        assert_eq!(
            owners.set(physical, input, false),
            Ok(Some(Transition::Release(input)))
        );
    }
    assert_eq!(owners.held_count(), 0);
}

#[test]
fn another_source_cannot_release_a_press_and_cleanup_is_idempotent() {
    let a = InputSource::Controller(epoch(1));
    let b = InputSource::Controller(epoch(2));
    let mut owners = InputOwners::default();
    let key = Input::Key(30);
    assert_eq!(owners.set(a, key, true), Ok(Some(Transition::Press(key))));
    assert_eq!(owners.set(a, key, true), Ok(None));
    assert_eq!(owners.set(b, key, false), Ok(None));
    assert_eq!(owners.held_count(), 1);
    assert_eq!(owners.release_source(a), vec![Transition::Release(key)]);
    assert_eq!(owners.release_source(a), vec![]);
}

#[test]
fn physical_disconnect_also_preserves_the_controllers_held_key() {
    let physical = InputSource::Physical(1);
    let controller = InputSource::Controller(epoch(1));
    let mut owners = InputOwners::default();
    let key = Input::Key(30);
    owners.set(controller, key, true).unwrap();
    owners.set(physical, key, true).unwrap();
    assert_eq!(owners.release_source(physical), vec![]);
    assert_eq!(
        owners.release_source(controller),
        vec![Transition::Release(key)]
    );
}

#[test]
fn ownership_limit_never_prevents_release_or_inserts_a_partial_press() {
    let mut owners = InputOwners::default();
    let key = Input::Key(30);
    for source in 0..MAX_HELD_INPUTS as u64 {
        owners
            .set(InputSource::Physical(source), key, true)
            .unwrap();
    }
    let controller = InputSource::Controller(epoch(1));
    assert_eq!(owners.set(controller, key, true), Err(OwnershipError::Full));
    assert_eq!(owners.held_count(), MAX_HELD_INPUTS);
    assert_eq!(owners.release_source(controller), vec![]);
    assert_eq!(owners.set(InputSource::Physical(0), key, true), Ok(None));
    assert_eq!(owners.set(InputSource::Physical(0), key, false), Ok(None));
    assert_eq!(owners.set(controller, key, true), Ok(None));
    assert_eq!(owners.held_count(), MAX_HELD_INPUTS);
}
