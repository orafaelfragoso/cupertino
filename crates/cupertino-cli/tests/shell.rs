//! `cupertino shell list|restart|use-installed` against a stand-in for the desktop's server on a private session bus:
//! what the commands print, how the server's answers and refusals map to failures, and that `use-installed` removes
//! the development folder's file before it restarts. Skipped when `dbus-daemon` is not installed.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cupertino_cli::{Context, Output, Runner, REGISTRY};
use cupertino_test_bus::PrivateBus;
use support::{args, envelope};

type Record = (String, String, u32, u64, u64, String, bool, String, String);

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Session1.Error")]
enum StandInError {
    #[zbus(error)]
    ZBus(zbus::Error),
    UntrustedCaller(String),
}

/// The server's shell process interface, with a dock that may start from `dev` and a login process that may not.
struct StandIn {
    dev: PathBuf,
    /// Where the launcher's installed program is.
    bin: PathBuf,
    /// The names restarted, and whether the dock's development file was there at each restart.
    restarts: Arc<Mutex<Vec<(String, bool)>>>,
}

impl StandIn {
    fn dock(&self, pid: u32, from_dev: bool) -> Record {
        let program = if from_dev {
            self.dev.join("cupertino-dock").display().to_string()
        } else {
            "/usr/bin/cupertino-dock".into()
        };
        (
            "dock".into(),
            "running".into(),
            pid,
            98_765,
            1_794_398_405,
            program,
            from_dev,
            "/usr/bin/cupertino-dock".into(),
            self.dev.join("cupertino-dock").display().to_string(),
        )
    }
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl StandIn {
    fn list_shell_processes(&self) -> Vec<Record> {
        vec![
            self.dock(41, true),
            (
                "login".into(),
                "running".into(),
                42,
                98_000,
                1_794_398_400,
                "/usr/bin/cupertino-login".into(),
                false,
                "/usr/bin/cupertino-login".into(),
                String::new(),
            ),
            (
                "osd".into(),
                "waiting".into(),
                0,
                0,
                0,
                String::new(),
                false,
                "/usr/bin/cupertino-osd".into(),
                self.dev.join("cupertino-osd").display().to_string(),
            ),
            (
                "launcher".into(),
                "running".into(),
                44,
                98_800,
                1_794_398_405,
                self.dev.join("cupertino-launcher").display().to_string(),
                true,
                self.bin.join("cupertino-launcher").display().to_string(),
                self.dev.join("cupertino-launcher").display().to_string(),
            ),
        ]
    }

    fn restart_shell_process(&self, name: String) -> Result<Record, StandInError> {
        let dev_file = self.dev.join("cupertino-dock").exists();
        self.restarts.lock().unwrap().push((name.clone(), dev_file));
        match name.as_str() {
            "dock" => Ok(self.dock(43, dev_file)),
            "refused" => Err(StandInError::UntrustedCaller(
                "this process may not use this method".into(),
            )),
            _ => Err(StandInError::ZBus(
                zbus::fdo::Error::InvalidArgs(format!("no shell process is named {name:?}")).into(),
            )),
        }
    }
}

struct Session {
    bus: PrivateBus,
    _connection: zbus::blocking::Connection,
    dev: tempfile::TempDir,
    bin: tempfile::TempDir,
    restarts: Arc<Mutex<Vec<(String, bool)>>>,
}

impl Session {
    fn start() -> Option<Session> {
        let bus = PrivateBus::start()?;
        let dev = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let restarts = Arc::new(Mutex::new(Vec::new()));
        let connection = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .serve_at(
                "/org/cupertino/Session1",
                StandIn {
                    dev: dev.path().to_owned(),
                    bin: bin.path().to_owned(),
                    restarts: Arc::clone(&restarts),
                },
            )
            .unwrap()
            .name("org.cupertino.Session1")
            .unwrap()
            .build()
            .unwrap();
        Some(Session {
            bus,
            _connection: connection,
            dev,
            bin,
            restarts,
        })
    }

    fn run(&self, words: &[&str]) -> Output {
        run_on(&self.bus.address, words)
    }

