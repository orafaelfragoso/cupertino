//! The shell global and the session observer over a real wire: binding by version, the state read at connect, the
//! changes after it and the requests of the session object.

use crate::support;

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_protocols::server::cupertino_session_v1::{Activation, DisplayOption, Request};
use cupertino_shell_client::{AppActivation, SessionChange, SessionState, Shell, ShellEvent};
use cupertino_wayland_client::app::AppEvent;
use cupertino_wayland_client::Event;
use support::{extension, extensions, shell_events, Offer, Seen, Wire};

fn session_requests(wire: &Wire) -> Vec<Request> {
    wire.take()
        .into_iter()
        .filter_map(|s| match s {
            Seen::Session(r) => Some(r),
            _ => None,
        })
        .collect()
}

/// Every state change among `events`, applied to `state` in order.
fn apply(state: &mut SessionState, events: &[Event]) -> Vec<SessionChange> {
    extensions(events)
        .into_iter()
        .filter_map(|e| state.apply(e))
        .collect()
}

#[test]
fn the_shell_binds_from_version_nine_and_never_below() {
    for (offered, bound) in [
        (None, false),
        (Some(8), false),
        (Some(9), true),
        (Some(20), true),
        (Some(26), true),
        (Some(28), true),
    ] {
        let mut wire = Wire::new(Offer {
            shell: offered,
            ..Offer::all()
        });
        assert_eq!(
            Shell::bind(&mut wire.conn).is_some(),
            bound,
            "offered at {offered:?}"
        );
    }
}

#[test]
fn the_state_at_connect_is_read_before_observe_returns() {
    let reduced = SessionState {
        dark: true,
        display: DisplayOptions {
            reduce_motion: true,
            increase_contrast: true,
            ..DisplayOptions::default()
        },
        locked: true,
    };
    for (row, greets, state_at_connect, left) in [
        (
            "a full greeting",
            true,
            reduced,
            vec![ShellEvent::LockRequested { explicit: true }],
        ),
        ("nothing said", false, SessionState::default(), vec![]),
    ] {
        let mut wire = Wire::all();
        if greets {
            wire.server(|s| {
                s.greet_session = Some(Box::new(|session| {
                    session.appearance(1);
                    session.display_options(
                        DisplayOption::ReduceMotion | DisplayOption::IncreaseContrast,
                    );
                    session.locked();
                    session.lock_requested(1);
                }));
            });
        }
        let shell = Shell::bind(&mut wire.conn).expect("shell");
        let (_session, state, rest) = shell.observe(&mut wire.conn).expect("observe");
        assert_eq!(state, state_at_connect, "{row}");
        assert_eq!(
            shell_events(&rest),
            left,
            "{row}: what is not state stays for the caller's loop"
        );
    }
}

#[test]
fn changes_after_connect_update_the_state_in_order_and_reach_the_shell_and_app_loops() {
    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (_session, mut state, _) = shell.observe(&mut wire.conn).expect("observe");

    wire.server(|s| {
        let session = &s.sessions[0];
        session.appearance(1);
        session.display_options(DisplayOption::ReduceTransparency);
        session.locked();
        session.lock_requested(0);
        session.unlocked();
        session.log_out_requested();
        session.display_options(DisplayOption::DifferentiateWithoutColor);
        session.appearance(0);
    });
    let events = wire.events();
    assert_eq!(
        apply(&mut state, &events),
        [
            SessionChange::Appearance { dark: true },
            SessionChange::DisplayOptions(DisplayOptions {
                reduce_transparency: true,
                ..DisplayOptions::default()
            }),
            SessionChange::Lock { locked: true },
            SessionChange::Lock { locked: false },
            SessionChange::DisplayOptions(DisplayOptions {
                differentiate_without_color: true,
                ..DisplayOptions::default()
            }),
            SessionChange::Appearance { dark: false },
        ],
        "a lock or log-out request is not session state"
    );
    assert!(!state.dark && !state.locked);
    assert!(state.display.differentiate_without_color && !state.display.reduce_transparency);

    assert_eq!(
        shell_events(&events),
        [
            ShellEvent::SessionLock { locked: true },
            ShellEvent::LockRequested { explicit: false },
            ShellEvent::SessionLock { locked: false },
            ShellEvent::LogOutRequested,
        ]
    );
    let app = extension::<AppEvent>(&events);
    assert!(matches!(app[0], AppEvent::Appearance { dark: true }));
    assert!(matches!(
        app[1],
        AppEvent::DisplayOptions(DisplayOptions {
            reduce_transparency: true,
            ..
        })
    ));
    assert!(matches!(
        app[2],
        AppEvent::DisplayOptions(DisplayOptions {
            differentiate_without_color: true,
            reduce_motion: false,
            ..
        })
    ));
    assert!(matches!(app[3], AppEvent::Appearance { dark: false }));
}

#[test]
fn display_options_with_reserved_bits_fail_the_connection() {
    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (_session, _, _) = shell.observe(&mut wire.conn).expect("observe");
    wire.server(|s| {
        s.sessions[0].display_options(DisplayOption::from_bits_retain(0x100));
    });
    let result = wire
        .conn
        .roundtrip()
        .and_then(|()| wire.conn.wait(Some(std::time::Duration::ZERO)).map(|_| ()));
    assert!(matches!(
        result,
        Err(cupertino_wayland_client::Error::Protocol(_))
    ));
}

#[test]
fn an_app_activation_is_sent_from_version_twenty_and_answered_by_serial() {
    let mut wire = Wire::new(Offer::shell(19));
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (session, _, _) = shell.observe(&mut wire.conn).expect("observe");
    wire.take();
    assert!(!session.activate_app(1, "org.example.App.desktop"));
    wire.events();
    assert!(session_requests(&wire).is_empty(), "version 19");

    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (session, _, _) = shell.observe(&mut wire.conn).expect("observe");
    wire.take();
    assert!(session.activate_app(41, "org.example.App.desktop"));
    wire.events();
    assert!(matches!(
        &session_requests(&wire)[..],
        [Request::ActivateApp { serial: 41, desktop_id }] if desktop_id == "org.example.App.desktop"
    ));

    wire.server(|s| {
        s.sessions[0].app_activation(41, Activation::Activated);
        s.sessions[0].app_activation(42, Activation::Resident);
        s.sessions[0].app_activation(43, Activation::NotRunning);
    });
    assert_eq!(
        wire.shell_events(),
        [
            ShellEvent::AppActivation {
                serial: 41,
                result: AppActivation::Activated
            },
            ShellEvent::AppActivation {
                serial: 42,
                result: AppActivation::Resident
            },
            ShellEvent::AppActivation {
                serial: 43,
                result: AppActivation::NotRunning
            },
        ]
    );

    drop(session);
    wire.events();
    assert!(
        matches!(&session_requests(&wire)[..], [Request::Destroy]),
        "dropping the session ends the observer"
    );
}
