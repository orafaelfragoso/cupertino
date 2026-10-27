use cupertino_login_ipc::{
    decode, encode, encode_secret, is_login_name, Account, Decoder, Event, FrameError, Outcome,
    PowerAction, PowerRefusal, ProtocolRefusal, Request, Secret, State, MAX_REQUEST_FRAME,
    MAX_SECRET_BYTES, VERSION,
};
use zeroize::Zeroizing;

/// Push `frame` into `decoder` the way a socket reader does: never more than it asks for.
fn feed(decoder: &mut Decoder, frame: &[u8]) -> Result<Option<Vec<u8>>, FrameError> {
    let mut rest = frame;
    while !rest.is_empty() {
        let take = decoder.wanted().min(rest.len());
        if let Some(body) = decoder.push(&rest[..take])? {
            return Ok(Some(body.to_vec()));
        }
        rest = &rest[take..];
    }
    Ok(None)
}

#[test]
fn a_frame_survives_any_chunking() {
    let frame = encode(&Request::Hello { version: VERSION }, MAX_REQUEST_FRAME).unwrap();
    for chunk in 1..=frame.len() {
        let mut decoder = Decoder::new(MAX_REQUEST_FRAME);
        let mut body = None;
        for piece in frame.chunks(chunk) {
            let mut rest = piece;
            while !rest.is_empty() {
                let take = decoder.wanted().min(rest.len());
                body = decoder.push(&rest[..take]).unwrap().or(body);
                rest = &rest[take..];
            }
        }
        let Some(Request::Hello { version }) = body.map(|b| decode::<Request>(&b).unwrap()) else {
            panic!("chunk {chunk}")
        };
        assert_eq!(version, VERSION);
    }
}

#[test]
fn a_length_over_the_cap_is_refused_before_the_body_is_read() {
    let mut decoder = Decoder::new(MAX_REQUEST_FRAME);
    let prefix = u32::try_from(MAX_REQUEST_FRAME + 1).unwrap().to_le_bytes();
    assert_eq!(decoder.push(&prefix), Err(FrameError::TooLarge));
    let mut at_cap = Decoder::new(MAX_REQUEST_FRAME);
    assert_eq!(
        at_cap.push(&u32::try_from(MAX_REQUEST_FRAME).unwrap().to_le_bytes()),
        Ok(None)
    );
    assert_eq!(at_cap.wanted(), MAX_REQUEST_FRAME);
}

#[test]
fn an_empty_frame_and_an_overrun_are_refused() {
    assert_eq!(
        Decoder::new(16).push(&0u32.to_le_bytes()),
        Err(FrameError::Empty)
    );
    assert_eq!(Decoder::new(16).push(&[0; 5]), Err(FrameError::Overrun));
}

#[test]
fn a_message_over_the_cap_is_not_encoded() {
    let request = Request::Hello { version: VERSION };
    let exact = encode(&request, 1024).unwrap().len() - 4;
    assert!(encode(&request, exact).is_ok());
    assert_eq!(
        encode(&request, exact - 1).unwrap_err(),
        FrameError::TooLarge
    );
}

#[test]
fn an_unknown_type_or_field_and_a_wrong_field_type_are_decode_errors() {
    for body in [
        r#"{"type":"Fly"}"#,
        r#"{"type":"Hello","version":1,"extra":1}"#,
        r#"{"type":"Hello","version":"one"}"#,
        r#"{"version":1}"#,
        "not json",
    ] {
        assert!(decode::<Request>(body.as_bytes()).is_err(), "{body}");
    }
}