    fn dev(&self) -> &Path {
        self.dev.path()
    }
}

fn run_on(address: &str, words: &[&str]) -> Output {
    let root = tempfile::tempdir().unwrap();
    let cx = Context::isolated(root.path()).with_var("DBUS_SESSION_BUS_ADDRESS", address);
    Runner::new(REGISTRY).context(cx).run(&args(words), "r1")
}

#[test]
fn the_list_shows_each_process_with_its_program_and_where_it_comes_from() {
    let Some(s) = Session::start() else { return };
    let out = s.run(&["shell", "list"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), 5, "{}", out.stdout);
    assert!(lines[0].starts_with("NAME"));
    assert!(
        lines[1].contains("dock")
            && lines[1].contains("running")
            && lines[1].contains("41")
            && lines[1].contains("development folder"),
        "{}",
        lines[1]
    );
    assert!(lines[2].contains("login") && lines[2].contains("installed"));
    assert!(
        lines[3].contains("osd")
            && lines[3].contains("waiting")
            && lines[3].contains("/usr/bin/cupertino-osd"),
        "{}",
        lines[3]
    );

    let v = envelope(&s.run(&["--format", "json", "shell", "list"]));
    let processes = v["data"]["processes"].as_array().unwrap();
    assert_eq!(processes.len(), 4);
    let dock = &processes[0];
    assert_eq!(dock["name"], "dock");
    assert_eq!(dock["pid"], 41);
    assert_eq!(dock["start_ticks"], 98_765);
    assert_eq!(dock["started"], 1_794_398_405u64);
    assert_eq!(dock["from_dev_folder"], true);
    assert_eq!(dock["installed"], "/usr/bin/cupertino-dock");
    assert_eq!(
        dock["dev_program_older_than_installed"], false,
        "no development file"
    );
    assert_eq!(processes[1]["dev_program"], serde_json::Value::Null);
    let osd = &processes[2];
    assert_eq!(
        (&osd["pid"], &osd["program"], &osd["started"]),
        (
            &serde_json::Value::Null,
            &serde_json::Value::Null,
            &serde_json::Value::Null
        )
    );
}

/// A file at `path` modified `seconds` after a fixed time.
fn file_at(path: &Path, seconds: u64) {
    let f = std::fs::File::create(path).unwrap();
    f.set_modified(
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_794_398_405 + seconds),
    )
    .unwrap();
}

