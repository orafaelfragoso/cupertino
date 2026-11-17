//! The typed messages: their JSON shape, round trips, refusals, limits, and the account name check.

use cupertino_login_ipc::message::{MAX_PICTURE_BYTES, MAX_USERS};
use cupertino_login_ipc::{
    decode, encode, is_login_name, Account, Event, Outcome, Picture, PowerAction, PowerRefusal,
    ProtocolRefusal, Request, UserRow, HEADER_BYTES, MAX_EVENT_FRAME, MAX_LOGIN_NAME_BYTES,
    MAX_REQUEST_FRAME,
};

fn json<T: serde::Serialize>(message: &T) -> String {
    serde_json::to_string(message).unwrap()
}

/// `value` encoded and decoded again is equal to itself.
fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
    value: T,
) {
    let text = json(&value);
    assert_eq!(decode::<T>(text.as_bytes()).unwrap(), value, "{text}");
}

fn row(uid: u32, picture: Option<Vec<u8>>, wallpaper: Option<&str>) -> UserRow {
    UserRow {
        uid,
        login: format!("user{uid}"),
        display_name: format!("User {uid}"),
        picture: picture.map(|b| Picture::new(b).unwrap()),
        logged_in: uid.is_multiple_of(2),
        wallpaper: wallpaper.map(str::to_owned),
    }
}

#[test]
fn every_message_has_a_stable_tagged_object_form() {
    let requests = [
        r#"{"type":"Hello","version":5}"#,
        r#"{"type":"Authenticate","account":{"type":"Listed","uid":1000}}"#,
        r#"{"type":"Authenticate","account":{"type":"Typed","name":"alice"}}"#,
        r#"{"type":"Answer","conversation":18446744073709551615,"secret":"6162"}"#,
        r#"{"type":"Cancel"}"#,
        r#"{"type":"AuthenticatePower","name":"root","action":"ShutDown"}"#,
        r#"{"type":"Power","action":"Restart"}"#,
        r#"{"type":"Power","action":"Sleep"}"#,
    ];
    for text in requests {
        let message: Request = decode(text.as_bytes()).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(json(&message), text);
    }
    let events = [
        r#"{"type":"Hello","version":5}"#,
        r#"{"type":"Welcome","users":[],"show_other":true,"power":{"restart":true,"shut_down":false,"sleep":true},"will_prompt":false,"retries_until_hint":3}"#,
        r#"{"type":"Users","users":[],"will_prompt":true}"#,
        r#"{"type":"Prompt","conversation":7,"echo":false,"text":"Password:"}"#,
        r#"{"type":"Notice","error":true,"text":"Try again"}"#,
        r#"{"type":"Result","outcome":"Accepted"}"#,
        r#"{"type":"SessionStarting","uid":1000}"#,
        r#"{"type":"Reset"}"#,
        r#"{"type":"Refused","reason":"Version"}"#,
        r#"{"type":"PowerRefused","reason":"OtherUsersLoggedIn"}"#,
    ];
    for text in events {
        let message: Event = decode(text.as_bytes()).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(json(&message), text);
    }
}

