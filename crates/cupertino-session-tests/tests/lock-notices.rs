//! Which connection the lock screen asks for its notifications (`cupertino_login::lock_notices`), against the real
//! server, headless, on a session bus of the test's own, with `cupertino-shell`'s host running the real notifications
//! component and the real on-screen displays (D20: checks are per component, never by process relations or program
//! paths). The server's supervisor starts this test binary as `cupertino-shell`: with [`host::MARK`] among its
//! arguments it runs the host with `cupertino_shell::OSD` and `cupertino_shell::NOTIFICATIONS`.
//! - The connection that owns `org.cupertino.NotificationCenter1`, the notifications component's own, is accepted and
//!   answers the list.
//! - The on-screen displays' connection (another component of the same host), the server's and the test's own are
//!   refused; a list asked of a name the test's connection owns is an error and is not asked.

// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

use std::io::{BufRead, BufReader};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};

use cupertino_login::lock_notices::{fetch, serves_lock_screen};
use cupertino_notifications::control::CENTRE_BUS_NAME;
use zbus::blocking::Connection;
use zbus::names::{BusName, OwnedUniqueName};

use rig::Rig;

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the real on-screen displays and
/// the real notifications component.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-lock-notices-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&cupertino_shell::OSD, &cupertino_shell::NOTIFICATIONS];

    /// The report file and the host's own arguments that follow [`MARK`], when this process is the host.
    pub fn arguments() -> Option<(PathBuf, Vec<String>)> {
        let args: Vec<String> = std::env::args().collect();
        let at = args.iter().position(|a| a == MARK)?;
        let report = PathBuf::from(args.get(at + 1)?);
        Some((report, args[at + 2..].to_vec()))
    }

    /// Be the host; these components write no report.
    pub fn run(_report: PathBuf, args: Vec<String>) -> ! {
        cupertino_shell::run(&ENTRIES, args)
    }
}

/// The host itself, when the supervisor started this binary with [`host::MARK`]; a test run does nothing here.
#[test]
fn host_program() {
    if let Some((report, args)) = host::arguments() {
        host::run(report, args);
    }
}

/// A session bus of the test's own, whose address the hosts inherit, and the address.
struct Bus(Child, String);

/// A session bus that starts no service: nothing a component calls reaches a program of the machine.
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

impl Bus {
    fn start(name: &str) -> Bus {
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("lock-notices-{name}-bus.conf"));
        std::fs::write(&config, BUS_CONFIG).expect("the bus's configuration");
        let mut daemon = Process::new("dbus-daemon")
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
        let address = address.trim().to_owned();
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        Bus(daemon, address)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A directory of the test's own, new and empty, of mode 0700: the session's runtime directory, or its settings.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("lock-notices-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .expect("a private directory");
    dir
}

/// `work` on a thread of its own while the server turns, so a call that waits for the server is answered.
fn aside<T: Send + 'static>(
    rig: &mut Rig,
    what: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let work = std::thread::spawn(work);
    rig.until(what, |_| work.is_finished());
    work.join().unwrap()
}

/// The connection that owns `name` now.
fn owner_of(session: &Connection, name: &str) -> Option<OwnedUniqueName> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    dbus.get_name_owner(BusName::try_from(name).ok()?).ok()
}

/// A server whose shell host runs `components`, its `org.cupertino.Session1` on `bus`, and the test's connection.
fn session(bus: &Bus, name: &str, components: &[&str]) -> (Rig, Connection) {
    std::env::set_var("XDG_RUNTIME_DIR", private_dir(&format!("{name}-runtime")));
    // no notifications.conf of the person's: every application with the defaults
    std::env::set_var("XDG_CONFIG_HOME", private_dir(&format!("{name}-config")));
    let mut rig = Rig::new(name, components, &[]);
    cupertino_server::quit::start_session_service_on(&mut rig.state, Some(bus.1.clone()));
    let session = Connection::session().expect("the test's bus");
    (rig, session)
}

#[test]
fn only_the_notifications_component_answers_the_lock_screen() {
    let bus = Bus::start("component");
    let (mut rig, session) = session(&bus, "lock-notices", &["osd", "notifications"]);
    let (mut centre, mut osd) = (None, None);
    rig.until(
        "the notifications and the on-screen displays own their names",
        |_| {
            centre = owner_of(&session, CENTRE_BUS_NAME);
            osd = owner_of(&session, cupertino_osd::control::BUS_NAME);
            centre.is_some() && osd.is_some()
        },
    );
    let (centre, osd) = (centre.unwrap(), osd.unwrap());
    assert_ne!(centre, osd, "each component has a connection of its own");
    let server = owner_of(&session, "org.cupertino.Session1").expect("the server owns Session1");
    let own = session.unique_name().expect("the test's name").to_owned();

    let ask = session.clone();
    let answers = aside(&mut rig, "the callers are resolved", move || {
        [&centre, &osd, &server, &own].map(|name| serves_lock_screen(&ask, name))
    });
    assert_eq!(
        answers,
        [true, false, false, false],
        "accepted: the notifications component, not the on-screen displays, the server or the test"
    );

    let ask = session.clone();
    let list = aside(&mut rig, "the list is answered", move || fetch(&ask));
    assert_eq!(
        list.expect("the notifications component is asked"),
        [],
        "nothing shows while the session is unlocked"
    );
}

#[test]
fn a_name_another_connection_owns_is_not_asked() {
    let bus = Bus::start("other");
    let (mut rig, session) = session(&bus, "lock-notices-other", &["osd"]);
    rig.until("the on-screen displays own their name", |_| {
        owner_of(&session, cupertino_osd::control::BUS_NAME).is_some()
    });
    session
        .request_name(CENTRE_BUS_NAME)
        .expect("the test takes the notifications' name");

    let ask = session.clone();
    let list = aside(&mut rig, "the list is refused", move || fetch(&ask));
    let error = list.expect_err("a name the test owns is not the notifications component's");
    assert!(
        error
            .to_string()
            .contains("not the notifications component"),
        "{error}"
    );
}
