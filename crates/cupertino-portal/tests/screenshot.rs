//! The Screenshot backend on a private bus, called as xdg-desktop-portal calls it, with a consent window and a capture
//! side the test answers.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use cupertino_portal::pick::Picked;
use cupertino_portal::request::FRONTEND;
use cupertino_portal::screenshot::{
    Answer, Event, Failure, Output, PickEvent, Screenshot, Shoot, MAX_ATTEMPTS,
};
use cupertino_portal::{BUS_NAME, PORTAL_PATH};
use cupertino_test_bus::{missing_tool, PrivateBus, WAIT};
use percent_encoding::percent_decode_str;
use tiny_skia::{Color, Pixmap};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const INTERFACE: &str = "org.freedesktop.impl.portal.Screenshot";
const HANDLE: &str = "/org/freedesktop/portal/desktop/request/1_7/t1";
const APP: &str = "org.example.Draw";

type Results = HashMap<String, OwnedValue>;
type Call = zbus::Result<(u32, Results)>;

struct Running {
    _bus: PrivateBus,
    _served: Connection,
    _home: tempfile::TempDir,
    pictures: PathBuf,
    events: mpsc::Receiver<Event>,
    shoots: mpsc::Receiver<Shoot>,
    /// What the colour picker is told; `None` when the backend has none.
    picks: Option<mpsc::Receiver<PickEvent>>,
    frontend: Connection,
    app: Connection,
}

fn start() -> Option<Running> {
    start_with(false)
}

/// The backend, with a colour picker when `picker`.
fn start_with(picker: bool) -> Option<Running> {
    let bus = PrivateBus::start()?;
    let served = bus.connect();
    let home = tempfile::tempdir().unwrap();
    let applications = home.path().join("applications");
    std::fs::create_dir(&applications).unwrap();
    std::fs::write(
        applications.join(format!("{APP}.desktop")),
        "[Desktop Entry]\nType=Application\nName=Drawing\nExec=draw\n",
    )
    .unwrap();
    let pictures = home.path().join("Pictures");
    let (events_sender, events) = mpsc::channel();
    let (shoots_sender, shoots) = mpsc::channel();
    let (events_sender, shoots_sender) = (Mutex::new(events_sender), Mutex::new(shoots_sender));
    let into = pictures.clone();
    let mut screenshot = Screenshot::new(
        vec![applications],
        move || into.clone(),
        move |event| {
            let _ = events_sender.lock().unwrap().send(event);
        },
        move |shoot| {
            let _ = shoots_sender.lock().unwrap().send(shoot);
        },
    );
    let mut picks = None;
    if picker {
        let (picks_sender, picked) = mpsc::channel();
        picks = Some(picked);
        screenshot = screenshot.with_picker(move |event| {
            let _ = picks_sender.send(event);
        });
    }
    served.object_server().at(PORTAL_PATH, screenshot).unwrap();
    served.request_name(BUS_NAME).unwrap();
    let frontend = bus.connect();
    frontend.request_name(FRONTEND).unwrap();
    let app = bus.connect();
    Some(Running {
        _bus: bus,
        _served: served,
        _home: home,
        pictures,
        events,
        shoots,
        picks,
        frontend,
        app,
    })
}

fn owned(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).unwrap()
}

/// Call `method` on another thread, so the test can answer the consent window meanwhile.
fn call(
    caller: &Connection,
    method: &'static str,
    app_id: String,
    options: Results,
) -> mpsc::Receiver<Call> {
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
                &(handle, app_id, "wayland:ab12", options),
            )
            .and_then(|m| m.body().deserialize::<(u32, Results)>());
        let _ = sender.send(reply);
    });
    done
}

