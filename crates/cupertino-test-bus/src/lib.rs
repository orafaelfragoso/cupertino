//! A private session bus for integration tests (`dbus-daemon --session`). The daemon is a child of the test, started
//! with `PR_SET_PDEATHSIG`: it is killed when the bus is dropped, and with the thread that started it when the test is
//! killed first, so a test that times out or is killed leaves no daemon behind. Every wait has a deadline of [`WAIT`]:
//! the daemon's address, and each method call of a connection made here. A test is skipped when `dbus-daemon` is not
//! installed ([`PrivateBus::start`] gives `None`).

use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// How long the daemon's address and each method call may take.
pub const WAIT: Duration = Duration::from_secs(5);

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
        let mut daemon = command.spawn().ok()?;
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

    /// A connection builder for the bus whose method calls time out after [`WAIT`].
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

    pub fn connect(&self) -> zbus::blocking::Connection {
        self.blocking_builder().build().expect("connect")
    }

    pub async fn connect_async(&self) -> zbus::Connection {
        self.builder().build().await.expect("connect")
    }
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

    #[test]
    fn a_bus_takes_the_configuration_it_is_given() {
        let dir = std::env::temp_dir().join(format!("cupertino-test-bus-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
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
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>"#,
                dir.display()
            ),
        )
        .unwrap();
        let Some(bus) = PrivateBus::start_with_config(&config) else {
            eprintln!("skipping: dbus-daemon is not available");
            return;
        };
        assert!(
            bus.address.contains(&*dir.to_string_lossy()),
            "{}",
            bus.address
        );
        assert!(bus.connect().unique_name().is_some());
        drop(bus);
        let _ = std::fs::remove_dir_all(&dir);
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
