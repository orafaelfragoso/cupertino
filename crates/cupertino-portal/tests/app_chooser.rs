//! The AppChooser backend on a private bus, called as xdg-desktop-portal calls it, with a chooser the test answers.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Mutex;

use cupertino_portal::app_chooser::{Answer, AppChooser, Choice, Event};
use cupertino_portal::request::FRONTEND;
use cupertino_portal::{BUS_NAME, PORTAL_PATH};
use cupertino_test_bus::{missing_tool, PrivateBus, WAIT};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const INTERFACE: &str = "org.freedesktop.impl.portal.AppChooser";
const HANDLE: &str = "/org/freedesktop/portal/desktop/request/1_7/t1";

type Results = HashMap<String, OwnedValue>;
type Call = zbus::Result<(u32, Results)>;

struct Running {
    _bus: PrivateBus,
    _served: Connection,
    _applications: tempfile::TempDir,
    events: mpsc::Receiver<Event>,
    frontend: Connection,
    app: Connection,
}

fn start() -> Option<Running> {
    let bus = PrivateBus::start()?;
    let applications = tempfile::tempdir().unwrap();
    for (id, name, icon) in [
        (
            "org.example.Viewer",
            "Document Viewer",
            "org.example.Viewer",
        ),
        ("org.example.Paths", "Paths", "/opt/paths/icon.png"),
        ("org.example.Escape", "Escape", "escape\u{1b}[2J"),
    ] {
        std::fs::write(
            applications.path().join(format!("{id}.desktop")),
            format!(
                "[Desktop Entry]\nType=Application\nName={name}\nIcon={icon}\nExec=viewer %U\n"
            ),
        )
        .unwrap();
    }
    let served = bus.connect();
    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    served
        .object_server()
        .at(
            PORTAL_PATH,
            AppChooser::new(vec![applications.path().to_path_buf()], move |event| {
                let _ = sender.lock().unwrap().send(event);
            }),
        )
        .unwrap();
    served.request_name(BUS_NAME).unwrap();
    let frontend = bus.connect();
    frontend.request_name(FRONTEND).unwrap();
    let app = bus.connect();
    Some(Running {
        _bus: bus,
        _served: served,
        _applications: applications,
        events,
        frontend,
        app,
    })
}

fn choices() -> Vec<&'static str> {
    vec!["org.example.Viewer", "org.example.Draw"]
}

fn call(caller: &Connection, choices: Vec<&'static str>, options: Results) -> mpsc::Receiver<Call> {
    let caller = caller.clone();
    let (sender, done) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = ObjectPath::try_from(HANDLE).unwrap();
        let reply = caller
            .call_method(
                Some(BUS_NAME),
                PORTAL_PATH,
                Some(INTERFACE),
                "ChooseApplication",
                &(handle, "org.example.Mail", "", choices, options),
            )
            .and_then(|m| m.body().deserialize::<(u32, Results)>());
        let _ = sender.send(reply);
    });
    done
}

fn update(caller: &Connection, choices: Vec<&str>) -> zbus::Result<()> {
    let handle = ObjectPath::try_from(HANDLE).unwrap();
    caller
        .call_method(
            Some(BUS_NAME),
            PORTAL_PATH,
            Some(INTERFACE),
            "UpdateChoices",
            &(handle, choices),
        )
        .map(drop)
}

fn owned(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).unwrap()
}

fn string(results: &Results, key: &str) -> String {
    String::try_from(results[key].try_clone().unwrap()).unwrap()
}

fn error<T: std::fmt::Debug>(result: zbus::Result<T>) -> String {
    match result {
        Err(zbus::Error::MethodError(name, _, _)) => name.to_string(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn choice(id: &str, name: &str, icon: Option<&str>) -> Choice {
    Choice {
        id: id.to_owned(),
        name: name.to_owned(),
        icon: icon.map(str::to_owned),
    }
}

#[test]
fn the_chosen_app_reaches_the_caller_with_the_activation_token() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let options = HashMap::from([
        (
            "last_choice".to_owned(),
            owned(Value::from("org.example.Draw")),
        ),
        (
            "uri".to_owned(),
            owned(Value::from("file:///home/me/report.pdf")),
        ),
        ("filename".to_owned(), owned(Value::from("report.pdf"))),
        (
            "content_type".to_owned(),
            owned(Value::from("application/pdf")),
        ),
        ("activation_token".to_owned(), owned(Value::from("token-1"))),
    ]);
    let done = call(&r.frontend, choices(), options);
    let Event::Ask { handle, ask, reply } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    assert_eq!(handle.as_str(), HANDLE);
    assert_eq!(ask.app_id, "org.example.Mail");
    assert_eq!(ask.target.as_deref(), Some("report.pdf"));
    assert_eq!(
        ask.choices,
        [
            choice(
                "org.example.Viewer",
                "Document Viewer",
                Some("org.example.Viewer")
            ),
            choice("org.example.Draw", "org.example.Draw", None),
        ],
        "an installed app shows its name and icon, any other its id"
    );
    assert_eq!(ask.last_choice.as_deref(), Some("org.example.Draw"));
    assert!(!ask.wants_token, "the caller sent a token of its own");

    // a dialog that made one anyway does not replace the caller's
    reply.send(Answer::Chosen {
        id: "org.example.Viewer".to_owned(),
        token: Some("made-by-the-server".to_owned()),
    });
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 0);
    assert_eq!(string(&results, "choice"), "org.example.Viewer");
    assert_eq!(string(&results, "activation_token"), "token-1");
}

#[test]
fn without_a_caller_token_the_choice_carries_the_one_the_server_made() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = call(&r.frontend, choices(), HashMap::new());
    let Event::Ask { ask, reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    assert!(ask.wants_token);
    reply.send(Answer::Chosen {
        id: "org.example.Viewer".to_owned(),
        token: Some("server-token-7".to_owned()),
    });
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 0);
    assert_eq!(string(&results, "choice"), "org.example.Viewer");
    assert_eq!(string(&results, "activation_token"), "server-token-7");

    // the server gave none: the app still opens, without one
    let done = call(&r.frontend, choices(), HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    reply.send(Answer::Chosen {
        id: "org.example.Draw".to_owned(),
        token: None,
    });
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 0);
    assert_eq!(string(&results, "choice"), "org.example.Draw");
    assert!(!results.contains_key("activation_token"));
}

