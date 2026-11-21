# cupertino-audiod

The Sound pane's mixer. The session bus starts `/usr/bin/cupertino-audiod` on the first call to
`org.cupertino.Audio1`. The process keeps running until its bus closes. Playback and the input level meter use the
audio system directly.

## `org.cupertino.Audio1` at `/org/cupertino/Audio1`

| Method | In → out | Operation |
|---|---|---|
| `ListDevices` | `s` → `a(usb)` | Devices in backend order: ID, name, default. |
| `SetDefault` | `su` → — | Select a listed ID for the given direction. |
| `GetVolume` | `s` → `db` | Read the default device's level and mute state. |
| `SetVolume` | `sdb` → `db` | Apply the level and mute state; return the level and mute state reached. |
| `GetBalance` | — → `d` | Read the default stereo output's balance. |
| `SetBalance` | `d` → — | Change the balance, preserving the louder channel's level. |

Directions are `input` and `output`. Levels run from 0 to 1, balance from −1 (left) to 1 (right). Invalid directions
and numbers give `InvalidArgs`, missing devices and backend failures give `Failed`, and unsupported controls give
`NotSupported`. Audio1 emits no change signal: each read asks the current backend.

The service serialises requests. Its `pipewire` module uses WirePlumber's `wpctl` for device selection and volumes and
`pactl` for stereo balance. If the default playback volume cannot be read, the service uses the ALSA playback master
under `/dev/snd` (its `alsa` module). A control device that answers `EACCES` or `ENOENT` is not ready yet (at login logind grants the session's access
a moment after the session starts): the request opens it again after 50, 100, 200, 400 and 800 ms before it fails, and
after such a failure the next requests within 30 s open once without waiting. That backend has no selectable devices, capture control or stereo balance. A master with no mute
switch reads as muted at zero and cannot be muted at a nonzero level. Settings uses this service; the control panel
and on-screen displays still use their own audio backends.

The `audio1` tests activate the service on a private session bus. To run just that test target, build
its stand-in first: `cargo build -p cupertino-audiod --example audio-backend-stand-in`, then
`cargo nextest run -p cupertino-audiod --test audio1`.

## Backends

Both backends are private modules of the service; the crates `cupertino-pipewire` and `cupertino-alsa` were folded
into it because it was their only user.

### `pipewire`: the default sink and source through `wpctl`

The volume and mute state of the default PipeWire sink and source, read and changed through WirePlumber's `wpctl`. A
level set here is the level `wpctl get-volume` then reports, on the same cubic scale, because `wpctl` does both. No
PipeWire library is linked.

- `Wpctl`: the program (`wpctl` on `PATH` by default): `volume(target)`, `set_level`, `set_muted`, `sinks()`,
  `sources()`, `set_default_sink`, `set_default_source`, `balance()` and `set_balance` (through `pactl`).
- `Target` is `DefaultSink` (`@DEFAULT_AUDIO_SINK@`) or `DefaultSource` (`@DEFAULT_AUDIO_SOURCE@`).
- Every call starts `wpctl` once and waits for it, at most `TIMEOUT` (1 s). A non-zero exit is `Error::Failed` with its
  standard error, for example when no PipeWire daemon runs or there is no default node; a `wpctl` still running at the
  timeout, as when the daemon is up but does not answer, is killed and fails the same way without a code. A program that
  cannot start is `Error::Spawn`.
- `set_level` clamps the level to 0 to 1 and does not touch the mute state. A node amplified above 100% reads as 1.
- A program another thread has just written can answer `ETXTBSY` for a moment; the spawn retries up to 20 times, 10 ms
  apart.
- `wpctl` prints two decimals, so a level reads back rounded to a hundredth.
- `parse_sinks` and `parse_nodes` read the entries under a heading in the `Audio` section of `wpctl status`, `*`
  marking the default; `parse_volume` reads `Volume: 0.40` or `Volume: 0.40 [MUTED]`; `parse_balance` reads the two
  channels of `pactl get-sink-volume`.

### `alsa`: the playback master through the kernel's control interface

The kernel's sound control interface without alsa-lib: `/dev/snd/controlC*` over ioctls. The only dependency is
`rustix` (`fs`). 64-bit kernels only. Only what the service uses is kept: no card info, no event reading.

| Module | Contents |
|---|---|
| `abi` | byte layouts and ioctl numbers of `snd_ctl_elem_id`, `_list`, `_info` and `_value`, parsed from and encoded to byte buffers |
| `tlv` | `DbCurve::parse`: `DB_SCALE`, `DB_MINMAX`, `DB_MINMAX_MUTE` and `DB_RANGE` gain curves from a TLV block; any other TLV type is "no dB information" |
| `volume` | `VolumeRange`: raw value to level 0..1 and back |
| `card` | `Card`: open a control device, `elements`, `read_values`, `write_values`, `db_curve` |
| `master` | `select_master`, `find_switch`, `Master`: the playback master volume and its mute switch |

- **Master selection.** Candidates are the mixer-device integer controls that are readable, writable, active, have a range
  and whose name ends in `Playback Volume`. The pick is `Master Playback Volume`, else `PCM Playback Volume`, else the
  first one in the card's order. Across cards the lowest rank wins, then the lowest card number. The mute switch is the
  boolean `... Playback Switch` of the same name stem, index and device. A master without one cannot be muted.
- **Level mapping** (the rule of alsa-utils' `volume_mapping.c`, written afresh). With dB data, a range of at most 24 dB is
  linear in dB. A wider range maps `10^((dB - max) / 60 dB)`, rescaled so its lowest step is 0 (a muted lowest step needs no
  rescaling). Without dB data the mapping is linear in the raw value. Setting a level picks the raw value with the nearest
  gain, and levels 0 and 1 are the ends.
- **Channels.** The level is the first channel's. A write sets every channel to the same raw value.
- **Structure sizes** (kernel `asound.h`): element id 64, list 80, info 272, value 1224, TLV header 8.
