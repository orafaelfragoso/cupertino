# cupertino-netlink

The kernel's network interfaces without a C library: rtnetlink (links, addresses, routes), generic netlink `nl80211`
(wireless interfaces, the current connection, scan results, events), `/dev/rfkill`, and an `AF_PACKET` socket with
IPv4/UDP framing for a DHCP client. Pure Rust over `rustix`; no `-sys` crate.

## Public API

| Item | Contents |
|---|---|
| `message` | netlink framing: `Header`, `Message`, `messages(bytes)`, `frame`, the `Body` builder |
| `attr` | attribute TLVs: `attrs(bytes)`, `Attr` accessors (`u8`..`u32`, `mac`, `ip`, `string`, `nested`) |
| `socket` | `Socket::open(protocol, groups, mode)`, `call` (request, ack or dump), `drain` (events), `AsFd` |
| `rtnetlink` | `Link`, `Address`, `Route`; `RouteSocket` (dump, `set_link_up`, add or remove an address or route); `RouteMonitor` for link, address and route events |
| `nl80211` | `Nl80211` (`interfaces`, `wiphys`, `stations`, `connection`, `scan_results`, `trigger_scan`); `Nl80211Monitor` (scan done, connect, disconnect); `is_unsupported` |
| `ie` | `ssid_and_security(elements, capability)`: SSID and `Security` (`Open`, `Owe`, `Wep`, `Wpa`, `Wpa2`, `Wpa3`, `Wpa2Wpa3`, `Enterprise`, `Unsupported`) from the RSN and WPA elements |
| `rfkill` | `Rfkill::open`, `read_events`, `set_soft_block`; `WlanSwitches`, the Wi-Fi state folded from events |
| `packet` | `PacketSocket` (broadcast send, receive), `build_udp_packet`, `parse_udp_packet` |
| `mac`, `error` | `MacAddr`; `Error` (system call, kernel errno, device, parse) and `ParseError` |

## How it works

### Why own codecs

Only a few dozen messages and attributes matter here. The `nl80211` side (scan results, the information elements that
tell WPA2 from WPA3) has no decoder to reuse, and the existing netlink crates would bring a second stack for rtnetlink
alone. The codecs are about 3,000 lines with their tests, over blocking, pollable (`AsFd`) `rustix` sockets, and every
parser is a pure function of bytes. Not audited: the current versions of `netlink-packet-*`, `neli` and `wl-nl80211`.

### Behaviour

- **Dumps use `AF_UNSPEC`.** Only IPv4 and IPv6 addresses and routes are kept; the others are skipped, not errors.
- **A reply is checked.** The sequence number must match, `NLMSG_ERROR` carries the errno (`Error::Kernel`), a dump ends
  at `NLMSG_DONE`, and datagrams from a sender other than the kernel (`pid != 0`) are dropped.
- **Multicast groups are joined through the bind mask.** `rustix` has no `NETLINK_ADD_MEMBERSHIP`, so a group id above
  32 is `Error::GroupOutOfRange`. The `nl80211` group ids are looked up by name at open (scan 23, mlme 25 on the recording
  machine).
- **Events can be lost.** `drain` reports `overflowed` (`ENOBUFS`) and `truncated` datagrams; the caller dumps again.
- **Wireless is found through `nl80211`.** rtnetlink does not say which links are wireless; `Link::class` takes the
  interface indexes `nl80211` lists. A kernel without `nl80211` gives `is_unsupported`.
- **Security comes from the elements, not the capability bits alone.** AKM suite types (IEEE 802.11 AKM suite selectors):
  PSK 2, 4, 6, 19, 20; SAE 8, 9, 24, 25; OWE 18; 802.1X 1, 3, 5, 11 to 17. The WPA element is the vendor element
  `00:50:F2` type 1. A network with both PSK and SAE is `Wpa2Wpa3`.
- **Writes** (`set_link_up`, address and route changes, rfkill) need `CAP_NET_ADMIN` or write access to `/dev/rfkill`;
  all reads are unprivileged. The packet socket needs `CAP_NET_RAW`.
- **One `unsafe` block**: the `sockaddr_ll` of `PacketSocket` implements `rustix`'s `SocketAddrArg`. The struct is built
  whole and a test checks its length.
- **Constants.** The 5 s read timeout of a request, the 64 KiB receive buffer and the 4 KiB frame buffer are approximate.

## Tests

`cargo test -p cupertino-netlink`. `tests/recorded.rs` runs the parsers on real kernel replies in `tests/fixtures/` (its
README says how they were recorded and scrubbed): link names and flags, the LAN address and the default route, the
`nl80211` family, the station, and the scan list (28 access points: open, WPA, WPA2 and the transition mode). Unit tests
build the cases a room does not have, byte by byte: WPA3 (SAE), OWE, WEP, enterprise, an unknown suite, truncated
elements, and the netlink framing errors. `examples/record.rs` records the fixtures. It only dumps (it never triggers a
scan) and refuses to write a file that still holds a real address.

