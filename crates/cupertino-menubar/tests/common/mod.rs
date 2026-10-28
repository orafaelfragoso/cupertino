//! A private session bus for the integration tests (`dbus-daemon --session`); a test is skipped when the daemon is not
//! installed. The daemon is a child of the test: killed on drop, and killed with the test's thread if the test is
//! killed first, so a timed-out test leaves no daemon behind. Every wait has a deadline: the daemon's address and each
//! method call.
#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use zbus::{block_on, connection, Connection};

/// How long the daemon's address and each method call may take.
pub const WAIT: Duration = Duration::from_secs(5);

pub struct PrivateBus {
    pub address: String,
    daemon: Child,
}

impl PrivateBus {
    pub fn start() -> Option<PrivateBus> {
        let mut command = Command::new("dbus-daemon");
        command
            .args(["--session", "--nofork", "--print-address=1"])
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

    /// A connection builder for the bus whose method calls time out after [`WAIT`].
    pub fn builder(&self) -> connection::Builder<'static> {
        connection::Builder::address(self.address.as_str())
            .expect("address")
            .method_timeout(WAIT)
    }

    pub async fn connect_async(&self) -> Connection {
        self.builder().build().await.expect("connect")
    }

    pub fn connect(&self) -> Connection {
        block_on(self.connect_async())
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}
