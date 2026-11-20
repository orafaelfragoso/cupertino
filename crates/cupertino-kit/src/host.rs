//! The machine: its host name and its network addresses.

use std::io;
use std::net::Ipv4Addr;

/// The host name (`uname`'s node name, what `gethostname` gives); `None` when it is empty.
pub fn name() -> Option<String> {
    let name = rustix::system::uname();
    let name = name.nodename().to_string_lossy();
    (!name.is_empty()).then(|| name.into_owned())
}

/// The IPv4 addresses of the machine's interfaces (`getifaddrs`), in the order the kernel lists them, loopback ones
/// included.
pub fn ipv4_addresses() -> io::Result<Vec<Ipv4Addr>> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` fills `list`, which is freed below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut addresses = Vec::new();
    let mut at = list;
    while !at.is_null() {
        // SAFETY: `at` walks the list `getifaddrs` gave, each entry valid until it is freed.
        let entry = unsafe { &*at };
        at = entry.ifa_next;
        if entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: the address is valid; its family says how to read it.
        let family = unsafe { (*entry.ifa_addr).sa_family };
        if i32::from(family) != libc::AF_INET {
            continue;
        }
        // SAFETY: an `AF_INET` address is a `sockaddr_in`.
        let address = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
        addresses.push(Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)));
    }
    // SAFETY: `list` came from `getifaddrs` and is freed once.
    unsafe { libc::freeifaddrs(list) };
    Ok(addresses)
}
