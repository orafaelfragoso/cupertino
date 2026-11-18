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

The service serialises requests. `cupertino-pipewire` uses WirePlumber's `wpctl` for device selection and volumes and
`pactl` for stereo balance. If the default playback volume cannot be read, the service uses the ALSA playback master
under `/dev/snd`. That backend has no selectable devices, capture control or stereo balance. A master with no mute
switch reads as muted at zero and cannot be muted at a nonzero level. Settings uses this service; the control panel
and on-screen displays still use their own audio backends.

The `audio1` tests activate the service on a private session bus. To run just that test target, build
its stand-in first: `cargo build -p cupertino-audiod --example audio-backend-stand-in`, then
`cargo nextest run -p cupertino-audiod --test audio1`.
