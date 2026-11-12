//! A private session bus for integration tests (`dbus-daemon --session`). The daemon is a child of the test, started
//! with `PR_SET_PDEATHSIG`: it is killed when the bus is dropped, and with the thread that started it when the test is
//! killed first, so a test that times out or is killed leaves no daemon behind. The daemon's address, the handshake of
//! [`PrivateBus::connect`] and [`PrivateBus::connect_async`], and each method call of a connection made here have a
//! deadline of [`WAIT`]. A test is skipped when `dbus-daemon` is not installed ([`PrivateBus::start`] gives `None`).

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How long the daemon's address, a connection's handshake and each method call may take.
pub const WAIT: Duration = Duration::from_secs(5);

/// The full name of this test program's test `name`, for running it again in a process of its own with `--exact`.
/// A crate's integration tests are modules of one program, so a test's name starts with its file's module
/// (`store::concurrent_writer_child`); `name` is the test's own name, or a name already qualified.
pub fn test_path(name: &str) -> String {
    static LIST: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let list = LIST.get_or_init(|| {
        let out = Command::new(std::env::current_exe().expect("this test program"))
            .args(["--list", "--format", "terse"])
            .env_remove("RUST_TEST_THREADS")
            .output()
            .expect("list this program's tests");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| line.strip_suffix(": test"))
            .map(str::to_owned)
            .collect()
    });
    let tail = format!("::{name}");
    let found: Vec<&String> = list
        .iter()
        .filter(|t| *t == name || t.ends_with(&tail))
        .collect();
    match found.as_slice() {
        [one] => (*one).clone(),
        [] => panic!("no test named {name} in this program"),
        many => panic!("{name} names several tests: {many:?}"),
    }
}

/// Report a missing D-Bus executable; required-backend runs must fail instead of skipping.
pub fn missing_tool(tool: &str) {
    assert!(
        std::env::var("CUPERTINO_DBUS_REQUIRED").as_deref() != Ok("1"),
        "{tool} missing and CUPERTINO_DBUS_REQUIRED=1"
    );
    eprintln!("SKIPPED: {tool} missing; set CUPERTINO_DBUS_REQUIRED=1 to fail instead");
}

pub struct PrivateBus {
    pub address: String,
    daemon: Child,
}

impl PrivateBus {
    /// Start a daemon with the session bus's configuration; `None` when `dbus-daemon` cannot be run. Call it on the
    /// test's own thread: the daemon dies with the thread that started it.
    pub fn start() -> Option<PrivateBus> {
        Self::start_with("--session".into())
    }

    /// Start a daemon with the configuration file `config` (a policy or a listening address of the test's own), which
    /// the caller keeps until the bus is dropped.
    pub fn start_with_config(config: &Path) -> Option<PrivateBus> {
        let mut arg = OsString::from("--config-file=");
        arg.push(config);
        Self::start_with(arg)
    }

    fn start_with(config: OsString) -> Option<PrivateBus> {
        let mut command = Command::new("dbus-daemon");
        command
            .arg(config)
            .args(["--nofork", "--print-address=1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Safety: the closure runs in the child between fork and exec and calls only prctl, which is
        // async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut daemon = match command.spawn() {
            Ok(daemon) => daemon,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing_tool("dbus-daemon");
                return None;
            }
            Err(error) => panic!("starting private dbus-daemon: {error}"),
        };
        let stdout = daemon.stdout.take().expect("piped stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let address = rx
            .recv_timeout(WAIT)
            .expect("dbus-daemon printed no address within 5 s");
        let address = address.trim().to_owned();
        assert!(!address.is_empty(), "dbus-daemon exited before it listened");
        Some(PrivateBus { address, daemon })
    }

    /// Whether the daemon has exited.
    pub fn has_exited(&mut self) -> bool {
        !matches!(self.daemon.try_wait(), Ok(None))
    }

    /// The daemon's process id, to stop and continue it (`kill -STOP`).
    pub fn pid(&self) -> u32 {
        self.daemon.id()
    }

    /// A connection builder for the bus whose method calls time out after [`WAIT`]; its handshake has no deadline.
    pub fn builder(&self) -> zbus::connection::Builder<'static> {
        zbus::connection::Builder::address(self.address.as_str())
            .expect("the daemon's address")
            .method_timeout(WAIT)
    }

