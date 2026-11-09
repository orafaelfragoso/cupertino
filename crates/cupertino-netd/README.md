# cupertino-netd

The network daemon, replacing NetworkManager for what apps and the shell use. It follows the kernel's links, runs a DHCPv4
client on each Ethernet and Wi-Fi device, writes a resolver file, and leaves Wi-Fi authentication to iwd (decision D2: no
WPA here). It serves `org.cupertino.Network1` and the small part of `org.freedesktop.NetworkManager` that apps poll. Pure
Rust over `zbus` (blocking API) and `cupertino-netlink`.

## Service

| | |
|---|---|
| Bus names | `org.cupertino.Network1` at `/org/cupertino/Network1`; `org.freedesktop.NetworkManager` at `/org/freedesktop/NetworkManager` (system bus by default) |
| Runs as | root, or with `CAP_NET_ADMIN` and `CAP_NET_RAW` |
| Unit file | [`cupertino-netd.service`]: runs `/opt/cupertino/bin/cupertino-netd --resolv-conf /run/cupertino/resolv.conf` after `dbus.socket` and `iwd.service` (which it wants), with runtime directory `cupertino` |
| D-Bus policy | [`org.cupertino.conf`]: root owns both names, everyone may call them |

## Public API

| Item | Contents |
|---|---|
| `model` | pure state: `Model::apply(Input) -> Vec<Effect>`; devices (`Unavailable`, `Disconnected`, `Configuring`, `Connected`), failures, the deduplicated scan list (`dedupe`, `Model::networks`) |
| `dhcp::message` | the RFC 2131 and 2132 codec: `Message::encode`, `Message::decode`, `Options` |
| `dhcp::client` | the state machine INIT, SELECTING, REQUESTING, BOUND, RENEWING, REBINDING with injected time: `Client::start`, `tick`, `receive`, `next_deadline`, `release` |
| `dhcp::transport` | `Transport` and `PacketTransport` (an `AF_PACKET` socket from `cupertino-netlink`) |
| `dns` | `render` and `write` of `resolv.conf` (default `/run/cupertino/resolv.conf`), replaced atomically |
| `supplicant` | the `Supplicant` trait, events and failures; `iwd` (feature `dbus`) is the implementation over `net.connman.iwd` |
| `engine` | `Engine`: joins the model to the machine (`Host`) and the supplicant; one `on_*` method per event source, `command` for the bus |
| `host` | `SystemHost`: rtnetlink, nl80211, rfkill and packet sockets |
| `snapshot`, `nm` | what the bus shows, and the NetworkManager values worked out from it |
| `auth` (feature `dbus`) | `Authorizer`, `LogindAuthorizer` (the real check), `AllowAll` |
| `daemon`, `interfaces` (feature `dbus`) | `Daemon::connect`, `claim_names`, `publish`; the D-Bus objects and their errors |

Features: `dbus` (default; zbus, `async-channel` and `cupertino-seat`), `app` (the `cupertino-netd` binary, implies `dbus`).

## Running the daemon

```sh
cargo run -p cupertino-netd --features app -- [--session-bus [--no-auth]] [--resolv-conf PATH]
```

- **System bus by default**, as root (or with `CAP_NET_ADMIN` and `CAP_NET_RAW`): it changes addresses and routes, opens
  packet sockets and writes `/dev/rfkill` (the system daemon refuses to start without access to that device; with `--session-bus` it serves with the radio switch unavailable). It needs an `iwd` service. `/etc/resolv.conf` is not touched: link it to the
  file named by `--resolv-conf` yourself. A link whose DHCP client cannot start (no `CAP_NET_RAW`: `socket(AF_PACKET)` is refused) is
  reported on stderr and gets no address from the daemon; the other links and the bus names are not affected.
- **`--session-bus`** serves on the bus named by `DBUS_SESSION_BUS_ADDRESS`. Unprivileged, it reads the real kernel state
  and reports the failures of the writes it cannot make. **`--no-auth`** skips the session check and needs `--session-bus`.
