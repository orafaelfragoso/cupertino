//! The ScreenCast backend on a private bus, called as xdg-desktop-portal calls it, with a chooser and a capture side
//! the test answers.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Mutex;

use cupertino_portal::request::FRONTEND;
use cupertino_portal::screen_cast::{
    Answer, Capture, CursorMode, Event, ScreenCast, Source, SourceTypes, Stream,
};
use cupertino_portal::{BUS_NAME, PORTAL_PATH};
use cupertino_test_bus::{missing_tool, PrivateBus, WAIT};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const INTERFACE: &str = "org.freedesktop.impl.portal.ScreenCast";
const SESSION: &str = "/org/freedesktop/portal/desktop/session/1_7/s1";
const START: &str = "/org/freedesktop/portal/desktop/request/1_7/t3";
const APP: &str = "org.example.Meeting";

type Results = HashMap<String, OwnedValue>;
type Call = zbus::Result<(u32, Results)>;
type Streams = Vec<(u32, HashMap<String, OwnedValue>)>;

struct Running {
    _bus: PrivateBus,
    _served: Connection,
    events: mpsc::Receiver<Event>,
    shares: mpsc::Receiver<Capture>,
    frontend: Connection,
    app: Connection,
}

fn display() -> Source {
    Source::Display {
        output: 42,
        name: "DP-1".to_owned(),
        description: "Studio Display".to_owned(),
    }
}

fn window() -> Source {
    Source::Window {
        identifier: "toplevel-7".to_owned(),
        title: "Notes\u{202e}txt".to_owned(),
        app_id: "org.example.Notes".to_owned(),
    }
}

fn start() -> Option<Running> {
    let bus = PrivateBus::start()?;
    let served = bus.connect();
    let (events_sender, events) = mpsc::channel();
    let (shares_sender, shares) = mpsc::channel();
    let (events_sender, shares_sender) = (Mutex::new(events_sender), Mutex::new(shares_sender));
    ScreenCast::new(
        || vec![display(), window()],
        move |event| {
            let _ = events_sender.lock().unwrap().send(event);
        },
        move |share| {
            let _ = shares_sender.lock().unwrap().send(share);
        },
    )
    .serve(&served)
    .unwrap();
    served.request_name(BUS_NAME).unwrap();
    let frontend = bus.connect();
    frontend.request_name(FRONTEND).unwrap();
    let app = bus.connect();
    Some(Running {
        _bus: bus,
        _served: served,
        events,
        shares,
        frontend,
        app,
    })
}

fn owned(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).unwrap()
}

/// Call `method` on another thread, so the test can answer the chooser meanwhile.
fn call(
    caller: &Connection,
    method: &'static str,
    handle: &'static str,
    options: Results,
) -> mpsc::Receiver<Call> {
    let caller = caller.clone();
    let (sender, done) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = ObjectPath::try_from(handle).unwrap();
        let session = ObjectPath::try_from(SESSION).unwrap();
        let reply = if method == "Start" {
            caller.call_method(
                Some(BUS_NAME),
                PORTAL_PATH,
                Some(INTERFACE),
                method,
                &(handle, session, APP, "wayland:ab12", options),
            )
        } else {
            caller.call_method(
                Some(BUS_NAME),
                PORTAL_PATH,
                Some(INTERFACE),
                method,
                &(handle, session, APP, options),
            )
        };
        let _ = sender.send(reply.and_then(|m| m.body().deserialize::<(u32, Results)>()));
    });
    done
}

fn answered(done: mpsc::Receiver<Call>) -> (u32, Results) {
    done.recv_timeout(WAIT).unwrap().unwrap()
}

