# cupertino-btd

The Bluetooth daemon, replacing the BlueZ daemon for what the shell needs: adapter power, discovery, nearby and paired
devices, pairing with a dialog the shell draws, connecting, and forgetting. It talks to the kernel through
`cupertino-bluetooth` (the management interface) and serves `org.cupertino.Bluetooth1`, not `org.bluez`. Pure Rust over `zbus` (blocking API) and
`rustix`. Layer: `services`.

## Service

| | |
|---|---|
| Bus name | `org.cupertino.Bluetooth1` at `/org/cupertino/Bluetooth1`, system bus by default (agent interface: `org.cupertino.Bluetooth1.Agent`) |
| Runs as | root, or with `CAP_NET_ADMIN` |
| Unit file | [`cupertino-btd.service`]: runs `/opt/cupertino/bin/cupertino-btd` after `dbus.socket`, with state directory `cupertino/bluetooth` |
| D-Bus policy | [`org.cupertino.conf`]: root owns the name, everyone may call it |

## Public API

| Item | Contents |
|---|---|
| `device` | `Device` (merge of discovery reports), `DeviceInfo`, `DeviceKind` (icon from appearance, then class), `DeviceRef` (`ADDR` or `ADDR/type`) |
| `pairing` | `Pairing` (state machine with injected time), `Prompt`, `AgentReply`, `PairError` |
| `model` | `Model`: adapters and devices as a pure model that takes packets, requests and time and returns `Effect`s (send, open a link, load or save keys, answer a caller, announce a change, ask the agent) |
| `request` | `Request`, `RequestError`, `Effect`, `Change`, `Ticket` |
| `keys` | `KeySet`, `KeyStore`: link keys and long term keys, one file per adapter |
| `transport` | `Transport` (what the daemon needs from the kernel), `KernelTransport` (the real management socket), `NoBluetooth` (the stand-in for a kernel without Bluetooth) |
| `auth`, `daemon`, `interfaces` (feature `dbus`) | `Authorizer`, `LogindAuthorizer`, `AllowAll`; `Daemon::start(Config)`, `run`; the D-Bus object, `DeviceView`, `BluetoothError` |

Features: `dbus` (default; zbus, `cupertino-seat`), `app` (the `cupertino-btd` binary, implies `dbus`).

## Running the daemon

```sh
cargo run -p cupertino-btd --features app -- [--session-bus [--no-auth]] [--keys-dir PATH] [--read-only]
```

- **System bus by default**, as root (or with `CAP_NET_ADMIN`), because the kernel refuses every changing command without it.
- **A kernel without Bluetooth** (`socket(BTPROTO_HCI)` fails with `EAFNOSUPPORT` or `EPROTONOSUPPORT`, in either bus mode) is a normal machine, not an error: the daemon warns on stderr, keeps `org.cupertino.Bluetooth1` and reports no adapter (`Available` false). Any other failure to open the socket still stops it.
- **`--keys-dir`** is where the keys live; default `/var/lib/cupertino/bluetooth/`.
- **`--read-only`** sends only the read commands and refuses every request with `ReadOnly`. It works unprivileged and has
  no device list.
- **`--session-bus`** serves on the session bus. **`--no-auth`** skips the session check and needs `--session-bus`.
## D-Bus

Every method taking `address` accepts `AA:BB:CC:DD:EE:FF`, or
with a suffix (`/bredr`, `/le-public`, `/le-random`) when the same address is known as two devices.

| Member | Meaning |
|---|---|
| property `Available` (b) | an adapter is ready (keys loaded, controller set up). Without one, writes fail with `NoAdapter` |
| property `Powered` (b, read/write) | the default adapter is on; writing it powers the adapter on or off. A refused write is `org.freedesktop.DBus.Error.AuthFailed` (a property setter cannot return a custom name) |
| property `Discovering` (b) | the user's discovery is on |
| property `Devices` (aa{sv}) | paired, connected and recently discovered devices |
| `StartDiscovery`, `StopDiscovery | discovery stops by itself after 60 s |
| `Pair(s) | pairs and bonds; returns when the pairing is over; asks the agent |
| `Connect(s)`, `Disconnect(s) | brings the baseband link up or down (see *Open issues*) |
| `Remove(s) | unpairs and deletes the keys, or forgets a discovered device |
| `RegisterAgent(o)`, `UnregisterAgent | the shell's pairing agent (below) |
| signals `DeviceAdded(a{sv})`, `DeviceChanged(a{sv})`, `DeviceRemoved(s) | `DeviceRemoved` carries the device's `Id`. Each also changes `Devices` |

