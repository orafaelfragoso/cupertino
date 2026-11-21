//! Devices plugged in and removed: a task of the window's loop waits on the kernel's device event socket for udev's
//! announcements and tells [`Changed::Hardware`] when a device of a kind the panes list comes or goes, so a touchpad
//! plugged in adds its row whatever pane shows. The socket carries nothing while no device comes or goes; a device's
//! other events (a battery's charge) are dropped in the task.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;

use crate::watch::Changed;

/// The multicast group udev announces a device on once its rules have run, so its record is written.
const UDEV_GROUP: u32 = 2;
/// The largest announcement read; a longer one is cut, and its first fields name it anyway.
const MESSAGE_BYTES: usize = 8192;
/// The kinds of device the panes list: input devices (a touchpad), power supplies (a battery), network interfaces,
/// sound cards, displays and Bluetooth adapters.
const SUBSYSTEMS: [&str; 6] = ["input", "power_supply", "net", "sound", "drm", "bluetooth"];

/// What starts udev's announcements, before the header that says where its fields start.
const UDEV_PREFIX: &[u8] = b"libudev\0";
/// Where udev's header holds the offset of the fields, a native-endian `u32`.
const FIELDS_OFFSET: std::ops::Range<usize> = 16..20;

/// Whether the device event `message` (`KEY=value` fields apart by NULs, after udev's header or the kernel's first
/// line) is a device of a kind the panes list coming or going.
pub fn plugged(message: &[u8]) -> bool {
    let fields = if message.starts_with(UDEV_PREFIX) {
        message
            .get(FIELDS_OFFSET)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_ne_bytes)
            .and_then(|start| message.get(start as usize..))
            .unwrap_or_default()
    } else {
        message
    };
    let mut action = None;
    let mut subsystem = None;
    for field in fields.split(|b| *b == 0) {
        if let Some(value) = field.strip_prefix(b"ACTION=") {
            action = Some(value);
        } else if let Some(value) = field.strip_prefix(b"SUBSYSTEM=") {
            subsystem = Some(value);
        }
    }
    matches!(action, Some(b"add" | b"remove"))
        && subsystem.is_some_and(|s| SUBSYSTEMS.iter().any(|k| k.as_bytes() == s))
}

/// The socket udev's announcements arrive on.
fn socket() -> io::Result<OwnedFd> {
    // SAFETY: plain socket calls on a descriptor this function owns; the address is a zeroed `sockaddr_nl` with its
    // family and group set, passed with its own size.
    unsafe {
        let fd = libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = OwnedFd::from_raw_fd(fd);
        let mut address: libc::sockaddr_nl = std::mem::zeroed();
        address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        address.nl_groups = UDEV_GROUP;
        let bound = libc::bind(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_nl).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        );
        if bound < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(fd)
    }
}

/// Wait for devices coming and going, as a task of the window's loop; `on` hears [`Changed::Hardware`] for each of a
/// kind the panes list. It ends only when the socket can't be made or read, and says why first.
pub async fn watch(on: Arc<dyn Fn(Changed) + Send + Sync>) {
    let socket = match socket().and_then(async_io::Async::new) {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("cupertino-settings: cannot hear devices coming and going: {e}; a device plugged in shows when the window reads the hardware again");
            return;
        }
    };
    let mut buffer = vec![0u8; MESSAGE_BYTES];
    loop {
        let read = socket
            .read_with(|fd| {
                // SAFETY: reads at most the buffer's length into the buffer, from the descriptor the task owns.
                let read = unsafe {
                    libc::recv(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), 0)
                };
                match usize::try_from(read) {
                    Ok(read) => Ok(read),
                    Err(_) => match io::Error::last_os_error() {
                        e if e.kind() == io::ErrorKind::Interrupted => {
                            Err(io::ErrorKind::WouldBlock.into())
                        }
                        e => Err(e),
                    },
                }
            })
            .await;
        match read {
            Ok(read) => {
                if plugged(&buffer[..read]) {
                    on(Changed::Hardware);
                }
            }
            // a burst the socket could not hold is a device coming or going too
            Err(e) if e.raw_os_error() == Some(libc::ENOBUFS) => on(Changed::Hardware),
            Err(e) => {
                eprintln!("cupertino-settings: device events: {e}; a device plugged in shows when the window reads the hardware again");
                return;
            }
        }
    }
}
