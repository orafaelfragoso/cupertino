//! A private bus for the integration tests (`dbus-daemon --session`, the daemons' pattern); a test is skipped when the
//! daemon is not installed. The fake services' tests use one as their "system" bus.
#![allow(dead_code)]

use std::process::{Command, Stdio};

pub struct PrivateBus {
    pub address: String,
    pid: String,
}

impl PrivateBus {
    pub fn start() -> Option<PrivateBus> {
        let out = Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout)
            .ok()
            .filter(|_| out.status.success())?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus {
            address: lines.next()?.to_string(),
            pid: lines.next()?.to_string(),
        })
    }

    pub fn builder(&self) -> zbus::connection::Builder<'static> {
        zbus::connection::Builder::address(self.address.as_str())
            .expect("the daemon's address parses")
    }

    pub fn connect(&self) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(self.address.as_str())
            .expect("the daemon's address parses")
            .build()
            .expect("connects to the private bus")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        Command::new("kill")
            .arg(&self.pid)
            .status()
            .expect("kill the private bus");
    }
}

/// The private bus, or `None` with a note when `dbus-daemon` is missing.
pub fn bus() -> Option<PrivateBus> {
    let bus = PrivateBus::start();
    if bus.is_none() {
        eprintln!("skipped: dbus-daemon is not available");
    }
    bus
}
