//! The connection state machine: which request moves where, what is refused, and how events move it.

use cupertino_login_ipc::{
    Account, Event, Outcome, PowerAction, PowerMenu, ProtocolRefusal, Request, Secret, State,
    VERSION,
};
use zeroize::Zeroizing;

fn hello() -> Request {
    Request::Hello { version: VERSION }
}
fn authenticate() -> Request {
    Request::Authenticate {
        account: Account::Listed { uid: 1000 },
    }
}
fn answer() -> Request {
    Request::Answer {
        conversation: 1,
        secret: Secret::new(Zeroizing::new("x".to_owned())).unwrap(),
    }
}
fn authenticate_power() -> Request {
    Request::AuthenticatePower {
        name: "root".into(),
        action: PowerAction::ShutDown,
    }
}
fn power() -> Request {
    Request::Power {
        action: PowerAction::Sleep,
    }
}
fn result(outcome: Outcome) -> Event {
    Event::Result { outcome }
}

const ALL: [State; 4] = [
    State::New,
    State::Idle,
    State::Authenticating,
    State::Starting,
];

#[test]
fn the_happy_path_walks_new_idle_authenticating_starting() {
    let state = State::New.next(&hello()).unwrap();
    assert_eq!(state, State::Idle);
    let state = state.next(&authenticate()).unwrap();
    assert_eq!(state, State::Authenticating);
    let state = state.next(&answer()).unwrap();
    assert_eq!(state, State::Authenticating);
    let state = state.after(&result(Outcome::Accepted));
    assert_eq!(state, State::Starting);
    assert_eq!(state.after(&Event::Reset {}), State::Idle);
}

#[test]
fn a_wrong_version_in_hello_is_a_version_refusal() {
    for version in [0, VERSION - 1, VERSION + 1, u32::MAX] {
        assert_eq!(
            State::New.next(&Request::Hello { version }).unwrap_err(),
            ProtocolRefusal::Version
        );
    }
}

#[test]
fn only_a_new_connection_accepts_hello() {
    for state in [State::Idle, State::Authenticating, State::Starting] {
        assert_eq!(state.next(&hello()).unwrap_err(), ProtocolRefusal::State);
    }
    // a wrong version after the greeting is a state error, not a version one
    assert_eq!(
        State::Idle
            .next(&Request::Hello { version: 1 })
            .unwrap_err(),
        ProtocolRefusal::State
    );
}

#[test]
fn nothing_but_hello_is_allowed_on_a_new_connection() {
    for request in [
        authenticate(),
        answer(),
        Request::Cancel {},
        authenticate_power(),
        power(),
    ] {
        assert_eq!(
            State::New.next(&request).unwrap_err(),
            ProtocolRefusal::State,
            "{request:?}"
        );
    }
}

#[test]
fn an_attempt_opens_only_from_idle() {
    for request in [authenticate(), authenticate_power()] {
        assert_eq!(State::Idle.next(&request).unwrap(), State::Authenticating);
        for state in [State::New, State::Authenticating, State::Starting] {
            assert_eq!(state.next(&request).unwrap_err(), ProtocolRefusal::State);
        }
    }
}

#[test]
fn an_answer_is_valid_only_while_authenticating() {
    for state in [State::New, State::Idle, State::Starting] {
        assert_eq!(state.next(&answer()).unwrap_err(), ProtocolRefusal::State);
    }
}

#[test]
fn cancel_returns_to_the_picker_and_is_harmless_elsewhere() {
    assert_eq!(
        State::Authenticating.next(&Request::Cancel {}).unwrap(),
        State::Idle
    );
    assert_eq!(State::Idle.next(&Request::Cancel {}).unwrap(), State::Idle);
    assert_eq!(
        State::Starting.next(&Request::Cancel {}).unwrap(),
        State::Starting
    );
}

#[test]
fn power_keeps_the_state_after_the_greeting() {
    for state in [State::Idle, State::Authenticating, State::Starting] {
        assert_eq!(state.next(&power()).unwrap(), state);
    }
}

#[test]
fn a_refused_request_leaves_the_state_to_the_caller() {
    // `next` takes the state by value and returns the refusal: nothing is half applied
    let state = State::Idle;
    assert!(state.next(&answer()).is_err());
    assert_eq!(state, State::Idle);
}

#[test]
fn a_rejected_attempt_returns_to_idle_for_every_outcome_but_accepted() {
    for outcome in [
        Outcome::Denied,
        Outcome::Locked,
        Outcome::Expired,
        Outcome::Refused,
        Outcome::Busy,
        Outcome::Internal,
    ] {
        assert_eq!(
            State::Authenticating.after(&result(outcome)),
            State::Idle,
            "{outcome:?}"
        );
    }
}

#[test]
fn a_result_outside_an_attempt_changes_nothing() {
    for state in [State::New, State::Idle, State::Starting] {
        for outcome in [Outcome::Accepted, Outcome::Denied] {
            assert_eq!(state.after(&result(outcome)), state);
        }
    }
}

#[test]
fn reset_ends_an_attempt_or_a_starting_session_and_nothing_else() {
    assert_eq!(State::Authenticating.after(&Event::Reset {}), State::Idle);
    assert_eq!(State::Starting.after(&Event::Reset {}), State::Idle);
    assert_eq!(State::Idle.after(&Event::Reset {}), State::Idle);
    assert_eq!(State::New.after(&Event::Reset {}), State::New);
}

#[test]
fn other_events_change_nothing() {
    let events = [
        Event::Hello { version: VERSION },
        Event::Welcome {
            users: vec![],
            show_other: true,
            power: PowerMenu {
                restart: true,
                shut_down: true,
                sleep: true,
            },
            will_prompt: false,
            retries_until_hint: 3,
        },
        Event::Users {
            users: vec![],
            will_prompt: true,
        },
        Event::Prompt {
            conversation: 1,
            echo: false,
            text: "Password:".into(),
        },
        Event::Notice {
            error: true,
            text: "x".into(),
        },
        Event::SessionStarting { uid: 1000 },
        Event::Refused {
            reason: ProtocolRefusal::State,
        },
        Event::PowerRefused {
            reason: cupertino_login_ipc::PowerRefusal::Failed,
        },
    ];
    for state in ALL {
        for event in &events {
            assert_eq!(state.after(event), state, "{event:?}");
        }
    }
}

#[test]
fn a_starting_session_accepts_no_new_attempt_until_reset() {
    assert!(State::Starting.next(&authenticate()).is_err());
    let idle = State::Starting.after(&Event::Reset {});
    assert_eq!(idle.next(&authenticate()).unwrap(), State::Authenticating);
}
