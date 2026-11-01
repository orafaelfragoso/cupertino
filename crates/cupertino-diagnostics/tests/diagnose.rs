//! The checks against a fixture tree: a root with installed files and `/run`, a home with the session's logs, an
//! environment and the unit states systemd would report.

use std::collections::HashMap;
use std::fs;
use std::os::unix::net::UnixListener;
use std::path::Path;

use cupertino_diagnostics::{run, Area, Check, Scope, Sources, Status, Unit, NOT_FOUND};

struct Fixture {
    dir: tempfile::TempDir,
    env: HashMap<String, String>,
    units: HashMap<String, Unit>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let env = HashMap::from([("HOME".to_owned(), home.display().to_string())]);
        Fixture {
            dir,
            env,
            units: HashMap::new(),
        }
    }

    fn in_session(mut self) -> Self {
        let runtime = self.dir.path().join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        for (key, value) in [
            ("XDG_SESSION_ID", "4"),
            ("XDG_CURRENT_DESKTOP", "Cupertino"),
            ("WAYLAND_DISPLAY", "wayland-1"),
        ] {
            self.env.insert(key.to_owned(), value.to_owned());
        }
        self.env
            .insert("XDG_RUNTIME_DIR".to_owned(), runtime.display().to_string());
        self
    }

    fn file(&self, path: &str, text: &str) {
        let path = self
            .dir
            .path()
            .join("root")
            .join(path.trim_start_matches('/'));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn state(&self, name: &str, text: &str) {
        let path = self
            .dir
            .path()
            .join("home/.local/state/cupertino")
            .join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn unit(self, name: &str, state: &str) -> Self {
        self.unit_file(name, state, "enabled")
    }

    fn unit_file(mut self, name: &str, state: &str, file: &str) -> Self {
        let unit = Unit {
            active: state.to_owned(),
            file: file.to_owned(),
        };
        self.units.insert(name.to_owned(), unit);
        self
    }

    fn run(&self, area: Area) -> Vec<Check> {
        let units = self.units.clone();
        let sources = Sources {
            sysfs: self.dir.path().join("sys"),
            root: self.dir.path().join("root"),
            env: self.env.clone(),
            unit_state: Box::new(move |_: Scope, name: &str| units.get(name).cloned()),
        };
        run(&sources, &[area]).checks
    }
}

fn find<'a>(checks: &'a [Check], id: &str) -> &'a Check {
    checks
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no check {id} in {checks:?}"))
}

fn fact<'a>(check: &'a Check, key: &str) -> &'a str {
    check
        .facts
        .iter()
        .find(|f| f.key == key)
        .map(|f| f.value.as_str())
        .unwrap_or_else(|| panic!("no fact {key} in {check:?}"))
}

#[test]
fn a_live_cupertino_session_is_ok_and_a_dead_display_fails() {
    let fixture = Fixture::new().in_session();
    fixture.file(
        "/run/systemd/sessions/4",
        "UID=1000\nTYPE=wayland\nCLASS=user\nACTIVE=1\nSERVICE=cupertino-session\nVTNR=2\nSEAT=seat0\n",
    );
    let socket = fixture.dir.path().join("runtime/wayland-1");
    let listener = UnixListener::bind(&socket).unwrap();
    let checks = fixture.run(Area::Session);
    let statuses: Vec<(&str, Status)> = checks.iter().map(|c| (c.id, c.status)).collect();
    assert_eq!(
        statuses,
        [
            ("session.logind", Status::Ok),
            ("session.desktop", Status::Ok),
            ("session.wayland", Status::Ok),
        ]
    );
    let logind = find(&checks, "session.logind");
    assert_eq!(logind.summary, "session 4: type wayland, active");
    assert_eq!(fact(logind, "pam_service"), "cupertino-session");
    assert_eq!(fact(logind, "vt"), "2");

    drop(listener);
    let checks = fixture.run(Area::Session);
    let wayland = find(&checks, "session.wayland");
    assert_eq!(wayland.status, Status::Fail, "{wayland:?}");
    assert!(wayland
        .summary
        .starts_with("nothing answers on wayland-1: "));
}

#[test]
fn outside_a_session_the_session_checks_are_skipped() {
    let checks = Fixture::new().run(Area::Session);
    assert_eq!(checks.len(), 1);
    assert_eq!(
        (checks[0].id, checks[0].status),
        ("session.logind", Status::Skip)
    );
}

#[test]
fn a_crash_is_the_last_end_with_its_server_log_line() {
    let fixture = Fixture::new();
    fixture.state(
        "session.log",
        "2026-11-01 20:00:00 starting\n\
         2026-11-01 20:10:00 the compositor ended normally after 600 s\n\
         2026-11-01 20:11:00 starting\n\
         2026-11-01 20:15:00 the compositor ended with status 134 after 240 s: the server was ended by SIGABRT\n",
    );
    fixture.state(
        "server.log",
        "2026-11-01T20:11:01Z INFO starting\nfatal signal SIGABRT; backtrace:\n   0: frame\n",
    );
    let checks = fixture.run(Area::Crash);
    let last = find(&checks, "crash.last");
    assert_eq!(last.status, Status::Warn);
    assert_eq!(
        last.summary,
        "the last session ended badly: 2026-11-01 20:15:00 the compositor ended with status 134 after 240 s: the \
         server was ended by SIGABRT"
    );
    assert_eq!(fact(last, "ended_badly"), "1 of 2 sessions");
    assert_eq!(fact(last, "server_log"), "fatal signal SIGABRT; backtrace:");

    fixture.state(
        "session.log",
        "2026-11-01 20:15:00 the compositor ended with status 101 after 240 s: panic: x\n\
         2026-11-01 21:00:00 the compositor was stopped by the session (status 143) after 10 s\n",
    );
    let checks = fixture.run(Area::Crash);
    let last = find(&checks, "crash.last");
    assert_eq!(last.status, Status::Ok, "{last:?}");
    assert_eq!(fact(last, "ended_badly"), "1 of 2 sessions");
}