/// The launcher's line of the table and its `dev_program_older_than_installed`.
fn launcher(s: &Session) -> (String, serde_json::Value) {
    let out = s.run(&["shell", "list"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let line = out
        .stdout
        .lines()
        .find(|l| l.starts_with("launcher"))
        .unwrap()
        .to_owned();
    let v = envelope(&s.run(&["--format", "json", "shell", "list"]));
    let older = v["data"]["processes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "launcher")
        .unwrap()["dev_program_older_than_installed"]
        .clone();
    (line, older)
}

#[test]
fn a_development_build_older_than_the_installed_program_is_marked() {
    let Some(s) = Session::start() else { return };
    let dev = s.dev().join("cupertino-launcher");
    let installed = s.bin.path().join("cupertino-launcher");

    // neither file, then only one of them: nothing to compare
    let (line, older) = launcher(&s);
    assert_eq!(older, false);
    assert!(!line.contains("(older"), "{line}");
    file_at(&dev, 0);
    assert_eq!(launcher(&s).1, false, "no installed program");
    std::fs::remove_file(&dev).unwrap();
    file_at(&installed, 60);
    assert_eq!(launcher(&s).1, false, "no development file");

    // a package built after the development file was copied
    file_at(&dev, 0);
    let (line, older) = launcher(&s);
    assert_eq!(older, true);
    assert!(
        line.contains("development folder (older than installed)"),
        "{line}"
    );

    // the same time, then a newer development file
    file_at(&dev, 60);
    assert_eq!(launcher(&s).1, false, "equal times");
    file_at(&dev, 120);
    let (line, older) = launcher(&s);
    assert_eq!(older, false);
    assert!(
        line.contains("development folder") && !line.contains("(older"),
        "{line}"
    );
}

#[test]
fn an_installed_program_whose_metadata_cannot_be_read_marks_nothing() {
    let Some(s) = Session::start() else { return };
    file_at(&s.dev().join("cupertino-launcher"), 0);
    // the stand-in names bin/cupertino-launcher; a regular file in place of bin makes its metadata unreadable
    let bin = s.bin.path().to_owned();
    std::fs::remove_dir(&bin).unwrap();
    file_at(&bin, 60);
    assert!(std::fs::metadata(bin.join("cupertino-launcher"))
        .is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound));
    let (line, older) = launcher(&s);
    assert_eq!(older, false);
    assert!(!line.contains("(older"), "{line}");
    std::fs::remove_file(&bin).unwrap();
    std::fs::create_dir(&bin).unwrap();
}

#[test]
fn a_restart_reports_the_new_process_once_it_runs() {
    let Some(s) = Session::start() else { return };
    let out = s.run(&["shell", "restart", "dock"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "dock restarted: pid 43, started 2026-11-11 12:00:05 UTC, /usr/bin/cupertino-dock (installed)\n"
    );
    let v = envelope(&s.run(&["--format", "json", "shell", "restart", "dock"]));
    assert_eq!(v["command"], "shell restart");
    assert_eq!(v["data"]["pid"], 43);
    assert_eq!(v["data"]["state"], "running");
}

#[test]
fn the_servers_refusals_are_failures_of_their_own() {
    let Some(s) = Session::start() else { return };
    let v = envelope(&s.run(&["--format", "json", "shell", "restart", "nope"]));
    assert_eq!(v["error"]["kind"], "invalid_usage", "{v}");
    assert_eq!(v["error"]["code"], "unknown_process");
    assert!(v["error"]["message"].as_str().unwrap().contains("nope"));

    let out = s.run(&["shell", "restart", "refused"]);
    assert_eq!(out.code, 4);
    assert!(
        out.stderr
            .contains("only an unsandboxed process of the session's own user"),
        "{}",
        out.stderr
    );
}

#[test]
fn use_installed_removes_the_development_file_then_restarts() {
    let Some(s) = Session::start() else { return };
    let file = s.dev().join("cupertino-dock");
    std::fs::write(&file, "build").unwrap();
    let v = envelope(&s.run(&["--format", "json", "shell", "use-installed", "dock"]));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["data"]["removed"], file.display().to_string());
    assert_eq!(v["data"]["process"]["from_dev_folder"], false);
    assert!(!file.exists());
    assert_eq!(
        *s.restarts.lock().unwrap(),
        [("dock".to_owned(), false)],
        "the file was gone before the restart"
    );

    // nothing left to remove: it still restarts on the installed program
    let out = s.run(&["shell", "use-installed", "dock"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout
            .starts_with("the development folder holds no dock program\n"),
        "{}",
        out.stdout
    );
    assert_eq!(s.restarts.lock().unwrap().len(), 2);
}

#[test]
fn use_installed_refuses_a_process_the_folder_does_not_apply_to_or_does_not_know() {
    let Some(s) = Session::start() else { return };
    let v = envelope(&s.run(&["--format", "json", "shell", "use-installed", "login"]));
    assert_eq!(v["error"]["code"], "no_dev_program");
    assert_eq!(v["error"]["kind"], "validation");
    let v = envelope(&s.run(&["--format", "json", "shell", "use-installed", "nope"]));
    assert_eq!(v["error"]["code"], "unknown_process");
    assert!(
        s.restarts.lock().unwrap().is_empty(),
        "nothing was restarted"
    );
}

#[test]
fn without_a_bus_or_a_server_the_commands_are_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let out = Runner::new(REGISTRY)
        .context(Context::isolated(root.path()))
        .run(&args(&["--format", "json", "shell", "list"]), "r1");
    let v = envelope(&out);
    assert_eq!(out.code, 3);
    assert_eq!(v["error"]["code"], "no_session_bus");

    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let out = run_on(
        &bus.address,
        &["--format", "json", "shell", "restart", "dock"],
    );
    let v = envelope(&out);
    assert_eq!(out.code, 3);
    assert_eq!(v["error"]["code"], "no_server");
}
