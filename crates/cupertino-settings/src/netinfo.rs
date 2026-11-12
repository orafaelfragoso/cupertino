//! What the Wi-Fi and Network panes show of each network interface: its addresses, router and name servers. The
//! kernel and the resolver's file are read directly, so the same details show whichever service manages the network.

use std::ffi::CStr;
use std::net::Ipv4Addr;
use std::path::Path;

use crate::live::Interface;

/// The most bytes read of one small file.
const MAX_TEXT: u64 = 256 * 1024;

fn read_small(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_TEXT)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The IPv4 address, as `a.b.c.d`, that a little-endian hexadecimal word of `/proc/net/route` stands for.
fn route_address(word: &str) -> Option<Ipv4Addr> {
    u32::from_str_radix(word, 16)
        .ok()
        .map(|n| Ipv4Addr::from(n.to_le_bytes()))
}

/// The default router of each interface in the text of `/proc/net/route`.
pub fn gateways(route: &str) -> Vec<(String, Ipv4Addr)> {
    route
        .lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            // destination 0 and mask 0 mark the default route; flags 0x2 say it has a gateway
            (f.len() >= 8 && f[1] == "00000000" && f[7] == "00000000")
                .then(|| Some((f[0].to_owned(), route_address(f[2])?)))
                .flatten()
        })
        .collect()
}

/// The name servers of the text of a `resolv.conf`.
pub fn name_servers(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver"))
        .map(|rest| rest.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The IPv4 address and prefix length of each interface, by name.
fn addresses() -> Vec<(String, Ipv4Addr, u8)> {
    let mut out = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list` with a chain it owns until freeifaddrs; nothing else is touched.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return out;
    }
    let mut at = list;
    while !at.is_null() {
        // SAFETY: `at` is a node of the chain getifaddrs returned, valid until freeifaddrs below.
        let entry = unsafe { &*at };
        at = entry.ifa_next;
        if entry.ifa_addr.is_null() || entry.ifa_netmask.is_null() {
            continue;
        }
        // SAFETY: the address pointer is not null; its family field is readable for every socket address.
        if i32::from(unsafe { (*entry.ifa_addr).sa_family }) != libc::AF_INET {
            continue;
        }
        // SAFETY: an AF_INET address and mask are sockaddr_in, which the pointers point at.
        let (address, mask) = unsafe {
            (
                (*(entry.ifa_addr as *const libc::sockaddr_in))
                    .sin_addr
                    .s_addr,
                (*(entry.ifa_netmask as *const libc::sockaddr_in))
                    .sin_addr
                    .s_addr,
            )
        };
        // SAFETY: the name is a NUL-terminated string owned by the node.
        let name = unsafe { CStr::from_ptr(entry.ifa_name) }
            .to_string_lossy()
            .into_owned();
        out.push((
            name,
            Ipv4Addr::from(u32::from_be(address)),
            u32::from_be(mask).count_ones() as u8,
        ));
    }
    // SAFETY: `list` came from getifaddrs and is freed once.
    unsafe { libc::freeifaddrs(list) };
    out
}

fn sys(name: &str, file: &str) -> Option<String> {
    read_small(&Path::new("/sys/class/net").join(name).join(file)).map(|t| t.trim().to_owned())
}

/// The interfaces of the machine that carry traffic for the person: Ethernet and Wi-Fi ones, not loopback, bridges or
/// virtual links.
pub fn read() -> Vec<Interface> {
    let Ok(dir) = std::fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let routes = read_small(Path::new("/proc/net/route"))
        .map(|t| gateways(&t))
        .unwrap_or_default();
    let dns = read_small(Path::new("/etc/resolv.conf"))
        .map(|t| name_servers(&t))
        .unwrap_or_default();
    let addresses = addresses();
    let mut out: Vec<Interface> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            // a link of a physical device has a `device` entry; Wi-Fi ones also a `wireless` directory
            let physical = path.join("device").exists();
            let wireless = path.join("wireless").exists() || path.join("phy80211").exists();
            if !physical && !wireless {
                return None;
            }
            let address = addresses.iter().find(|(n, ..)| *n == name);
            Some(Interface {
                kind: if wireless { "wifi" } else { "ethernet" }.to_owned(),
                mac: sys(&name, "address"),
                state: sys(&name, "operstate").unwrap_or_else(|| "unknown".to_owned()),
                ipv4: address.map(|(_, a, p)| format!("{a}/{p}")),
                gateway: routes
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, g)| g.to_string()),
                dns: if address.is_some() {
                    dns.clone()
                } else {
                    Vec::new()
                },
                speed: sys(&name, "speed")
                    .and_then(|s| s.parse::<i64>().ok())
                    .filter(|s| *s > 0)
                    .map(|s| s as u32),
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
    out
}
