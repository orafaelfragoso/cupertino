//! The ScreenCast backend on a private bus, called as xdg-desktop-portal calls it, with a chooser and a capture side
//! the test answers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Mutex;

use cupertino_defaults::screen_sharing::ScreenSharingConf;

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
    served: Connection,
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
    start_with(None, Vec::new())
}

/// The backend that keeps remembered displays in `grants` and reads desktop entries from `applications`.
fn start_with(grants: Option<PathBuf>, applications: Vec<PathBuf>) -> Option<Running> {
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
    .with_applications(applications)
    .with_grants(grants)
    .serve(&served)
    .unwrap();
    served.request_name(BUS_NAME).unwrap();
    let frontend = bus.connect();
    frontend.request_name(FRONTEND).unwrap();
    let app = bus.connect();
    Some(Running {
        _bus: bus,
        served,
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
    call_as(caller, method, handle, SESSION, APP, options)
}

/// [`call`] for the session at `session` of app `app`.
fn call_as(
    caller: &Connection,
    method: &'static str,
    handle: &str,
    session: &str,
    app: &'static str,
    options: Results,
) -> mpsc::Receiver<Call> {
    let caller = caller.clone();
    let (handle, session) = (handle.to_owned(), session.to_owned());
    let (sender, done) = mpsc::channel();
    std::thread::spawn(move || {
        let handle = ObjectPath::try_from(handle.as_str()).unwrap();
        let session = ObjectPath::try_from(session.as_str()).unwrap();
        let reply = if method == "Start" {
            caller.call_method(
                Some(BUS_NAME),
                PORTAL_PATH,
                Some(INTERFACE),
                method,
                &(handle, session, app, "wayland:ab12", options),
            )
        } else {
            caller.call_method(
                Some(BUS_NAME),
                PORTAL_PATH,
                Some(INTERFACE),
                method,
                &(handle, session, app, options),
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
    reply.send(Answer::Chosen {
        source: display(),
        remember: false,
    });
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
    reply.send(Answer::Chosen {
        source: window(),
        remember: false,
    });
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
    reply.send(Answer::Chosen {
        source: display(),
        remember: false,
    });
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

#[test]
fn the_portal_file_and_the_portals_conf_name_the_screen_cast() {
    let file = include_str!("../data/cupertino.portal");
    let interfaces = file
        .lines()
        .find_map(|l| l.strip_prefix("Interfaces="))
        .unwrap();
    assert!(interfaces.split(';').any(|i| i == INTERFACE));
    for conf in [
        include_str!("../data/cupertino-portals.conf"),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packaging/files/usr/share/xdg-desktop-portal/cupertino-portals.conf"
        )),
    ] {
        let backends = conf
            .lines()
            .find_map(|l| l.strip_prefix(INTERFACE)?.strip_prefix('='))
            .unwrap();
        assert_eq!(
            backends.split(';').next(),
            Some(BUS_NAME.rsplit('.').next().unwrap())
        );
    }
}

/// CreateSession and SelectSources for displays and windows of `app` at session `n`, asking to persist `persist`, with the
/// frontend's `restore` data when it has some.
fn select_as(
    r: &Running,
    n: u32,
    app: &'static str,
    persist: u32,
    restore: Option<&OwnedValue>,
) -> String {
    let session = format!("/org/freedesktop/portal/desktop/session/1_7/r{n}");
    let request = |m: u32| format!("/org/freedesktop/portal/desktop/request/1_7/r{n}_{m}");
    let created = call_as(
        &r.frontend,
        "CreateSession",
        &request(1),
        &session,
        app,
        HashMap::new(),
    );
    assert_eq!(answered(created).0, 0);
    let mut options = HashMap::from([
        ("types".to_owned(), owned(Value::from(3u32))),
        ("persist_mode".to_owned(), owned(Value::from(persist))),
    ]);
    if let Some(restore) = restore {
        options.insert("restore_data".to_owned(), restore.try_clone().unwrap());
    }
    let selected = call_as(
        &r.frontend,
        "SelectSources",
        &request(2),
        &session,
        app,
        options,
    );
    assert_eq!(answered(selected).0, 0);
    session
}

/// Start the session at `session` of `app` and answer the share the capture side is asked for; the call's answer and
/// the share kept running.
fn share_as(
    r: &Running,
    session: &str,
    app: &'static str,
    chooser: impl FnOnce(Event),
) -> (
    (u32, Results),
    Option<cupertino_portal::screen_cast::Running>,
) {
    let request = format!("{session}_start").replace("/session/", "/request/");
    let done = call_as(&r.frontend, "Start", &request, session, app, HashMap::new());
    if let Ok(event) = r.events.recv_timeout(std::time::Duration::from_millis(300)) {
        chooser(event);
    }
    let deadline = std::time::Instant::now() + WAIT;
    let running = loop {
        if let Ok(answer) = done.try_recv() {
            // answered without a share: cancelled or ended
            return (answer.unwrap(), None);
        }
        assert!(std::time::Instant::now() < deadline, "Start never answered");
        match r.shares.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(Capture::Start {
                started, running, ..
            }) => {
                started.send(Ok(Stream {
                    node: 60,
                    source_type: SourceTypes::MONITOR,
                    size: (1920, 1080),
                    position: Some((0, 0)),
                }));
                break Some(running);
            }
            Ok(Capture::Stop { .. }) => panic!("a share stopped before it started"),
            Err(_) => continue,
        }
    };
    (answered(done), running)
}

fn close_as(r: &Running, session: &str) {
    r.frontend
        .call_method(
            Some(BUS_NAME),
            session,
            Some("org.freedesktop.impl.portal.Session"),
            "Close",
            &(),
        )
        .unwrap();
    let Capture::Stop { .. } = r.shares.recv_timeout(WAIT).unwrap() else {
        panic!("expected the share to stop");
    };
}

fn persist_mode(results: &Results) -> u32 {
    u32::try_from(results["persist_mode"].try_clone().unwrap()).unwrap()
}

fn grants_file(dir: &Path) -> PathBuf {
    dir.join("screen-sharing.conf")
}

#[test]
fn a_remembered_display_is_restored_for_its_own_app_until_it_is_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let file = grants_file(dir.path());
    let Some(r) = start_with(Some(file.clone()), Vec::new()) else {
        return missing_tool("dbus-daemon");
    };

    // the person shares the display and ticks to let the app share it again
    let session = select_as(&r, 1, APP, 2, None);
    let ((response, results), _running) = share_as(&r, &session, APP, |event| {
        let Event::Ask { ask, reply, .. } = event else {
            panic!("expected a chooser");
        };
        assert!(ask.offer_remember, "the app asked to persist");
        reply.send(Answer::Chosen {
            source: display(),
            remember: true,
        });
    });
    assert_eq!(response, 0);
    assert_eq!(persist_mode(&results), 2);
    let restore = results["restore_data"].try_clone().unwrap();
    let (conf, _) = ScreenSharingConf::load(&file).unwrap();
    assert_eq!(conf.grants.len(), 1);
    assert_eq!(
        (
            conf.grants[0].app_id.as_str(),
            conf.grants[0].display.as_str()
        ),
        (APP, "DP-1")
    );
    close_as(&r, &session);

    // the frontend hands the data back for the same app: the display is shared without the chooser
    let session = select_as(&r, 2, APP, 2, Some(&restore));
    let ((response, results), _running) = share_as(&r, &session, APP, |_| {
        panic!("a restored share shows no chooser")
    });
    assert_eq!(response, 0);
    assert_eq!(persist_mode(&results), 2);
    assert_eq!(results["restore_data"], restore, "the same grant goes on");
    close_as(&r, &session);

    // another app with the same data, or the same app asking for less, is asked
    let other = "org.example.Other";
    let session = select_as(&r, 3, other, 2, Some(&restore));
    let mut asked = false;
    let ((response, _), _) = share_as(&r, &session, other, |event| {
        asked = matches!(event, Event::Ask { .. });
        if let Event::Ask { reply, .. } = event {
            reply.send(Answer::Cancelled);
        }
    });
    assert!(asked, "a grant restores only for the app it was given to");
    assert_eq!(response, 1);
    let session = select_as(&r, 4, APP, 0, Some(&restore));
    let mut asked = false;
    let ((response, _), _) = share_as(&r, &session, APP, |event| {
        asked = matches!(event, Event::Ask { .. });
        if let Event::Ask { reply, .. } = event {
            reply.send(Answer::Cancelled);
        }
    });
    assert!(asked, "an app that does not ask to persist is asked");
    assert_eq!(response, 1);

    // the person forgets it in Settings: the app is asked again
    let (mut conf, _) = ScreenSharingConf::load(&file).unwrap();
    let id = conf.grants[0].id.clone();
    assert!(conf.revoke(&id));
    conf.store(&file).unwrap();
    let session = select_as(&r, 5, APP, 2, Some(&restore));
    let mut asked = false;
    let ((response, _), _) = share_as(&r, &session, APP, |event| {
        let Event::Ask { reply, .. } = event else {
            panic!("expected a chooser");
        };
        asked = true;
        reply.send(Answer::Cancelled);
    });
    assert!(asked, "a forgotten grant restores nothing");
    assert_eq!(response, 1);
}

#[test]
fn a_share_not_ticked_to_remember_or_of_an_app_without_an_id_is_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let file = grants_file(dir.path());
    let Some(r) = start_with(Some(file.clone()), Vec::new()) else {
        return missing_tool("dbus-daemon");
    };
    let session = select_as(&r, 1, APP, 2, None);
    let ((response, results), _running) = share_as(&r, &session, APP, |event| {
        let Event::Ask { reply, .. } = event else {
            panic!("expected a chooser");
        };
        reply.send(Answer::Chosen {
            source: display(),
            remember: false,
        });
    });
    assert_eq!((response, persist_mode(&results)), (0, 0));
    assert!(!results.contains_key("restore_data"));
    close_as(&r, &session);

    let session = select_as(&r, 2, "", 2, None);
    let ((response, results), _running) = share_as(&r, &session, "", |event| {
        let Event::Ask { ask, reply, .. } = event else {
            panic!("expected a chooser");
        };
        assert!(
            !ask.offer_remember,
            "an app without an id cannot be told apart from another"
        );
        assert_eq!(ask.app_name, None);
        reply.send(Answer::Chosen {
            source: display(),
            remember: true,
        });
    });
    assert_eq!((response, persist_mode(&results)), (0, 0));
    assert!(ScreenSharingConf::load(&file).unwrap().0.grants.is_empty());
}

#[test]
fn a_window_is_restored_only_while_the_app_runs_and_not_after_the_person_stops_it() {
    let Some(r) = start_with(None, Vec::new()) else {
        return missing_tool("dbus-daemon");
    };
    let watcher = Watcher::serve(&r);
    let session = select_as(&r, 1, APP, 2, None);
    let ((response, results), _running) = share_as(&r, &session, APP, |event| {
        let Event::Ask { reply, .. } = event else {
            panic!("expected a chooser");
        };
        reply.send(Answer::Chosen {
            source: window(),
            remember: true,
        });
    });
    assert_eq!(response, 0);
    assert_eq!(
        persist_mode(&results),
        1,
        "a window lasts only while the app runs"
    );
    let restore = results["restore_data"].try_clone().unwrap();
    close_as(&r, &session);
    watcher.next();

    let session = select_as(&r, 2, APP, 1, Some(&restore));
    let ((response, results), _running) = share_as(&r, &session, APP, |_| {
        panic!("a restored share shows no chooser")
    });
    assert_eq!((response, persist_mode(&results)), (0, 1));

    // the person stops it from the menu bar: the share ends and the app must ask again
    let item = watcher.next();
    menu_event(&r, &item, 3, "clicked");
    let Capture::Stop { session: stopped } = r.shares.recv_timeout(WAIT).unwrap() else {
        panic!("expected the share to stop");
    };
    assert_eq!(stopped.as_str(), session);
    let session = select_as(&r, 3, APP, 1, Some(&restore));
    let mut asked = false;
    let _ = share_as(&r, &session, APP, |event| {
        asked = matches!(event, Event::Ask { .. });
        if let Event::Ask { reply, .. } = event {
            reply.send(Answer::Cancelled);
        }
    });
    assert!(asked, "a stopped share is not restored");
}

/// A stand-in for the menu bar's item registry, which hands on each registration.
struct Watcher {
    registered: mpsc::Receiver<String>,
    _connection: Connection,
}

struct Registry(Mutex<mpsc::Sender<String>>);

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Registry {
    fn register_status_notifier_item(&self, service: &str) {
        let _ = self.0.lock().unwrap().send(service.to_owned());
    }
}

impl Watcher {
    fn serve(r: &Running) -> Watcher {
        let connection = r._bus.connect();
        let (sender, registered) = mpsc::channel();
        connection
            .object_server()
            .at("/StatusNotifierWatcher", Registry(Mutex::new(sender)))
            .unwrap();
        connection
            .request_name("org.kde.StatusNotifierWatcher")
            .unwrap();
        Watcher {
            registered,
            _connection: connection,
        }
    }

    /// The `service/path` of the next item registered.
    fn next(&self) -> (String, String) {
        let id = self.registered.recv_timeout(WAIT).unwrap();
        let (name, path) = id.split_at(id.find('/').unwrap());
        (name.to_owned(), path.to_owned())
    }
}

fn menu_event(r: &Running, (name, path): &(String, String), id: i32, event: &str) {
    r.app
        .call_method(
            Some(name.as_str()),
            path.as_str(),
            Some("com.canonical.dbusmenu"),
            "Event",
            &(id, event, Value::from(0i32), 0u32),
        )
        .unwrap();
}

fn has_owner(r: &Running, name: &str) -> bool {
    zbus::blocking::fdo::DBusProxy::new(&r.app)
        .unwrap()
        .name_has_owner(zbus::names::BusName::try_from(name).unwrap())
        .unwrap()
}

#[test]
fn the_menu_bar_item_names_the_app_while_it_shares_and_stops_it() {
    let applications = tempfile::tempdir().unwrap();
    std::fs::write(
        applications.path().join(format!("{APP}.desktop")),
        "[Desktop Entry]\nType=Application\nName=Meeting\nIcon=org.example.Meeting\nExec=meeting\n",
    )
    .unwrap();
    let Some(r) = start_with(None, vec![applications.path().to_path_buf()]) else {
        return missing_tool("dbus-daemon");
    };
    let watcher = Watcher::serve(&r);
    let closed = zbus::blocking::MessageIterator::for_match_rule(
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.impl.portal.Session")
            .unwrap()
            .member("Closed")
            .unwrap()
            .build(),
        &r.frontend,
        None,
    )
    .unwrap();

    let session = select_as(&r, 1, APP, 0, None);
    let ((response, _), running) = share_as(&r, &session, APP, |event| {
        let Event::Ask { ask, reply, .. } = event else {
            panic!("expected a chooser");
        };
        assert_eq!(
            ask.app_name.as_deref(),
            Some("Meeting"),
            "named by its desktop entry"
        );
        assert_eq!(ask.app_icon.as_deref(), Some("org.example.Meeting"));
        assert!(!ask.offer_remember, "the app did not ask to persist");
        reply.send(Answer::Chosen {
            source: display(),
            remember: false,
        });
    });
    assert_eq!(response, 0);
    let item = watcher.next();
    assert!(has_owner(&r, &item.0), "the item has a name of its own");
    let property = |name: &str| -> OwnedValue {
        let reply = r
            .app
            .call_method(
                Some(item.0.as_str()),
                item.1.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.kde.StatusNotifierItem", name),
            )
            .unwrap();
        let value: OwnedValue = reply.body().deserialize().unwrap();
        value
    };
    assert_eq!(String::try_from(property("Status")).unwrap(), "Active");
    assert!(bool::try_from(property("ItemIsMenu")).unwrap());
    let layout = r
        .app
        .call_method(
            Some(item.0.as_str()),
            item.1.as_str(),
            Some("com.canonical.dbusmenu"),
            "GetLayout",
            &(0i32, -1i32, Vec::<String>::new()),
        )
        .unwrap();
    type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);
    let (_, (_, _, children)): (u32, Node) = layout.body().deserialize().unwrap();
    let labels: Vec<String> = children
        .into_iter()
        .map(|child| {
            let (_, properties, _): Node = child.try_into().unwrap();
            properties
                .get("label")
                .map(|l| String::try_from(l.try_clone().unwrap()).unwrap())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        labels,
        ["“Meeting” is sharing “Studio Display”", "", "Stop Sharing"]
    );

    menu_event(&r, &item, 3, "clicked");
    let Capture::Stop { session: stopped } = r.shares.recv_timeout(WAIT).unwrap() else {
        panic!("expected the share to stop");
    };
    assert_eq!(stopped.as_str(), session);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(closed.into_iter().next().map(|m| m.is_ok()));
    });
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        Some(true),
        "the frontend is told"
    );
    drop(running);
    let gone = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        !has_owner(&r, &item.0)
    });
    assert!(gone, "the item leaves the menu bar with the share");
    let _ = &r.served;
}