#[test]
fn a_decode_error_names_the_class_and_never_the_value() {
    let error = decode::<Request>(br#"{"type":"Hello","version":"hunter2"}"#).unwrap_err();
    assert_eq!(error.class(), "data");
    assert!(!error.to_string().contains("hunter2"), "{error}");
    assert!(!format!("{error:?}").contains("hunter2"), "{error:?}");
}

#[test]
fn the_first_message_must_be_hello_with_this_version() {
    assert_eq!(
        State::New.next(&Request::Hello { version: VERSION }),
        Ok(State::Idle)
    );
    assert_eq!(
        State::New.next(&Request::Hello {
            version: VERSION + 1
        }),
        Err(ProtocolRefusal::Version)
    );
    assert_eq!(
        State::Idle.next(&Request::Hello { version: VERSION }),
        Err(ProtocolRefusal::State)
    );
}

#[test]
fn events_round_trip_through_a_frame() {
    let event = Event::Refused {
        reason: ProtocolRefusal::Version,
    };
    let frame = encode(&event, 1024).unwrap();
    let mut decoder = Decoder::new(1024);
    let body = feed(&mut decoder, &frame).unwrap().unwrap();
    assert!(matches!(
        decode::<Event>(&body).unwrap(),
        Event::Refused {
            reason: ProtocolRefusal::Version
        }
    ));
}

fn secret(text: &str) -> Secret {
    Secret::new(Zeroizing::new(text.to_owned())).unwrap()
}

#[test]
fn a_secret_round_trips_and_its_text_never_shows_in_debug_output() {
    let request = Request::Answer {
        conversation: 7,
        secret: secret("hunter2 \"quoted\" \\ é"),
    };
    let frame = encode_secret(&request, MAX_REQUEST_FRAME).unwrap();
    let Request::Answer { secret, .. } = decode::<Request>(&frame[4..]).unwrap() else {
        panic!("not an answer")
    };
    assert_eq!(secret.expose(), "hunter2 \"quoted\" \\ é");
    assert_eq!(format!("{secret:?}"), "<secret>");
    assert_eq!(
        format!(
            "{:?}",
            Request::Answer {
                conversation: 7,
                secret
            }
        ),
        "Answer { conversation: 7, secret: <secret> }"
    );
}

#[test]
fn a_secret_on_the_wire_is_hex_so_the_json_parser_never_unescapes_it() {
    let frame = encode_secret(
        &Request::Answer {
            conversation: 7,
            secret: secret("a\"b"),
        },
        MAX_REQUEST_FRAME,
    )
    .unwrap();
    assert_eq!(
        &frame[4..],
        br#"{"type":"Answer","conversation":7,"secret":"612262"}"#
    );
}

#[test]
fn the_longest_password_fits_a_request_frame_and_one_byte_more_is_not_a_secret() {
    let longest = "x".repeat(MAX_SECRET_BYTES);
    let frame = encode_secret(
        &Request::Answer {
            conversation: 7,
            secret: secret(&longest),
        },
        MAX_REQUEST_FRAME,
    )
    .unwrap();
    assert!(frame.len() - 4 <= MAX_REQUEST_FRAME);
    assert!(Secret::new(Zeroizing::new("x".repeat(MAX_SECRET_BYTES + 1))).is_err());
    // the same bound holds for a secret that arrives on the wire
    let too_long = format!(
        r#"{{"type":"Answer","secret":"{}"}}"#,
        "78".repeat(MAX_SECRET_BYTES + 1)
    );
    assert!(decode::<Request>(too_long.as_bytes()).is_err());
}

#[test]
fn a_malformed_secret_is_a_decode_error_that_does_not_echo_it() {
    for body in [
        r#"{"type":"Answer","secret":"zz"}"#,
        r#"{"type":"Answer","secret":"abc"}"#,
        r#"{"type":"Answer","secret":"c328"}"#,
        r#"{"type":"Answer","secret":12345}"#,
        r#"{"type":"Answer","secret":"hunter2"}"#,
    ] {
        let error = decode::<Request>(body.as_bytes()).unwrap_err();
        assert!(
            !error.to_string().contains("hunter2") && !format!("{error:?}").contains("hunter2"),
            "{body}"
        );
        assert!(!error.to_string().contains("12345"), "{body}");
    }
}

#[test]
fn an_oversized_answer_frame_is_refused_at_the_length_prefix() {
    let mut decoder = Decoder::new(MAX_REQUEST_FRAME);
    assert_eq!(
        decoder.push(&3000u32.to_le_bytes()),
        Err(FrameError::TooLarge)
    );
}

#[test]
fn the_state_machine_follows_an_attempt() {
    let hello = Request::Hello { version: VERSION };
    let authenticate = Request::Authenticate {
        account: Account::Listed { uid: 1000 },
    };
    let answer = Request::Answer {
        conversation: 7,
        secret: secret("x"),
    };
    let cancel = Request::Cancel {};
    let authenticate_power = Request::AuthenticatePower {
        name: "admin".to_owned(),
        action: PowerAction::Restart,
    };
    assert_eq!(State::Idle.next(&authenticate), Ok(State::Authenticating));
    // an administrator's authentication for a power action is an attempt like any other
    assert_eq!(
        State::Idle.next(&authenticate_power),
        Ok(State::Authenticating)
    );
    assert_eq!(
        State::Authenticating.next(&answer),
        Ok(State::Authenticating)
    );
    assert_eq!(State::Authenticating.next(&cancel), Ok(State::Idle));
    // an abort with nothing to abort is ignored, also while the session starts
    assert_eq!(State::Idle.next(&cancel), Ok(State::Idle));
    assert_eq!(State::Starting.next(&cancel), Ok(State::Starting));
    // the power menu is reachable on every screen after Hello, and changes nothing about the attempt
    let power = Request::Power {
        action: PowerAction::Restart,
    };
    for state in [State::Idle, State::Authenticating, State::Starting] {
        assert_eq!(state.next(&power), Ok(state));
    }
    for (state, request) in [
        (State::New, &power),
        (State::New, &authenticate),
        (State::New, &authenticate_power),
        (State::New, &answer),
        (State::New, &cancel),
        (State::Idle, &hello),
        (State::Idle, &answer),
        (State::Authenticating, &authenticate),
        (State::Authenticating, &authenticate_power),
        (State::Authenticating, &hello),
        (State::Starting, &authenticate),
        (State::Starting, &authenticate_power),
        (State::Starting, &answer),
        (State::Starting, &hello),
    ] {
        assert_eq!(
            state.next(request),
            Err(ProtocolRefusal::State),
            "{state:?} {request:?}"
        );
    }
}

#[test]
fn events_move_the_state_from_attempt_to_session_and_back() {
    let result = |outcome| Event::Result { outcome };
    assert_eq!(
        State::Authenticating.after(&result(Outcome::Accepted)),
        State::Starting
    );
    for outcome in [
        Outcome::Denied,
        Outcome::Locked,
        Outcome::Expired,
        Outcome::Refused,
        Outcome::Busy,
        Outcome::Internal,
    ] {
        assert_eq!(State::Authenticating.after(&result(outcome)), State::Idle);
    }
    assert_eq!(State::Starting.after(&Event::Reset), State::Idle);
    assert_eq!(State::Authenticating.after(&Event::Reset), State::Idle);
    assert_eq!(
        State::Authenticating.after(&Event::Notice {
            error: false,
            text: "x".into()
        }),
        State::Authenticating
    );
}

#[test]
fn typed_account_names_follow_the_limit_of_the_name_entry() {
    for good in ["a", "alice", "alice.b-c_d", "x".repeat(32).as_str(), "élan"] {
        assert!(is_login_name(good), "{good:?}");
    }
    for bad in [
        "",
        "-rf",
        "a b",
        "a\tb",
        "a:b",
        "a/b",
        "a\\b",
        "a\0b",
        "a\u{7f}",
        &"x".repeat(33),
        "tab\t",
    ] {
        assert!(!is_login_name(bad), "{bad:?}");
    }
}

#[test]
fn a_power_request_has_one_wire_form_and_no_other_action_decodes() {
    let request = decode::<Request>(br#"{"type":"Power","action":"ShutDown"}"#).unwrap();
    assert!(matches!(
        request,
        Request::Power {
            action: PowerAction::ShutDown
        }
    ));
    for body in [
        r#"{"type":"Power","action":"Hibernate"}"#,
        r#"{"type":"Power"}"#,
        r#"{"type":"Power","action":"Restart","extra":1}"#,
        r#"{"type":"Power","action":"restart"}"#,
    ] {
        assert!(decode::<Request>(body.as_bytes()).is_err(), "{body}");
    }
    let refused = encode(
        &Event::PowerRefused {
            reason: PowerRefusal::OtherUsersLoggedIn,
        },
        1024,
    )
    .unwrap();
    assert_eq!(
        &refused[4..],
        br#"{"type":"PowerRefused","reason":"OtherUsersLoggedIn"}"#
    );
}
