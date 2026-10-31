# cupertino-devices

Device discovery for the compositor without libudev. It reads `/sys` and the kernel's
`NETLINK_KOBJECT_UEVENT` socket directly. The only dependency is `rustix` (`fs`, `net`).

## Public API

| Module | Contents |
|---|---|
| `device` | `enumerate(sysfs_root, Subsystem::{Input, Drm})`: `event*` and `card*` nodes, lowest number first. A `Device` has its syspath, sysname, `/dev` node (`DEVNAME`), `DevT` (`MAJOR`/`MINOR`), `uevent` properties and `parent` / `parents |
| `input_id` | `classify(&InputCapabilities)` and `classify_device(&Device, sysfs_root)`: keyboard, mouse, touchpad, touchscreen, tablet, tablet pad, joystick, pointing stick, accelerometer, switch, and `clickpad` (`INPUT_PROP_BUTTONPAD`). Same rules as udev's `input_id` builtin |
| `capabilities` | `CapabilityMask`: the hex-word bitmasks of `capabilities/*` and `properties` (highest word first) |
| `gpu` | `gpus`, `primary_gpu`, `select_primary`: the card whose PCI parent has `boot_vga` = 1, else the first card; `select_render`: the card with a given node, else the primary one |
| `monitor` | `Monitor`: a non-blocking uevent socket on the kernel group (`AsFd`, register it with the event loop; `drain` returns `DeviceEvent`s). `parse_datagram is the pure parser |
| `seat` | `seat_name`: `ID_SEAT`, else `seat0` |
| `dev_t` | `DevT`, the device number newtype |

Every function that reads sysfs takes the root as a parameter (`/sys` in production), and every parser takes bytes or
text. The tests therefore run on `tests/fixtures/sys` (a hand-written tree modelled on a keyboard, a mouse, a clickpad
touchpad, a touchscreen and two GPUs) and on recorded uevent datagrams.

## How it works

- **No udevd database.** There is no `ID_SEAT`, so every device is on `seat0`.
- **Kernel events arrive before udevd tags the node.** Open nodes through the session (logind), which
  grants access itself.
- **Messages are filtered.** udevd's `libudev` datagrams and datagrams not sent by the kernel are dropped. A
  malformed kernel datagram is an error, not a skip.
- **Mask words are 64 bits**, as on every 64-bit kernel.
- **The input device lives above the node.** The capabilities and `properties` belong to the directory,
  so `classify_device` walks up from to the first directory with `capabilities/ev`.
- **Provenance.** The decision rules follow systemd's `src/udev/udev-builtin-input_id.c` (GPL-2.0-or-later), written
  afresh in Rust.

## Tests

```
cargo test -p cupertino-devices
cargo run -p cupertino-devices --example list-devices   # opt-in: classifies this machine's real /sys
```
