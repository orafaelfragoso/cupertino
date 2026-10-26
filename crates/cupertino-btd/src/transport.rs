//! The daemon's way to the controllers, behind a trait so that tests can stand in for the kernel.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use cupertino_bluetooth::{
    Address, Command, ControlSocket, ControllerIndex, DeviceId, Drained, Error, Link, Result,
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