fn screenshot(caller: &Connection, options: Results) -> mpsc::Receiver<Call> {
    call(caller, "Screenshot", APP.to_owned(), options)
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

fn display(position: (i32, i32), size: (i32, i32), red: u8) -> Output {
    let mut image = Pixmap::new(size.0 as u32, size.1 as u32).unwrap();
    image.fill(Color::from_rgba8(red, 0, 0, 255));
    Output {
        position,
        size,
        image,
    }
}

#[test]
fn consent_captures_every_display_into_one_private_png() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let options = HashMap::from([
        ("modal".to_owned(), owned(Value::from(true))),
        ("interactive".to_owned(), owned(Value::from(true))),
        (
            "permission_store_checked".to_owned(),
            owned(Value::from(true)),
        ),
        ("target".to_owned(), owned(Value::from(1u32))),
    ]);
    let done = screenshot(&r.frontend, options);
    let Event::Ask { handle, ask, reply } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a consent window");
    };
    assert_eq!(handle.as_str(), HANDLE);
    assert_eq!(ask.app_id, APP);
    assert_eq!(
        ask.app_name.as_deref(),
        Some("Drawing"),
        "the desktop entry's name"
    );
    assert_eq!(ask.parent_window, "wayland:ab12");
    assert!(
        r.shoots.try_recv().is_err(),
        "nothing is captured before consent"
    );

    reply.send(Answer::Granted);
    let shoot = r.shoots.recv_timeout(WAIT).unwrap();
    shoot.shot.send(Ok(vec![
        display((0, 0), (64, 48), 200),
        display((64, 0), (32, 48), 100),
    ]));
    let (response, results) = answered(done);
    assert_eq!(response, 0);
    let uri = String::try_from(results["uri"].try_clone().unwrap()).unwrap();
    let path = uri.strip_prefix("file://").unwrap();
    let path = PathBuf::from(percent_decode_str(path).decode_utf8().unwrap().into_owned());
    assert_eq!(path.parent(), Some(r.pictures.as_path()));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("Screenshot ") && name.contains(" at ") && name.ends_with(".png"),
        "{name}"
    );
    let image = Pixmap::decode_png(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!((image.width(), image.height()), (96, 48), "both displays");
    assert_eq!(image.pixel(70, 10).unwrap().red(), 100);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn cancel_answers_1_and_captures_nothing() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = screenshot(&r.frontend, HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a consent window");
    };
    reply.send(Answer::Cancelled);
    let (response, results) = answered(done);
    assert_eq!((response, results.len()), (1, 0));
    assert!(r.shoots.try_recv().is_err());
}

#[test]
fn closing_the_request_takes_the_window_down_and_answers_2() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = screenshot(&r.frontend, HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a consent window");
    };
    r.frontend
        .call_method(
            Some(BUS_NAME),
            HANDLE,
            Some("org.freedesktop.impl.portal.Request"),
            "Close",
            &(),
        )
        .unwrap();
    let Event::Close { handle } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected the window to close");
    };
    assert_eq!(handle.as_str(), HANDLE);
    assert_eq!(answered(done).0, 2);
    drop(reply);
    assert!(r.shoots.try_recv().is_err());
}

/// Ask for a screenshot and grant it.
fn granted(r: &Running) -> mpsc::Receiver<Call> {
    let done = screenshot(&r.frontend, HashMap::new());
    let Event::Ask { reply, .. } = r.events.recv_timeout(WAIT).unwrap() else {
        panic!("expected a consent window");
    };
    reply.send(Answer::Granted);
    done
}

fn passing() -> Failure {
    Failure::Passing("copying a display failed: BufferConstraints".to_owned())
}

#[test]
fn a_capture_that_fails_once_is_taken_again() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = granted(&r);
    r.shoots
        .recv_timeout(WAIT)
        .unwrap()
        .shot
        .send(Err(passing()));
    let shoot = r.shoots.recv_timeout(WAIT).unwrap();
    shoot.shot.send(Ok(vec![display((0, 0), (64, 48), 200)]));
    let (response, results) = answered(done);
    assert_eq!(response, 0);
    assert!(results.contains_key("uri"));
    assert_eq!(std::fs::read_dir(&r.pictures).unwrap().count(), 1);
}