fn error(result: Call) -> String {
    match result {
        Err(zbus::Error::MethodError(name, _, _)) => name.to_string(),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// CreateSession and SelectSources for windows and displays with the cursor in the stream.
fn select(r: &Running) {
    let (response, _) = answered(call(
        &r.frontend,
        "CreateSession",
        "/org/freedesktop/portal/desktop/request/1_7/t1",
        HashMap::new(),
    ));
    assert_eq!(response, 0);
    let options = HashMap::from([
        ("types".to_owned(), owned(Value::from(3u32))),
        ("cursor_mode".to_owned(), owned(Value::from(2u32))),
        ("multiple".to_owned(), owned(Value::from(false))),
        ("persist_mode".to_owned(), owned(Value::from(2u32))),
    ]);
    let (response, _) = answered(call(
        &r.frontend,
        "SelectSources",
        "/org/freedesktop/portal/desktop/request/1_7/t2",
        options,
    ));
    assert_eq!(response, 0);
}

fn session_close(caller: &Connection) -> zbus::Result<zbus::message::Message> {
    caller.call_method(
        Some(BUS_NAME),
        SESSION,
        Some("org.freedesktop.impl.portal.Session"),
        "Close",
        &(),
    )
}

#[test]
fn start_shares_the_chosen_display_as_a_stream_until_the_session_closes() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    select(&r);
    let done = call(&r.frontend, "Start", START, HashMap::new());
    let Event::Ask { handle, ask, reply } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    assert_eq!(handle.as_str(), START);
    assert_eq!(ask.app_id, APP);
    assert_eq!(ask.parent_window, "wayland:ab12");
    assert_eq!(
        ask.sources,
        [
            display(),
            Source::Window {
                identifier: "toplevel-7".to_owned(),
                title: "Notestxt".to_owned(),
                app_id: "org.example.Notes".to_owned(),
            }
        ],
        "a window title loses its direction override"
    );
    reply.send(Answer::Chosen(display()));
    let Capture::Start {
        session,
        source,
        cursor,
        started,
        running,
    } = r.shares.recv_timeout(WAIT).unwrap()
    else {
        panic!("expected a share");
    };
    assert_eq!(session.as_str(), SESSION);
    assert_eq!((source, cursor), (display(), CursorMode::Embedded));
    started.send(Ok(Stream {
        node: 57,
        source_type: SourceTypes::MONITOR,
        size: (2560, 1440),
        position: Some((-2560, 0)),
    }));
    let (response, results) = answered(done);
    assert_eq!(response, 0);
    let streams: Streams = results["streams"].try_clone().unwrap().try_into().unwrap();
    assert_eq!(streams.len(), 1);
    let (node, properties) = &streams[0];
    assert_eq!(*node, 57);
    let property = |key: &str| properties[key].try_clone().unwrap();
    assert_eq!(u32::try_from(property("source_type")).unwrap(), 1);
    assert_eq!(
        <(i32, i32)>::try_from(property("size")).unwrap(),
        (2560, 1440)
    );
    assert_eq!(
        <(i32, i32)>::try_from(property("position")).unwrap(),
        (-2560, 0)
    );
    assert_eq!(
        u32::try_from(results["persist_mode"].try_clone().unwrap()).unwrap(),
        0,
        "nothing persists, whatever was asked"
    );
    assert!(!results.contains_key("restore_data"));

    assert!(
        session_close(&r.app).is_err(),
        "only the frontend closes a session"
    );
    session_close(&r.frontend).unwrap();
    let Capture::Stop { session } = r.shares.recv_timeout(WAIT).unwrap() else {
        panic!("expected the share to stop");
    };
    assert_eq!(session.as_str(), SESSION);
    drop(running);
    let gone = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        session_close(&r.frontend).is_err()
    });
    assert!(gone, "the session object is gone");
}

#[test]
fn a_share_that_ends_on_its_own_closes_the_session_with_closed() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let closed = zbus::blocking::MessageIterator::for_match_rule(
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.impl.portal.Session")
            .unwrap()
            .member("Closed")
            .unwrap()
            .path(SESSION)
            .unwrap()
            .build(),
        &r.frontend,
        None,
    )
    .unwrap();
    select(&r);
    let done = call(&r.frontend, "Start", START, HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    reply.send(Answer::Chosen(window()));
    let Capture::Start {
        started, running, ..
    } = r.shares.recv_timeout(WAIT).unwrap()
    else {
        panic!("expected a share");
    };
    started.send(Ok(Stream {
        node: 58,
        source_type: SourceTypes::WINDOW,
        size: (800, 600),
        position: None,
    }));
    let (response, results) = answered(done);
    assert_eq!(response, 0);
    let streams: Streams = results["streams"].try_clone().unwrap().try_into().unwrap();
    assert_eq!(
        u32::try_from(streams[0].1["source_type"].try_clone().unwrap()).unwrap(),
        2
    );
    assert!(
        !streams[0].1.contains_key("position"),
        "a window has no position"
    );

    // the window closed
    drop(running);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(closed.into_iter().next().map(|m| m.is_ok()));
    });
    assert_eq!(rx.recv_timeout(WAIT).unwrap(), Some(true));
    assert!(
        session_close(&r.frontend).is_err(),
        "the session object is gone"
    );
    assert!(r.shares.try_recv().is_err(), "nothing is left to stop");
}

