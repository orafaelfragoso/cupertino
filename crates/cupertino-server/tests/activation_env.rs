//! The session's variables on a shared user bus, against a private bus with a stand-in for the systemd user manager:
//! they are published and the session target started, unless another graphical session is active or the bus has no manager.
//! A server that supervises the portal publishes them once the portal owns its bus name, or after a timeout.
#![cfg(feature = "dbus")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::activation_env::{publish, Outcome, SESSION_TARGET};
use cupertino_server::backend::headless;
use cupertino_server::cli::{
    publish_session_when_ready, start_shell_supervision, PORTAL_READY_TIMEOUT,
};
use cupertino_server::config::{BuiltIn, Config, PORTAL_BUS_NAME};
use cupertino_server::dbus::DbusHandle;
use cupertino_server::Cupertino;
use cupertino_test_bus::PrivateBus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
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
    /// The units asked for, in order: the first call of a publication.
    units: Vec<String>,
    unset: Vec<String>,
    environment: Vec<String>,
    stopped: Vec<String>,
    started: Vec<(String, String)>,
}

struct FakeManager {
    calls: Arc<Mutex<Calls>>,
    /// The manager's `Environment` before the session publishes; `None` answers the property with an error.
    environment: Option<Vec<String>>,
    graphical_session_loaded: bool,
    ours_loaded: bool,
}

#[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
impl FakeManager {
    fn get_unit(&self, name: &str) -> Result<OwnedObjectPath, SystemdError> {
        self.calls.lock().unwrap().units.push(name.into());
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

    #[zbus(property)]
    fn environment(&self) -> zbus::fdo::Result<Vec<String>> {
        self.environment
            .clone()
            .ok_or_else(|| zbus::fdo::Error::Failed("no environment".into()))
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
    let environment = Some(vec!["QT_QPA_PLATFORM=xcb".into()]);
    session_with_manager_environment(bus, graphical, ours, environment).await
}

async fn session_with_manager_environment(
    bus: &PrivateBus,
    graphical: Option<&'static str>,
    ours: Option<&'static str>,
    environment: Option<Vec<String>>,
) -> (zbus::Connection, zbus::Connection, Arc<Mutex<Calls>>) {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let manager = FakeManager {
        calls: calls.clone(),
        environment,
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
            publish(&session, &VARIABLES, &[]).await.unwrap(),
            Outcome::Applied { defaults: vec![] }
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
            publish(&session, &variables, &[]).await.unwrap(),
            Outcome::Applied { defaults: vec![] }
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
            publish(&session, &VARIABLES, &[]).await.unwrap(),
            Outcome::Applied { defaults: vec![] }
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
            publish(&session, &VARIABLES, &[]).await.unwrap(),
            Outcome::Applied { defaults: vec![] }
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
            publish(&session, &VARIABLES, &[]).await.unwrap(),
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
            publish(&session, &VARIABLES, &[]).await.unwrap(),
            Outcome::NoSystemd
        );
    });
}

#[test]
fn a_default_is_published_only_where_the_user_manager_has_no_value_of_its_own() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        let (session, _systemd, calls) = session_with_manager(&bus, Some("inactive"), None).await;
        let defaults = [
            ("QT_QPA_PLATFORM", "wayland;xcb"),
            ("ELECTRON_OZONE_PLATFORM_HINT", "auto"),
        ];
        assert_eq!(
            publish(&session, &VARIABLES, &defaults).await.unwrap(),
            Outcome::Applied {
                defaults: vec!["ELECTRON_OZONE_PLATFORM_HINT".into()]
            },
            "the manager's QT_QPA_PLATFORM=xcb is the user's choice"
        );
        let calls = calls.lock().unwrap();
        assert!(calls
            .environment
            .contains(&"ELECTRON_OZONE_PLATFORM_HINT=auto".to_string()));
        assert!(!calls
            .environment
            .iter()
            .any(|e| e.starts_with("QT_QPA_PLATFORM=")));
    });
}

#[test]
fn an_unreadable_manager_environment_publishes_the_session_without_defaults() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        let (session, _systemd, calls) =
            session_with_manager_environment(&bus, Some("inactive"), None, None).await;
        assert_eq!(
            publish(
                &session,
                &VARIABLES,
                &[("ELECTRON_OZONE_PLATFORM_HINT", "auto")]
            )
            .await
            .unwrap(),
            Outcome::Applied { defaults: vec![] }
        );
        let calls = calls.lock().unwrap();
        assert!(calls
            .environment
            .contains(&"WAYLAND_DISPLAY=wayland-cupertino".to_string()));
        assert!(!calls
            .environment
            .iter()
            .any(|e| e.starts_with("ELECTRON_OZONE_PLATFORM_HINT=")));
        assert_eq!(calls.started, [(SESSION_TARGET.into(), "replace".into())]);
    });
}