#[test]
fn every_value_round_trips() {
    for outcome in [
        Outcome::Accepted,
        Outcome::Denied,
        Outcome::Locked,
        Outcome::Expired,
        Outcome::Refused,
        Outcome::Busy,
        Outcome::Internal,
    ] {
        round_trip(outcome);
    }
    for refusal in [ProtocolRefusal::Version, ProtocolRefusal::State] {
        round_trip(refusal);
    }
    for refusal in [
        PowerRefusal::OtherUsersLoggedIn,
        PowerRefusal::Inhibited,
        PowerRefusal::Disabled,
        PowerRefusal::Failed,
    ] {
        round_trip(refusal);
    }
    for action in [
        PowerAction::Restart,
        PowerAction::ShutDown,
        PowerAction::Sleep,
    ] {
        round_trip(action);
    }
    for account in [
        Account::Listed { uid: 0 },
        Account::Listed { uid: u32::MAX },
        Account::Typed {
            name: "al\u{e9}".into(),
        },
    ] {
        round_trip(account);
    }

    let event = Event::Users {
        users: vec![
            row(
                1000,
                Some(vec![0, 1, 0xfe, 0xff]),
                Some("/var/lib/cupertino-login/wallpapers/1000.jpg"),
            ),
            row(1001, None, None),
        ],
        will_prompt: false,
    };
    let frame = encode(&event, MAX_EVENT_FRAME).unwrap();
    let back: Event = decode(&frame[HEADER_BYTES..]).unwrap();
    assert_eq!(json(&back), json(&event));
    match back {
        Event::Users { users, will_prompt } => {
            assert!(!will_prompt);
            assert_eq!(
                users[0].picture.as_ref().unwrap().bytes(),
                [0, 1, 0xfe, 0xff]
            );
            assert_eq!(users[1], row(1001, None, None));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_message_of_another_shape_is_refused() {
    let rows: [(&str, Result<(), _>); 15] = [
        ("an unknown outcome", decode::<Outcome>(br#""Allowed""#).map(drop)),
        ("an unknown power action", decode::<PowerAction>(br#""Hibernate""#).map(drop)),
        ("a negative uid", decode::<Account>(br#"{"type":"Listed","uid":-1}"#).map(drop)),
        (
            "a uid over 32 bits",
            decode::<Account>(br#"{"type":"Listed","uid":4294967296}"#).map(drop),
        ),
        (
            "a listed account with a name",
            decode::<Account>(br#"{"type":"Listed","uid":1,"name":"x"}"#).map(drop),
        ),
        ("a typed account without a name", decode::<Account>(br#"{"type":"Typed"}"#).map(drop)),
        ("a negative version", decode::<Request>(br#"{"type":"Hello","version":-1}"#).map(drop)),
        (
            "a version over 32 bits",
            decode::<Request>(br#"{"type":"Hello","version":4294967296}"#).map(drop),
        ),
        ("a fractional version", decode::<Request>(br#"{"type":"Hello","version":1.5}"#).map(drop)),
        (
            "a negative conversation",
            decode::<Event>(br#"{"type":"Prompt","conversation":-1,"echo":true,"text":""}"#)
                .map(drop),
        ),
        (
            "a user row with an unknown field",
            decode::<UserRow>(
                br#"{"uid":1,"login":"a","display_name":"A","picture":null,"logged_in":false,"x":1}"#,
            )
            .map(drop),
        ),
        (
            "a user row without a login",
            decode::<UserRow>(br#"{"uid":1,"display_name":"A","picture":null,"logged_in":false}"#)
                .map(drop),
        ),
        // Reset is a message without fields, so it refuses an unknown field like every other message
        ("Reset with a field", decode::<Event>(br#"{"type":"Reset","x":1}"#).map(drop)),
        ("Cancel with a field", decode::<Request>(br#"{"type":"Cancel","x":1}"#).map(drop)),
        ("a picture of upper case hex", decode::<Picture>(br#""AB01""#).map(drop)),
    ];
    for (row, result) in rows {
        assert!(result.is_err(), "{row} is refused");
    }
    assert!(matches!(
        decode::<Event>(br#"{"type":"Reset"}"#),
        Ok(Event::Reset {})
    ));
}

#[test]
fn a_user_row_leaves_out_what_it_does_not_have() {
    let text = json(&row(1, None, None));
    assert!(!text.contains("wallpaper"), "{text}");
    assert!(text.contains(r#""picture":null"#), "{text}");
    assert_eq!(decode::<UserRow>(text.as_bytes()).unwrap().wallpaper, None);
    let with = json(&row(1, None, Some("/w.jpg")));
    assert!(with.contains(r#""wallpaper":"/w.jpg""#), "{with}");
    let no_picture = r#"{"uid":1,"login":"a","display_name":"A","logged_in":false}"#;
    assert_eq!(
        decode::<UserRow>(no_picture.as_bytes()).unwrap().picture,
        None,
        "an absent option reads as None"
    );
}

#[test]
fn a_picture_is_capped_checked_hex_on_the_wire_and_prints_only_its_size() {
    assert!(Picture::new(vec![0; MAX_PICTURE_BYTES]).is_ok());
    assert!(Picture::new(vec![0; MAX_PICTURE_BYTES + 1]).is_err());
    assert_eq!(Picture::new(vec![]).unwrap().bytes(), b"");
    assert_eq!(
        format!("{:?}", Picture::new(vec![1, 2, 3]).unwrap()),
        "<picture 3 bytes>"
    );

    let picture = Picture::new(vec![0xab, 0x01]).unwrap();
    assert_eq!(json(&picture), r#""ab01""#);
    assert_eq!(decode::<Picture>(br#""ab01""#).unwrap(), picture);
    let over = format!("\"{}\"", "00".repeat(MAX_PICTURE_BYTES + 1));
    for bad in [r#""AB01""#, r#""abc""#, r#""xy""#, "12", "null", &over] {
        assert!(decode::<Picture>(bad.as_bytes()).is_err(), "{bad:.20}");
    }
}

#[test]
fn the_caps_hold_a_full_user_list_and_the_longest_request() {
    // the limits the module documents: 1024 rows and 1 MiB of pictures in one list
    let users: Vec<UserRow> = (0..MAX_USERS as u32)
        .map(|uid| {
            let picture = (uid < 4).then(|| vec![0xff; MAX_PICTURE_BYTES]);
            row(
                uid,
                picture,
                Some("/var/lib/cupertino-login/wallpapers/65534.jpg"),
            )
        })
        .collect();
    let event = Event::Users {
        users,
        will_prompt: true,
    };
    assert!(encode(&event, MAX_EVENT_FRAME).is_ok());
    assert!(
        encode(&event, MAX_REQUEST_FRAME).is_err(),
        "a request that fits the request cap is never a user list"
    );
    let request = Request::Authenticate {
        account: Account::Typed {
            name: "x".repeat(MAX_LOGIN_NAME_BYTES),
        },
    };
    assert!(encode(&request, MAX_REQUEST_FRAME).is_ok());
}

#[test]
fn login_names_follow_useradd() {
    let longest = "x".repeat(MAX_LOGIN_NAME_BYTES);
    let too_long = "x".repeat(MAX_LOGIN_NAME_BYTES + 1);
    // 16 two-byte characters are 32 bytes; 17 are 34
    let longest_wide = "\u{e9}".repeat(MAX_LOGIN_NAME_BYTES / 2);
    let too_wide = "\u{e9}".repeat(MAX_LOGIN_NAME_BYTES / 2 + 1);
    for good in [
        "a",
        "alice",
        "user.name",
        "user_name-1",
        "a$",
        "\u{e9}t\u{e9}",
        &longest,
        &longest_wide,
    ] {
        assert!(is_login_name(good), "{good}");
    }
    for bad in [
        "",
        &too_long,
        &too_wide,
        "-a",
        "a b",
        " a",
        "a ",
        "a\tb",
        "a\nb",
        "a\0b",
        "a:b",
        "a/b",
        "a\\b",
        "../x",
        "a\u{7f}",
        "a\u{a0}b",
        "a\u{2003}b",
    ] {
        assert!(!is_login_name(bad), "{bad:?}");
    }
}