#[test]
fn a_cancelled_chooser_answers_1_and_shares_nothing() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    select(&r);
    let done = call(&r.frontend, "Start", START, HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    reply.send(Answer::Cancelled);
    let (response, results) = answered(done);
    assert_eq!((response, results.len()), (1, 0));
    assert!(r.shares.try_recv().is_err());
}

#[test]
fn closing_the_session_while_the_chooser_shows_takes_it_down_and_answers_2() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    select(&r);
    let done = call(&r.frontend, "Start", START, HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    session_close(&r.frontend).unwrap();
    let Event::Close { handle } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected the chooser to close");
    };
    assert_eq!(handle.as_str(), START);
    let (response, _) = answered(done);
    assert_eq!(response, 2);
    drop(reply);
}

#[test]
fn the_frontend_leaving_the_bus_stops_its_shares_and_ends_their_sessions() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    select(&r);
    let done = call(&r.frontend, "Start", START, HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    reply.send(Answer::Chosen(display()));
    let Capture::Start {
        started, running, ..
    } = r.shares.recv_timeout(WAIT).unwrap()
    else {
        panic!("expected a share");
    };
    started.send(Ok(Stream {
        node: 59,
        source_type: SourceTypes::MONITOR,
        size: (1920, 1080),
        position: Some((0, 0)),
    }));
    assert_eq!(answered(done).0, 0);

    r.frontend.release_name(FRONTEND).unwrap();
    let Capture::Stop { session } = r.shares.recv_timeout(WAIT).unwrap() else {
        panic!("expected the share to stop");
    };
    assert_eq!(session.as_str(), SESSION);
    drop(running);
    let gone = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        r.app
            .call_method(
                Some(BUS_NAME),
                SESSION,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.freedesktop.impl.portal.Session", "version"),
            )
            .is_err()
    });
    assert!(gone, "the session object is gone");
}

#[test]
fn only_the_frontend_may_call_and_the_calls_come_in_order() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let t1 = "/org/freedesktop/portal/desktop/request/1_7/t1";
    assert_eq!(
        error(
            call(&r.app, "CreateSession", t1, HashMap::new())
                .recv_timeout(WAIT)
                .unwrap()
        ),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    assert_eq!(
        error(
            call(&r.frontend, "SelectSources", t1, HashMap::new())
                .recv_timeout(WAIT)
                .unwrap()
        ),
        "org.freedesktop.DBus.Error.InvalidArgs",
        "no session yet"
    );
    answered(call(&r.frontend, "CreateSession", t1, HashMap::new()));
    assert_eq!(
        error(
            call(&r.frontend, "CreateSession", t1, HashMap::new())
                .recv_timeout(WAIT)
                .unwrap()
        ),
        "org.freedesktop.DBus.Error.InvalidArgs",
        "the handle is taken"
    );
    assert_eq!(
        error(
            call(&r.frontend, "Start", START, HashMap::new())
                .recv_timeout(WAIT)
                .unwrap()
        ),
        "org.freedesktop.DBus.Error.InvalidArgs",
        "Start before SelectSources"
    );
    for (key, value) in [
        ("types", owned(Value::from(4u32))),
        ("types", owned(Value::from(0u32))),
        ("cursor_mode", owned(Value::from(4u32))),
        ("types", owned(Value::from("monitor"))),
    ] {
        let done = call(
            &r.frontend,
            "SelectSources",
            t1,
            HashMap::from([(key.to_owned(), value)]),
        );
        assert_eq!(
            error(done.recv_timeout(WAIT).unwrap()),
            "org.freedesktop.DBus.Error.InvalidArgs",
            "{key}"
        );
    }
    assert!(r.events.try_recv().is_err(), "no chooser was shown");
}

#[test]
fn the_properties_name_the_served_types_and_modes_and_version_5() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let properties = zbus::blocking::fdo::PropertiesProxy::builder(&r.app)
        .destination(BUS_NAME)
        .unwrap()
        .path(PORTAL_PATH)
        .unwrap()
        .build()
        .unwrap();
    let interface = zbus::names::InterfaceName::try_from(INTERFACE).unwrap();
    let all = properties.get_all(interface).unwrap();
    let get = |name: &str| u32::try_from(all[name].try_clone().unwrap()).unwrap();
    assert_eq!(get("AvailableSourceTypes"), 3);
    assert_eq!(get("AvailableCursorModes"), 3);
    assert_eq!(get("version"), 5);
}
