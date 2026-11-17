//! Secrets: never printed, hex on the wire, a hard length cap, and refusals that do not echo the text.

use cupertino_login_ipc::{
    decode, encode_secret, Request, Secret, HEADER_BYTES, MAX_REQUEST_FRAME, MAX_SECRET_BYTES,
};
use zeroize::Zeroizing;

fn secret(text: &str) -> Secret {
    Secret::new(Zeroizing::new(text.to_owned())).expect("short enough")
}

fn answer(secret: &str) -> Vec<u8> {
    format!(r#"{{"type":"Answer","conversation":1,"secret":{secret}}}"#).into_bytes()
}

#[test]
fn a_secret_prints_as_a_placeholder() {
    let s = secret("hunter2");
    assert_eq!(format!("{s:?}"), "<secret>");
    assert_eq!(format!("{s:#?}"), "<secret>");
    let request = Request::Answer {
        conversation: 1,
        secret: s,
    };
    assert!(!format!("{request:?}").contains("hunter2"));
}

#[test]
fn a_secret_holds_at_most_the_cap_in_bytes() {
    for (row, text, held) in [
        ("empty", String::new(), true),
        ("the cap", "a".repeat(MAX_SECRET_BYTES), true),
        ("one byte over", "a".repeat(MAX_SECRET_BYTES + 1), false),
        (
            "two-byte characters at the cap",
            "\u{e9}".repeat(MAX_SECRET_BYTES / 2),
            true,
        ),
        (
            "two-byte characters over",
            "\u{e9}".repeat(MAX_SECRET_BYTES / 2 + 1),
            false,
        ),
    ] {
        match Secret::new(Zeroizing::new(text.clone())) {
            Ok(s) => {
                assert!(held, "{row} is refused");
                assert_eq!(s.expose(), text, "{row}");
            }
            Err(_) => assert!(!held, "{row} is held"),
        }
    }
}

#[test]
fn a_secret_travels_as_lower_case_hex_and_round_trips_any_text() {
    let frame = encode_secret(
        &Request::Answer {
            conversation: 3,
            secret: secret("Ab\u{e9}"),
        },
        MAX_REQUEST_FRAME,
    )
    .unwrap();
    assert_eq!(
        std::str::from_utf8(&frame[HEADER_BYTES..]).unwrap(),
        r#"{"type":"Answer","conversation":3,"secret":"4162c3a9"}"#
    );

    for text in [
        "",
        "plain",
        "quote\"back\\slash",
        "line\nbreak\u{0}nul",
        "\u{65e5}\u{672c}\u{8a9e} \u{1F512}",
    ] {
        let frame = encode_secret(
            &Request::Answer {
                conversation: 1,
                secret: secret(text),
            },
            MAX_REQUEST_FRAME,
        )
        .unwrap();
        let json = std::str::from_utf8(&frame[HEADER_BYTES..]).unwrap();
        assert!(
            !json.contains('\\'),
            "the text never needs an escape: {json}"
        );
        match decode::<Request>(&frame[HEADER_BYTES..]).unwrap() {
            Request::Answer {
                conversation,
                secret,
            } => {
                assert_eq!(conversation, 1);
                assert_eq!(secret.expose(), text);
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_secret_on_the_wire_must_be_lower_case_hex_of_text_within_the_cap() {
    let at_cap = format!("\"{}\"", "61".repeat(MAX_SECRET_BYTES));
    assert!(decode::<Request>(&answer(&at_cap)).is_ok());
    let over = format!("\"{}\"", "61".repeat(MAX_SECRET_BYTES + 1));
    for bad in [
        r#""abc""#, // odd length
        r#""4A""#,  // upper case
        r#""zz""#,  // not a digit
        r#""4 ""#,  // a space
        r#""+1""#,  // a sign is not a digit
        r#""4é""#,  // a non-ASCII character of two bytes: even length, no hex digit
        r#""ff""#,  // 0xff is not UTF-8
        r#""80""#,  // a lone continuation byte
        "1",
        "null",
        "[]",
        "true",
        r#"{"a":1}"#,
        &over,
    ] {
        assert!(decode::<Request>(&answer(bad)).is_err(), "{bad}");
    }
}
