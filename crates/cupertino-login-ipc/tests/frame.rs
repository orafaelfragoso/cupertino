//! Framing: the length prefix, the caps, the decoder fed in any chunking, and what a malformed body reports.

use cupertino_login_ipc::{
    decode, encode, encode_secret, Account, DecodeError, Decoder, Event, FrameError, Request,
    Secret, HEADER_BYTES, MAX_EVENT_FRAME, MAX_REQUEST_FRAME,
};
use zeroize::Zeroizing;

fn secret(text: &str) -> Secret {
    Secret::new(Zeroizing::new(text.to_owned())).expect("short enough")
}

/// Feed `bytes` to `decoder` in pieces of at most `chunk` bytes; the frames that complete.
fn feed(decoder: &mut Decoder, bytes: &[u8], chunk: usize) -> Result<Vec<Vec<u8>>, FrameError> {
    let mut frames = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let take = rest.len().min(chunk).min(decoder.wanted());
        let (now, later) = rest.split_at(take);
        if let Some(body) = decoder.push(now)? {
            frames.push(body.to_vec());
        }
        rest = later;
    }
    Ok(frames)
}

#[test]
fn a_frame_is_a_little_endian_length_then_the_json() {
    let frame = encode(&Request::Cancel {}, MAX_REQUEST_FRAME).unwrap();
    let json = br#"{"type":"Cancel"}"#;
    assert_eq!(&frame[..HEADER_BYTES], &(json.len() as u32).to_le_bytes());
    assert_eq!(&frame[HEADER_BYTES..], json);
}

