# cupertino-bluetooth

The kernel's Bluetooth management interface (`AF_BLUETOOTH`, `BTPROTO_HCI`, `HCI_CHANNEL_CONTROL`) as typed Rust: a codec
for the commands and events the Bluetooth daemon needs, a parser for advertising and extended inquiry data, and two small
sockets. Pure Rust over `rustix` (`linux-raw-sys` is its syscall table). The format comes from `doc/mgmt-protocol.rst` in the BlueZ tree (formerly
`mgmt-api.txt`); the kernel implements it in `net/bluetooth/mgmt.c`.

## Public API

| Item | Contents |
|---|---|
| `address` | `Address` (wire order; `Display` is `AA:BB:CC:DD:EE:FF`), `AddressType` (`bredr`, `le-public`, `le-random`), `DeviceId`, `AddressTypes` |
| `command` | `Command` and `Command::encode(index)`: Read Version, Read Index List, Read Controller Info, Set Powered / Discoverable / Connectable / Bondable, Set IO Capability, Start / Stop Discovery, Confirm Name, Pair Device, User Confirmation and Passkey replies, PIN negative reply, Unpair Device, Disconnect, Get Connections, Load Link Keys, Load Long Term Keys; `Opcode` constants |
| `event` | `decode_packet(bytes) -> Packet`; `Event`: Command Complete / Status, Index Added / Removed, New Settings, New Link Key, New Long Term Key, Device Connected / Disconnected, Connect Failed, PIN Code / User Confirmation / User Passkey Request, Authentication Failed, Device Found, Discovering, Device Unpaired, Passkey Notify; an event the crate does not know decodes to `Event::Other` |
| `reply` | decoders for the return parameters of Command Complete: `Version`, `ControllerInfo`, `parse_index_list`, `parse_settings`, `parse_device`, `parse_address_types`, `parse_connections` |
| `eir` | `Eir::parse`: flags, names (complete or shortened), TX power, class of device, appearance, 16/32/128-bit service UUIDs (`Uuid`) |
| `key` | `LinkKey`, `LongTermKey` and their types; `KeyValue` prints as `KeyValue(..) so a key never reaches a log |
| `status` | `Status` (0x00 to 0x14), `Settings` bits, `ClassOfDevice` with `MajorClass` |
| `socket` | `ControlSocket::open`, `send(index, &Command)`, `drain -> Drained { packets, errors }`; `AsFd`, so callers poll it |
| `link` | `Link::open(adapter, device): a non-blocking L2CAP socket bound to the adapter and connecting to the device, to hold a baseband link open |

The only `unsafe` is in `sockaddr.rs`: `rustix` has no Bluetooth socket address, so two `SocketAddrArg` impls write the
kernel's `sockaddr_hci` and `sockaddr_l2` into stack arrays. Everything else is `#![deny(unsafe_code)]`.

## How it works

- **Header.** Every packet starts with opcode or event code, controller index and parameter length, each a little-endian
  `u16`. Index `0xFFFF` means "no controller". A packet whose length field disagrees with the bytes read, or whose
  parameters do not have exactly the documented layout, is a `ParseError`, not a guess.
- **Without privilege.** The socket opens for an ordinary user, but the kernel lets it send only the read commands (version, index list, controller info and similar) and
  sends it only controller and settings events. It refuses every other command with status 0x14 (Permission Denied).
  Pairing, powering and discovery need `CAP_NET_ADMIN`. Only the three read commands were exercised here. The kernel does
  not send New Settings to the socket that issued the command, so a caller must apply the settings in the reply.
- **Connecting.** No management command connects. The baseband link comes up when something connects an L2CAP socket
  (BR/EDR: PSM 1, the service discovery channel; low energy: CID 4, the attribute protocol). `Link` does that, and
  dropping it closes the socket.
- **Names.** Remote names are untrusted: invalid UTF-8 becomes U+FFFD so that a hostile name cannot hide a device.
  Device Found with RSSI 127 means "not available" and decodes to `None`.
- **Load Link Keys** always sends `Debug_Keys` as 0.

## Tests

```
cargo test -p cupertino-bluetooth
cargo run -p cupertino-bluetooth --example record-bluetooth -- [--listen SECONDS] > capture.txt
```

`tests/commands.rs` checks the exact bytes of every command, written field by field from the document, and the decoding of
the replies. `tests/events.rs` decodes every event to literal values and checks the failures (wrong length, truncated
field). `tests/recorded.rs` with `tests/fixtures/recorded-mgmt.txt` checks that the codec reproduces three recorded
commands byte for byte (Read Version, Read Index List, Read Controller Info) and decodes the three replies.

The recording comes from the real controller of the development machine, with the address replaced by
`AA:BB:CC:DD:EE:01` and the name by `cupertino-test` (management version 1.23, one controller, Bluetooth version 12,
manufacturer 0x5D, class of device `0x7c0104`). Every other fixture (all events, all other commands, the EIR cases) is built by hand from the document. No pairing or
discovery traffic was recorded, because that needs privilege and a device. `examples/record-bluetooth.rs` is read-only and refuses
to record events that could carry an address or a key. A 3 s listen on the unprivileged socket produced no events.

## Open issues

Not covered: the Bluetooth 5 extended advertising commands, Add / Remove Device (the kernel's own auto-connect list), OOB
data and Read Local OOB Data, Set Local Name and Set Class of Device (the daemon never changes them), and SSP-only and
secure-connection toggles.
