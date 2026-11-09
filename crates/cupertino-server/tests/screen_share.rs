//! Screen sharing end to end: the real `cupertino-portal` binary with the capture role against the in-process server,
//! on a private D-Bus daemon and a private PipeWire daemon (never the user's). The test calls ScreenCast as
//! xdg-desktop-portal does, presses Return on the chooser the portal maps once it is armed, and reads frames of the
//! headless output from the stream `Start` names, as a browser would. Skips (passes) without an EGL device, `dbus-daemon` or `pipewire`.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cupertino_screencast::fake::{Consumer, Daemon, Seen};
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::Cupertino;
use cupertino_test_bus::{missing_tool, PrivateBus};
use smithay::backend::input::KeyState;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const OUTPUT: (i32, i32) = (640, 400);
/// How long the test lets the chooser stand before it types: longer than the portal's half-second arming delay, during
/// which the chooser shares nothing.
const ARMED: Duration = Duration::from_millis(700);
/// Evdev code of Return plus the xkb offset of 8.
const TAB: u32 = 15 + 8;
const DOWN: u32 = 108 + 8;
const RETURN: u32 = 28 + 8;
const BACKEND: &str = "org.freedesktop.impl.portal.desktop.cupertino";
const FRONTEND: &str = "org.freedesktop.portal.Desktop";
const PORTAL: &str = "/org/freedesktop/portal/desktop";
const SCREEN_CAST: &str = "org.freedesktop.impl.portal.ScreenCast";
const SESSION: &str = "/org/freedesktop/portal/desktop/session/1_7/share";
const APP: &str = "org.example.Meeting";

type Results = HashMap<String, OwnedValue>;

