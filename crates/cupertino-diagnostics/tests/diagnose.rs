//! The checks against a fixture tree: a root with installed files and `/run`, a home with the session's logs, an
//! environment and the unit states systemd would report.

use std::collections::HashMap;
use std::fs;
use std::os::unix::net::UnixListener;
use std::path::Path;

use cupertino_diagnostics::events::JournalResult;
use cupertino_diagnostics::{
    run, Area, Check, JournalFn, JournalFollowFn, Scope, Sources, Status, Unit, NOT_FOUND,
};

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
            journal: no_journal(),
            journal_follow: no_journal_follow(),
        };
        run(&sources, &[area]).checks
    }
}

/// A machine without journalctl; these areas never ask it.
fn no_journal() -> JournalFn {
    Box::new(|_| JournalResult::Unavailable("no journalctl in the fixture".to_owned()))
}

fn no_journal_follow() -> JournalFollowFn {
    Box::new(|_| {
        let mut command = std::process::Command::new("true");
        command.env("CUPERTINO_ASSET_SEARCH_PATH", "");
        command
    })
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
    assert_eq!(find(&checks, "crash.gpu_reset").status, Status::Ok);
}

#[test]
fn gpu_resets_are_counted_and_the_last_one_is_told_in_parts() {
    let fixture = Fixture::new();
    fixture.state(
        "session.log",
        "2026-11-01 20:00:00 starting\n\
         2026-11-01 20:05:00 the GPU was reset: gpu=/dev/dri/card1 reason=another program caused it; renderer remade, the \
         clients stayed connected\n\
         2026-11-01 20:06:00 the GPU was reset: gpu=/dev/dri/card1 reason=the rendering context was lost (EGL: \
         context lost); renderer not remade (EGL context: bad alloc), the session ends\n\
         2026-11-01 20:06:01 the compositor ended with status 1 after 360 s\n",
    );
    let checks = fixture.run(Area::Crash);
    let reset = find(&checks, "crash.gpu_reset");
    assert_eq!(reset.status, Status::Warn);
    assert_eq!(
        reset.summary,
        "the GPU was reset 2 times, the last at 2026-11-01 20:06:00: renderer not remade (EGL context: bad alloc), the \
         session ends"
    );
    assert_eq!(fact(reset, "remade"), "1 of 2");
    assert_eq!(fact(reset, "last_gpu"), "/dev/dri/card1");
    assert_eq!(
        fact(reset, "last_reason"),
        "the rendering context was lost (EGL: context lost)"
    );
    assert_eq!(find(&checks, "crash.last").status, Status::Warn);
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

/// `services.network` for netd's (active, file) state and NetworkManager's, `None` when systemd has no answer for it.
fn network(netd: (&str, &str), network_manager: Option<(&str, &str)>) -> (Status, String) {
    let mut fixture = Fixture::new().unit_file("cupertino-netd.service", netd.0, netd.1);
    if let Some((active, file)) = network_manager {
        fixture = fixture.unit_file("NetworkManager.service", active, file);
    }
    let check = find(&fixture.run(Area::Services), "services.network").clone();
    (check.status, check.summary)
}

#[test]
fn a_daemon_in_any_off_state_is_ok_while_the_distribution_service_does_its_job() {
    let running = Some(("active", "enabled"));
    let ok = |summary: &str| (Status::Ok, summary.to_owned());
    for (netd, expected) in [
        (
            ("inactive", "static"),
            ok("cupertino-netd.service is inactive (static): managed by NetworkManager"),
        ),
        (
            ("inactive", "indirect"),
            ok("cupertino-netd.service is inactive (indirect): managed by NetworkManager"),
        ),
        (
            ("inactive", "disabled"),
            ok("cupertino-netd.service is inactive (disabled): managed by NetworkManager"),
        ),
        (
            ("inactive", "masked"),
            ok("cupertino-netd.service is inactive (masked): managed by NetworkManager"),
        ),
        (
            ("inactive", "enabled"),
            ok("cupertino-netd.service is inactive (enabled): managed by NetworkManager"),
        ),
        (
            ("failed", "static"),
            ok("cupertino-netd.service is failed (static): managed by NetworkManager"),
        ),
        (
            ("inactive", ""),
            ok("cupertino-netd.service is inactive: managed by NetworkManager"),
        ),
        (
            (NOT_FOUND, ""),
            ok("cupertino-netd.service is not installed: managed by NetworkManager"),
        ),
        (("active", "static"), ok("cupertino-netd.service is active")),
    ] {
        assert_eq!(network(netd, running), expected, "{netd:?}");
    }
    assert_eq!(
        network(("inactive", "static"), Some(("activating", "enabled"))),
        ok("cupertino-netd.service is inactive (static): managed by NetworkManager")
    );
}

#[test]
fn bluez_enabled_but_stopped_without_an_adapter_does_the_bluetooth_job() {
    let fixture = Fixture::new()
        .unit_file("cupertino-btd.service", "inactive", "static")
        .unit_file("bluetooth.service", "inactive", "enabled");
    let bluetooth = find(&fixture.run(Area::Services), "services.bluetooth").clone();
    assert_eq!(
        (bluetooth.status, bluetooth.summary.as_str()),
        (
            Status::Ok,
            "cupertino-btd.service is inactive (static): managed by BlueZ"
        )
    );
}

#[test]
fn an_off_daemon_warns_when_nothing_does_its_job() {
    let unserved = (
        Status::Warn,
        "cupertino-netd.service is inactive: the network menu and settings cannot change anything"
            .to_owned(),
    );
    for network_manager in [
        Some(("inactive", "disabled")),
        Some(("inactive", "static")),
        Some(("failed", "enabled")),
        Some((NOT_FOUND, "")),
        None,
    ] {
        assert_eq!(
            network(("inactive", "static"), network_manager),
            unserved,
            "{network_manager:?}"
        );
    }
    assert_eq!(
        network(("failed", "static"), Some(("inactive", "disabled"))),
        (
            Status::Fail,
            "cupertino-netd.service is failed: the network menu and settings cannot change anything".to_owned()
        )
    );
    let fixture = Fixture::new()
        .unit_file("cupertino-btd.service", "inactive", "static")
        .unit_file("bluetooth.service", NOT_FOUND, "")
        // nothing takes over power profiles, so an off powerd still warns
        .unit_file("cupertino-powerd.service", "inactive", "static");
    let checks = fixture.run(Area::Services);
    for (id, summary) in [
        (
            "services.bluetooth",
            "cupertino-btd.service is inactive: no Bluetooth menu",
        ),
        (
            "services.power",
            "cupertino-powerd.service is inactive: no battery or power profile information",
        ),
    ] {
        let check = find(&checks, id);
        assert_eq!(
            (check.status, check.summary.as_str()),
            (Status::Warn, summary),
            "{id}"
        );
    }
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
fn a_session_with_only_a_software_renderer_warns_until_another_session_starts() {
    let fixture = Fixture::new();
    let start = "2026-11-02 08:59:59 starting\n";
    let software = "2026-11-02 09:00:00 only a software renderer is available: gpu=/dev/dri/card1; the CPU draws \
                    the desktop\n";
    let end =
        "2026-11-02 10:00:00 the compositor was stopped by the session (status 143) after 3600 s\n";
    let renderer = |log: &str| {
        fixture.state("session.log", log);
        find(&fixture.run(Area::Gpu), "gpu.renderer").clone()
    };

    for log in [
        format!("{start}{software}"),
        format!("{end}{start}{software}{end}"),
    ] {
        let check = renderer(&log);
        assert_eq!(check.status, Status::Warn, "{log}");
        assert_eq!(
            check.summary,
            "the last session had only a software renderer on /dev/dri/card1: the CPU draws the desktop"
        );
        assert_eq!(fact(&check, "time"), "2026-11-02 09:00:00");
        assert_eq!(fact(&check, "gpu"), "/dev/dri/card1");
    }
    for log in [
        format!("{start}{software}{end}{start}"),
        format!("{start}{software}{end}2026-11-02 10:05:00 starting with a limit of 600 s\n"),
    ] {
        let check = renderer(&log);
        assert_eq!(
            (check.status, check.summary.as_str()),
            (Status::Ok, "the last session rendered on a GPU"),
            "{log}"
        );
    }
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
        journal: no_journal(),
        journal_follow: no_journal_follow(),
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

/// The packaged shell list's dock and lock screen, and a development folder for the dock, the menu bar and the lock
/// screen (which may not start from it).
const DEV_FOLDER_CONFIG: &str = r#"{
  "shell": [
    {"name": "dock", "command": ["/usr/bin/cupertino-dock"], "roles": ["session", "dock"]},
    {"name": "menubar", "command": ["/usr/bin/cupertino-menubar"], "roles": ["session", "menu_bar"]},
    {"name": "login", "command": ["/usr/bin/cupertino-login"], "roles": ["session", "session_lock"]}
  ],
  "dev_folder": {"path": "/home/me/dev", "components": ["dock", "menubar", "login"]}
}"#;

impl Fixture {
    /// The file at `path` below the root, modified `seconds` after a fixed time.
    fn file_at(&self, path: &str, seconds: u64) {
        self.file(path, "program");
        let path = self
            .dir
            .path()
            .join("root")
            .join(path.trim_start_matches('/'));
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(
                std::time::SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_secs(1_794_398_405 + seconds),
            )
            .unwrap();
    }
}

#[test]
fn a_development_build_older_than_its_installed_program_warns_with_the_command_that_removes_it() {
    let fixture = Fixture::new();
    fixture.file("/etc/cupertino/compositor.json", DEV_FOLDER_CONFIG);
    for program in ["dock", "menubar", "login"] {
        fixture.file_at(&format!("/usr/bin/cupertino-{program}"), 60);
    }
    fixture.file_at("/home/me/dev/cupertino-dock", 0);
    fixture.file_at("/home/me/dev/cupertino-menubar", 120);
    // the lock screen never starts from the folder, so its file is not looked at
    fixture.file_at("/home/me/dev/cupertino-login", 0);
    let checks = fixture.run(Area::Shell);
    let check = find(&checks, "shell.dev_folder");
    assert_eq!(check.status, Status::Warn);
    assert!(check.summary.contains("dock"), "{}", check.summary);
    assert!(!check.summary.contains("menubar"), "{}", check.summary);
    assert_eq!(fact(check, "folder"), "/home/me/dev");
    assert_eq!(
        fact(check, "older"),
        "dock: /home/me/dev/cupertino-dock is older than /usr/bin/cupertino-dock"
    );
    assert_eq!(
        fact(check, "build"),
        "menubar: /home/me/dev/cupertino-menubar"
    );
    assert!(!check.facts.iter().any(|f| f.value.contains("login")));
    assert!(
        check
            .fix
            .as_deref()
            .unwrap()
            .starts_with("`cupertino shell use-installed dock` removes"),
        "{:?}",
        check.fix
    );
}

#[test]
fn development_builds_as_new_as_their_programs_or_without_one_are_ok() {
    let fixture = Fixture::new();
    fixture.file("/etc/cupertino/compositor.json", DEV_FOLDER_CONFIG);
    let check = || find(&fixture.run(Area::Shell), "shell.dev_folder").clone();
    assert_eq!(
        (check().status, check().summary),
        (
            Status::Ok,
            "the development folder holds no build".to_owned()
        )
    );

    // a build without an installed program, and one of the same time as its program
    fixture.file_at("/home/me/dev/cupertino-dock", 0);
    fixture.file_at("/home/me/dev/cupertino-menubar", 60);
    fixture.file_at("/usr/bin/cupertino-menubar", 60);
    let ok = check();
    assert_eq!(ok.status, Status::Ok, "{ok:?}");
    assert_eq!(ok.fix, None);
    assert_eq!(
        ok.facts.iter().filter(|f| f.key == "build").count(),
        2,
        "{ok:?}"
    );
}

#[test]
fn without_a_declared_development_folder_the_shell_check_is_skipped() {
    let fixture = Fixture::new();
    let skip = |fixture: &Fixture| find(&fixture.run(Area::Shell), "shell.dev_folder").status;
    assert_eq!(skip(&fixture), Status::Skip, "no system file");
    fixture.file(
        "/etc/cupertino/compositor.json",
        r#"{"shell": [{"name": "dock", "command": ["/usr/bin/cupertino-dock"]}]}"#,
    );
    fixture.file_at("/usr/bin/cupertino-dock", 60);
    assert_eq!(skip(&fixture), Status::Skip, "no dev_folder key");
}

#[test]
fn every_running_process_reports_its_renderer_and_one_that_fell_back_from_the_gpu_warns() {
    let fixture = Fixture::new().in_session();
    let check = |fixture: &Fixture| find(&fixture.run(Area::Gpu), "gpu.client_renderers").clone();
    assert_eq!(check(&fixture).status, Status::Skip);

    let records = fixture.dir.path().join("runtime/cupertino/renderers");
    fs::create_dir_all(&records).unwrap();
    let record = |pid: u32, program: &str, renderer: &str, reason: &str| {
        fs::write(
            records.join(pid.to_string()),
            format!("program={program}\nrenderer={renderer}\nreason={reason}\n"),
        )
        .unwrap();
    };
    record(41, "cupertino-dock", "cpu", "the GPU path is not turned on");
    record(42, "cupertino-files", "gpu", "the GPU");
    record(43, "gone", "cpu", "no GPU device: no libEGL");
    fixture.file("/proc/41/stat", "");
    fixture.file("/proc/42/stat", "");
    let seen = check(&fixture);
    assert_eq!(
        (seen.status, seen.summary.as_str()),
        (
            Status::Ok,
            "1 of 2 processes draw on the GPU, the rest on the CPU"
        )
    );
    let facts: Vec<&str> = seen.facts.iter().map(|f| f.value.as_str()).collect();
    assert_eq!(
        facts,
        [
            "cupertino-dock (41): cpu, the GPU path is not turned on",
            "cupertino-files (42): gpu, the GPU"
        ]
    );

    fixture.file("/proc/43/stat", "");
    let seen = check(&fixture);
    assert_eq!(seen.status, Status::Warn);
    assert_eq!(
        seen.summary,
        "1 of 3 processes draw on the CPU after the GPU failed; gone (43): no GPU device: no libEGL"
    );
}
