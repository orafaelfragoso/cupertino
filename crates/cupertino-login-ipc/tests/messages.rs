//! The typed messages: their JSON shape, round trips, limits, and the account name check.

use cupertino_login_ipc::{
    decode, encode, is_login_name, Account, Event, Outcome, Picture, PowerAction, PowerRefusal,
    ProtocolRefusal, Request, UserRow, MAX_EVENT_FRAME, MAX_LOGIN_NAME_BYTES, MAX_REQUEST_FRAME,
    SOCKET_PATH, VERSION,
};

fn json<T: serde::Serialize>(message: &T) -> String {
    serde_json::to_string(message).unwrap()
}

/// `text` decoded and encoded again is the same text.
fn stable<T: serde::Serialize + serde::de::DeserializeOwned>(text: &str) {
    let message: T = decode(text.as_bytes()).unwrap_or_else(|e| panic!("{text}: {e}"));
    assert_eq!(json(&message), text);
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
fn the_constants_are_the_ones_both_sides_ship_with() {
    assert_eq!(VERSION, 5);
    assert_eq!(SOCKET_PATH, "/run/cupertino-login/greeter.sock");
    assert_eq!(MAX_REQUEST_FRAME, 2048);
    assert_eq!(MAX_EVENT_FRAME, 4 * 1024 * 1024);
    assert_eq!(MAX_LOGIN_NAME_BYTES, 32);
}

#[test]
fn every_request_has_a_tagged_object_form() {
    stable::<Request>(r#"{"type":"Hello","version":5}"#);
    stable::<Request>(r#"{"type":"Authenticate","account":{"type":"Listed","uid":1000}}"#);
    stable::<Request>(r#"{"type":"Authenticate","account":{"type":"Typed","name":"alice"}}"#);
    stable::<Request>(r#"{"type":"Answer","conversation":18446744073709551615,"secret":"6162"}"#);
    stable::<Request>(r#"{"type":"Cancel"}"#);
    stable::<Request>(r#"{"type":"AuthenticatePower","name":"root","action":"ShutDown"}"#);
    stable::<Request>(r#"{"type":"Power","action":"Restart"}"#);
    stable::<Request>(r#"{"type":"Power","action":"Sleep"}"#);
}

#[test]
fn every_event_has_a_tagged_object_form() {
    stable::<Event>(r#"{"type":"Hello","version":5}"#);
    stable::<Event>(
        r#"{"type":"Welcome","users":[],"show_other":true,"power":{"restart":true,"shut_down":false,"sleep":true},"will_prompt":false,"retries_until_hint":3}"#,
    );
    stable::<Event>(r#"{"type":"Users","users":[],"will_prompt":true}"#);
    stable::<Event>(r#"{"type":"Prompt","conversation":7,"echo":false,"text":"Password:"}"#);
    stable::<Event>(r#"{"type":"Notice","error":true,"text":"Try again"}"#);
    stable::<Event>(r#"{"type":"Result","outcome":"Accepted"}"#);
    stable::<Event>(r#"{"type":"SessionStarting","uid":1000}"#);
    stable::<Event>(r#"{"type":"Reset"}"#);
    stable::<Event>(r#"{"type":"Refused","reason":"Version"}"#);
    stable::<Event>(r#"{"type":"PowerRefused","reason":"OtherUsersLoggedIn"}"#);
}

#[test]
fn every_outcome_and_refusal_round_trips() {
    for outcome in [
        Outcome::Accepted,
        Outcome::Denied,
        Outcome::Locked,
        Outcome::Expired,
        Outcome::Refused,
        Outcome::Busy,
        Outcome::Internal,
    ] {
        let text = json(&outcome);
        assert_eq!(decode::<Outcome>(text.as_bytes()).unwrap(), outcome);
    }
    for refusal in [ProtocolRefusal::Version, ProtocolRefusal::State] {
        let text = json(&refusal);
        assert_eq!(decode::<ProtocolRefusal>(text.as_bytes()).unwrap(), refusal);
    }
    for refusal in [
        PowerRefusal::OtherUsersLoggedIn,
        PowerRefusal::Inhibited,
        PowerRefusal::Disabled,
        PowerRefusal::Failed,
    ] {
        let text = json(&refusal);
        assert_eq!(decode::<PowerRefusal>(text.as_bytes()).unwrap(), refusal);
    }
    for action in [
        PowerAction::Restart,
        PowerAction::ShutDown,
        PowerAction::Sleep,
    ] {
        let text = json(&action);
        assert_eq!(decode::<PowerAction>(text.as_bytes()).unwrap(), action);
    }
    assert!(decode::<Outcome>(br#""Allowed""#).is_err());
    assert!(decode::<PowerAction>(br#""Hibernate""#).is_err());
}

#[test]
fn accounts_round_trip_and_refuse_other_shapes() {
    for account in [
        Account::Listed { uid: 0 },
        Account::Listed { uid: u32::MAX },
        Account::Typed {
            name: "al\u{e9}".into(),
        },
    ] {
        let text = json(&account);
        assert_eq!(decode::<Account>(text.as_bytes()).unwrap(), account);
    }
    assert!(decode::<Account>(br#"{"type":"Listed","uid":-1}"#).is_err());
    assert!(decode::<Account>(br#"{"type":"Listed","uid":4294967296}"#).is_err());
    assert!(decode::<Account>(br#"{"type":"Listed","uid":1,"name":"x"}"#).is_err());
    assert!(decode::<Account>(br#"{"type":"Typed"}"#).is_err());
}

#[test]
fn numbers_out_of_range_are_refused() {
    assert!(decode::<Request>(br#"{"type":"Hello","version":-1}"#).is_err());
    assert!(decode::<Request>(br#"{"type":"Hello","version":4294967296}"#).is_err());
    assert!(decode::<Request>(br#"{"type":"Hello","version":1.5}"#).is_err());
    assert!(
        decode::<Event>(br#"{"type":"Prompt","conversation":-1,"echo":true,"text":""}"#).is_err()
    );
}

#[test]
fn a_user_list_with_pictures_round_trips() {
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
    let back: Event = decode(&frame[4..]).unwrap();
    assert_eq!(json(&back), json(&event));
    match back {
        Event::Users { users, will_prompt } => {
            assert!(!will_prompt);
            assert_eq!(users.len(), 2);
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
fn a_missing_wallpaper_is_absent_from_the_json_and_reads_back_as_none() {
    let text = json(&row(1, None, None));
    assert!(!text.contains("wallpaper"), "{text}");
    assert!(text.contains(r#""picture":null"#), "{text}");
    let back: UserRow = decode(text.as_bytes()).unwrap();
    assert_eq!(back.wallpaper, None);
    let with = json(&row(1, None, Some("/w.jpg")));
    assert!(with.contains(r#""wallpaper":"/w.jpg""#));
}

#[test]
fn a_user_row_refuses_unknown_and_missing_fields() {
    let good = r#"{"uid":1,"login":"a","display_name":"A","picture":null,"logged_in":false}"#;
    assert!(decode::<UserRow>(good.as_bytes()).is_ok());
    let extra =
        r#"{"uid":1,"login":"a","display_name":"A","picture":null,"logged_in":false,"x":1}"#;
    assert!(decode::<UserRow>(extra.as_bytes()).is_err());
    let missing = r#"{"uid":1,"login":"a","display_name":"A","logged_in":false}"#;
    assert!(
        decode::<UserRow>(missing.as_bytes()).is_ok(),
        "an absent option reads as None"
    );
    let no_login = r#"{"uid":1,"display_name":"A","picture":null,"logged_in":false}"#;
    assert!(decode::<UserRow>(no_login.as_bytes()).is_err());
}

#[test]
fn a_picture_is_capped_and_prints_only_its_size() {
    let largest = 256 * 1024;
    assert!(Picture::new(vec![0; largest]).is_ok());
    assert!(Picture::new(vec![0; largest + 1]).is_err());
    assert_eq!(
        format!("{:?}", Picture::new(vec![1, 2, 3]).unwrap()),
        "<picture 3 bytes>"
    );
    assert_eq!(Picture::new(vec![]).unwrap().bytes(), b"");
}

#[test]
fn a_picture_on_the_wire_is_lower_case_hex_and_checked() {
    let picture = Picture::new(vec![0xab, 0x01]).unwrap();
    assert_eq!(json(&picture), r#""ab01""#);
    assert_eq!(decode::<Picture>(br#""ab01""#).unwrap(), picture);
    for bad in [r#""AB01""#, r#""abc""#, r#""xy""#, "12", "null"] {
        assert!(decode::<Picture>(bad.as_bytes()).is_err(), "{bad}");
    }
    let over = format!("\"{}\"", "00".repeat(256 * 1024 + 1));
    assert!(decode::<Picture>(over.as_bytes()).is_err());
}

#[test]
fn the_event_cap_holds_a_full_user_list() {
    // the limits the module documents: 1024 rows and 1 MiB of pictures in one list
    let users: Vec<UserRow> = (0..1024)
        .map(|uid| {
            let picture = (uid < 4).then(|| vec![0xff; 256 * 1024]);
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
    // the same list does not fit the other direction's cap
    assert!(encode(&event, MAX_REQUEST_FRAME).is_err());
}

#[test]
fn login_names_follow_useradd() {
    for good in [
        "a",
        "alice",
        "user.name",
        "user_name-1",
        "a$",
        &"x".repeat(32),
        "\u{e9}t\u{e9}",
    ] {
        assert!(is_login_name(good), "{good}");
    }
    assert!(!is_login_name(""));
    assert!(!is_login_name(&"x".repeat(33)));
    for bad in [
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

#[test]
fn the_name_limit_counts_bytes() {
    // 16 two-byte characters are 32 bytes; 17 are 34
    assert!(is_login_name(&"\u{e9}".repeat(16)));
    assert!(!is_login_name(&"\u{e9}".repeat(17)));
}

#[test]
fn a_request_that_fits_the_request_cap_is_never_a_user_list() {
    let request = Request::Authenticate {
        account: Account::Typed {
            name: "x".repeat(MAX_LOGIN_NAME_BYTES),
        },
    };
    assert!(encode(&request, MAX_REQUEST_FRAME).is_ok());
}

#[test]
fn a_message_without_fields_refuses_an_unknown_field() {
    assert!(matches!(
        decode::<Event>(br#"{"type":"Reset"}"#),
        Ok(Event::Reset {})
    ));
    assert!(decode::<Event>(br#"{"type":"Reset","x":1}"#).is_err());
    assert!(decode::<Request>(br#"{"type":"Cancel","x":1}"#).is_err());
}
