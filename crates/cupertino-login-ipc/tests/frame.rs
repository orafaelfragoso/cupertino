//! Framing: the length prefix, the caps, the decoder fed in any chunking, and what a malformed body reports.

use cupertino_login_ipc::{
    decode, encode, encode_secret, Account, Decoder, Event, FrameError, Request, Secret,
    HEADER_BYTES, MAX_EVENT_FRAME, MAX_REQUEST_FRAME,
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

fn version(body: &[u8]) -> u32 {
    match decode::<Request>(body).unwrap() {
        Request::Hello { version } => version,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_frame_is_a_little_endian_length_then_the_json() {
    let frame = encode(&Request::Cancel {}, MAX_REQUEST_FRAME).unwrap();
    let json = br#"{"type":"Cancel"}"#;
    assert_eq!(&frame[..HEADER_BYTES], &(json.len() as u32).to_le_bytes());
    assert_eq!(&frame[HEADER_BYTES..], json);

    let answer = || Request::Answer {
        conversation: 9,
        secret: secret("hunter2"),
    };
    assert_eq!(
        encode(&answer(), MAX_REQUEST_FRAME).unwrap().to_vec(),
        encode_secret(&answer(), MAX_REQUEST_FRAME)
            .unwrap()
            .to_vec(),
        "encode_secret makes the same frame as encode"
    );
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
        let versions: Vec<u32> = frames.iter().map(|body| version(body)).collect();
        assert_eq!(versions, [1, 2, 3], "chunk {chunk}");
    }
}

#[test]
fn wanted_counts_down_the_header_then_the_body() {
    let mut decoder = Decoder::new(64);
    assert_eq!(decoder.wanted(), HEADER_BYTES);
    assert!(
        decoder.push(&[]).unwrap().is_none(),
        "nothing changes nothing"
    );
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
fn the_decoder_refuses_a_bad_length_before_taking_any_body() {
    let header = |length: u32| length.to_le_bytes().to_vec();
    // what each row pushes, under which limit, and what the decoder says
    type Row = (&'static str, usize, Vec<Vec<u8>>, Result<(), FrameError>);
    let rows: [Row; 6] = [
        (
            "more than the header",
            64,
            vec![vec![0; 5]],
            Err(FrameError::Overrun),
        ),
        (
            "more than the body",
            64,
            vec![header(1), b"ab".to_vec()],
            Err(FrameError::Overrun),
        ),
        ("a zero length", 64, vec![header(0)], Err(FrameError::Empty)),
        (
            "one over the limit",
            64,
            vec![header(65)],
            Err(FrameError::TooLarge),
        ),
        (
            "the largest length, without allocating or wrapping",
            MAX_EVENT_FRAME,
            vec![header(u32::MAX)],
            Err(FrameError::TooLarge),
        ),
        ("the limit itself", 64, vec![header(64)], Ok(())),
    ];
    for (row, limit, pushes, expected) in rows {
        let mut decoder = Decoder::new(limit);
        let mut got = Ok(());
        for bytes in &pushes {
            if let Err(error) = decoder.push(bytes) {
                got = Err(error);
                break;
            }
        }
        assert_eq!(got, expected, "{row}");
    }
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
fn encoding_caps_the_json_not_the_header() {
    let long_name = Request::Authenticate {
        account: Account::Typed {
            name: "x".repeat(100),
        },
    };
    assert_eq!(encode(&long_name, 40).unwrap_err(), FrameError::TooLarge);
    assert_eq!(
        encode_secret(&long_name, 40).unwrap_err(),
        FrameError::TooLarge
    );

    let json_length = encode(&Request::Cancel {}, 1000).unwrap().len() - HEADER_BYTES;
    assert!(encode(&Request::Cancel {}, json_length).is_ok());
    assert_eq!(
        encode(&Request::Cancel {}, json_length - 1).unwrap_err(),
        FrameError::TooLarge
    );

    let longest_password = encode_secret(
        &Request::Answer {
            conversation: u64::MAX,
            secret: secret(&"e".repeat(512)),
        },
        MAX_REQUEST_FRAME,
    )
    .unwrap();
    assert!(
        longest_password.len() - HEADER_BYTES <= MAX_REQUEST_FRAME,
        "the longest password fits the request cap"
    );
}

#[test]
fn decode_refuses_anything_but_one_known_message_and_names_the_class() {
    let request = |body: &[u8]| decode::<Request>(body).map(drop);
    let rows: [(&str, Result<(), _>, &str); 9] = [
        ("not JSON", request(b"{not json"), "syntax"),
        (
            "an unknown tag",
            request(br#"{"type":"NoSuchRequest"}"#),
            "data",
        ),
        ("cut off", request(br#"{"type":"Hello","version":"#), "eof"),
        (
            "an unknown field",
            request(br#"{"type":"Hello","version":5,"extra":1}"#),
            "data",
        ),
        (
            "a field on Cancel",
            request(br#"{"type":"Cancel","x":1}"#),
            "data",
        ),
        (
            "a missing field",
            decode::<Event>(br#"{"type":"Hello"}"#).map(drop),
            "data",
        ),
        ("an array", request(b"[]"), "data"),
        ("nothing", request(b""), "eof"),
        (
            "trailing bytes",
            request(br#"{"type":"Cancel"} x"#),
            "syntax",
        ),
    ];
    for (row, result, class) in rows {
        assert_eq!(result.unwrap_err().class(), class, "{row}");
    }
    assert!(request(b"null").is_err(), "null");
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
        format!(r#"{{"type":"Answer","conversation":1,"secret":"{hex_password}6"}}"#),
    ] {
        let error = decode::<Request>(body.as_bytes()).unwrap_err();
        let shown = format!("{error} {error:?}");
        assert!(
            !shown.contains("password") && !shown.contains("7061"),
            "{shown}"
        );
    }
}
