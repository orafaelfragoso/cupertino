//! The FileChooser backend on a private bus, called as xdg-desktop-portal calls it, with a dialog the test answers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Mutex;

use cupertino_portal::file_chooser::{Answer, Event, FileChooser, Filter, Kind, Pattern};
use cupertino_portal::request::FRONTEND;
use cupertino_portal::{BUS_NAME, PORTAL_PATH};
use cupertino_test_bus::{missing_tool, PrivateBus, WAIT};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const INTERFACE: &str = "org.freedesktop.impl.portal.FileChooser";
const HANDLE: &str = "/org/freedesktop/portal/desktop/request/1_7/t1";

type Results = HashMap<String, OwnedValue>;
type Call = zbus::Result<(u32, Results)>;

struct Running {
    _bus: PrivateBus,
    _served: Connection,
    events: mpsc::Receiver<Event>,
    frontend: Connection,
    app: Connection,
}

fn start() -> Option<Running> {
    let bus = PrivateBus::start()?;
    let served = bus.connect();
    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    served
        .object_server()
        .at(
            PORTAL_PATH,
            FileChooser::new(move |event| {
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
        events,
        frontend,
        app,
    })
}

fn call(caller: &Connection, method: &'static str, options: Results) -> mpsc::Receiver<Call> {
    let caller = caller.clone();
    let (sender, done) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = ObjectPath::try_from(HANDLE).unwrap();
        let reply = caller
            .call_method(
                Some(BUS_NAME),
                PORTAL_PATH,
                Some(INTERFACE),
                method,
                &(handle, "org.example.Editor", "", "Open a Document", options),
            )
            .and_then(|m| m.body().deserialize::<(u32, Results)>());
        let _ = sender.send(reply);
    });
    done
}

fn owned(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).unwrap()
}

fn bytes(path: &str) -> OwnedValue {
    let mut bytes = path.as_bytes().to_vec();
    bytes.push(0);
    owned(Value::from(bytes))
}

fn images() -> (String, Vec<(u32, String)>) {
    (
        "Images".to_owned(),
        vec![(0, "*.png".to_owned()), (1, "image/jpeg".to_owned())],
    )
}

fn uris(results: &Results) -> Vec<String> {
    Vec::<String>::try_from(results["uris"].try_clone().unwrap()).unwrap()
}

#[test]
fn an_open_shows_the_options_and_answers_the_chosen_files_as_uris() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let options = HashMap::from([
        ("multiple".to_owned(), owned(Value::from(true))),
        ("accept_label".to_owned(), owned(Value::from("_Attach"))),
        ("filters".to_owned(), owned(Value::from(vec![images()]))),
        ("current_folder".to_owned(), bytes("/home/me/Pictures")),
        (
            "choices".to_owned(),
            owned(Value::from(vec![(
                "encoding",
                "Encoding",
                vec![("utf8", "Unicode"), ("latin1", "Western")],
                "utf8",
            )])),
        ),
    ]);
    let done = call(&r.frontend, "OpenFile", options);
    let Event::Ask { handle, ask, reply } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a dialog");
    };
    assert_eq!(handle.as_str(), HANDLE);
    assert_eq!(ask.kind, Kind::Open);
    assert_eq!(ask.app_id, "org.example.Editor");
    assert_eq!(ask.title, "Open a Document");
    assert_eq!(ask.accept_label.as_deref(), Some("_Attach"));
    assert!(ask.modal && ask.multiple && !ask.directory);
    assert_eq!(ask.current_folder, Some(PathBuf::from("/home/me/Pictures")));
    let filter = Filter {
        name: "Images".to_owned(),
        patterns: vec![
            Pattern::Glob("*.png".to_owned()),
            Pattern::Mime("image/jpeg".to_owned()),
        ],
    };
    assert_eq!(ask.filters, std::slice::from_ref(&filter));
    assert_eq!(ask.choices[0].options.len(), 2);

    reply.send(Answer::Chosen {
        paths: vec![
            PathBuf::from("/home/me/Pictures/a b.png"),
            PathBuf::from("/home/me/Pictures/c.png"),
        ],
        filter: Some(filter),
        choices: vec![("encoding".to_owned(), "latin1".to_owned())],
    });
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 0);
    assert_eq!(
        uris(&results),
        [
            "file:///home/me/Pictures/a%20b.png",
            "file:///home/me/Pictures/c.png"
        ]
    );
    let current: (String, Vec<(u32, String)>) = results["current_filter"]
        .try_clone()
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(current, images());
    let choices: Vec<(String, String)> =
        results["choices"].try_clone().unwrap().try_into().unwrap();
    assert_eq!(choices, [("encoding".to_owned(), "latin1".to_owned())]);
}