- **Events.** One `poll` over the rtnetlink and `nl80211` monitors, `/dev/rfkill`, the supplicant, the bus and one packet
  socket per DHCP client, with the earliest DHCP timer as the timeout.
- **Failures.** A startup failure, a name held by another daemon (NetworkManager), or a failing monitor ends the process.
  A failed effect (an address that cannot be added, iwd missing) is printed, moves the device to the matching state
  (`no-address`, `connection-failed`), and the next event retries.

## D-Bus

| Where | What |
|---|---|
| `org.cupertino.Network1` at `/org/cupertino/Network1` | properties `WirelessEnabled` (read and write), `Devices` (`a(ussssss)`: index, name, `ethernet` or `wifi`, state, SSID, failure, address), `State` (`disconnected`, `connecting`, `connected-local`, `connected`); methods `Scan`, `GetNetworks` (`a(sisbb)`: SSID, signal in dBm or `i32::MIN` when unknown, security, saved, connected), `Connect(ssid, passphrase)`, `Disconnect`; signals `NetworksChanged`, `ConnectionFailed(device, reason)`; errors `NotAuthorized`, `NoWifiDevice`, `WifiUnavailable`, `UnknownNetwork`, `PassphraseRequired`, `UnsupportedSecurity`, `Failed` |
| `org.freedesktop.NetworkManager` at `/org/freedesktop/NetworkManager` | properties `State`, `Connectivity`, `NetworkingEnabled`, `WirelessEnabled` (read and write), `PrimaryConnectionType`; method `CheckConnectivity`; signal `StateChanged`. Nothing else of NetworkManager exists. |

## How it works

- **Reading is open, writing is not.** `WirelessEnabled`, `Scan`, `Connect` and `Disconnect` need the caller's process,
  taken from the bus daemon with `GetConnectionCredentials`, to belong to a logind session that is `Active` and has a
  seat (rule 5). No polkit. The `Authorizer` is a copy of the one in `cupertino-powerd`, so the daemons
  stay independent. The credentials are asked on a second connection to the bus (`cupertino_peer::lookup::Lookup`, a few
  questions at once), never on the one that reads the calls: each answer carries a process descriptor, the bus daemon
  queues at most 64 of them for one connection of the system bus and refuses every message to it beyond that, so answers
  queued behind a flood of calls (or while the thread waits for logind) would have shut every other caller out.
- **One Wi-Fi device.** The methods act on the first Wi-Fi device by index; further ones are listed in `Devices`.
- **The radio switch** is the rfkill software block of every Wi-Fi radio. `WirelessEnabled` changes when the kernel reports
  the switch, not when the write returns. A hardware block makes the device `unavailable` and leaves the setting on.
- **Scan list.** The `nl80211` list read after each scan ends: one entry per SSID and security with the strongest access
  point, strongest first, ties by name, hidden networks left out. A network is `saved` when iwd has a profile of the same
  SSID and kind (open, passphrase, 802.1X).
- **Connecting.** `Connect` returns when the attempt has begun. The device goes `configuring`, then `connected` once iwd
  has associated and DHCP has answered. `ConnectionFailed` and the device's `failure` report a failed attempt. A
  passphrase is needed only for a network iwd has no profile for. It goes to iwd once, through the agent this process
  registers, for that network only, and is not kept (the pending copy is zeroed when it is dropped). The agent answers `RequestPassphrase` only to the connection that owns
  `net.connman.iwd` when the call arrives, as the state watcher follows it (`cupertino_peer::signal::from_owner`; the call asks
  the bus nothing, so a flood of calls from other users cannot stop the agent): the bus policy lets every user reach this process, so any other caller that named
  the network while the connection waits would be handed the passphrase. iwd's `Station.State` changes count only from that
  owner too, so another user cannot drop the lease with a forged `disconnected` (`iwd` tests
  `only_iwd_is_handed_the_passphrase`, `only_iwd_says_a_station_disconnected`). WEP and 802.1X networks without a profile are refused. iwd answers every
  failed handshake with the same error, so a failure after a supplied passphrase is `wrong-passphrase` (approximate).
