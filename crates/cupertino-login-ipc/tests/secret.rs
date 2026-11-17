//! Secrets: never printed, hex on the wire, a hard length cap, and refusals that do not echo the text.

use cupertino_login_ipc::{
    decode, encode_secret, Request, Secret, MAX_REQUEST_FRAME, MAX_SECRET_BYTES,
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
fn the_longest_secret_is_512_bytes() {
    assert_eq!(MAX_SECRET_BYTES, 512);
    let s = Secret::new(Zeroizing::new("a".repeat(MAX_SECRET_BYTES))).unwrap();
    assert_eq!(s.expose().len(), MAX_SECRET_BYTES);
    let error = Secret::new(Zeroizing::new("a".repeat(MAX_SECRET_BYTES + 1))).unwrap_err();
    assert_eq!(error.to_string(), "a secret is at most 512 bytes");
}

#[test]
fn the_cap_counts_bytes_not_characters() {
    assert!(Secret::new(Zeroizing::new("\u{e9}".repeat(256))).is_ok());
    assert!(Secret::new(Zeroizing::new("\u{e9}".repeat(257))).is_err());
}

#[test]
fn an_empty_secret_is_allowed() {
    assert_eq!(secret("").expose(), "");
}

#[test]
fn the_wire_form_is_lower_case_hex() {
    let frame = encode_secret(
        &Request::Answer {
            conversation: 3,
            secret: secret("Ab\u{e9}"),
        },
        MAX_REQUEST_FRAME,
    )
    .unwrap();
    let json = std::str::from_utf8(&frame[4..]).unwrap();
    assert_eq!(
        json,
        r#"{"type":"Answer","conversation":3,"secret":"4162c3a9"}"#
    );
}

#[test]
fn a_secret_round_trips_including_characters_json_escapes() {
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
        let json = std::str::from_utf8(&frame[4..]).unwrap();
        assert!(
            !json.contains('\\'),
            "the text never needs an escape: {json}"
        );
        match decode::<Request>(&frame[4..]).unwrap() {
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
fn malformed_hex_is_refused() {
    for bad in [
        r#""abc""#, // odd length
        r#""4A""#,  // upper case
        r#""zz""#,  // not a digit
        r#""4 ""#,  // a space
        r#""+1""#,  // a sign is not a digit
        r#""4é""#,  // a non-ASCII character of two bytes: even length, no hex digit
    ] {
        assert!(decode::<Request>(&answer(bad)).is_err(), "{bad}");
    }
}

#[test]
fn a_secret_that_is_not_text_is_refused() {
    // 0xff is not UTF-8
    assert!(decode::<Request>(&answer(r#""ff""#)).is_err());
    // a lone continuation byte
    assert!(decode::<Request>(&answer(r#""80""#)).is_err());
}

#[test]
fn a_secret_that_is_not_a_string_is_refused() {
    for bad in ["1", "null", "[]", "true", r#"{"a":1}"#] {
        assert!(decode::<Request>(&answer(bad)).is_err(), "{bad}");
    }
}

#[test]
fn a_hex_secret_over_the_cap_is_refused() {
    let at_cap = "61".repeat(MAX_SECRET_BYTES);
    assert!(decode::<Request>(&answer(&format!("\"{at_cap}\""))).is_ok());
    let over = "61".repeat(MAX_SECRET_BYTES + 1);
    assert!(decode::<Request>(&answer(&format!("\"{over}\""))).is_err());
}

#[test]
fn a_refused_secret_does_not_echo_the_text_in_the_error() {
    let error = decode::<Request>(&answer(r#""70617373776f72646""#)).unwrap_err();
    assert!(!format!("{error} {error:?}").contains("7061"));
}