#[test]
fn a_request_round_trips_through_encode_decoder_and_decode() {
    let frame = encode(&Request::Hello { version: 5 }, MAX_REQUEST_FRAME).unwrap();
    let mut decoder = Decoder::new(MAX_REQUEST_FRAME);
    let frames = feed(&mut decoder, &frame, usize::MAX).unwrap();
    assert_eq!(frames.len(), 1);
    match decode::<Request>(&frames[0]).unwrap() {
        Request::Hello { version } => assert_eq!(version, 5),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_decoder_reads_the_same_frames_at_every_chunk_size() {
    let mut stream = Vec::new();
    for version in [1, 2, 3] {
        stream.extend_from_slice(&encode(&Request::Hello { version }, 64).unwrap());
    }
    for chunk in 1..=stream.len() {
        let mut decoder = Decoder::new(64);
        let frames = feed(&mut decoder, &stream, chunk).unwrap();
        assert_eq!(frames.len(), 3, "chunk {chunk}");
        for (i, body) in frames.iter().enumerate() {
            match decode::<Request>(body).unwrap() {
                Request::Hello { version } => assert_eq!(version, i as u32 + 1),
                other => panic!("{other:?}"),
            }
        }
    }
}

#[test]
fn wanted_counts_down_the_header_then_the_body() {
    let mut decoder = Decoder::new(64);
    assert_eq!(decoder.wanted(), HEADER_BYTES);
    assert!(decoder.push(&[3, 0]).unwrap().is_none());
    assert_eq!(decoder.wanted(), 2);
    assert!(decoder.push(&[0, 0]).unwrap().is_none());
    assert_eq!(decoder.wanted(), 3);
    assert!(decoder.push(b"ab").unwrap().is_none());
    assert_eq!(decoder.wanted(), 1);
    let body = decoder.push(b"c").unwrap().expect("complete");
    assert_eq!(&body[..], b"abc");
    assert_eq!(decoder.wanted(), HEADER_BYTES, "ready for the next frame");
}

#[test]
fn pushing_nothing_changes_nothing() {
    let mut decoder = Decoder::new(64);
    assert!(decoder.push(&[]).unwrap().is_none());
    assert_eq!(decoder.wanted(), HEADER_BYTES);
}

#[test]
fn more_bytes_than_wanted_is_an_overrun() {
    let mut decoder = Decoder::new(64);
    assert_eq!(decoder.push(&[0; 5]).unwrap_err(), FrameError::Overrun);
    let mut decoder = Decoder::new(64);
    decoder.push(&[1, 0, 0, 0]).unwrap();
    assert_eq!(decoder.push(b"ab").unwrap_err(), FrameError::Overrun);
}

#[test]
fn a_zero_length_is_empty() {
    let mut decoder = Decoder::new(64);
    assert_eq!(decoder.push(&[0; 4]).unwrap_err(), FrameError::Empty);
}

#[test]
fn a_length_over_the_limit_is_refused_before_any_body_is_taken() {
    let mut decoder = Decoder::new(64);
    assert_eq!(
        decoder.push(&65u32.to_le_bytes()).unwrap_err(),
        FrameError::TooLarge
    );
    // the limit itself is allowed
    let mut decoder = Decoder::new(64);
    assert!(decoder.push(&64u32.to_le_bytes()).unwrap().is_none());
    assert_eq!(decoder.wanted(), 64);
}

#[test]
fn the_largest_length_does_not_allocate_or_wrap() {
    let mut decoder = Decoder::new(MAX_EVENT_FRAME);
    assert_eq!(
        decoder.push(&u32::MAX.to_le_bytes()).unwrap_err(),
        FrameError::TooLarge
    );
}

#[test]
fn a_truncated_stream_never_yields_a_frame() {
    let frame = encode(&Request::Hello { version: 5 }, 64).unwrap();
    for cut in 0..frame.len() {
        let mut decoder = Decoder::new(64);
        assert!(
            feed(&mut decoder, &frame[..cut], 3).unwrap().is_empty(),
            "cut {cut}"
        );
    }
}

#[test]
fn encoding_refuses_a_message_over_the_limit() {
    let request = Request::Authenticate {
        account: Account::Typed {
            name: "x".repeat(100),
        },
    };
    assert_eq!(encode(&request, 40).unwrap_err(), FrameError::TooLarge);
    assert_eq!(
        encode_secret(&request, 40).unwrap_err(),
        FrameError::TooLarge
    );
}

#[test]
fn the_limit_is_on_the_json_not_the_header() {
    let json_length = encode(&Request::Cancel {}, 1000).unwrap().len() - HEADER_BYTES;
    assert!(encode(&Request::Cancel {}, json_length).is_ok());
    assert_eq!(
        encode(&Request::Cancel {}, json_length - 1).unwrap_err(),
        FrameError::TooLarge
    );
}

#[test]
fn encode_secret_makes_the_same_frame_as_encode() {
    let answer = || Request::Answer {
        conversation: 9,
        secret: secret("hunter2"),
    };
    assert_eq!(
        encode(&answer(), MAX_REQUEST_FRAME).unwrap().to_vec(),
        encode_secret(&answer(), MAX_REQUEST_FRAME)
            .unwrap()
            .to_vec()
    );
}

#[test]
fn the_longest_password_fits_the_request_cap() {
    let frame = encode_secret(
        &Request::Answer {
            conversation: u64::MAX,
            secret: secret(&"e".repeat(512)),
        },
        MAX_REQUEST_FRAME,
    )
    .unwrap();
    assert!(frame.len() - HEADER_BYTES <= MAX_REQUEST_FRAME);
}

#[test]
fn decoding_reports_only_the_class_of_a_failure() {
    let syntax = decode::<Request>(b"{not json").unwrap_err();
    assert_eq!(syntax.class(), "syntax");
    let data = decode::<Request>(br#"{"type":"NoSuchRequest"}"#).unwrap_err();
    assert_eq!(data.class(), "data");
    let eof = decode::<Request>(br#"{"type":"Hello","version":"#).unwrap_err();
    assert_eq!(eof.class(), "eof");
    assert_eq!(syntax.to_string(), "malformed message (syntax)");
}

#[test]
fn a_decode_error_never_contains_the_offending_value() {
    // a password sent where a number belongs, as an unknown field or tag, or as a malformed secret
    let hex_password = "70617373776f7264";
    for body in [
        format!(r#"{{"type":"Hello","version":"{hex_password}"}}"#),
        format!(r#"{{"type":"Cancel","{hex_password}":1}}"#),
        format!(r#"{{"type":"{hex_password}"}}"#),
        format!(r#"{{"type":"Answer","conversation":1,"secret":"{hex_password}Z"}}"#),
    ] {
        let error: DecodeError = decode::<Request>(body.as_bytes()).unwrap_err();
        let shown = format!("{error} {error:?}");
        assert!(
            !shown.contains("password") && !shown.contains(hex_password),
            "{shown}"
        );
    }
}

#[test]
fn unknown_tags_and_fields_are_refused() {
    assert!(decode::<Request>(br#"{"type":"Hello","version":5,"extra":1}"#).is_err());
    assert!(decode::<Request>(br#"{"type":"Cancel","x":1}"#).is_err());
    assert!(
        decode::<Event>(br#"{"type":"Hello"}"#).is_err(),
        "a missing field"
    );
    assert!(decode::<Request>(b"[]").is_err());
    assert!(decode::<Request>(b"null").is_err());
    assert!(decode::<Request>(b"").is_err());
}

#[test]
fn a_frame_with_trailing_bytes_in_the_body_is_malformed() {
    assert!(decode::<Request>(br#"{"type":"Cancel"} x"#).is_err());
}

#[test]
fn the_errors_say_what_they_are() {
    assert_eq!(
        FrameError::TooLarge.to_string(),
        "frame larger than the cap"
    );
    assert_eq!(FrameError::Empty.to_string(), "empty frame");
    assert_eq!(
        FrameError::Overrun.to_string(),
        "more bytes than the decoder asked for"
    );
}