A device is a dictionary. Always present: `Id`, `Address`, `AddressType`, `Icon`, `Paired`,
`Bonded`, `Connected`, `Pairing`, `Uuids`. Left out when unknown: `Name`, `Class` (u), `Appearance` (q), `Rssi` (n, dBm).
`Icon` is one of `phone`, `computer`, `tablet`, `watch`, `display`, `headphones`, `headset`, `speaker`, `audio`,
`keyboard`, `mouse`, `trackpad`, `gamepad`, `remote-control`, `camera`, `printer`, `unknown`. The class and appearance
tables are as designed (Bluetooth SIG assigned numbers).

**Errors** are `org.cupertino.Bluetooth1.<name>`: `NotAuthorized`, `InvalidArguments`, `NoAdapter`, `NotPowered`,
`UnknownDevice`, `Ambiguous`, `AlreadyPaired`, `NotPaired`, `AlreadyConnected`, `NotConnected`, `Busy`, `ConnectFailed`,
`Rejected`, `AuthenticationFailed`, `Timeout`, `Cancelled`, `Agent`, `AgentExists`, `PinUnsupported`, `Blocked`,
`NotSupported`, `PermissionDenied`, `ReadOnly`, `Failed`.

**The agent.** A client registers an object with `RegisterAgent`; the daemon then calls it on the connection that
registered it, at `org.cupertino.Bluetooth1.Agent`:

| Method | Meaning |
|---|---|
| `ConfirmPasskey(t id, a{sv} device, u passkey, b just_works) -> b` | confirm. With `just_works` there is no number to compare. Fail with `org.cupertino.Bluetooth1.Agent.Rejected` to refuse |
| `RequestPasskey(t id, a{sv} device) -> u` | the passkey the remote device shows. `...Agent.Rejected` refuses |
| `DisplayPasskey(t id, a{sv} device, u passkey, y entered) | show a passkey to type on the remote device; `entered` digits so far |
| `Cancel(t id) | pairing `id` is over; close the dialog |

Another client's live agent is not replaced (`AgentExists`); the agent of a client that left is. With no agent, a pairing
that needs an answer fails with `Agent`. The daemon pairs with IO capability KeyboardDisplay (the kernel turns it into
DisplayYesNo on BR/EDR).

## How it works

- **Reading is open, writing is not.** Every method and the `Powered` setter need the caller's process (from
  `GetConnectionCredentials`) to belong to a logind session that is `Active` and has a seat (`cupertino-seat`). No
  polkit. Registering an agent is a write, because the agent receives the numbers of a pairing.
- **One writer at a time.** The model sits behind one lock that is never held across an `await`. One task sends property
  changes, signals and agent calls in order. An agent call runs in its own task, so a slow user delays no signal.
- **Pairing.** A refusal, a timeout, a remote failure and a controller error end in different errors. A pairing the other
  side started is tracked too, and its prompts go to the agent. Legacy PIN pairing is refused (`PinUnsupported`). The 30 s SMP limit is as designed (Bluetooth Core, Vol 3 Part H, "SMP timeout"; cited
  from memory, not re-checked against the PDF); the daemon waits 60 s for the whole pairing.