#[test]
fn the_app_going_away_mid_share_takes_the_share_and_its_item_down() {
    let Some(r) = start_with(None, Vec::new()) else {
        return missing_tool("dbus-daemon");
    };
    let watcher = Watcher::serve(&r);
    let session = select_as(&r, 1, APP, 0, None);
    let (_, running) = share_as(&r, &session, APP, |event| {
        if let Event::Ask { reply, .. } = event {
            reply.send(Answer::Chosen {
                source: display(),
                remember: false,
            });
        }
    });
    let item = watcher.next();
    // xdg-desktop-portal closes the session of an app that left the bus
    close_as(&r, &session);
    drop(running);
    let gone = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(20));
        !has_owner(&r, &item.0)
    });
    assert!(gone, "the item goes with the share");
}

#[test]
fn the_app_going_away_mid_dialog_closes_the_chooser_and_answers_2() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    select(&r);
    let done = call(&r.frontend, "Start", START, HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    // xdg-desktop-portal closes the request of an app that left the bus
    assert!(
        r.app
            .call_method(
                Some(BUS_NAME),
                START,
                Some("org.freedesktop.impl.portal.Request"),
                "Close",
                &()
            )
            .is_err(),
        "only the frontend closes a request"
    );
    r.frontend
        .call_method(
            Some(BUS_NAME),
            START,
            Some("org.freedesktop.impl.portal.Request"),
            "Close",
            &(),
        )
        .unwrap();
    let Event::Close { handle } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected the chooser to close");
    };
    assert_eq!(handle.as_str(), START);
    assert_eq!(answered(done).0, 2);
    reply.send(Answer::Chosen {
        source: display(),
        remember: false,
    });
    assert!(r.shares.try_recv().is_err(), "a late answer shares nothing");
}
