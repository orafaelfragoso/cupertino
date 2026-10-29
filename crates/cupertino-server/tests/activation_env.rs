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
    environment: Vec<String>,
    started: Vec<(String, String)>,
}

struct FakeManager {
    calls: Arc<Mutex<Calls>>,
    graphical_session_loaded: bool,
}

#[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
impl FakeManager {
    fn get_unit(&self, name: &str) -> Result<OwnedObjectPath, SystemdError> {
        if name == "graphical-session.target" && self.graphical_session_loaded {
            return Ok(OwnedObjectPath::try_from(GRAPHICAL_UNIT).unwrap());
        }
        Err(SystemdError::NoSuchUnit(format!("{name} is not loaded")))
    }

    fn set_environment(&self, assignments: Vec<String>) {
        self.calls.lock().unwrap().environment = assignments;
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
) -> (zbus::Connection, zbus::Connection, Arc<Mutex<Calls>>) {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let manager = FakeManager {
        calls: calls.clone(),
        graphical_session_loaded: graphical.is_some(),
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
        let (session, _systemd, calls) = session_with_manager(&bus, Some("inactive")).await;
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
    });
}

#[test]
fn a_manager_without_the_graphical_session_loaded_counts_as_idle() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        let (session, _systemd, calls) = session_with_manager(&bus, None).await;
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
        let (session, _systemd, calls) = session_with_manager(&bus, Some("active")).await;
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
