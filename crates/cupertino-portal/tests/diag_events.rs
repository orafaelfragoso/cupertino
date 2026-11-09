//! The diagnostic events of the portal backend, read from the user's event store of a state directory of the test's
//! own: refused calls, a share that does not start, remembered shares that cannot be written or read, a screenshot
//! that cannot be copied, and the binary's panic and exit with an error. Each test runs its body in a process of its
//! own (one emitter per process) and also checks that what the frontend is answered is unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc;
use std::sync::Mutex;

use cupertino_diagnostic_events::{self as events, store, Record, Severity};
use cupertino_portal::request::FRONTEND;
use cupertino_portal::screen_cast::{
    Answer, Capture, Event, ScreenCast, Source, SourceTypes, Stream,
};
use cupertino_portal::screenshot::{self, Failure, Screenshot};
use cupertino_portal::{BUS_NAME, PORTAL_PATH};
use cupertino_test_bus::{missing_tool, PrivateBus, WAIT};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const SCREEN_CAST: &str = "org.freedesktop.impl.portal.ScreenCast";
const SCREENSHOT: &str = "org.freedesktop.impl.portal.Screenshot";
const APP: &str = "org.example.Meeting";
const OTHER: &str = "org.example.Other";
const SESSION: &str = "/org/freedesktop/portal/desktop/session/1_7/s1";

type Results = HashMap<String, OwnedValue>;
type Call = zbus::Result<(u32, Results)>;

/// Set in the child process: the test named runs its body.
const CHILD: &str = "DIAG_TEST_CHILD";

fn is_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

/// Runs the test `name` of this binary again in a process of its own whose user store is under `state`; `None` when
/// the child skipped its test.
fn child(name: &str, state: &Path) -> Option<Output> {
    let output = Command::new(std::env::current_exe().unwrap())
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .args(["--exact", name, "--nocapture", "--test-threads", "1"])
        .env(CHILD, "1")
        .env("XDG_STATE_HOME", state)
        .env("HOME", state.join("home"))
        .env_remove("XDG_SESSION_ID")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the child failed: {stderr}");
    if stderr.contains("SKIPPED:") {
        eprintln!("{stderr}");
        return None;
    }
    Some(output)
}

/// The records of the user store under `state`.
fn stored(state: &Path) -> Vec<Record> {
    store::read(&state.join("cupertino/events"))
        .unwrap()
        .records
        .into_iter()
        .map(|stored| stored.record)
        .collect()
}

/// The records of `id` under `state`.
fn of(state: &Path, id: &str) -> Vec<Record> {
    stored(state).into_iter().filter(|r| r.id == id).collect()
}

fn text<'a>(record: &'a Record, key: &str) -> &'a str {
    record
        .fields
        .get(key)
        .unwrap_or_else(|| panic!("no field {key} in {}", record.to_json()))
        .as_str()
        .unwrap()
}

fn init() {
    events::init(events::component!(), "portal", events::Sink::UserStore);
}

fn owned(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).unwrap()
}

