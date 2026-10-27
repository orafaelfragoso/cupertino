# Recorded kernel replies

Binary captures of what the kernel answered a netlink dump on a laptop with one Ethernet port, one USB Wi-Fi adapter
(associated to a WPA2 network with 27 neighbours in range) and Docker's bridges, on 2026-10-23. Each file is the
datagrams of one reply concatenated, headers and `NLMSG_DONE` included, as `Socket::call` returns them.

## Files

| File | Request |
|---|---|
| `rtm-getlink.bin` | `RTM_GETLINK`, dump |
| `rtm-getaddr.bin` | `RTM_GETADDR`, dump |
| `rtm-getroute.bin` | `RTM_GETROUTE`, dump |
| `genl-getfamily-nl80211.bin` | `CTRL_CMD_GETFAMILY` for `nl80211` |
| `nl80211-get-interface.bin` | `NL80211_CMD_GET_INTERFACE`, dump |
| `nl80211-get-wiphy.bin` | `NL80211_CMD_GET_WIPHY` with `NL80211_ATTR_SPLIT_WIPHY_DUMP`, first part of each PHY only |
| `nl80211-get-station.bin` | `NL80211_CMD_GET_STATION`, dump, for the Wi-Fi interface |
| `nl80211-get-scan.bin` | `NL80211_CMD_GET_SCAN`, dump, for the Wi-Fi interface (the kernel's cached list; no scan was triggered) |

## Re-recording

Re-record with `cargo run -p cupertino-netlink --example record-netlink -- crates/cupertino-netlink/tests/fixtures`
from `prototypes/`. All requests are read-only dumps and need no capability. Recording again changes the numbers
the tests assert (a different room has different neighbours), so update `tests/recorded.rs` with it.

## Scrubbing

The recording tool replaces, before writing, with values of the same size: every hardware address by
`02:00:00:00:xx:xx` (also where it hides in IPv6 identifiers, bridge identifiers and `enx...` alternative names),
every SSID by `netN-xxx`, the contents of vendor elements other than the WPA element (they can carry device names),
and IPv6 prefixes and non-hardware interface identifiers. It refuses to write when any replaced value is still found
in a file. Nothing in a dump is a key or a passphrase.
