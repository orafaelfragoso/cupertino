//! The connection state machine: which request moves where, what is refused, and how events move it.

use cupertino_login_ipc::{
    Account, Event, Outcome, PowerAction, PowerMenu, PowerRefusal, ProtocolRefusal, Request,
    Secret, State, VERSION,
};
use zeroize::Zeroizing;

use State::{Authenticating, Idle, New, Starting};

fn hello(version: u32) -> Request {
    Request::Hello { version }
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

#[test]
fn a_request_moves_the_state_or_is_refused_with_the_reason() {
    use ProtocolRefusal::{State as Wrong, Version};
    let rows: Vec<(State, Request, Result<State, ProtocolRefusal>)> = vec![
        (New, hello(VERSION), Ok(Idle)),
        (New, hello(0), Err(Version)),
        (New, hello(VERSION - 1), Err(Version)),
        (New, hello(VERSION + 1), Err(Version)),
        (New, hello(u32::MAX), Err(Version)),
        (Idle, hello(VERSION), Err(Wrong)),
        (Authenticating, hello(VERSION), Err(Wrong)),
        (Starting, hello(VERSION), Err(Wrong)),
        // a wrong version after the greeting is a state error, not a version one
        (Idle, hello(1), Err(Wrong)),
        (New, authenticate(), Err(Wrong)),
        (New, answer(), Err(Wrong)),
        (New, Request::Cancel {}, Err(Wrong)),
        (New, authenticate_power(), Err(Wrong)),
        (New, power(), Err(Wrong)),
        (Idle, authenticate(), Ok(Authenticating)),
        (Idle, authenticate_power(), Ok(Authenticating)),
        (Authenticating, authenticate(), Err(Wrong)),
        (Authenticating, authenticate_power(), Err(Wrong)),
        (Starting, authenticate(), Err(Wrong)),
        (Starting, authenticate_power(), Err(Wrong)),
        (Authenticating, answer(), Ok(Authenticating)),
        (Idle, answer(), Err(Wrong)),
        (Starting, answer(), Err(Wrong)),
        (Authenticating, Request::Cancel {}, Ok(Idle)),
        (Idle, Request::Cancel {}, Ok(Idle)),
        (Starting, Request::Cancel {}, Ok(Starting)),
        (Idle, power(), Ok(Idle)),
        (Authenticating, power(), Ok(Authenticating)),
        (Starting, power(), Ok(Starting)),
    ];
    for (state, request, expected) in rows {
        assert_eq!(state.next(&request), expected, "{state:?} on {request:?}");
    }
}

#[test]
fn an_event_ends_an_attempt_or_a_starting_session_and_otherwise_changes_nothing() {
    let rejected = [
        Outcome::Denied,
        Outcome::Locked,
        Outcome::Expired,
        Outcome::Refused,
        Outcome::Busy,
        Outcome::Internal,
    ];
    let mut rows: Vec<(State, Event, State)> = vec![
        (Authenticating, result(Outcome::Accepted), Starting),
        (Authenticating, Event::Reset {}, Idle),
        (Starting, Event::Reset {}, Idle),
        (Idle, Event::Reset {}, Idle),
        (New, Event::Reset {}, New),
    ];
    rows.extend(
        rejected
            .into_iter()
            .map(|outcome| (Authenticating, result(outcome), Idle)),
    );
    for state in [New, Idle, Starting] {
        for outcome in [Outcome::Accepted, Outcome::Denied] {
            rows.push((state, result(outcome), state));
        }
    }
    let others = || {
        [
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
                reason: PowerRefusal::Failed,
            },
        ]
    };
    for state in [New, Idle, Authenticating, Starting] {
        rows.extend(others().into_iter().map(|event| (state, event, state)));
    }
    for (state, event, expected) in rows {
        assert_eq!(state.after(&event), expected, "{state:?} after {event:?}");
    }
}
