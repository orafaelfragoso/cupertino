//! Devices plugged in and removed: a thread waits on the kernel's device event socket for udev's announcements and
//! tells [`Changed::Hardware`] when a device of a kind the panes list comes or goes, so a touchpad plugged in adds
//! its row whatever pane shows. The socket carries nothing while no device comes or goes; a device's other events (a
//! battery's charge) are dropped in the thread.

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

/// Wait for devices coming and going on a thread of its own; `on` hears [`Changed::Hardware`] for each of a kind the
/// panes list.
pub fn start(on: Arc<dyn Fn(Changed) + Send + Sync>) -> io::Result<()> {
    let fd = socket()?;
    std::thread::Builder::new()
        .name("settings-hotplug".into())
        .spawn(move || {
            let mut buffer = vec![0u8; MESSAGE_BYTES];
            loop {
                // SAFETY: reads at most the buffer's length into the buffer, from the descriptor the thread owns.
                let read = unsafe {
                    libc::recv(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), 0)
                };
                if read < 0 {
                    let e = io::Error::last_os_error();
                    // a burst the socket could not hold is a device coming or going too
                    match e.raw_os_error() {
                        Some(libc::EINTR) => continue,
                        Some(libc::ENOBUFS) => {
                            on(Changed::Hardware);
                            continue;
                        }
                        _ => {
                            eprintln!("cupertino-settings: device events: {e}; a device plugged in shows when the window reads the hardware again");
                            return;
                        }
                    }
                }
                if plugged(&buffer[..read as usize]) {
                    on(Changed::Hardware);
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// udev's announcement of `fields`: its header, whose last word runs into the first field.
    fn message(fields: &[&str]) -> Vec<u8> {
        const HEADER: u32 = 40;
        let mut bytes = UDEV_PREFIX.to_vec();
        bytes.extend_from_slice(&0xfeed_cafe_u32.to_be_bytes());
        for word in [HEADER, HEADER, 0, 0x1234, 0, 0, 0xffff_ffff] {
            bytes.extend_from_slice(&word.to_ne_bytes());
        }
        assert_eq!(bytes.len(), HEADER as usize);
        for field in fields {
            bytes.extend_from_slice(field.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn a_device_of_a_listed_kind_coming_or_going_is_told_and_nothing_else() {
        for subsystem in SUBSYSTEMS {
            for action in ["add", "remove"] {
                assert!(
                    plugged(&message(&[
                        &format!("ACTION={action}"),
                        "DEVPATH=/devices/x",
                        &format!("SUBSYSTEM={subsystem}"),
                    ])),
                    "{subsystem} {action}"
                );
            }
        }
        assert!(
            !plugged(&message(&[
                "ACTION=change",
                "SUBSYSTEM=power_supply",
                "POWER_SUPPLY_CAPACITY=80"
            ])),
            "a battery's charge is no device coming"
        );
        assert!(
            !plugged(&message(&["ACTION=add", "SUBSYSTEM=usb"])),
            "a kind no pane lists"
        );
        assert!(
            !plugged(&message(&["ACTION=add", "SUBSYSTEM=inputs"])),
            "the whole name"
        );
        assert!(!plugged(&message(&["SUBSYSTEM=input"])), "no action");
        assert!(!plugged(b""), "nothing");
        assert!(!plugged(b"libudev\0"), "a header cut short");
        assert!(
            plugged(b"add@/devices/x\0ACTION=add\0DEVPATH=/devices/x\0SUBSYSTEM=input\0"),
            "the kernel's own announcement"
        );
    }

    #[test]
    fn the_socket_opens_for_a_process_without_privileges() {
        // binding the announcement group needs no privilege; a sandbox without the socket refuses it plainly
        match socket() {
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::EPROTONOSUPPORT | libc::EPERM | libc::EACCES)
                ) => {}
            Err(e) => panic!("{e}"),
        }
    }
}