#[test]
fn the_portal_backend_and_its_conf_route_screen_sharing_here() {
    let fixture = Fixture::new();
    let checks = fixture.run(Area::Portals);
    assert_eq!(find(&checks, "portals.backend").status, Status::Warn);
    assert_eq!(find(&checks, "portals.conf").status, Status::Warn);

    fixture.file(
        "/usr/share/xdg-desktop-portal/portals/cupertino.portal",
        "[portal]\nDBusName=org.freedesktop.impl.portal.desktop.cupertino\n\
         Interfaces=org.freedesktop.impl.portal.Settings;org.freedesktop.impl.portal.ScreenCast;\n",
    );
    fixture.file(
        "/usr/share/xdg-desktop-portal/cupertino-portals.conf",
        "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.ScreenCast=cupertino\n",
    );
    let checks = fixture.run(Area::Portals);
    let backend = find(&checks, "portals.backend");
    assert_eq!(backend.status, Status::Ok);
    assert_eq!(
        backend.summary,
        "the Cupertino backend serves Settings, ScreenCast"
    );
    assert_eq!(find(&checks, "portals.conf").status, Status::Ok);
}

#[test]
fn unit_states_map_to_statuses_and_a_missing_manager_skips() {
    let fixture = Fixture::new()
        .unit("cupertino-login.service", "active")
        .unit("cupertino-netd.service", "failed")
        .unit("cupertino-powerd.service", NOT_FOUND);
    let checks = fixture.run(Area::Services);
    let statuses: Vec<(&str, Status)> = checks.iter().map(|c| (c.id, c.status)).collect();
    assert_eq!(
        statuses,
        [
            ("services.login", Status::Ok),
            ("services.network", Status::Fail),
            ("services.power", Status::Warn),
            ("services.bluetooth", Status::Skip),
        ]
    );
    assert_eq!(
        find(&checks, "services.network").fix.as_deref(),
        Some("`systemctl status cupertino-netd.service` and `journalctl -b -u cupertino-netd.service` say why")
    );
    assert!(find(&checks, "services.power")
        .summary
        .contains("is not installed"));
}

#[test]
fn a_disabled_daemon_is_ok_while_the_distribution_service_does_its_job() {
    let fixture = Fixture::new()
        .unit_file("cupertino-netd.service", "inactive", "disabled")
        .unit("NetworkManager.service", "active")
        // masked counts as disabled, and BlueZ is enabled but stopped by its condition without an adapter
        .unit_file("cupertino-btd.service", "inactive", "masked")
        .unit("bluetooth.service", "inactive");
    let checks = fixture.run(Area::Services);
    let network = find(&checks, "services.network");
    assert_eq!(
        (network.status, network.summary.as_str()),
        (
            Status::Ok,
            "cupertino-netd.service is disabled: managed by NetworkManager"
        )
    );
    let bluetooth = find(&checks, "services.bluetooth");
    assert_eq!(
        (bluetooth.status, bluetooth.summary.as_str()),
        (
            Status::Ok,
            "cupertino-btd.service is disabled: managed by BlueZ"
        )
    );
}

#[test]
fn a_disabled_daemon_warns_when_nothing_does_its_job() {
    let fixture = Fixture::new()
        .unit_file("cupertino-netd.service", "inactive", "disabled")
        .unit_file("NetworkManager.service", "inactive", "disabled")
        .unit_file("cupertino-btd.service", "inactive", "disabled")
        .unit_file("bluetooth.service", NOT_FOUND, "")
        // nothing takes over power profiles, so a disabled powerd still warns
        .unit_file("cupertino-powerd.service", "inactive", "disabled");
    let checks = fixture.run(Area::Services);
    for id in ["services.network", "services.bluetooth", "services.power"] {
        assert_eq!(find(&checks, id).status, Status::Warn, "{id}");
    }
    let enabled = Fixture::new()
        .unit("cupertino-netd.service", "inactive")
        .unit("NetworkManager.service", "active");
    assert_eq!(
        find(&enabled.run(Area::Services), "services.network").status,
        Status::Warn
    );
}

#[test]
fn a_machine_without_a_gpu_fails_the_gpu_check() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.dir.path().join("sys/class/drm")).unwrap();
    let checks = fixture.run(Area::Gpu);
    assert_eq!(
        (checks[0].id, checks[0].status),
        ("gpu.driver", Status::Fail)
    );
}

#[test]
fn the_json_form_names_every_field() {
    let fixture = Fixture::new();
    fixture.state("session.log", "");
    let sources = Sources {
        sysfs: fixture.dir.path().join("sys"),
        root: fixture.dir.path().join("root"),
        env: fixture.env.clone(),
        unit_state: Box::new(|_, _| None),
    };
    let report = run(&sources, &[Area::Logs]);
    let json: serde_json::Value = serde_json::to_value(&report).unwrap();
    let home = Path::new(&fixture.env["HOME"]);
    let state = home.join(".local/state/cupertino");
    assert_eq!(
        json,
        serde_json::json!({
            "schema": 1,
            "checks": [{
                "id": "logs.paths",
                "area": "logs",
                "status": "ok",
                "summary": format!("the session's logs are in {}", state.display()),
                "facts": [
                    {"key": "session_log", "value": format!("{} (0 bytes)", state.join("session.log").display())},
                    {"key": "journal", "value": "`journalctl -b -u cupertino-login.service` has the login screen's and the sessions' output"},
                ],
            }],
        })
    );
}