#[test]
fn a_save_answers_one_file_and_a_cancel_answers_1() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let options = HashMap::from([
        (
            "current_name".to_owned(),
            owned(Value::from("Untitled.txt")),
        ),
        ("multiple".to_owned(), owned(Value::from(true))),
    ]);
    let done = call(&r.frontend, "SaveFile", options);
    let Event::Ask { ask, reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a dialog");
    };
    assert_eq!(ask.kind, Kind::Save);
    assert_eq!(ask.current_name.as_deref(), Some("Untitled.txt"));
    assert!(!ask.multiple, "a save never picks several files");
    reply.send(Answer::Chosen {
        paths: vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")],
        filter: None,
        choices: Vec::new(),
    });
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 0);
    assert_eq!(uris(&results), ["file:///tmp/a.txt"]);

    let done = call(&r.frontend, "OpenFile", HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a dialog");
    };
    reply.send(Answer::Cancelled);
    let (response, results) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!((response, results.len()), (1, 0));
}

#[test]
fn closing_the_request_takes_the_dialog_down_and_answers_2() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = call(&r.frontend, "OpenFile", HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a dialog");
    };
    let close = |caller: &Connection| {
        caller.call_method(
            Some(BUS_NAME),
            HANDLE,
            Some("org.freedesktop.impl.portal.Request"),
            "Close",
            &(),
        )
    };
    assert!(close(&r.app).is_err(), "only the frontend closes a request");
    close(&r.frontend).unwrap();
    let Event::Close { handle } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected the dialog to close");
    };
    assert_eq!(handle.as_str(), HANDLE);
    let (response, _) = done.recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(response, 2);
    drop(reply);
    assert!(close(&r.frontend).is_err(), "the request object is gone");
}

#[test]
fn only_the_frontend_may_ask_and_bad_options_are_refused_before_a_dialog() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let error = |result: Call| match result {
        Err(zbus::Error::MethodError(name, _, _)) => name.to_string(),
        other => panic!("expected an error, got {other:?}"),
    };
    let done = call(&r.app, "OpenFile", HashMap::new());
    assert_eq!(
        error(done.recv_timeout(WAIT).unwrap()),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    for (key, value) in [
        ("current_name", owned(Value::from("../escape.txt"))),
        (
            "filters",
            owned(Value::from(vec![("Odd", vec![(7u32, "*")])])),
        ),
        (
            "filters",
            owned(Value::from(vec![("Long", vec![(0u32, "*?".repeat(129))])])),
        ),
        ("current_folder", bytes("relative/folder")),
        ("multiple", owned(Value::from("yes"))),
    ] {
        let done = call(
            &r.frontend,
            "SaveFile",
            HashMap::from([(key.to_owned(), value)]),
        );
        assert_eq!(
            error(done.recv_timeout(WAIT).unwrap()),
            "org.freedesktop.DBus.Error.InvalidArgs",
            "{key}"
        );
    }
    assert!(r.events.try_recv().is_err(), "no dialog was shown");
}

#[test]
fn the_portal_file_and_the_portals_conf_name_the_file_chooser() {
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
