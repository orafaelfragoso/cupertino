# cupertino-pipewire

The volume and mute state of the default PipeWire sink and source, read and changed through WirePlumber's `wpctl`. A
level set here is the level `wpctl get-volume` then reports, on the same cubic scale, because `wpctl` does both. No
PipeWire library is linked. Layer: `platform`.

## Public API

| Item | Contents |
|---|---|
| `Wpctl` | the program (`wpctl` on `PATH` by default): `volume(target)`, `set_level(target, level)`, `set_muted(target, muted)`, `sinks()`, `set_default_sink(id)` |
| `LevelWriter` | one target's volume written on a thread of its own: `set(volume)` returns at once (the latest value asked wins, the mute state is written only when it changes, a failed write is reported by the next call), `wait_written()` waits for the last one |
| `Sink` | `id` (the `wpctl set-default` argument), `name` and `default`, one output of `wpctl status` |
| `parse_sinks` | the entries under `Sinks:` in the `Audio` section of `wpctl status` output, `*` marking the default |
| `Target` | `DefaultSink` (`@DEFAULT_AUDIO_SINK@`), `DefaultSource` (`@DEFAULT_AUDIO_SOURCE@`) |
| `Volume` | `level` 0 to 1 and `muted` |
| `parse_volume` | the `get-volume` output: `Volume: 0.40`, or `Volume: 0.40 [MUTED]` |
| `fake::FakeWpctl` | feature `fake`: a stand-in `wpctl` script that keeps the sink and the source (and with `set_sinks` the sink list and its default) in files, for the tests of the crates that use this one |

## How it works

- Every call starts `wpctl` once and waits for it, at most `TIMEOUT` (1 s; `with_timeout` changes it). A non-zero exit
  is `Error::Failed` with its standard error, for example when no PipeWire daemon runs or there is no default node; a
  `wpctl` still running at the timeout, as when the daemon is up but does not answer, is killed and fails the same way
  without a code. A program that cannot start is `Error::Spawn`.
- `set_level` clamps the level to 0 to 1 and does not touch the mute state. A node amplified above 100% reads as 1.
- A program another thread has just written can answer `ETXTBSY` for a moment; the spawn retries up to 20 times, 10 ms apart.
- A slider or a held volume key goes through a `LevelWriter`: a `wpctl` run takes tens of milliseconds, longer than a
  frame, so a write on the UI thread drops frames, and values asked faster than `wpctl` runs would queue up behind it.
- `wpctl` prints two decimals, so a level reads back rounded to a hundredth.

The Sound pane lists the sinks and sets the default one when there are two or more. The overlay, the control panel and the Sound pane use the default sink, and use the ALSA master (`cupertino-alsa`)
when no PipeWire daemon answers.

## Tests

```
cargo test -p cupertino-pipewire
```
