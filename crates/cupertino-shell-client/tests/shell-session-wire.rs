//! The shell global and the session observer over a real wire: binding by version, the state read at connect, the
//! changes after it and the requests of the session object.

mod support;

use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_protocols::server::cupertino_session_v1::{Activation, DisplayOption, Request};
use cupertino_shell_client::{AppActivation, SessionChange, SessionState, Shell, ShellEvent};
use cupertino_wayland_client::app::AppEvent;
use cupertino_wayland_client::Event;
use support::{extensions, Offer, Seen, Wire};

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
    for (offered, bound) in [(9, true), (20, true), (26, true), (8, false)] {
        let mut wire = Wire::new(Offer::shell(offered));
        assert_eq!(Shell::bind(&mut wire.conn).is_some(), bound, "{offered}");
    }
    let mut wire = Wire::new(Offer {
        shell: None,
        ..Offer::all()
    });
    assert!(Shell::bind(&mut wire.conn).is_none());
}

#[test]
fn the_state_at_connect_is_read_before_observe_returns() {
    let mut wire = Wire::all();
    wire.server(|s| {
        s.greet_session = Some(Box::new(|session| {
            session.appearance(1);
            session.display_options(DisplayOption::ReduceMotion | DisplayOption::IncreaseContrast);
            session.locked();
            session.lock_requested(1);
        }));
    });
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (_session, state, rest) = shell.observe(&mut wire.conn).expect("observe");
    assert_eq!(
        state,
        SessionState {
            dark: true,
            display: DisplayOptions {
                reduce_motion: true,
                increase_contrast: true,
                ..DisplayOptions::default()
            },
            locked: true,
        }
    );
    // what is not state stays for the caller's loop
    assert_eq!(
        support::shell_events(&rest),
        [ShellEvent::LockRequested { explicit: true }]
    );
}

#[test]
fn a_session_that_said_nothing_is_the_default_state() {
    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (_session, state, rest) = shell.observe(&mut wire.conn).expect("observe");
    assert_eq!(state, SessionState::default());
    assert!(!state.dark && !state.locked);
    assert!(rest.is_empty());
}

#[test]
fn changes_after_connect_update_the_state_in_order() {
    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (_session, mut state, _) = shell.observe(&mut wire.conn).expect("observe");

    wire.server(|s| {
        s.sessions[0].appearance(1);
        s.sessions[0].display_options(DisplayOption::ReduceTransparency);
        s.sessions[0].locked();
        s.sessions[0].unlocked();
        s.sessions[0].appearance(0);
    });
    let events = wire.events();
    let changes = apply(&mut state, &events);
    assert_eq!(
        changes,
        [
            SessionChange::Appearance { dark: true },
            SessionChange::DisplayOptions(DisplayOptions {
                reduce_transparency: true,
                ..DisplayOptions::default()
            }),
            SessionChange::Lock { locked: true },
            SessionChange::Lock { locked: false },
            SessionChange::Appearance { dark: false },
        ]
    );
    assert!(!state.dark && !state.locked);
    assert!(state.display.reduce_transparency);
}

#[test]
fn events_that_are_not_session_state_are_not_taken() {
    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (_session, mut state, _) = shell.observe(&mut wire.conn).expect("observe");

    wire.server(|s| {
        s.sessions[0].lock_requested(0);
        s.sessions[0].log_out_requested();
    });
    let events = wire.events();
    assert!(apply(&mut state, &events).is_empty());
    assert_eq!(state, SessionState::default());
    assert_eq!(
        support::shell_events(&events),
        [
            ShellEvent::LockRequested { explicit: false },
            ShellEvent::LogOutRequested
        ]
    );
}

#[test]
fn the_lock_state_comes_as_a_shell_event_too() {
    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (_session, _, _) = shell.observe(&mut wire.conn).expect("observe");
    wire.server(|s| {
        s.sessions[0].locked();
        s.sessions[0].unlocked();
    });
    assert_eq!(
        wire.shell_events(),
        [
            ShellEvent::SessionLock { locked: true },
            ShellEvent::SessionLock { locked: false }
        ]
    );
}

#[test]
fn appearance_and_display_options_arrive_as_application_events() {
    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (_session, _, _) = shell.observe(&mut wire.conn).expect("observe");
    wire.server(|s| {
        s.sessions[0].appearance(1);
        s.sessions[0].display_options(DisplayOption::DifferentiateWithoutColor);
    });
    let events = wire.events();
    let app: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Extension(e) => e.downcast_ref::<AppEvent>(),
            _ => None,
        })
        .collect();
    assert!(matches!(app[0], AppEvent::Appearance { dark: true }));
    assert!(matches!(
        app[1],
        AppEvent::DisplayOptions(DisplayOptions {
            differentiate_without_color: true,
            reduce_motion: false,
            ..
        })
    ));
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
fn an_app_activation_is_sent_with_its_serial_and_answered_by_serial() {
    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (session, _, _) = shell.observe(&mut wire.conn).expect("observe");
    wire.take();

    assert!(session.activate_app(41, "org.example.App.desktop"));
    wire.events();
    let requests = session_requests(&wire);
    assert!(matches!(
        &requests[..],
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
}

#[test]
fn an_app_activation_is_not_sent_to_a_server_older_than_version_twenty() {
    let mut wire = Wire::new(Offer::shell(19));
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (session, _, _) = shell.observe(&mut wire.conn).expect("observe");
    wire.take();
    assert!(!session.activate_app(1, "org.example.App.desktop"));
    wire.events();
    assert!(session_requests(&wire).is_empty());
}

#[test]
fn dropping_the_session_ends_the_observer() {
    let mut wire = Wire::all();
    let shell = Shell::bind(&mut wire.conn).expect("shell");
    let (session, _, _) = shell.observe(&mut wire.conn).expect("observe");
    wire.take();
    drop(session);
    wire.events();
    assert!(matches!(&session_requests(&wire)[..], [Request::Destroy]));
}
