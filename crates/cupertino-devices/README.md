# cupertino-devices

Device discovery and input for the compositor without libudev or libinput. It reads `/sys` and the kernel's
`NETLINK_KOBJECT_UEVENT` socket directly, and reads evdev devices from an fd. The only dependency is `rustix` (`fs`,
`net`, `time`).

## Public API

| Module | Contents |
|---|---|
| `device` | `enumerate(sysfs_root, Subsystem::{Input, Drm})`: `event*` and `card*` nodes, lowest number first. A `Device` has its syspath, sysname, `/dev` node (`DEVNAME`), `DevT` (`MAJOR`/`MINOR`), `uevent` properties and `parent` / `parents |
| `input_id` | `classify(&InputCapabilities)` and `classify_device(&Device, sysfs_root)`: keyboard, mouse, touchpad, touchscreen, tablet, tablet pad, joystick, pointing stick, accelerometer, switch, and `clickpad` (`INPUT_PROP_BUTTONPAD`). Same rules as udev's `input_id` builtin |
| `capabilities` | `CapabilityMask`: the hex-word bitmasks of `capabilities/*` and `properties` (highest word first) |
| `gpu` | `gpus`, `primary_gpu`, `select_primary`: the card whose PCI parent has `boot_vga` = 1, else the first card; `select_render`: the card with a given card or render node (each `Gpu` lists its `render_node`), else the primary one, with a `RenderChoice` saying which |
| `preflight` | `run(sysfs_root)`: the primary GPU's driver and what it needs before a compositor takes the display (`nvidia_drm` loaded with `modeset=Y`, the nvidia GBM backend), as a `Report` of notes, warnings and problems, each with its fix; `None` without a GPU. The server checks it before the TTY backend starts, and `cupertino-diagnose` reports it |
| `monitor` | `Monitor`: a non-blocking uevent socket on the kernel group (`AsFd`, register it with the event loop; `drain` returns `DeviceEvent`s). `parse_datagram is the pure parser |
| `seat` | `seat_name`: `ID_SEAT`, else `seat0` |
| `dev_t` | `DevT`, the device number newtype |
| `evdev` | Linux evdev input, replacing libinput's device layer; see [evdev](#evdev) |

Every function that reads sysfs takes the root as a parameter (`/sys` in production), and every parser takes bytes or
text.

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

## evdev

Pure Rust over `rustix` ioctls; no libinput, libevdev or libudev. The session opens `/dev/input/eventN` and hands over
an `OwnedFd`; the module never opens paths.

| Item | Contents |
|---|---|
| `Device` | `open(fd)` sets non-blocking and `CLOCK_MONOTONIC`, queries the device, reads its current state; `read_frames` reads batches until `EAGAIN`; `grab` is `EVIOCGRAB`; `AsFd` for calloop |
| `DeviceInfo` | name, id, properties, event-type and key/rel/switch bitmaps, and an `AbsInfo` per absolute axis (min, max, fuzz, flat, `Resolution`) |
| `Framer` | state machine from `InputEvent` to `Frame`, no fd needed; `apply_snapshot` is the `SYN_DROPPED` resync over plain data |
| `Frame` | keys (pressed, released, repeat), held buttons, relative motion, wheel in 1/120 detents, absolute pointer, full multitouch contact set, finger count, switches |

- `SYN_DROPPED` discards the partial frame and everything to the next `SYN_REPORT`, then `Device` re-reads
  `EVIOCGKEY`, `EVIOCGSW`, `EVIOCGABS` and `EVIOCGMTSLOTS` and emits a `resynced` frame (stuck keys come out as releases).
- A device with `REL_WHEEL_HI_RES` is read through that axis only; one without it reports each legacy step as 120.
- A resolution of 0 is `Resolution::Unknown`; `AbsInfo::units_to_mm` then returns `None`.
- Structural violations (bad key value, slot out of range, contact data without a tracking id, multitouch protocol A) are
  `FrameError`s, never skipped. Event codes outside the modelled set (`EV_MSC`, `ABS_PRESSURE`, ...) are ignored.
- Opcodes and constants are written out from `linux/input.h` and `linux/input-event-codes.h`; only the 64-bit
  `input_event` layout is supported.

## Examples

```
cargo run -p cupertino-devices --example list-devices   # opt-in: classifies this machine's real /sys
cargo run -p cupertino-devices --example print-frames -- /dev/input/event5   # an evdev node's capabilities and frames; needs read access
```
