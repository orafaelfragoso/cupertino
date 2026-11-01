# cupertino-pipewire

The volume and mute state of the default PipeWire sink and source, read and changed through WirePlumber's `wpctl`. A
level set here is the level `wpctl get-volume` then reports, on the same cubic scale, because `wpctl` does both. No
PipeWire library is linked. Layer: `platform`.

## Public API

| Item | Contents |
|---|---|
| `Wpctl` | the program (`wpctl` on `PATH` by default): `volume(target)`, `set_level(target, level)`, `set_muted(target, muted)` |
| `Target` | `DefaultSink` (`@DEFAULT_AUDIO_SINK@`), `DefaultSource` (`@DEFAULT_AUDIO_SOURCE@`) |
| `Volume` | `level` 0 to 1 and `muted` |
| `parse_volume` | the `get-volume` output: `Volume: 0.40`, or `Volume: 0.40 [MUTED]` |
| `fake::FakeWpctl` | feature `fake`: a stand-in `wpctl` script that keeps the sink and the source in files, for the tests of the crates that use this one |

## How it works

- Every call starts `wpctl` once and waits for it, at most `TIMEOUT` (1 s; `with_timeout` changes it). A non-zero exit
  is `Error::Failed` with its standard error, for example when no PipeWire daemon runs or there is no default node; a
  `wpctl` still running at the timeout, as when the daemon is up but does not answer, is killed and fails the same way
  without a code. A program that cannot start is `Error::Spawn`.
- `set_level` clamps the level to 0 to 1 and does not touch the mute state. A node amplified above 100% reads as 1.
- `wpctl` prints two decimals, so a level reads back rounded to a hundredth.

The overlay, the control panel and the Sound pane use the default sink, and use the ALSA master (`cupertino-alsa`)
when no PipeWire daemon answers.

## Tests

```
cargo test -p cupertino-pipewire
```
