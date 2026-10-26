# cupertino-alsa

The kernel's sound control interface for the compositor without alsa-lib: `/dev/snd/controlC*` over ioctls. The only
dependency is `rustix` (`fs`). 64-bit kernels only. Layer: `platform`.

## Public API

| Module | Contents |
|---|---|
| `abi` | byte layouts and ioctl numbers of `snd_ctl_card_info`, `snd_ctl_elem_id`, `_list`, `_info`, `_value` and `snd_ctl_event`, parsed from and encoded to byte buffers; `Interface`, `ElementType`, `Access` |
| `tlv` | `DbCurve::parse`: `DB_SCALE`, `DB_MINMAX`, `DB_MINMAX_MUTE` and `DB_RANGE` gain curves from a TLV block; any other TLV type is "no dB information" |
| `volume` | `VolumeRange`: raw value to level 0..1 and back, and one press (`step`) |
| `card` | `Card`: open a control device, `info`, `elements`, `read_values`, `write_values`, `db_curve`, `subscribe`, `read_events` |
| `master` | `select_master`, `find_switch`, `Master`: the playback master volume and its mute switch |

Every function that opens a device takes the device directory (`/dev/snd` in production), and every parser takes bytes, so
the tests run on literal byte layouts.

## How it works

- **Master selection.** Candidates are the mixer-device integer controls that are readable, writable, active, have a range
  and whose name ends in `Playback Volume`. The pick is `Master Playback Volume`, else `PCM Playback Volume`, else the
  first one in the card's order. Across cards the lowest rank wins, then the lowest card number. The mute switch is the
  boolean `... Playback Switch` of the same name stem, index and device. A master without one cannot be muted.
- **Level mapping** (the rule of alsa-utils' `volume_mapping.c`, written afresh). With dB data, a range of at most 24 dB is
  linear in dB. A wider range maps `10^((dB - max) / 60 dB)`, rescaled so its lowest step is 0 (a muted lowest step needs no
  rescaling). Without dB data the mapping is linear in the raw value. Setting a level picks the raw value with the nearest
  gain, and levels 0 and 1 are the ends.
- **A press moves at least one raw step** unless it is at the end, so a control with few steps does not stick.
- **Channels.** The level is the first channel's. A write sets every channel to the same raw value.
- **Events.** `subscribe` asks the kernel to queue events on the descriptor. `read_events` is non-blocking and returns what
  is queued (`AsFd` gives the descriptor to poll).
- **Structure sizes** (asserted in the tests): card info 376, element id 64, list 80, info 272, value 1224, event 72, TLV
  header 8.

## Tests

```
cargo test -p cupertino-alsa
cargo run -p cupertino-alsa --example list   # opt-in, read-only: this machine's cards, controls and master candidate
```
