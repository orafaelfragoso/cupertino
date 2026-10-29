//! The session's variables on a shared user bus, against a private bus with a stand-in for the systemd user manager:
//! they are published and the session target started, unless another graphical session is active or the bus has no manager.
#![cfg(feature = "dbus")]

use std::sync::{Arc, Mutex};

use cupertino_server::activation_env::{publish, Outcome, SESSION_TARGET};
use cupertino_test_bus::PrivateBus;
use zbus::zvariant::OwnedObjectPath;
use zbus::{block_on, connection};

const VARIABLES: [(&str, &str); 3] = [
    ("WAYLAND_DISPLAY", "wayland-cupertino"),
    ("XDG_CURRENT_DESKTOP", "Cupertino"),
    ("XDG_SESSION_TYPE", "wayland"),
];
const GRAPHICAL_UNIT: &str = "/org/freedesktop/systemd1/unit/graphical_2dsession_2etarget";
const OUR_UNIT: &str = "/org/freedesktop/systemd1/unit/cupertino_2dsession_2etarget";

/// What the real manager answers for a unit that is not loaded.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.systemd1")]
enum SystemdError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NoSuchUnit(String),
}

#[derive(Default)]
struct Calls {
    unset: Vec<String>,
    environment: Vec<String>,
    stopped: Vec<String>,
    started: Vec<(String, String)>,
}

struct FakeManager {
    calls: Arc<Mutex<Calls>>,
    graphical_session_loaded: bool,
    ours_loaded: bool,
}

#[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
impl FakeManager {
    fn get_unit(&self, name: &str) -> Result<OwnedObjectPath, SystemdError> {
        if name == "graphical-session.target" && self.graphical_session_loaded {
            return Ok(OwnedObjectPath::try_from(GRAPHICAL_UNIT).unwrap());
        }
        if name == "cupertino-session.target" && self.ours_loaded {
            return Ok(OwnedObjectPath::try_from(OUR_UNIT).unwrap());
        }
        Err(SystemdError::NoSuchUnit(format!("{name} is not loaded")))
    }

    fn unset_and_set_environment(&self, names: Vec<String>, assignments: Vec<String>) {
        let mut calls = self.calls.lock().unwrap();
        calls.unset = names;
        calls.environment = assignments;
    }

    fn stop_unit(&self, name: &str, _mode: &str) -> OwnedObjectPath {
        self.calls.lock().unwrap().stopped.push(name.into());
        OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/2").unwrap()
    }

    fn start_unit(&self, name: &str, mode: &str) -> OwnedObjectPath {
        self.calls
            .lock()
            .unwrap()
            .started
            .push((name.into(), mode.into()));
        OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/1").unwrap()
    }
}

struct FakeUnit(&'static str);

#[zbus::interface(name = "org.freedesktop.systemd1.Unit")]
impl FakeUnit {
    #[zbus(property)]
    fn active_state(&self) -> String {
        self.0.into()
    }
}

async fn session_with_manager(
    bus: &PrivateBus,
    graphical: Option<&'static str>,
    ours: Option<&'static str>,
) -> (zbus::Connection, zbus::Connection, Arc<Mutex<Calls>>) {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let manager = FakeManager {
        calls: calls.clone(),
        graphical_session_loaded: graphical.is_some(),
        ours_loaded: ours.is_some(),
    };
    let mut builder = connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.freedesktop.systemd1")
        .unwrap()
        .serve_at("/org/freedesktop/systemd1", manager)
        .unwrap();
    if let Some(state) = graphical {
        builder = builder.serve_at(GRAPHICAL_UNIT, FakeUnit(state)).unwrap();
    }
    if let Some(state) = ours {
        builder = builder.serve_at(OUR_UNIT, FakeUnit(state)).unwrap();
    }
    let systemd = builder.build().await.unwrap();
    let session = connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    (session, systemd, calls)
}

#[test]
fn an_idle_user_manager_gets_the_variables_and_the_session_target() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        let (session, _systemd, calls) = session_with_manager(&bus, Some("inactive"), None).await;
        assert_eq!(
            publish(&session, &VARIABLES).await.unwrap(),
            Outcome::Applied
        );
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.environment,
            [
                "WAYLAND_DISPLAY=wayland-cupertino",
                "XDG_CURRENT_DESKTOP=Cupertino",
                "XDG_SESSION_TYPE=wayland"
            ]
        );
        assert_eq!(calls.started, [(SESSION_TARGET.into(), "replace".into())]);
        assert_eq!(
            calls.unset,
            ["DISPLAY", "XAUTHORITY"],
            "a left-over X display must not reach the session's services"
        );
    });
}

#[test]
fn the_sessions_own_x_display_is_published_not_unset() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        let (session, _systemd, calls) = session_with_manager(&bus, Some("inactive"), None).await;
        let mut variables = VARIABLES.to_vec();
        variables.extend([
            ("DISPLAY", ":1"),
            ("XAUTHORITY", "/run/user/1000/cupertino-xauth-1"),
        ]);
        assert_eq!(
            publish(&session, &variables).await.unwrap(),
            Outcome::Applied
        );
        let calls = calls.lock().unwrap();
        assert!(calls.unset.is_empty(), "{:?}", calls.unset);
        assert!(calls.environment.contains(&"DISPLAY=:1".to_string()));
        assert!(calls
            .environment
            .contains(&"XAUTHORITY=/run/user/1000/cupertino-xauth-1".to_string()));
    });
}

#[test]
fn a_leftover_session_target_of_ours_is_stopped_and_the_session_goes_on() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        // the graphical session is active only because our own target, left by a session that was killed, binds it
        let (session, _systemd, calls) =
            session_with_manager(&bus, Some("active"), Some("active")).await;
        assert_eq!(
            publish(&session, &VARIABLES).await.unwrap(),
            Outcome::Applied
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.stopped, [SESSION_TARGET]);
        assert_eq!(calls.started, [(SESSION_TARGET.into(), "replace".into())]);
    });
}

#[test]
fn a_manager_without_the_graphical_session_loaded_counts_as_idle() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        let (session, _systemd, calls) = session_with_manager(&bus, None, None).await;
        assert_eq!(
            publish(&session, &VARIABLES).await.unwrap(),
            Outcome::Applied
        );
        assert_eq!(calls.lock().unwrap().started.len(), 1);
    });
}

#[test]
fn another_active_graphical_session_is_left_alone() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        let (session, _systemd, calls) = session_with_manager(&bus, Some("active"), None).await;
        assert_eq!(
            publish(&session, &VARIABLES).await.unwrap(),
            Outcome::AnotherSession
        );
        let calls = calls.lock().unwrap();
        assert!(calls.environment.is_empty() && calls.started.is_empty());
    });
}

#[test]
fn a_bus_without_a_user_manager_is_left_alone() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        let session = connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        assert_eq!(
            publish(&session, &VARIABLES).await.unwrap(),
            Outcome::NoSystemd
        );
    });
}