#[test]
fn a_cancel_answers_1_and_a_choice_not_offered_answers_2_without_a_choice() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = call(&r.frontend, choices(), HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    reply.send(Answer::Cancelled);
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!((response, results.len()), (1, 0));

    let done = call(&r.frontend, choices(), HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    reply.send(Answer::Chosen {
        id: "org.example.Elsewhere".to_owned(),
        token: Some("server-token".to_owned()),
    });
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!((response, results.len()), (2, 0));
}

#[test]
fn update_choices_replaces_what_the_chooser_offers() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = call(&r.frontend, choices(), HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    assert_eq!(
        error(update(&r.app, vec!["org.example.Draw"])),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    update(&r.frontend, vec!["org.example.Viewer", "org.example.Notes"]).unwrap();
    let Event::Update { handle, choices } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected new choices");
    };
    assert_eq!(handle.as_str(), HANDLE);
    assert_eq!(
        choices,
        [
            choice(
                "org.example.Viewer",
                "Document Viewer",
                Some("org.example.Viewer")
            ),
            choice("org.example.Notes", "org.example.Notes", None),
        ]
    );

    reply.send(Answer::Chosen {
        id: "org.example.Draw".to_owned(),
        token: None,
    });
    let (response, _) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 2, "a choice the update took away is refused");
    assert_eq!(
        error(update(&r.frontend, vec!["org.example.Viewer"])),
        "org.freedesktop.DBus.Error.InvalidArgs",
        "no chooser is open at the handle any more"
    );
}

#[test]
fn an_icon_with_a_path_separator_or_a_control_character_is_left_out() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = call(
        &r.frontend,
        vec!["org.example.Paths", "org.example.Escape"],
        HashMap::new(),
    );
    let Event::Ask { ask, reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    assert_eq!(
        ask.choices,
        [
            choice("org.example.Paths", "Paths", None),
            choice("org.example.Escape", "Escape", None),
        ]
    );
    reply.send(Answer::Cancelled);
    done.recv_timeout(WAIT).unwrap().unwrap();
}

#[test]
fn only_the_frontend_may_ask_and_bad_arguments_are_refused_before_a_chooser() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = call(&r.app, choices(), HashMap::new());
    assert_eq!(
        error(done.recv_timeout(WAIT).unwrap()),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    for (choices, key, value) in [
        (vec!["../../bin/sh"], "modal", owned(Value::from(true))),
        (vec![""], "modal", owned(Value::from(true))),
        (choices(), "modal", owned(Value::from("yes"))),
        (choices(), "filename", owned(Value::from("x".repeat(1025)))),
    ] {
        let done = call(
            &r.frontend,
            choices.clone(),
            HashMap::from([(key.to_owned(), value)]),
        );
        assert_eq!(
            error(done.recv_timeout(WAIT).unwrap()),
            "org.freedesktop.DBus.Error.InvalidArgs",
            "{choices:?} {key}"
        );
    }
    assert!(r.events.try_recv().is_err(), "no chooser was shown");
}

#[test]
fn the_portal_file_and_the_portals_conf_name_the_app_chooser() {
    let file = include_str!("../data/cupertino.portal");
    let interfaces = file
        .lines()
        .find_map(|l| l.strip_prefix("Interfaces="))
        .unwrap();
    assert!(interfaces.split(';').any(|i| i == INTERFACE));
    let conf = include_str!("../data/cupertino-portals.conf");
    let backends = conf
        .lines()
        .find_map(|l| l.strip_prefix(INTERFACE)?.strip_prefix('='))
        .unwrap();
    assert_eq!(
        backends.split(';').next(),
        Some(BUS_NAME.rsplit('.').next().unwrap())
    );
}

#[test]
fn a_cancel_and_a_close_carry_no_token() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let with_token =
        || HashMap::from([("activation_token".to_owned(), owned(Value::from("token-1")))]);
    let done = call(&r.frontend, choices(), with_token());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    reply.send(Answer::Cancelled);
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 1);
    assert!(results.is_empty(), "{results:?}");

    let done = call(&r.frontend, choices(), with_token());
    // the chooser stays open: its reply is kept until the close
    let Event::Ask { handle, reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    r.frontend
        .call_method(
            Some(BUS_NAME),
            handle.as_str(),
            Some("org.freedesktop.impl.portal.Request"),
            "Close",
            &(),
        )
        .unwrap();
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 2);
    assert!(results.is_empty(), "{results:?}");
    let Event::Close { handle: closed } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected the chooser to be taken down");
    };
    assert_eq!(closed, handle);
    drop(reply);
}
