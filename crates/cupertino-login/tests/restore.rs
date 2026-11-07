//! The applications a log out saves and the next login starts, against a private session bus (`dbus-daemon --session`)
//! and a fixture home: a stand-in for the server's `org.cupertino.Session1` names the applications as the quit sequence
//! asks them to quit, their identities are saved as desktop file IDs, and the next login starts each once through its
//! desktop entry. Skipped when the daemon is not available.
#![cfg(feature = "dbus")]

use std::collections::{HashSet, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cupertino_apps::entry::{index_dirs, Locale, Visibility};
use cupertino_login::restore::{self, Applications, SessionMark, Skip, Taken};
use cupertino_login::session;
use cupertino_test_bus::PrivateBus;

/// The server's quit interface as the agent sees it: each call names the next application, then `("", 0)`.
struct Session1 {
    apps: Mutex<VecDeque<&'static str>>,
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl Session1 {
    fn quit_next_application(&self) -> (String, u32) {
        match self.apps.lock().unwrap().pop_front() {
            Some(app) => (app.to_owned(), 1),
            None => (String::new(), 0),
        }
    }
}

/// A desktop entry `id` in `applications` whose program appends `name` to `marker`.
fn application(applications: &Path, bin: &Path, marker: &Path, id: &str, name: &str, extra: &str) {
    let program = bin.join(name);
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho {name} >> '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        applications.join(id),
        format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec={} %F\n{extra}",
            program.display()
        ),
    )
    .unwrap();
}

fn installed(applications: &Path) -> Applications {
    Applications::new(index_dirs(
        &[applications.to_path_buf()],
        &Locale::default(),
        &Visibility {
            current_desktops: vec!["Cupertino".into()],
            path_dirs: Vec::new(),
            skip_try_exec: true,
        },
    ))
}

fn lines(marker: &Path) -> Vec<String> {
    let mut lines: Vec<String> = std::fs::read_to_string(marker)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    lines.sort();
    lines
}

#[test]
fn a_log_out_saves_the_open_applications_and_the_next_login_starts_each_once() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let applications = home.path().join(".local/share/applications");
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&applications).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let marker = home.path().join("started");
    application(
        &applications,
        &bin,
        &marker,
        "org.example.Editor.desktop",
        "editor",
        "",
    );
    application(
        &applications,
        &bin,
        &marker,
        "org.example.Viewer.desktop",
        "viewer",
        "StartupWMClass=viewer-window\n",
    );
    application(
        &applications,
        &bin,
        &marker,
        "org.example.Hidden.desktop",
        "hidden",
        "NoDisplay=true\n",
    );
    application(
        &applications,
        &bin,
        &marker,
        "cupertino-menubar.desktop",
        "menubar",
        "",
    );
    let state: PathBuf = restore::state_path(Some(home.path().join(".local/state")), None).unwrap();

    // the server's interface names the applications in the order it quits them
    let _server = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.cupertino.Session1")
        .unwrap()
        .serve_at(
            "/org/cupertino/Session1",
            Session1 {
                apps: Mutex::new(VecDeque::from([
                    "org.example.Editor",
                    "cupertino-menubar",
                    "viewer-window",
                    "org.example.Hidden",
                    "org.example.NoEntry",
                    "org.example.editor",
                ])),
            },
        )
        .unwrap()
        .build()
        .unwrap();
    let agent = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap();
    let mut quit = Vec::new();
    loop {
        let (app, windows) = session::quit_next_application(&agent).expect("QuitNextApplication");
        if app.is_empty() || windows == 0 {
            break;
        }
        quit.push(app);
    }
    let (apps, skipped) =
        restore::saved_list(quit.iter().map(String::as_str), &installed(&applications));
    assert_eq!(
        apps,
        ["org.example.Editor.desktop", "org.example.Viewer.desktop"]
    );
    assert_eq!(
        skipped,
        [
            ("cupertino-menubar".to_string(), Skip::Shell),
            ("org.example.Hidden".to_string(), Skip::NoEntry),
            ("org.example.NoEntry".to_string(), Skip::NoEntry),
            ("org.example.editor".to_string(), Skip::Duplicate),
        ]
    );
    let ended = SessionMark::new("boot-a", "2").unwrap();
    restore::save(&state, true, apps.clone(), Some(ended)).unwrap();
    let saved = std::fs::read_to_string(&state).unwrap();
    assert!(
        !saved.contains('/') && !saved.contains("Exec"),
        "only IDs are stored:\n{saved}"
    );

    // the viewer is removed before the next login
    std::fs::remove_file(applications.join("org.example.Viewer.desktop")).unwrap();
    let next = SessionMark::new("boot-b", "2").unwrap();
    let Taken::Start(ids) = restore::take_at_login(&state, Some(&next)).unwrap() else {
        panic!("the list is handed out at the next login");
    };
    assert_eq!(ids, apps);
    let report = restore::launch(
        &ids,
        &installed(&applications),
        &HashSet::new(),
        |_, command| cupertino_apps::launch::spawn("restore-test", command.to_command()),
    );
    assert_eq!(report.started, ["org.example.Editor.desktop"]);
    assert_eq!(
        report.skipped,
        [("org.example.Viewer.desktop".to_string(), Skip::NoEntry)]
    );
    let end = Instant::now() + Duration::from_secs(10);
    while lines(&marker).is_empty() {
        assert!(Instant::now() < end, "the editor never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        restore::take_at_login(&state, Some(&next)).unwrap(),
        Taken::Nothing,
        "a second login process in the same session starts nothing"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(lines(&marker), ["editor"]);
}

#[test]
fn a_failed_start_is_reported_and_the_rest_still_start() {
    let home = tempfile::tempdir().unwrap();
    let applications = home.path().join("applications");
    std::fs::create_dir_all(&applications).unwrap();
    let marker = home.path().join("started");
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        applications.join("org.example.Missing.desktop"),
        "[Desktop Entry]\nType=Application\nName=Missing\nExec=/nonexistent/program\n",
    )
    .unwrap();
    application(
        &applications,
        &bin,
        &marker,
        "org.example.Editor.desktop",
        "editor",
        "",
    );
    let ids = vec![
        "org.example.Missing.desktop".to_string(),
        "org.example.Editor.desktop".to_string(),
    ];
    let report = restore::launch(
        &ids,
        &installed(&applications),
        &HashSet::new(),
        |_, command| cupertino_apps::launch::spawn("restore-test", command.to_command()),
    );
    assert_eq!(report.started, ["org.example.Editor.desktop"]);
    assert_eq!(
        report.skipped,
        [("org.example.Missing.desktop".to_string(), Skip::Failed)]
    );
}