    /// Like [`PrivateBus::builder`], for a blocking connection.
    pub fn blocking_builder(&self) -> zbus::blocking::connection::Builder<'static> {
        zbus::blocking::connection::Builder::address(self.address.as_str())
            .expect("the daemon's address")
            .method_timeout(WAIT)
    }

    /// A blocking connection; panics when the handshake takes longer than [`WAIT`].
    pub fn connect(&self) -> zbus::blocking::Connection {
        zbus::block_on(self.connect_async()).into()
    }

    /// A connection; panics when the handshake takes longer than [`WAIT`].
    pub async fn connect_async(&self) -> zbus::Connection {
        let build = async { Some(self.builder().build().await) };
        let deadline = async {
            async_io::Timer::after(WAIT).await;
            None
        };
        futures_lite::future::or(build, deadline)
            .await
            .expect("the bus handshake did not finish within 5 s: does the configuration allow receive_sender?")
            .expect("connect")
    }
}

/// Writes into `dir` the configuration of a bus that lets everyone own every name and send everything, with the daemon's
/// built-in limits: those of a system bus, which queues at most 64 descriptors and 127 MiB for one connection and awaits
/// at most 128 replies for one connection (a session bus raises them all). Start it with
/// [`PrivateBus::start_with_config`]; keep `dir` until the bus is dropped.
pub fn system_limits_config(dir: &Path) -> std::path::PathBuf {
    let config = dir.join("system-limits.conf");
    std::fs::write(
        &config,
        format!(
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>system</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>"#,
            dir.display()
        ),
    )
    .expect("writing the bus configuration");
    config
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn config(dir: &Path, policy: &str) -> std::path::PathBuf {
        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
{policy}
  </policy>
</busconfig>"#,
                dir.display()
            ),
        )
        .unwrap();
        config
    }

    #[test]
    fn a_bus_takes_the_configuration_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let policy = r#"    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>"#;
        let Some(bus) = PrivateBus::start_with_config(&config(dir.path(), policy)) else {
            eprintln!("skipping: dbus-daemon is not available");
            return;
        };
        assert!(
            bus.address.contains(&*dir.path().to_string_lossy()),
            "{}",
            bus.address
        );
        assert!(bus.connect().unique_name().is_some());
    }

    #[test]
    fn a_handshake_without_a_reply_fails_within_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let policy = r#"    <allow send_destination="*"/>
    <deny receive_sender="*"/>"#;
        let Some(bus) = PrivateBus::start_with_config(&config(dir.path(), policy)) else {
            eprintln!("skipping: dbus-daemon is not available");
            return;
        };
        let started = Instant::now();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bus.connect()))
            .expect_err("a bus that withholds the reply to Hello cannot connect");
        assert!(started.elapsed() < WAIT + Duration::from_secs(2));
        let message = failure.downcast_ref::<String>().unwrap();
        assert!(message.contains("handshake did not finish"), "{message}");
    }

    #[test]
    fn a_bus_with_the_system_limits_takes_names_and_calls() {
        let dir = tempfile::tempdir().unwrap();
        let Some(bus) = PrivateBus::start_with_config(&system_limits_config(dir.path())) else {
            return;
        };
        let service = bus.connect();
        service.request_name("org.example.Limits").unwrap();
        let client = bus.connect();
        let owner = zbus::blocking::fdo::DBusProxy::new(&client)
            .unwrap()
            .get_name_owner("org.example.Limits".try_into().unwrap())
            .unwrap();
        assert_eq!(owner.as_str(), service.unique_name().unwrap().as_str());
    }

    #[test]
    fn the_daemon_ends_with_the_bus() {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("skipping: dbus-daemon is not available");
            return;
        };
        let conn = bus.connect();
        assert!(conn.unique_name().is_some());
        let pid = bus.pid();
        drop(conn);
        drop(bus);
        assert!(
            std::fs::metadata(format!("/proc/{pid}")).is_err(),
            "the daemon was reaped"
        );
    }
}