#[test]
fn a_capture_that_keeps_failing_answers_2_after_the_last_attempt_and_writes_nothing() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = granted(&r);
    for _ in 0..MAX_ATTEMPTS {
        r.shoots
            .recv_timeout(WAIT)
            .unwrap()
            .shot
            .send(Err(passing()));
    }
    let (response, results) = answered(done);
    assert_eq!((response, results.len()), (2, 0));
    assert!(r.shoots.try_recv().is_err(), "no attempt past the bound");
    assert!(!r.pictures.exists());
}

#[test]
fn a_capture_that_cannot_pass_answers_2_at_once_and_writes_nothing() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = granted(&r);
    let shoot = r.shoots.recv_timeout(WAIT).unwrap();
    shoot.shot.send(Err(Failure::Lasting(
        "the capture offers neither XRGB8888 nor ARGB8888 shm buffers".to_owned(),
    )));
    let (response, results) = answered(done);
    assert_eq!((response, results.len()), (2, 0));
    assert!(r.shoots.try_recv().is_err(), "not taken again");
    assert!(!r.pictures.exists());
}

#[test]
fn closing_the_request_during_the_capture_answers_2_and_takes_nothing_again() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let done = granted(&r);
    let shoot = r.shoots.recv_timeout(WAIT).unwrap();
    r.frontend
        .call_method(
            Some(BUS_NAME),
            HANDLE,
            Some("org.freedesktop.impl.portal.Request"),
            "Close",
            &(),
        )
        .unwrap();
    assert_eq!(answered(done).0, 2);
    shoot.shot.send(Err(passing()));
    assert!(r.shoots.recv_timeout(Duration::from_millis(500)).is_err());
    assert!(!r.pictures.exists());
}

#[test]
fn only_the_frontend_may_call_and_bad_arguments_show_no_window() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    assert_eq!(
        error(screenshot(&r.app, HashMap::new())),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    assert_eq!(
        error(call(&r.app, "PickColor", APP.to_owned(), HashMap::new())),
        "org.freedesktop.DBus.Error.AccessDenied"
    );
    for (key, value) in [
        ("target", owned(Value::from(2u32))),
        ("target", owned(Value::from(true))),
        ("modal", owned(Value::from("yes"))),
        ("interactive", owned(Value::from(1u32))),
    ] {
        assert_eq!(
            error(screenshot(
                &r.frontend,
                HashMap::from([(key.to_owned(), value)])
            )),
            "org.freedesktop.DBus.Error.InvalidArgs",
            "{key}"
        );
    }
    assert_eq!(
        error(call(
            &r.frontend,
            "Screenshot",
            "a".repeat(1025),
            HashMap::new()
        )),
        "org.freedesktop.DBus.Error.InvalidArgs",
        "an app id over its bound"
    );
    assert!(r.events.try_recv().is_err(), "no window was shown");
}

#[test]
fn pick_color_without_a_picker_answers_2_and_captures_nothing() {
    let Some(r) = start() else {
        return missing_tool("dbus-daemon");
    };
    let (response, results) = answered(pick_color(&r));
    assert_eq!((response, results.len()), (2, 0));
    assert!(r.events.try_recv().is_err());
    assert!(r.shoots.try_recv().is_err());
}

fn pick_color(r: &Running) -> mpsc::Receiver<Call> {
    call(&r.frontend, "PickColor", APP.to_owned(), HashMap::new())
}

/// Ask for a colour, copy the displays as the capture side does and take what the picker is told.
fn picking(r: &Running) -> (mpsc::Receiver<Call>, PickEvent) {
    let done = pick_color(r);
    let shoot = r.shoots.recv_timeout(WAIT).unwrap();
    assert!(
        r.picks.as_ref().unwrap().try_recv().is_err(),
        "the picker shows only once the displays are copied"
    );
    shoot.shot.send(Ok(vec![
        display((0, 0), (64, 48), 200),
        display((64, 0), (32, 48), 100),
    ]));
    let event = r.picks.as_ref().unwrap().recv_timeout(WAIT).unwrap();
    (done, event)
}

