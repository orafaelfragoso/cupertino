//! The daemon's way to the controllers, behind a trait so that tests can stand in for the kernel.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use cupertino_bluetooth::{
    Address, Command, ControlSocket, ControllerIndex, DeviceId, Drained, Error, Link, Result,
    OPEN_CALL,
};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::io::Errno;

/// What the daemon needs from the kernel. The methods take `&self` and may be called from several
/// threads: the loop thread waits in `receive` while bus calls `send`.
pub trait Transport: Send + Sync {
    /// Sends one command to a controller (or to none, with [`ControllerIndex::NONE`]).
    fn send(&self, index: ControllerIndex, command: &Command) -> Result<()>;

    /// Waits up to `timeout` (forever for `None`) for packets and returns what arrived, which may
    /// be nothing.
    fn receive(&self, timeout: Option<Duration>) -> Result<Drained>;

    /// Starts a baseband connection from `adapter` to `device` and holds it open until
    /// [`Transport::close_link`].
    fn open_link(&self, adapter: Address, device: DeviceId) -> Result<()>;

    /// Drops the held connection, if any.
    fn close_link(&self, adapter: Address, device: DeviceId);
}

/// The kernel's management socket.
pub struct KernelTransport {
    socket: ControlSocket,
    links: Mutex<HashMap<(Address, DeviceId), Link>>,
}

impl KernelTransport {
    pub fn open() -> Result<Self> {
        Ok(Self {
            socket: ControlSocket::open()?,
            links: Mutex::new(HashMap::new()),
        })
    }
}

/// The stand-in for a kernel without Bluetooth: no controller ever appears, so the daemon keeps its bus name and
/// reports no adapter.
pub struct NoBluetooth;

impl NoBluetooth {
    /// Whether `error` from [`KernelTransport::open`] says the kernel has no Bluetooth (the creation of the socket failed
    /// with no `AF_BLUETOOTH`, or no HCI protocol in it), which is not a failure to start.
    pub fn is_missing(error: &Error) -> bool {
        matches!(
            error,
            Error::Syscall {
                call: OPEN_CALL,
                source: Errno::AFNOSUPPORT | Errno::PROTONOSUPPORT,
            }
        )
    }
}

impl Transport for NoBluetooth {
    /// There is no kernel to answer: the one command the daemon sends without an adapter, reading the index list, goes
    /// nowhere, and the list stays empty.
    fn send(&self, _index: ControllerIndex, _command: &Command) -> Result<()> {
        Ok(())
    }

    fn receive(&self, timeout: Option<Duration>) -> Result<Drained> {
        std::thread::sleep(timeout.unwrap_or(Duration::from_secs(3600)));
        Ok(Drained::default())
    }

    fn open_link(&self, _adapter: Address, _device: DeviceId) -> Result<()> {
        Err(Error::Syscall {
            call: "socket(BTPROTO_L2CAP)",
            source: Errno::AFNOSUPPORT,
        })
    }

    fn close_link(&self, _adapter: Address, _device: DeviceId) {}
}

impl Transport for KernelTransport {
    fn send(&self, index: ControllerIndex, command: &Command) -> Result<()> {
        self.socket.send(index, command)
    }

    fn receive(&self, timeout: Option<Duration>) -> Result<Drained> {
        let timeout = timeout.map(|timeout| Timespec {
            tv_sec: timeout.as_secs() as i64,
            tv_nsec: i64::from(timeout.subsec_nanos()),
        });
        loop {
            let mut fds = [PollFd::new(&self.socket, PollFlags::IN)];
            match poll(&mut fds, timeout.as_ref()) {
                Ok(_) => break,
                Err(Errno::INTR) => continue,
                Err(source) => {
                    return Err(Error::Syscall {
                        call: "poll",
                        source,
                    })
                }
            }
        }
        self.socket.drain()
    }

    fn open_link(&self, adapter: Address, device: DeviceId) -> Result<()> {
        let link = Link::open(adapter, device)?;
        self.links
            .lock()
            .expect("no thread panics while holding the links")
            .insert((adapter, device), link);
        Ok(())
    }

    fn close_link(&self, adapter: Address, device: DeviceId) {
        self.links
            .lock()
            .expect("no thread panics while holding the links")
            .remove(&(adapter, device));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_kernel_without_bluetooth_counts_as_missing() {
        let error = |source| Error::Syscall {
            call: OPEN_CALL,
            source,
        };
        assert!(NoBluetooth::is_missing(&error(Errno::AFNOSUPPORT)));
        assert!(NoBluetooth::is_missing(&error(Errno::PROTONOSUPPORT)));
        assert!(!NoBluetooth::is_missing(&error(Errno::ACCESS)));
        assert!(!NoBluetooth::is_missing(&error(Errno::PERM)));
        // the same errno from another call is that call's failure
        assert!(!NoBluetooth::is_missing(&Error::Syscall {
            call: "bind",
            source: Errno::AFNOSUPPORT,
        }));
    }
}
