# cupertino-evdev

Linux evdev input for the compositor, replacing libinput's device layer. Pure Rust over `rustix` ioctls; no
libinput, libevdev or libudev. The session opens `/dev/input/eventN` and hands over an `OwnedFd`; this crate never
opens paths.

## Public API

| Item | Contents |
|---|---|
| `Device` | `open(fd)` sets non-blocking and `CLOCK_MONOTONIC`, queries the device, reads its current state; `read_frames` reads batches until `EAGAIN`; `grab` is `EVIOCGRAB`; `AsFd` for calloop |
| `DeviceInfo` | name, id, properties, event-type and key/rel/switch bitmaps, and an `AbsInfo` per absolute axis (min, max, fuzz, flat, `Resolution`) |
| `Framer` | state machine from `InputEvent` to `Frame`, no fd needed; `apply_snapshot` is the `SYN_DROPPED` resync over plain data |
| `Frame` | keys (pressed, released, repeat), held buttons, relative motion, wheel in 1/120 detents, absolute pointer, full multitouch contact set, finger count, switches |

## How it works

- `SYN_DROPPED` discards the partial frame and everything to the next `SYN_REPORT`, then `Device` re-reads
  `EVIOCGKEY`, `EVIOCGSW`, `EVIOCGABS` and `EVIOCGMTSLOTS` and emits a `resynced` frame (stuck keys come out as releases).
- A device with `REL_WHEEL_HI_RES` is read through that axis only; one without it reports each legacy step as 120.
- A resolution of 0 is `Resolution::Unknown`; `AbsInfo::units_to_mm` then returns `None`.
- Structural violations (bad key value, slot out of range, contact data without a tracking id, multitouch protocol A) are
  `FrameError`s, never skipped. Event codes outside the modelled set (`EV_MSC`, `ABS_PRESSURE`, ...) are ignored.
- Opcodes and constants are written out from `linux/input.h` and `linux/input-event-codes.h`; only the 64-bit
  `input_event` layout is supported.

## Tests

Tests drive `Framer` with event sequences in `tests/fixtures/*.events` (`<sec>.<usec> <type> <code> <value>`). The fixtures
are hand-written to match what kernels emit. None was captured from hardware.

`cargo run -p cupertino-evdev --example print-frames -- /dev/input/event5` prints the capabilities and frames of a real
device. It needs read access to the node.
