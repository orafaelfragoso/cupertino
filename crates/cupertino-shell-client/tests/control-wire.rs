//! Desktop control and consent over a real wire: binding by version, run epochs, the events of a run and of the
//! authority, and the requests with the epoch in every one.

use crate::support;

use cupertino_app_protocols::semantics::{
    self, Action, ActionReply, Data, Invocation, Observation, Outcome, OutputGeometry,
};
use cupertino_protocols::server::control::cupertino_control_authority_v1::{
    self as authority_wire, Request as AuthorityRequest,
};
use cupertino_protocols::server::control::cupertino_control_session_v1::{
    KeyState, Phase, Refusal, Request,
};
use cupertino_shell_client::{
    Anchor, ConsentAnswer, ConsentApp, ControlAuthority, ControlEvent, ControlSession,
    DesktopControl, Error, KeyboardInteractivity, Layer, LayerShell, LayerSurfaceConfig, Margin,
};
use cupertino_wayland_client::OutputId;
use std::os::fd::AsFd;
use support::{extension, pixel_file, Offer, Seen, Wire};

fn control_requests(wire: &Wire) -> Vec<Request> {
    wire.take()
        .into_iter()
        .filter_map(|s| match s {
            Seen::ControlSession(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn authority_requests(wire: &Wire) -> Vec<AuthorityRequest> {
    wire.take()
        .into_iter()
        .filter_map(|s| match s {
            Seen::Authority(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn control_events(wire: &mut Wire) -> Vec<ControlEvent> {
    extension::<ControlEvent>(&wire.events())
}

/// A bound control with a session whose run the server created as epoch `(hi, lo)`.
fn run(offer: Offer, hi: u32, lo: u32) -> (Wire, DesktopControl, ControlSession) {
    let mut wire = Wire::new(offer);
    let control = DesktopControl::bind(&mut wire.conn).expect("control");
    let session = control.session(&wire.conn);
    wire.events();
    wire.server(move |s| s.control_sessions[0].created(hi, lo, 31));
    control_events(&mut wire);
    wire.take();
    (wire, control, session)
}

fn observation() -> Observation {
    Observation {
        geometry: OutputGeometry {
            revision: 3,
            name: "DP-1".into(),
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
            scale: 1.0,
            transform: "normal".into(),
        },
        epoch: 5,
        revision: 9,
        geometry_revision: 3,
        windows: vec![],
    }
}

fn set_value(text: String) -> Invocation {
    Invocation {
        window: 3,
        revision: 9,
        node: 12,
        action: Action::SetValue,
        data: Some(Data::Text(text)),
    }
}

#[test]
fn the_control_and_the_authority_bind_only_at_the_versions_they_speak() {
    for (offered, bound) in [
        (None, None),
        (Some(2), None),
        (Some(3), Some(3)),
        (Some(4), Some(4)),
        (Some(5), Some(5)),
    ] {
        let mut wire = Wire::new(Offer {
            control: offered,
            ..Offer::all()
        });
        assert_eq!(
            DesktopControl::bind(&mut wire.conn).map(|c| c.version()),
            bound,
            "control offered at {offered:?}"
        );
    }
    for (offered, binds) in [(None, false), (Some(1), true), (Some(2), true)] {
        let mut wire = Wire::new(Offer {
            authority: offered,
            ..Offer::all()
        });
        assert_eq!(
            ControlAuthority::bind(&mut wire.conn).is_some(),
            binds,
            "authority offered at {offered:?}"
        );
    }
}

#[test]
fn each_session_has_no_epoch_until_the_server_creates_its_run() {
    let mut wire = Wire::all();
    let control = DesktopControl::bind(&mut wire.conn).expect("control");
    let first = control.session(&wire.conn);
    let second = control.session(&wire.conn);
    wire.events();
    assert_eq!((first.epoch(), second.epoch()), (0, 0));

    wire.server(|s| {
        s.control_sessions[1].created(0, 8, 1);
        s.control_sessions[0].created(0x1, 0x2, 0b10011);
    });
    assert_eq!(
        control_events(&mut wire),
        [
            ControlEvent::Created {
                epoch: 8,
                available: 1
            },
            ControlEvent::Created {
                epoch: 0x1_0000_0002,
                available: 0b10011
            }
        ]
    );
    assert_eq!(first.epoch(), 0x1_0000_0002);
    assert_eq!(second.epoch(), 8);
}

#[test]
fn the_events_of_a_run_carry_its_epoch_and_their_wire_numbers() {
    let (mut wire, _control, _session) = run(Offer::all(), 2, 9);
    let epoch = (2u64 << 32) | 9;
    wire.server(|s| {
        let session = &s.control_sessions[0];
        session.status(4, Phase::Active);
        session.geometry(3, -10, 20, 1920, 1080, 1.5, "DP-1".into());
        session.completed(5);
        session.refused(6, Refusal::StaleGeometry);
        session.semantic_changed(1, 2);
    });
    assert_eq!(
        control_events(&mut wire),
        [
            ControlEvent::Status {
                epoch,
                serial: 4,
                phase: 2
            },
            ControlEvent::Geometry {
                epoch,
                revision: 3,
                x: -10,
                y: 20,
                width: 1920,
                height: 1080,
                scale: 1.5,
                name: "DP-1".into()
            },
            ControlEvent::Completed { epoch, serial: 5 },
            ControlEvent::Refused {
                epoch,
                serial: 6,
                reason: 3
            },
            ControlEvent::SemanticChanged {
                epoch,
                revision: (1 << 32) | 2
            },
        ]
    );

    wire.server(|s| {
        let session = &s.control_sessions[0];
        for phase in [
            Phase::Inactive,
            Phase::AwaitingConsent,
            Phase::Active,
            Phase::Suspended,
            Phase::Revoked,
        ] {
            session.status(1, phase);
        }
        for reason in [
            Refusal::StaleEpoch,
            Refusal::NotGranted,
            Refusal::Denied,
            Refusal::StaleGeometry,
            Refusal::OutsideScope,
            Refusal::Unsupported,
            Refusal::Blocked,
            Refusal::NoAuthority,
            Refusal::Busy,
            Refusal::InvalidInput,
            Refusal::UnknownOutcome,
            Refusal::Unidentified,
        ] {
            session.refused(1, reason);
        }
    });
    let events = control_events(&mut wire);
    let phases: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            ControlEvent::Status { phase, .. } => Some(*phase),
            _ => None,
        })
        .collect();
    let reasons: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            ControlEvent::Refused { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect();
    assert_eq!(phases, [0, 1, 2, 3, 4]);
    assert_eq!(reasons, (0..=11).collect::<Vec<u32>>());
}

#[test]
fn a_payload_is_read_only_from_a_sealed_file_of_its_length() {
    let (mut wire, _control, _session) = run(Offer::all(), 0, 5);
    let reply = ActionReply {
        id: 2,
        epoch: 5,
        revision: 9,
        outcome: Outcome::Applied,
    };
    let sent = reply.clone();
    wire.server(move |s| {
        let session = &s.control_sessions[0];
        let (file, len) = semantics::seal(&observation(), semantics::MAX_OBSERVATION).unwrap();
        session.observation(11, file.as_fd(), len);
        let (file, len) = semantics::seal(&sent, semantics::MAX_FRAME).unwrap();
        session.action_finished(12, file.as_fd(), len);

        // a plain file is not sealed, so it is refused whatever it holds
        let json = br#"{"id":1,"epoch":5,"revision":9,"outcome":"applied"}"#;
        let file = pixel_file(json);
        session.observation(1, file.as_fd(), json.len() as u32);
        // a sealed file whose length differs from the one sent
        let (file, len) = semantics::seal(&observation(), semantics::MAX_OBSERVATION).unwrap();
        session.observation(2, file.as_fd(), len + 1);
        // an action reply announced longer than a frame
        let (file, len) = semantics::seal(&observation(), semantics::MAX_OBSERVATION).unwrap();
        session.action_finished(3, file.as_fd(), len.max(semantics::MAX_FRAME as u32 + 1));
        session.completed(4);
    });
    assert_eq!(
        control_events(&mut wire),
        [
            ControlEvent::Observation {
                epoch: 5,
                serial: 11,
                data: observation()
            },
            ControlEvent::ActionFinished {
                epoch: 5,
                serial: 12,
                reply
            },
            ControlEvent::Completed {
                epoch: 5,
                serial: 4
            },
        ]
    );
}

#[test]
fn requests_name_the_run_in_every_message() {
    let (mut wire, _control, session) = run(Offer::all(), 0xAB, 0xCD);
    let output = wire.conn.outputs()[0].id;

    session.start(&wire.conn, 1, output, 0b10101).unwrap();
    session.motion(2, 7, 10.5, -3.25);
    session.button(3, 7, 272, true);
    session.button(4, 7, 272, false);
    session.key(5, 7, 30, true);
    session.key(6, 7, 30, false);
    session.axis(7, 7, 0.0, 15.0);
    session.resume(8);
    session.observe(9);
    session.stop();
    wire.events();

    let requests = control_requests(&wire);
    let epoch = |hi: &u32, lo: &u32| (*hi, *lo) == (0xAB, 0xCD);
    assert!(matches!(
        &requests[0],
        Request::Start { serial: 1, epoch_hi, epoch_lo, capabilities, .. }
            if epoch(epoch_hi, epoch_lo) && u32::from(*capabilities) == 0b10101
    ));
    assert!(matches!(
        &requests[1],
        Request::Motion { serial: 2, epoch_hi, epoch_lo, revision: 7, x, y }
            if epoch(epoch_hi, epoch_lo) && *x == 10.5 && *y == -3.25
    ));
    assert!(matches!(
        &requests[2],
        Request::Button { serial: 3, epoch_hi, epoch_lo, revision: 7, code: 272, state }
            if epoch(epoch_hi, epoch_lo) && state.into_result().ok() == Some(KeyState::Pressed)
    ));
    assert!(matches!(
        &requests[3],
        Request::Button { serial: 4, state, .. } if state.into_result().ok() == Some(KeyState::Released)
    ));
    assert!(matches!(
        &requests[4],
        Request::Key { serial: 5, epoch_hi, epoch_lo, revision: 7, code: 30, state }
            if epoch(epoch_hi, epoch_lo) && state.into_result().ok() == Some(KeyState::Pressed)
    ));
    assert!(matches!(
        &requests[5],
        Request::Key { serial: 6, state, .. } if state.into_result().ok() == Some(KeyState::Released)
    ));
    assert!(matches!(
        &requests[6],
        Request::Axis { serial: 7, epoch_hi, epoch_lo, revision: 7, horizontal, vertical }
            if epoch(epoch_hi, epoch_lo) && *horizontal == 0.0 && *vertical == 15.0
    ));
    assert!(matches!(
        &requests[7],
        Request::Resume { serial: 8, epoch_hi, epoch_lo } if epoch(epoch_hi, epoch_lo)
    ));
    assert!(matches!(
        &requests[8],
        Request::Observe { serial: 9, epoch_hi, epoch_lo } if epoch(epoch_hi, epoch_lo)
    ));
    assert!(matches!(requests[9], Request::Stop));
    assert_eq!(requests.len(), 10);
}

#[test]
fn an_action_is_sent_as_a_sealed_file_holding_the_call() {
    let (mut wire, _control, session) = run(Offer::all(), 0, 4);
    let call = set_value("hello".into());
    session.perform(77, &call).expect("perform");
    wire.events();

    let mut requests = control_requests(&wire);
    let Request::Perform {
        serial,
        epoch_lo,
        payload,
        length,
        ..
    } = requests.remove(0)
    else {
        panic!("not a perform");
    };
    assert_eq!((serial, epoch_lo), (77, 4));
    let read: Invocation = semantics::read(payload, length, semantics::MAX_FRAME).expect("sealed");
    assert_eq!(read, call);
}

#[test]
fn a_request_the_server_would_refuse_is_refused_here_and_sends_nothing() {
    let (mut wire, _control, session) = run(Offer::all(), 0, 1);
    let unplugged = session.start(&wire.conn, 1, OutputId::from_registry_name(9999), 1);
    assert!(
        matches!(unplugged, Err(Error::UnknownOutput)),
        "a run on an unplugged output"
    );
    let too_long = set_value("x".repeat(semantics::MAX_FRAME + 1));
    assert!(
        session.perform(1, &too_long).is_err(),
        "an action over the frame limit"
    );
    wire.events();
    assert!(control_requests(&wire).is_empty());
}

#[test]
fn naming_the_app_needs_version_four_and_selecting_the_capture_version_five() {
    for (version, names, selects) in [(3, false, false), (4, true, false), (5, true, true)] {
        let (mut wire, _control, session) = run(
            Offer {
                control: Some(version),
                ..Offer::all()
            },
            1,
            2,
        );
        assert_eq!(session.names_apps(), names, "version {version}");
        if names {
            session.identify(4242, (7u64 << 32) | 9);
        }
        session.select_capture();
        wire.events();
        let requests = control_requests(&wire);
        let identified = requests.iter().any(|r| {
            matches!(
                r,
                Request::Identify {
                    pid: 4242,
                    start_hi: 7,
                    start_lo: 9
                }
            )
        });
        let selected = requests.iter().any(|r| matches!(r, Request::SelectCapture));
        assert_eq!(identified, names, "version {version}: identify");
        assert_eq!(selected, selects, "version {version}: select capture");
    }
}

#[test]
fn dropping_a_session_the_control_or_the_authority_destroys_it() {
    let (mut wire, control, session) = run(Offer::all(), 1, 2);
    drop(session);
    wire.events();
    assert!(matches!(control_requests(&wire)[..], [Request::Destroy]));
    drop(control);
    wire.events();
    assert!(wire.take().iter().any(|s| matches!(s, Seen::Control(_))));

    let authority = ControlAuthority::bind(&mut wire.conn).expect("authority");
    wire.events();
    wire.take();
    drop(authority);
    wire.events();
    assert!(matches!(
        authority_requests(&wire)[..],
        [AuthorityRequest::Destroy]
    ));
}

#[test]
fn a_consent_request_names_its_run_output_and_app() {
    let mut wire = Wire::all();
    let _authority = ControlAuthority::bind(&mut wire.conn).expect("authority");
    wire.events();
    wire.server(|s| {
        let authority = &s.authorities[0];
        authority.app(
            0,
            9,
            authority_wire::AppKind::Desktop,
            "org.example.Agent.desktop".into(),
            "Agent".into(),
            3,
            1,
        );
        authority.requested(0, 9, 0b11, "DP-1".into(), 1);
        authority.app(
            0,
            10,
            authority_wire::AppKind::Executable,
            "/usr/bin/agent".into(),
            "agent".into(),
            1,
            0,
        );
        authority.changed(0, 9, 2);
    });
    assert_eq!(
        control_events(&mut wire),
        [
            ControlEvent::ConsentApp {
                epoch: 9,
                app: ConsentApp {
                    identity: "org.example.Agent.desktop".into(),
                    desktop: true,
                    name: "Agent".into(),
                    services: 3,
                    remembered: true,
                }
            },
            ControlEvent::ConsentRequested {
                epoch: 9,
                capabilities: 0b11,
                output: "DP-1".into(),
                resuming: true,
            },
            ControlEvent::ConsentApp {
                epoch: 10,
                app: ConsentApp {
                    identity: "/usr/bin/agent".into(),
                    desktop: false,
                    name: "agent".into(),
                    services: 1,
                    remembered: false,
                }
            },
            ControlEvent::ConsentChanged { epoch: 9, phase: 2 },
        ]
    );
}

#[test]
fn an_answer_is_remembered_only_where_the_version_knows_it() {
    for (version, allow, deny, dismiss) in [(1, 1, 0, 0), (2, 1, 2, 0)] {
        let mut wire = Wire::new(Offer {
            authority: Some(version),
            ..Offer::all()
        });
        let authority = ControlAuthority::bind(&mut wire.conn).expect("authority");
        wire.events();
        wire.take();
        let epoch = (3u64 << 32) | 4;
        authority.answer(epoch, ConsentAnswer::Allow, 100);
        authority.answer(epoch, ConsentAnswer::Deny, 101);
        authority.answer(epoch, ConsentAnswer::Dismiss, 102);
        authority.stop(epoch);
        wire.events();
        let requests = authority_requests(&wire);
        let expected = [(allow, 100), (deny, 101), (dismiss, 102)];
        for (request, (allow, serial)) in requests.iter().zip(expected) {
            assert!(
                matches!(
                    request,
                    AuthorityRequest::Answer { epoch_hi: 3, epoch_lo: 4, allow: a, serial: s }
                        if *a == allow && *s == serial
                ),
                "version {version}: {request:?}"
            );
        }
        assert!(matches!(
            requests[3],
            AuthorityRequest::Stop {
                epoch_hi: 3,
                epoch_lo: 4
            }
        ));
        assert_eq!(requests.len(), 4);
    }
}

#[test]
fn the_consent_panel_is_attached_by_its_surface() {
    let mut wire = Wire::all();
    let authority = ControlAuthority::bind(&mut wire.conn).expect("authority");
    let layers = LayerShell::bind(&mut wire.conn).expect("layer shell");
    let surface = layers
        .layer_surface(
            &mut wire.conn,
            &LayerSurfaceConfig {
                namespace: "consent".into(),
                output: None,
                layer: Layer::Overlay,
                anchor: Anchor::empty(),
                size: (300, 200),
                margin: Margin::default(),
                exclusive_zone: 0,
                keyboard: KeyboardInteractivity::Exclusive,
            },
        )
        .expect("surface")
        .surface();
    wire.events();
    wire.take();
    authority.set_surface(&wire.conn, surface).expect("set");
    wire.events();
    assert!(matches!(
        authority_requests(&wire)[..],
        [AuthorityRequest::SetSurface { .. }]
    ));
}
