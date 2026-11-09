//! The app chooser end to end: the real `cupertino-portal` binary against the in-process server, on a private D-Bus
//! daemon (never the user's). The test calls AppChooser as xdg-desktop-portal does with no activation token, presses
//! Return on the chooser the portal maps, and checks the answer carries a token the server made for the chosen app, one
//! it lets activate. Skips (passes) without an EGL device or `dbus-daemon`.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::handlers::TokenGrant;
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
/// Evdev code of Return plus the xkb offset of 8.
const RETURN: u32 = 28 + 8;
const BACKEND: &str = "org.freedesktop.impl.portal.desktop.cupertino";
const FRONTEND: &str = "org.freedesktop.portal.Desktop";
const PORTAL: &str = "/org/freedesktop/portal/desktop";
const APP_CHOOSER: &str = "org.freedesktop.impl.portal.AppChooser";
const CHOSEN: &str = "org.example.Viewer";

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

/// What the server decided about `token` when it was made, while it is still unused.
fn grant(state: &Cupertino, token: &str) -> Option<TokenGrant> {
    state
        .xdg_activation_state
        .tokens()
        .find(|(t, _)| t.as_str() == token)
        .and_then(|(_, data)| data.user_data.get::<TokenGrant>().cloned())
}

#[test]
fn a_choice_without_a_caller_token_carries_one_the_server_made_for_the_chosen_app() {
    let Some(bus) = PrivateBus::start() else {
        return missing_tool("dbus-daemon");
    };
    let portal = ShellProcess {
        name: "cupertino-portal".into(),
        command: vec![
            "env".into(),
            "CUPERTINO_ASSET_SEARCH_PATH=".into(),
            format!("DBUS_SESSION_BUS_ADDRESS={}", bus.address),
            portal_binary().display().to_string(),
        ],
        roles: vec![],
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

    let (sender, answered) = mpsc::channel();
    let caller = frontend.clone();
    std::thread::spawn(move || {
        let handle =
            ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1_7/open").unwrap();
        let options = Results::from([
            (
                "last_choice".to_owned(),
                OwnedValue::try_from(Value::from(CHOSEN)).unwrap(),
            ),
            (
                "filename".to_owned(),
                OwnedValue::try_from(Value::from("report.pdf")).unwrap(),
            ),
        ]);
        let reply = caller
            .call_method(
                Some(BACKEND),
                PORTAL,
                Some(APP_CHOOSER),
                "ChooseApplication",
                &(
                    handle,
                    "org.example.Mail",
                    "",
                    vec![CHOSEN, "org.example.Draw"],
                    options,
                ),
            )
            .and_then(|m| m.body().deserialize::<(u32, Results)>());
        let _ = sender.send(reply);
    });

    server.wait_for("the chooser", |s| titled(s, "Open With").then_some(()));
    server.key(RETURN);
    let (response, results) = server
        .wait_for("the answer to ChooseApplication", |_| {
            answered.try_recv().ok()
        })
        .unwrap();
    assert_eq!(response, 0, "Return opens the selected app");
    let text = |key: &str| String::try_from(results[key].try_clone().unwrap()).unwrap();
    assert_eq!(text("choice"), CHOSEN);
    let token = text("activation_token");
    let grant = grant(&server.state, &token).expect("the server made the token and it is unused");
    assert!(
        grant.valid,
        "made for the Return the person typed, so it may activate"
    );
    assert_eq!(grant.app_id.as_deref(), Some(CHOSEN));
}
