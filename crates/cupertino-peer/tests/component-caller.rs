//! D20 bus caller resolution: server (owner of `Session1`), accepted shell host component, a program the server started
//! itself (`ProgramOf`), or other caller; the calls in that order against a stand-in of `Session1` on a private bus.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command as Process, Stdio};

use cupertino_peer::component::{
    classify_caller, process_start_ticks, resolve_component, ComponentCaller, SESSION_BUS_NAME,
    SESSION_OBJECT_PATH,
};
use cupertino_peer::ProcessHandle;

#[test]
fn classify_caller_resolves_server_host_components_and_outsiders_in_order() {
    let host_shared = (100u32, 5000u64);
    let host_separate = (200u32, 9000u64);
    let hosts = [host_shared, host_separate];

    let rows = [
        (
            "owner of Session1 is always the server",
            Some(":1.1"),
            ":1.1",
            Some(host_shared),
            Some("menubar"),
            Some("portal"),
            ComponentCaller::Server,
        ),
        (
            "accepted connection of shared host resolves to its component",
            Some(":1.1"),
            ":1.42",
            Some(host_shared),
            Some("agent"),
            Some("portal"),
            ComponentCaller::Component("agent".into()),
        ),
        (
            "accepted connection of separate host resolves identically",
            Some(":1.1"),
            ":1.77",
            Some(host_separate),
            Some("dock"),
            Some("portal"),
            ComponentCaller::Component("dock".into()),
        ),
        (
            "unaccepted connection from a host process is Other, never a program",
            Some(":1.1"),
            ":1.43",
            Some(host_shared),
            Some(""),
            Some("portal"),
            ComponentCaller::Other,
        ),
        (
            "a process the server started itself is the program ProgramOf names",
            Some(":1.1"),
            ":1.50",
            Some((999, 1234)),
            Some("agent"),
            Some("portal"),
            ComponentCaller::Program("portal".into()),
        ),
        (
            "non-host process ProgramOf does not name is Other even if ComponentOf would name something",
            Some(":1.1"),
            ":1.99",
            Some((999, 1234)),
            Some("agent"),
            Some(""),
            ComponentCaller::Other,
        ),
        (
            "reused pid with different start time is no host, and Other when ProgramOf names nothing",
            Some(":1.1"),
            ":1.42",
            Some((100, 9999)),
            Some("agent"),
            Some(""),
            ComponentCaller::Other,
        ),
        (
            "a ProgramOf call that fails is Other",
            Some(":1.1"),
            ":1.50",
            Some((999, 1234)),
            None,
            None,
            ComponentCaller::Other,
        ),
        (
            "anonymous caller with no process is Other",
            Some(":1.1"),
            ":1.42",
            None,
            Some("agent"),
            Some("portal"),
            ComponentCaller::Other,
        ),
    ];

    for (label, owner, sender, proc_id, comp, program, expected) in rows {
        let got = classify_caller(
            owner,
            sender,
            proc_id,
            &hosts,
            || comp.map(str::to_string),
            || program.map(str::to_string),
        );
        assert_eq!(got, expected, "{label}");
    }
}

/// A private session bus daemon, stopped when dropped, and its address.
struct Daemon(Child, String);

impl Daemon {
    fn start() -> Daemon {
        // a socket's path is short: the daemon draws a name of its own in the temporary directory
        let mut child = Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!(
                "--address=unix:tmpdir={}",
                std::env::temp_dir().display()
            ))
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        assert!(!address.trim().is_empty(), "dbus-daemon gives its address");
        let address = address.trim().to_owned();
        Daemon(child, address)
    }

    fn connect(&self) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(self.1.as_str())
            .unwrap()
            .build()
            .unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A stand-in of `org.cupertino.Session1` with no hosts, which names one connection the portal.
struct Session {
    portal: String,
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl Session {
    fn shell_hosts(&self) -> Vec<(u32, u64)> {
        Vec::new()
    }

    fn component_of(&self, _unique_name: String) -> String {
        String::new()
    }

    fn program_of(&self, unique_name: String) -> String {
        if unique_name == self.portal {
            "portal".into()
        } else {
            String::new()
        }
    }
}

#[test]
fn a_caller_of_no_host_is_the_program_program_of_names_over_the_bus() {
    let daemon = Daemon::start();
    let portal = daemon.connect();
    let portal_name = portal.unique_name().unwrap().to_owned();
    let server = zbus::blocking::connection::Builder::address(daemon.1.as_str())
        .unwrap()
        .serve_at(
            SESSION_OBJECT_PATH,
            Session {
                portal: portal_name.to_string(),
            },
        )
        .unwrap()
        .name(SESSION_BUS_NAME)
        .unwrap()
        .build()
        .expect("the stand-in of org.cupertino.Session1 serves");
    let server_name = server.unique_name().unwrap().to_owned();
    let service = daemon.connect();
    let other = daemon.connect();
    let other_name = other.unique_name().unwrap().to_owned();

    let resolve = |name: &zbus::names::UniqueName<'_>| {
        zbus::block_on(resolve_component(service.inner(), name))
    };
    assert_eq!(
        resolve(&portal_name),
        ComponentCaller::Program("portal".into())
    );
    assert_eq!(resolve(&other_name), ComponentCaller::Other);
    assert_eq!(resolve(&server_name), ComponentCaller::Server);
}

#[test]
fn process_start_ticks_reads_current_process_and_rejects_exited_child() {
    let own = ProcessHandle::open(std::process::id()).unwrap();
    assert!(process_start_ticks(&own).is_some());

    let mut child = std::process::Command::new("true").spawn().unwrap();
    let handle = ProcessHandle::open(child.id()).ok();
    let _ = child.wait();
    if let Some(h) = handle {
        assert_eq!(process_start_ticks(&h), None);
    }
}