- **DHCP.** Timers follow RFC 2131: retransmission at 4, 8, 16, 32 and 64 s, T1 at half and T2 at 0.875 of the lease
  unless the server sets them, a 10 s wait after a NAK. The RFC's random 1 s jitter is left out (approximate). The lease
  counts from the first copy of the REQUEST. INIT-REBOOT is not done. No answer for the whole schedule is `no-address`,
  and the client keeps asking.
- **Addresses and routes.** A lease adds its address and a default route through the first router (protocol DHCP, metric
  100 for Ethernet and 600 for Wi-Fi, NetworkManager's defaults, approximate as ours). A lost or changed lease removes them.
- **DNS.** The servers of the lowest-metric connected device's lease, at most three, with the domain as the search list.
  The file is written only on change and kept while no device is connected. RDNSS is not read.
- **IPv6** relies on the kernel's SLAAC; there is no DHCPv6.
- **NetworkManager values.** `State` is 70 (connected, global) with a default route, 50 with an address only, 40 while a
  device configures, 20 otherwise. There is no connectivity probe: `Connectivity` is 0 (unknown) when anything is
  connected, as NetworkManager reports with checking off, and 1 (none) otherwise. `CheckConnectivity` returns it.
  `PrimaryConnectionType` is `802-3-ethernet` or `802-11-wireless` for the device with the default route.
- **The supplicant could not be tested live.** `iwd` is not installed on the development machine. The calls follow the
  iwd 3.12 sources (`src/station.c`, `src/network.c`, `src/agent.c`). Against a bus without iwd, every Wi-Fi call fails
  with "the supplicant (net.connman.iwd) is not on the bus".

## Diagnostic events

The daemon records versioned diagnostic events to the system journal (`CUPERTINO_DIAG_SCHEMA=1`,
`SYSLOG_IDENTIFIER=cupertino-netd`), each beside the line or error it already gives; `main` installs the recorder
before anything else. An SSID is recorded only as its salted hash (`ssid#` and eight hex digits), never as text.

| Event | Severity | Fields | Where |
|---|---|---|---|
| `network.backend.start_failed` | error | `step` (`logind`, `rtnetlink`, `nl80211`, `host`, `supplicant`, `rfkill`, `bus`, `bus_name`), `kind` (the error's class, such as `name_taken`), `error` | a step of the start fails |
| `network.request.failed` | warn | `interface`, `method` (`Scan`, `Connect`, `Disconnect`, `WirelessEnabled`), `error` (the D-Bus error name), `ssid` (hashed, for `Connect`) | a call or property write is answered with an error |
| `network.exit_error` | error | `error` | the daemon ends with an error, or the arguments are wrong |
| `network.panic` | fatal | `panic`, `location` | any panic |

## Open issues

`zbus` stays on 5.x like the other services; 6.x exists.

## Tests

```
cargo test -p cupertino-netd --features app
```

Unit tests cover the DHCP codec (a DISCOVER byte for byte, an OFFER built byte by byte) and state machine with injected time (full lease,
foreign transaction, backoff and giving up, NAK, T1 renewal, T2 rebinding, expiry, server-set T1 and T2, infinite lease,
release), the `model` (deduplication and order, the radio switch, success, a wrong passphrase, losing the access point,
switching, roaming), and the `engine` against a fake machine and supplicant. `tests/dbus.rs` runs on a private
`dbus-daemon` (skipped when missing): the NetworkManager values, `StateChanged`, `PropertiesChanged`, `WirelessEnabled`
written by an authorised caller (the request reaches the engine) and refused for one outside the active session (nothing reaches the engine), `GetNetworks` after a fake scan, `Connect`
arguments, errors named on the bus, `ConnectionFailed` once, a second daemon refused, and, on a bus with the system bus's
limits, 3000 calls sent at once while the authorizer is held (a slow logind) not making the bus daemon refuse another
caller: it gets the daemon's own answer.

Not covered: a real DHCP exchange (no `dnsmasq` on the development machine), the real `LogindAuthorizer` (covered in
`cupertino-powerd`), iwd, and the packet socket on a wire.