#[test]
fn a_picked_colour_is_answered_as_three_srgb_doubles_without_a_consent_window() {
    let Some(r) = start_with(true) else {
        return missing_tool("dbus-daemon");
    };
    let (done, event) = picking(&r);
    let PickEvent::Pick {
        handle,
        displays,
        reply,
    } = event
    else {
        panic!("expected a pick");
    };
    assert_eq!(handle.as_str(), HANDLE);
    let placed: Vec<_> = displays.iter().map(|d| (d.position, d.size)).collect();
    assert_eq!(
        placed,
        [((0, 0), (64, 48)), ((64, 0), (32, 48))],
        "every display"
    );
    reply.send(Picked::Colour([1.0, 0.5, 0.0]));
    let (response, results) = answered(done);
    assert_eq!(response, 0);
    let colour = <(f64, f64, f64)>::try_from(results["color"].try_clone().unwrap()).unwrap();
    assert_eq!(colour, (1.0, 0.5, 0.0));
    assert_eq!(results.len(), 1, "the colour and nothing else");
    assert!(r.events.try_recv().is_err(), "the pick is the consent");
    assert!(!r.pictures.exists(), "nothing is written");
}

#[test]
fn escape_in_the_picker_answers_1() {
    let Some(r) = start_with(true) else {
        return missing_tool("dbus-daemon");
    };
    let (done, event) = picking(&r);
    let PickEvent::Pick { reply, .. } = event else {
        panic!("expected a pick");
    };
    reply.send(Picked::Cancelled);
    let (response, results) = answered(done);
    assert_eq!((response, results.len()), (1, 0));
}

#[test]
fn a_picker_that_fails_answers_2() {
    let Some(r) = start_with(true) else {
        return missing_tool("dbus-daemon");
    };
    let (done, event) = picking(&r);
    drop(event);
    let (response, results) = answered(done);
    assert_eq!((response, results.len()), (2, 0));
}

#[test]
fn closing_the_request_takes_the_picker_down_and_answers_2() {
    let Some(r) = start_with(true) else {
        return missing_tool("dbus-daemon");
    };
    let (done, event) = picking(&r);
    r.frontend
        .call_method(
            Some(BUS_NAME),
            HANDLE,
            Some("org.freedesktop.impl.portal.Request"),
            "Close",
            &(),
        )
        .unwrap();
    let PickEvent::Close { handle } = r.picks.as_ref().unwrap().recv_timeout(WAIT).unwrap() else {
        panic!("expected the picker to close");
    };
    assert_eq!(handle.as_str(), HANDLE);
    assert_eq!(answered(done).0, 2);
    drop(event);
}

#[test]
fn a_pick_whose_displays_cannot_be_copied_answers_2_and_shows_no_picker() {
    let Some(r) = start_with(true) else {
        return missing_tool("dbus-daemon");
    };
    let done = pick_color(&r);
    let shoot = r.shoots.recv_timeout(WAIT).unwrap();
    shoot
        .shot
        .send(Err(Failure::Lasting("no display".to_owned())));
    assert_eq!(answered(done).0, 2);
    assert!(r.picks.as_ref().unwrap().try_recv().is_err());
}

#[test]
fn the_properties_name_the_screen_target_and_version_3() {
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
    assert_eq!(get("AvailableTargets"), 1);
    assert_eq!(get("version"), 3);
}

#[test]
fn the_portal_file_and_the_portals_conf_name_the_screenshot() {
    let file = include_str!("../data/cupertino.portal");
    let interfaces = file
        .lines()
        .find_map(|l| l.strip_prefix("Interfaces="))
        .unwrap();
    assert!(interfaces.split(';').any(|i| i == INTERFACE));
    let backends = include_str!("../data/cupertino-portals.conf")
        .lines()
        .find_map(|l| l.strip_prefix(INTERFACE)?.strip_prefix('='))
        .unwrap();
    assert_eq!(
        backends.split(';').next(),
        Some(BUS_NAME.rsplit('.').next().unwrap())
    );
}