struct Server {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Server {
    fn start(procs: Vec<ShellProcess>) -> Option<Server> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let cfg = Config {
            shell_processes: procs,
            ..Config::default()
        };
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            cfg,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        start_shell_supervision(&state, &event_loop).expect("the supervision timer is inserted");
        Some(Server { state, event_loop })
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    /// Step the server until `cond` gives a value; panics after [`common::HANG_GUARD`].
    fn wait_for<T>(&mut self, what: &str, mut cond: impl FnMut(&mut Cupertino) -> Option<T>) -> T {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if let Some(value) = cond(&mut self.state) {
                return value;
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn key(&mut self, code: u32) {
        for state in [KeyState::Pressed, KeyState::Released] {
            self.state.keyboard_event_now(Keycode::new(code), state, 0);
            self.step();
        }
    }
}

/// The portal artifact prepared before the test runner starts.
fn portal_binary() -> PathBuf {
    common::fixtures::artifact("cupertino-portal", "bin", "cupertino-portal")
}

fn titled(state: &Cupertino, title: &str) -> bool {
    state.windows.iter().filter(|w| w.mapped).any(|w| {
        with_states(w.surface(), |s| {
            s.data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .title
                .as_deref()
                == Some(title)
        })
    })
}

fn owned(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).unwrap()
}

fn call(
    frontend: &zbus::blocking::Connection,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> (u32, Results) {
    frontend
        .call_method(Some(BACKEND), PORTAL, Some(SCREEN_CAST), method, body)
        .unwrap()
        .body()
        .deserialize()
        .unwrap()
}

#[test]
fn the_portal_shares_the_chosen_display_as_a_stream_of_its_frames() {
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    let Some(pipewire) = Daemon::start(common::HANG_GUARD) else {
        eprintln!("SKIPPED: pipewire is not installed or did not start");
        return;
    };
    let portal = ShellProcess {
        name: "cupertino-portal".into(),
        command: vec![
            "env".into(),
            "CUPERTINO_ASSET_SEARCH_PATH=".into(),
            format!("DBUS_SESSION_BUS_ADDRESS={}", bus.address),
            format!("PIPEWIRE_REMOTE={}", pipewire.remote().display()),
            portal_binary().display().to_string(),
        ],
        roles: vec![ShellRole::Capture],
    };
    let Some(mut server) = Server::start(vec![portal]) else {
        return;
    };
    let frontend = bus.connect();
    frontend.request_name(FRONTEND).unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&frontend).unwrap();
    server.wait_for("the portal on the bus", |_| {
        dbus.name_has_owner(BACKEND.try_into().unwrap())
            .unwrap()
            .then_some(())
    });

    let request = |n: u32| {
        ObjectPath::try_from(format!("/org/freedesktop/portal/desktop/request/1_7/t{n}")).unwrap()
    };
    let session = ObjectPath::try_from(SESSION).unwrap();
    let none = Results::new();
    assert_eq!(
        call(
            &frontend,
            "CreateSession",
            &(request(1), &session, APP, &none)
        )
        .0,
        0
    );
    let options = Results::from([
        ("types".to_owned(), owned(Value::from(1u32))),
        ("cursor_mode".to_owned(), owned(Value::from(1u32))),
    ]);
    assert_eq!(
        call(
            &frontend,
            "SelectSources",
            &(request(2), &session, APP, &options)
        )
        .0,
        0
    );
    let (sender, started) = mpsc::channel();
    let caller = frontend.clone();
    let start = (request(3), session.clone());
    std::thread::spawn(move || {
        let _ = sender.send(call(
            &caller,
            "Start",
            &(start.0, start.1, APP, "", Results::new()),
        ));
    });

    server.wait_for("the chooser", |s| titled(s, "Share Screen").then_some(()));
    let armed = Instant::now() + ARMED;
    server.wait_for("the chooser to arm", |_| {
        (Instant::now() >= armed).then_some(())
    });
    for key in [TAB, DOWN, RETURN] {
        server.key(key);
    }
    server.wait_for("the chooser to close", |s| {
        (!titled(s, "Share Screen")).then_some(())
    });
    let (response, results) = server.wait_for("the answer to Start", |_| started.try_recv().ok());
    assert_eq!(
        response, 0,
        "Return shares the display picked with the keyboard"
    );
    let streams: Vec<(u32, Results)> = results["streams"].try_clone().unwrap().try_into().unwrap();
    assert_eq!(streams.len(), 1);
    let (node, properties) = &streams[0];
    let property = |key: &str| properties[key].try_clone().unwrap();
    assert_eq!(u32::try_from(property("source_type")).unwrap(), 1);
    assert_eq!(<(i32, i32)>::try_from(property("size")).unwrap(), OUTPUT);
    assert_eq!(
        <(i32, i32)>::try_from(property("position")).unwrap(),
        (0, 0)
    );

    let consumer = Consumer::link(&pipewire.remote(), *node);
    let frame = server.wait_for("a frame of the output", |_| {
        consumer.next(Duration::from_millis(1), |s| {
            matches!(s, Seen::Frame { .. })
        })
    });
    let Seen::Frame {
        size,
        stride,
        pixels,
        pts,
        ..
    } = frame
    else {
        unreachable!()
    };
    let (width, height) = (OUTPUT.0 as u32, OUTPUT.1 as u32);
    assert_eq!((size, stride), (width * height * 4, width as i32 * 4));
    assert!(
        pixels.iter().any(|&b| b != 0),
        "the frame holds the rendered output"
    );
    assert!(pts > 0, "the frame carries its presentation time");

    frontend
        .call_method(
            Some(BACKEND),
            SESSION,
            Some("org.freedesktop.impl.portal.Session"),
            "Close",
            &(),
        )
        .unwrap();
    let gone = server.wait_for("the session to end", |_| {
        Some(
            frontend
                .call_method(
                    Some(BACKEND),
                    SESSION,
                    Some("org.freedesktop.DBus.Properties"),
                    "Get",
                    &("org.freedesktop.impl.portal.Session", "version"),
                )
                .is_err(),
        )
        .filter(|gone| *gone)
    });
    assert!(gone);
}