- **Keys.** After a pairing the keys are written to `<keys-dir>/AA-BB-CC-DD-EE-FF.keys` (the adapter's address) and loaded
  into the kernel at startup with Load Link Keys and Load Long Term Keys. A corrupt file, or one readable by group or
  others, is refused and left as it is, and the adapter stays unavailable, because writing a later pairing's keys over
  it would destroy the old ones. A failed write is printed, and the pairing stands until the next restart.
- **Connect** opens an L2CAP socket to the device and keeps it, because the kernel has no "connect" command. It completes
  when the kernel reports the connection, fails with `ConnectFailed` or `Timeout`, and `Disconnect` or the device leaving
  closes the socket. Get Connections finds devices already connected at startup.
- **Devices.** One entry per address and address type. A complete name replaces the old one, a shortened name only fills
  a gap, and a report without RSSI keeps the last one. A device that was only discovered leaves the list 120 s after its
  last report. Paired and connected devices stay, and paired names are kept in the key file.

### Key file format (version 1)

A text file, `0600` in a `0700` directory, replaced atomically (temporary file, `fsync`, rename):

```
cupertino-bluetooth-keys 1
device 11:22:33:44:55:01 bredr
name 436166c3a92048656164736574
link 08 04 30313233343536373839616263646566
device 11:22:33:44:55:02 le-random
ltk 03 1 16 beef 0102030405060708 a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5
```

`name` is the UTF-8 name in hex. `link <type> <pin length> <key>`: type is the kernel's key type in hex, the PIN length
decimal, the key 16 bytes in hex. `ltk <type> <central 0|1> <encryption size> <diversifier> <random> <key>`. A `link` is
for a `bredr` device, an `ltk` for a low-energy one, each at most once per device. A missing file is an empty set, and a
deviation names its line.

### Constants

All approximate unless noted.

| Constant | Value |
|---|---|
| `PAIRING_TIMEOUT` | 60 s (the 30 s SMP limit is as designed) |
| `DISCOVERY_DURATION` | 60 s, so a crashed shell does not leave the radio scanning |
| `CONNECT_TIMEOUT` | 30 s |
| `REPLY_TIMEOUT` | 30 s |
| `DEVICE_TTL` | 120 s |
| `LOOP_TICK` | 1 s |

## Diagnostic events

The daemon records versioned diagnostic events to the system journal (`CUPERTINO_DIAG_SCHEMA=1`,
`SYSLOG_IDENTIFIER=cupertino-btd`), each beside the line or error it already gives; `main` installs the recorder before
anything else. Adapter and device addresses are recorded only as salted hashes (`bluetooth#` and eight hex digits).

| Event | Severity | Fields | Where |
|---|---|---|---|
| `bluetooth.backend.start_failed` | error | `step` (`logind`, `management_socket`, `bus`, `keys`), `kind` (the error's class), `error`, `adapter` (hashed, for `keys`) | a step of the start fails, or an adapter's keys cannot be loaded and it stays unavailable |
| `bluetooth.request.failed` | warn | `interface`, `method` (`StartDiscovery`, `StopDiscovery`, `Pair`, `Connect`, `Disconnect`, `Remove`, `RegisterAgent`, `UnregisterAgent`, `Powered`), `error` (the D-Bus error name), `address` (hashed, when the call names a device) | a call or the `Powered` write is answered with an error |
| `bluetooth.exit_error` | error | `error` | the daemon ends with an error, or the arguments are wrong |
| `bluetooth.panic` | fatal | `panic`, `location` | any panic |

The salt of the hashes is `diagnostic-salt` in the unit's `StateDirectory`; the key store does not read it.

## Open issues

Out of scope: the link is paired, bonded and connected, but no profile runs on it.

- **HID** (keyboards, mice, trackpads, gamepads) needs HID over GATT (a GATT client over the ATT channel, reading the HID report map, feeding `uhid` or the input
  stack) or HID over L2CAP (control and interrupt channels, PSM 0x11 and 0x13, plus the service discovery record). Next
  step: a GATT client in `cupertino-bluetooth` and a `uhid` bridge.
- **Audio** (A2DP, HFP) needs AVDTP, SBC or AAC negotiation and a PipeWire node. Deferred with D3 (the audio decision).
- **File transfer** (OBEX) is not started.
- **Service discovery.** `Uuids` lists only what the device advertises while discovered. There is no SDP or GATT service
  discovery on a connected device.
- **Several adapters.** Only the lowest ready controller is shown and driven.
- **Resolvable private addresses.** A bonded low-energy device that changes its address is not recognised, and the
  Identity Resolving Key is neither requested nor stored.
- **Legacy PIN pairing**, **discoverable mode** and **adapter name** are not supported.
- **A running system.** Nothing ran against a real pairing (see Tests).

## Tests

```
cargo test -p cupertino-btd --features app
```

`tests/model.rs` covers discovery merge (names, RSSI, class, appearance, UUIDs, icons, expiry), bring-up, powering,
discovery rounds, connecting, unpairing, the reply timeout and a controller that disappears. `tests/pairing.rs` covers
numeric comparison accepted and rejected, passkey entry and display, a timeout, a remote failure, a missing agent, a
pairing started by the other side, PIN refusal and powering off mid-pairing. `tests/keys.rs` covers the documented text, a
round trip with modes `0600` and `0700`, refusal of a corrupt file and of one readable by others, and every deviation
naming its line. `tests/dbus.rs` runs on a private `dbus-daemon` (skipped when missing) with a fake management interface
(`tests/common/controller.rs`): `Powered` toggled by an authorised and a refused caller, `StartDiscovery` producing devices
and `DeviceAdded`, a pairing that calls the registered agent and stores the key, an agent that refuses, a pairing with no
agent, and agent registration (live and dead owners).

The controller and every management packet are written by hand from `doc/mgmt-protocol.rst`; the only recorded traffic is
in `cupertino-bluetooth` (three read commands and their replies). The binary was also started on a private `dbus-daemon` with
`--session-bus --no-auth --read-only` against this machine's real management socket: `Available` and `Powered` read `true` (the adapter's actual state), `Devices` was empty,
and `StartDiscovery` failed with `the daemon runs read-only`. No command that changes anything was sent to the real
adapter, and no real device was paired.