struct Cast {
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

/// The ScreenCast backend on a private bus, keeping remembered displays in `grants`.
fn cast(grants: Option<PathBuf>) -> Option<Cast> {
    let bus = PrivateBus::start()?;
    let served = bus.connect();
    let (events_sender, events) = mpsc::channel();
    let (shares_sender, shares) = mpsc::channel();
    let (events_sender, shares_sender) = (Mutex::new(events_sender), Mutex::new(shares_sender));
    ScreenCast::new(
        || vec![display()],
        move |event| {
            let _ = events_sender.lock().unwrap().send(event);
        },
        move |share| {
            let _ = shares_sender.lock().unwrap().send(share);
        },
    )
    .with_grants(grants)
    .serve(&served)
    .unwrap();
    served.request_name(BUS_NAME).unwrap();
    let frontend = bus.connect();
    frontend.request_name(FRONTEND).unwrap();
    let app = bus.connect();
    Some(Cast {
        _bus: bus,
        _served: served,
        events,
        shares,
        frontend,
        app,
    })
}

/// `method` of the ScreenCast interface for `session` of `app`, on another thread.
fn call(
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
                Some(SCREEN_CAST),
                method,
                &(handle, session, app, "wayland:ab12", options),
            )
        } else {
            caller.call_method(
                Some(BUS_NAME),
                PORTAL_PATH,
                Some(SCREEN_CAST),
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

fn error(done: mpsc::Receiver<Call>) -> String {
    match done.recv_timeout(WAIT).unwrap() {
        Err(zbus::Error::MethodError(name, _, _)) => name.to_string(),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// CreateSession and SelectSources for session `n` of `app`, which asks to persist as `persist`.
fn select(
    r: &Cast,
    n: u32,
    app: &'static str,
    persist: u32,
    restore: Option<OwnedValue>,
) -> String {
    let session = format!("/org/freedesktop/portal/desktop/session/1_7/r{n}");
    let request = |m: u32| format!("/org/freedesktop/portal/desktop/request/1_7/r{n}_{m}");
    let created = call(
        &r.frontend,
        "CreateSession",
        &request(1),
        &session,
        app,
        HashMap::new(),
    );
    assert_eq!(answered(created).0, 0);
    let mut options = HashMap::from([
        ("types".to_owned(), owned(Value::from(1u32))),
        ("persist_mode".to_owned(), owned(Value::from(persist))),
    ]);
    if let Some(restore) = restore {
        options.insert("restore_data".to_owned(), restore);
    }
    let selected = call(
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

/// Start `session` of `app`; the person chooses the display in the chooser, remembering it when `remember`.
fn start(r: &Cast, session: &str, app: &'static str, remember: bool) -> mpsc::Receiver<Call> {
    let request = format!("{session}_start").replace("/session/", "/request/");
    let done = call(&r.frontend, "Start", &request, session, app, HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    reply.send(Answer::Chosen {
        source: display(),
        remember,
    });
    done
}

/// The capture side's next share request, started with `started`.
fn share(r: &Cast, started: Result<Stream, String>) {
    let Capture::Start { started: s, .. } = r.shares.recv_timeout(WAIT).unwrap() else {
        panic!("expected a share");
    };
    s.send(started);
}

fn stream() -> Stream {
    Stream {
        node: 57,
        source_type: SourceTypes::MONITOR,
        size: (2560, 1440),
        position: Some((0, 0)),
    }
}

#[test]
fn a_refused_call_is_recorded_with_its_interface_method_app_and_error() {
    if !is_child() {
        let state = tempfile::tempdir().unwrap();
        if child(
            "a_refused_call_is_recorded_with_its_interface_method_app_and_error",
            state.path(),
        )
        .is_none()
        {
            return;
        }
        let failed = of(state.path(), "portal.request.failed");
        assert_eq!(failed.len(), 2);
        let denied = &failed[0];
        assert_eq!(denied.severity, Severity::Warn);
        assert_eq!(denied.component, "cupertino-portal");
        assert_eq!(text(denied, "interface"), SCREEN_CAST);
        assert_eq!(text(denied, "method"), "CreateSession");
        assert_eq!(text(denied, "app_id"), APP);
        assert_eq!(
            text(denied, "error"),
            "org.freedesktop.DBus.Error.AccessDenied"
        );
        let invalid = &failed[1];
        assert_eq!(text(invalid, "method"), "SelectSources");
        assert_eq!(text(invalid, "app_id"), APP);
        assert_eq!(
            text(invalid, "error"),
            "org.freedesktop.DBus.Error.InvalidArgs"
        );
        return;
    }
    init();
    let Some(r) = cast(None) else {
        return missing_tool("dbus-daemon");
    };
    // an app calling the backend itself is refused as before
    let request = "/org/freedesktop/portal/desktop/request/1_7/t1";
    let done = call(
        &r.app,
        "CreateSession",
        request,
        SESSION,
        APP,
        HashMap::new(),
    );
    assert_eq!(error(done), "org.freedesktop.DBus.Error.AccessDenied");
    // a cursor mode that is not served, from the frontend
    let done = call(
        &r.frontend,
        "CreateSession",
        request,
        SESSION,
        APP,
        HashMap::new(),
    );
    assert_eq!(answered(done).0, 0);
    let options = HashMap::from([("cursor_mode".to_owned(), owned(Value::from(7u32)))]);
    let done = call(
        &r.frontend,
        "SelectSources",
        "/org/freedesktop/portal/desktop/request/1_7/t2",
        SESSION,
        APP,
        options,
    );
    assert_eq!(error(done), "org.freedesktop.DBus.Error.InvalidArgs");
}

#[test]
fn a_share_that_does_not_start_is_recorded_beside_the_same_answer_and_line() {
    if !is_child() {
        let state = tempfile::tempdir().unwrap();
        let Some(out) = child(
            "a_share_that_does_not_start_is_recorded_beside_the_same_answer_and_line",
            state.path(),
        ) else {
            return;
        };
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(
                "cupertino-portal: the share of /org/freedesktop/portal/desktop/session/1_7/r1 did not start: \
                 the display went away"
            ),
            "{stderr}"
        );
        let failed = of(state.path(), "portal.screencast.failed");
        assert_eq!(failed.len(), 1);
        let failed = &failed[0];
        assert_eq!(failed.severity, Severity::Error);
        assert_eq!(text(failed, "stage"), "start");
        assert_eq!(text(failed, "app_id"), APP);
        assert_eq!(text(failed, "error"), "the display went away");
        assert!(of(state.path(), "portal.request.failed").is_empty());
        return;
    }
    init();
    let Some(r) = cast(None) else {
        return missing_tool("dbus-daemon");
    };
    let session = select(&r, 1, APP, 0, None);
    let done = start(&r, &session, APP, false);
    share(&r, Err("the display went away".to_owned()));
    let (response, results) = answered(done);
    assert_eq!((response, results.len()), (2, 0));
}

#[test]
fn remembered_shares_that_cannot_be_written_or_read_are_recorded_and_the_share_goes_on() {
    if !is_child() {
        let state = tempfile::tempdir().unwrap();
        let Some(out) = child(
            "remembered_shares_that_cannot_be_written_or_read_are_recorded_and_the_share_goes_on",
            state.path(),
        ) else {
            return;
        };
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("cupertino-portal: the share is kept only while the app runs: "),
            "{stderr}"
        );
        assert!(stderr.contains(" restores nothing: "), "{stderr}");
        let failed = of(state.path(), "portal.permission.store_failed");
        assert_eq!(failed.len(), 2);
        let (save, load) = (&failed[0], &failed[1]);
        assert_eq!(save.severity, Severity::Warn);
        assert_eq!(text(save, "operation"), "save");
        assert_eq!(text(save, "app_id"), APP);
        assert!(!text(save, "error").is_empty());
        assert_eq!(text(load, "operation"), "load");
        assert_eq!(text(load, "app_id"), OTHER);
        assert!(!text(load, "error").is_empty());
        return;
    }
    init();
    // the grants file's directory is a regular file: it can be neither written nor read
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("not-a-directory");
    std::fs::write(&blocked, "").unwrap();
    let Some(r) = cast(Some(blocked.join("screen-sharing.conf"))) else {
        return missing_tool("dbus-daemon");
    };
    // the person ticks to remember the display: it is shared, and kept only while the app runs
    let session = select(&r, 1, APP, 2, None);
    let done = start(&r, &session, APP, true);
    share(&r, Ok(stream()));
    let (response, results) = answered(done);
    assert_eq!(response, 0);
    assert_eq!(
        u32::try_from(results["persist_mode"].try_clone().unwrap()).unwrap(),
        1
    );
    let restore = results["restore_data"].try_clone().unwrap();
    // another app hands the data back: the file is read, and the chooser shows
    let session = select(&r, 2, OTHER, 2, Some(restore));
    let request = format!("{session}_start").replace("/session/", "/request/");
    let done = call(
        &r.frontend,
        "Start",
        &request,
        &session,
        OTHER,
        HashMap::new(),
    );
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a chooser");
    };
    reply.send(Answer::Cancelled);
    assert_eq!(answered(done).0, 1);
}

#[test]
fn a_screenshot_whose_displays_cannot_be_copied_is_recorded_beside_the_same_answer() {
    if !is_child() {
        let state = tempfile::tempdir().unwrap();
        let Some(out) = child(
            "a_screenshot_whose_displays_cannot_be_copied_is_recorded_beside_the_same_answer",
            state.path(),
        ) else {
            return;
        };
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(
                "cupertino-portal: the screenshot for /org/freedesktop/portal/desktop/request/1_7/t1 failed: \
                 no display"
            ),
            "{stderr}"
        );
        let failed = of(state.path(), "portal.screenshot.failed");
        assert_eq!(failed.len(), 2);
        let shot = &failed[0];
        assert_eq!(shot.severity, Severity::Error);
        assert_eq!(text(shot, "method"), "Screenshot");
        assert_eq!(text(shot, "app_id"), APP);
        assert_eq!(text(shot, "error"), "no display");
        let pick = &failed[1];
        assert_eq!(text(pick, "method"), "PickColor");
        assert_eq!(text(pick, "app_id"), APP);
        assert_eq!(text(pick, "error"), "no display");
        return;
    }
    init();
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    let served = bus.connect();
    let home = tempfile::tempdir().unwrap();
    let pictures = home.path().join("Pictures");
    let (events_sender, asks) = mpsc::channel();
    let (shoots_sender, shoots) = mpsc::channel();
    let (picks_sender, _picks) = mpsc::channel();
    let (events_sender, shoots_sender) = (Mutex::new(events_sender), Mutex::new(shoots_sender));
    let into = pictures.clone();
    let backend = Screenshot::new(
        Vec::new(),
        move || into.clone(),
        move |event| {
            let _ = events_sender.lock().unwrap().send(event);
        },
        move |shoot| {
            let _ = shoots_sender.lock().unwrap().send(shoot);
        },
    )
    .with_picker(move |event| {
        let _ = picks_sender.send(event);
    });
    served.object_server().at(PORTAL_PATH, backend).unwrap();
    served.request_name(BUS_NAME).unwrap();
    let frontend = bus.connect();
    frontend.request_name(FRONTEND).unwrap();
    let ask = |method: &'static str, handle: &'static str| {
        let frontend = frontend.clone();
        let (sender, done) = mpsc::channel();
        std::thread::spawn(move || {
            let reply = frontend
                .call_method(
                    Some(BUS_NAME),
                    PORTAL_PATH,
                    Some(SCREENSHOT),
                    method,
                    &(
                        ObjectPath::try_from(handle).unwrap(),
                        APP,
                        "wayland:ab12",
                        Results::new(),
                    ),
                )
                .and_then(|m| m.body().deserialize::<(u32, Results)>());
            let _ = sender.send(reply);
        });
        done
    };
    // the person grants the screenshot, and the displays cannot be copied: answered 2 as before, nothing written
    let done = ask(
        "Screenshot",
        "/org/freedesktop/portal/desktop/request/1_7/t1",
    );
    let screenshot::Event::Ask { reply, .. } = asks.recv_timeout(WAIT).unwrap() else {
        panic!("expected a consent window");
    };
    reply.send(screenshot::Answer::Granted);
    shoots
        .recv_timeout(WAIT)
        .unwrap()
        .shot
        .send(Err(Failure::Lasting("no display".to_owned())));
    assert_eq!(answered(done).0, 2);
    assert!(!pictures.exists());
    // the same for a colour pick
    let done = ask(
        "PickColor",
        "/org/freedesktop/portal/desktop/request/1_7/t2",
    );
    shoots
        .recv_timeout(WAIT)
        .unwrap()
        .shot
        .send(Err(Failure::Lasting("no display".to_owned())));
    assert_eq!(answered(done).0, 2);
}

#[cfg(feature = "app")]
mod binary {
    use super::*;

    fn portal(state: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cupertino-portal"));
        command
            .env_clear()
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env("XDG_STATE_HOME", state)
            .env("HOME", state.join("home"))
            .env("XDG_RUNTIME_DIR", state.join("runtime"));
        command
    }

    #[test]
    fn a_panic_is_recorded_and_the_process_ends_with_the_panic_status() {
        let state = tempfile::tempdir().unwrap();
        let out = portal(state.path())
            .env(events::TEST_PANIC_ENV, "portal")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(101));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("panicked"), "{stderr}");
        let panics = of(state.path(), "portal.panic");
        assert_eq!(panics.len(), 1);
        assert_eq!(panics[0].severity, Severity::Fatal);
        assert_eq!(panics[0].component, "cupertino-portal");
        assert_eq!(
            text(&panics[0], "panic"),
            "CUPERTINO_DIAG_TEST_PANIC asked this process to panic"
        );
    }

    #[test]
    fn an_exit_without_a_server_is_recorded_beside_the_same_message_and_status() {
        let state = tempfile::tempdir().unwrap();
        let out = portal(state.path()).output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        let message = stderr
            .strip_prefix("cupertino-portal: ")
            .and_then(|rest| rest.strip_suffix('\n'))
            .unwrap_or_else(|| panic!("{stderr}"));
        let exits = of(state.path(), "portal.exit_error");
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].severity, Severity::Error);
        assert_eq!(text(&exits[0], "error"), message);
    }
}