#[test]
fn our_default_left_in_the_manager_by_a_killed_session_is_taken_back_and_recorded() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    block_on(async {
        let environment = Some(vec!["QT_QPA_PLATFORM=wayland;xcb".into()]);
        let (session, _systemd, _calls) =
            session_with_manager_environment(&bus, Some("active"), Some("active"), environment)
                .await;
        assert_eq!(
            publish(&session, &VARIABLES, &[("QT_QPA_PLATFORM", "wayland;xcb")])
                .await
                .unwrap(),
            Outcome::Applied {
                defaults: vec!["QT_QPA_PLATFORM".into()]
            }
        );
    });
}

/// A server whose session bus worker is on `bus`, supervising a stand-in portal that never takes its bus name, the
/// publication of its session asked for (`publish_session_when_ready`). The stand-in is a script in `dir`.
fn server_publishing_with_a_portal(
    bus: &PrivateBus,
    dir: &std::path::Path,
) -> (Cupertino, EventLoop<'static, Cupertino>) {
    use std::os::unix::fs::PermissionsExt;
    // a prefix the test's user owns, as a development install
    let bin = dir.join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let portal = bin.join("cupertino-portal");
    std::fs::write(&portal, "#!/bin/sh\nexec sleep 60\n").unwrap();
    std::fs::set_permissions(&portal, std::fs::Permissions::from_mode(0o755)).unwrap();
    let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let mut state = Cupertino::new(
        Display::new().unwrap(),
        event_loop.handle(),
        event_loop.get_signal(),
        Config::default(),
        true,
        &cupertino_assets::Assets::builtin(),
    )
    .unwrap();
    state.dbus = DbusHandle::start_on(Some(bus.address.clone()), || {});
    state
        .supervisor
        .add_built_ins([(BuiltIn::Portal, Some(portal))], Instant::now());
    start_shell_supervision(&state, &event_loop).unwrap();
    publish_session_when_ready(&state);
    (state, event_loop)
}

/// Dispatch the server until `calls` shows a publication, or `limit` passes; how long it took.
fn published_within(
    state: &mut Cupertino,
    event_loop: &mut EventLoop<'static, Cupertino>,
    calls: &Mutex<Calls>,
    limit: Duration,
) -> Option<Duration> {
    let start = Instant::now();
    while start.elapsed() < limit {
        headless::step(state, event_loop, Duration::from_millis(20)).unwrap();
        if !calls.lock().unwrap().units.is_empty() {
            return Some(start.elapsed());
        }
    }
    None
}

/// Another graphical session is active in these tests, so a publication only asks the manager about units and changes
/// nothing, here or in the runtime directory.
#[test]
fn the_session_is_published_once_the_portal_owns_its_bus_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (session, _systemd, calls) = block_on(session_with_manager(&bus, Some("active"), None));
    let dir = tempfile::tempdir().unwrap();
    let (mut state, mut event_loop) = server_publishing_with_a_portal(&bus, dir.path());
    assert_eq!(
        published_within(&mut state, &mut event_loop, &calls, Duration::from_secs(1)),
        None,
        "published before the portal owns its name"
    );
    block_on(session.request_name(PORTAL_BUS_NAME)).unwrap();
    let waited = published_within(&mut state, &mut event_loop, &calls, PORTAL_READY_TIMEOUT)
        .expect("published once the portal owns its name");
    assert!(
        waited < PORTAL_READY_TIMEOUT - Duration::from_secs(1),
        "{waited:?}"
    );
    assert!(
        state.supervisor.pid_of(BuiltIn::Portal.name()).is_some(),
        "the portal runs while the server waits"
    );
}

#[test]
fn the_session_is_published_after_the_timeout_when_the_portal_never_owns_its_bus_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (_session, _systemd, calls) = block_on(session_with_manager(&bus, Some("active"), None));
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let (mut state, mut event_loop) = server_publishing_with_a_portal(&bus, dir.path());
    published_within(
        &mut state,
        &mut event_loop,
        &calls,
        3 * PORTAL_READY_TIMEOUT,
    )
    .expect("published after the timeout");
    let waited = start.elapsed();
    assert!(
        waited >= PORTAL_READY_TIMEOUT && waited < 2 * PORTAL_READY_TIMEOUT,
        "{waited:?}"
    );
}
