//! `cupertino shell list`, `restart` and `use-installed` act on the components of the shell host through the server's
//! `org.cupertino.Session1` on a session bus of the test's own, and `dev-folder enable` names components. The server is
//! a stand-in that answers as the real one does (`ListShellProcesses`, `RestartShellProcess`, `ComponentOf`).

use cupertino_cli::{Context, EffectiveUser, Output, Runner, REGISTRY};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// A session bus that starts no service.
const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

struct Bus(Child, String);

impl Bus {
    fn start(name: &str) -> Bus {
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-bus.conf"));
        std::fs::write(&config, BUS_CONFIG).expect("the bus's configuration");
        let mut daemon = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        assert!(!address.trim().is_empty(), "dbus-daemon gives its address");
        Bus(daemon, address.trim().to_owned())
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `(name, state, pid, start_ticks, started, program, from_dev_folder, installed, dev_program)`
type Record = (String, String, u32, u64, u64, String, bool, String, String);

fn record(name: &str, pid: u32, dev_program: &str) -> Record {
    (
        name.into(),
        "running".into(),
        pid,
        1,
        1,
        "/usr/bin/cupertino-shell".into(),
        false,
        "/usr/bin/cupertino-shell".into(),
        dev_program.into(),
    )
}

/// What the stand-in server knows: its processes, the component each bus connection was accepted as, and what it was
/// asked to restart.
struct Session {
    processes: Vec<Record>,
    components: Arc<Mutex<HashMap<String, String>>>,
    restarted: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl Session {
    fn list_shell_processes(&self) -> Vec<Record> {
        self.processes.clone()
    }

    fn restart_shell_process(&self, name: String) -> zbus::fdo::Result<Record> {
        self.restarted.lock().unwrap().push(name.clone());
        let host = self.processes.first().expect("a host runs");
        let is_component = self.components.lock().unwrap().values().any(|c| *c == name);
        if is_component {
            let mut record = host.clone();
            record.0 = name;
            return Ok(record);
        }
        match self.processes.iter().find(|p| p.0 == name) {
            Some(process) => Ok(process.clone()),
            None => Err(zbus::fdo::Error::InvalidArgs(format!(
                "no shell process is named {name:?}"
            ))),
        }
    }

    fn component_of(&self, unique_name: String) -> String {
        self.components
            .lock()
            .unwrap()
            .get(&unique_name)
            .cloned()
            .unwrap_or_default()
    }
}

/// The stand-in server and the connections of the components it accepted, on a bus of the test's own.
struct Desktop {
    _bus: Bus,
    address: String,
    _server: zbus::blocking::Connection,
    _components: Vec<zbus::blocking::Connection>,
    restarted: Arc<Mutex<Vec<String>>>,
}

fn desktop(test: &str, components: &[&str], processes: Vec<Record>) -> Desktop {
    let bus = Bus::start(test);
    let known = Arc::new(Mutex::new(HashMap::new()));
    let restarted = Arc::new(Mutex::new(Vec::new()));
    let connect =
        || zbus::blocking::connection::Builder::address(bus.1.as_str()).expect("the bus's address");
    let mut connections = Vec::new();
    for name in components {
        let connection = connect().build().expect("a component connects");
        known.lock().unwrap().insert(
            connection.unique_name().expect("a unique name").to_string(),
            (*name).to_owned(),
        );
        connections.push(connection);
    }
    let server = connect()
        .name("org.cupertino.Session1")
        .unwrap()
        .serve_at(
            "/org/cupertino/Session1",
            Session {
                processes,
                components: known,
                restarted: restarted.clone(),
            },
        )
        .unwrap()
        .build()
        .expect("the server's connection");
    Desktop {
        address: bus.1.clone(),
        _bus: bus,
        _server: server,
        _components: connections,
        restarted,
    }
}

fn command(desktop: &Desktop, root: &Path, args: &[&str]) -> Output {
    let context = Context::isolated(root).with_var("DBUS_SESSION_BUS_ADDRESS", &desktop.address);
    let args: Vec<String> = ["--format", "json"]
        .into_iter()
        .chain(args.iter().copied())
        .map(String::from)
        .collect();
    Runner::new(REGISTRY)
        .context(context)
        .run(&args, "shell-test")
}

fn json(output: &Output) -> Value {
    serde_json::from_str(&output.stdout)
        .unwrap_or_else(|e| panic!("JSON on stdout ({e}): {output:?}"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn shell_list_names_the_components_of_the_host_and_the_host_that_holds_them() {
    let host = std::process::id();
    let desktop = desktop(
        "list",
        &["menubar", "dock"],
        vec![record("host", host, ""), record("portal", host + 1, "")],
    );
    let root = scratch("list");
    let output = command(&desktop, &root, &["shell", "list"]);
    assert_eq!(output.code, 0, "{output:?}");
    let data = &json(&output)["data"];
    let components: Vec<(&str, &str)> = data["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap(), c["host"].as_str().unwrap()))
        .collect();
    assert_eq!(components, [("dock", "host"), ("menubar", "host")]);
    assert_eq!(data["processes"].as_array().unwrap().len(), 2);

    let human = {
        let context =
            Context::isolated(&root).with_var("DBUS_SESSION_BUS_ADDRESS", &desktop.address);
        let args = ["shell", "list"].map(String::from);
        Runner::new(REGISTRY)
            .context(context)
            .run(&args, "shell-test")
    };
    assert!(
        human.stdout.contains("COMPONENT") && human.stdout.contains("dock"),
        "{human:?}"
    );
}

#[test]
fn shell_restart_asks_the_server_to_restart_the_component_not_a_process() {
    let host = std::process::id();
    let desktop = desktop("restart", &["dock"], vec![record("host", host, "")]);
    let root = scratch("restart");
    let output = command(&desktop, &root, &["shell", "restart", "dock"]);
    assert_eq!(output.code, 0, "{output:?}");
    let data = &json(&output)["data"];
    assert_eq!(data["name"], "dock");
    assert_eq!(data["pid"], host);
    assert_eq!(*desktop.restarted.lock().unwrap(), ["dock"]);

    let unknown = command(&desktop, &root, &["shell", "restart", "nothing"]);
    assert_eq!(
        unknown.code, 2,
        "an unknown name is a usage failure: {unknown:?}"
    );
    assert_eq!(json(&unknown)["error"]["code"], "unknown_process");
}

#[test]
fn shell_use_installed_takes_the_hosts_file_of_a_component_away_and_restarts_the_component() {
    let host = std::process::id();
    let root = scratch("use-installed");
    let file = root.join("cupertino-shell");
    std::fs::write(&file, b"build").unwrap();
    let desktop = desktop(
        "use-installed",
        &["dock", "menubar"],
        vec![
            record("host", host, ""),
            record("host-dock", host, file.to_str().unwrap()),
        ],
    );

    // a component in the shared host has no file of its own
    let shared = command(&desktop, &root, &["shell", "use-installed", "menubar"]);
    assert_eq!(shared.code, 5, "{shared:?}");
    assert_eq!(json(&shared)["error"]["code"], "no_dev_program");
    assert!(file.exists());
    assert!(desktop.restarted.lock().unwrap().is_empty());

    let unknown = command(&desktop, &root, &["shell", "use-installed", "nothing"]);
    assert_eq!(unknown.code, 2, "{unknown:?}");
    assert_eq!(json(&unknown)["error"]["code"], "unknown_process");

    let output = command(&desktop, &root, &["shell", "use-installed", "dock"]);
    assert_eq!(output.code, 0, "{output:?}");
    assert_eq!(json(&output)["data"]["removed"], file.to_str().unwrap());
    assert!(!file.exists());
    assert_eq!(*desktop.restarted.lock().unwrap(), ["dock"]);
}

/// The packaged file's shape: the components the session runs, and one the person turned off.
const SYSTEM_CONFIG: &str = r#"{
  "shell": {
    "components": ["menubar", "dock", "launcher", "osd", "agent"],
    "off": ["osd"],
    "alone": []
  }
}
"#;

#[test]
fn dev_folder_enable_names_the_components_that_may_run_from_the_folder() {
    let root = scratch("dev-folder");
    let config = root.join("etc/cupertino/compositor.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, SYSTEM_CONFIG).unwrap();
    let folder = root.join("dev");
    std::fs::create_dir(&folder).unwrap();
    std::fs::set_permissions(&folder, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    // Safety: getuid and getgid have no preconditions.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let root_user = EffectiveUser(0);
    let run = |args: &[&str]| {
        let context = Context::isolated(&root)
            .with_var("SUDO_UID", uid.to_string())
            .with_var("SUDO_GID", gid.to_string())
            .provide(&root_user);
        let args: Vec<String> = ["--format", "json", "dev-folder", "enable", "--folder"]
            .into_iter()
            .chain([folder.to_str().unwrap()])
            .chain(args.iter().copied())
            .map(String::from)
            .collect();
        Runner::new(REGISTRY)
            .context(context)
            .run(&args, "dev-folder-test")
    };

    for (names, code) in [
        ("agent", "ineligible_component"),
        ("launcher,osd", "unknown_component"),
    ] {
        let refused = run(&["--components", names]);
        assert_eq!(refused.code, 5, "{names}: {refused:?}");
        assert_eq!(json(&refused)["error"]["code"], code, "{names}");
    }
    assert_eq!(std::fs::read_to_string(&config).unwrap(), SYSTEM_CONFIG);

    let all = run(&[]);
    assert_eq!(all.code, 0, "{all:?}");
    assert_eq!(
        json(&all)["data"]["components"],
        serde_json::json!(["menubar", "dock", "launcher"]),
        "every component the session runs but the agent and the one turned off"
    );
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(
        written["dev_folder"]["components"],
        serde_json::json!(["menubar", "dock", "launcher"])
    );
}
