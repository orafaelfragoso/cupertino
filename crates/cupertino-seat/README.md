# cupertino-seat

The compositor's seat session over systemd-logind's D-Bus protocol (`org.freedesktop.login1`), replacing libseat.
Pure Rust over `zbus` (blocking API, no `-sys` crate); no libseat, no libsystemd.

## Public API

| Item | Contents |
|---|---|
| `LogindSession` | Smithay's `Session`: `open` (stat for the `dev_t`, `TakeDevice`), `close` (`ReleaseDevice`), `change_vt` (`Seat.SwitchTo`), `is_active`, `seat`. Cloneable handle |
| `LogindNotifier` | calloop `EventSource` emitting `Event::PauseSession` / `Event::ActivateSession`. Owns the session: dropping it calls `ReleaseControl` |
| `state::Machine` | the pure bookkeeping: `dev_t` to descriptor number, per-device active / pause pending / paused / gone, session running or not |
| `logind` | the thin D-Bus layer: lookup, `TakeControl(false), device calls, signal decoding |
| `inspect` | reads a session's `Id`, `Type`, `Active`, `VTNr` and `Seat` without taking control |

## How it works

- **Session lookup.** `XDG_SESSION_ID` when set (`GetSession`), else the process id (`GetSessionByPID`). A failing lookup
  is an error, not a retry with the other method. Calls and the signal subscription are bound to logind's unique bus name,
  so no other peer can pause or resume the session.
- **Pause.** The session is reported paused on the first `PauseDevice` of any held device (logind sends those before
  `Active` turns false). `PauseDeviceComplete` for a `"pause"` type goes out after the loop callback returned, and not for
  a device the callback already closed. `"force"` needs no answer. `"gone"` keeps the entry until the holder closes the
  descriptor, which then skips `ReleaseDevice`.
- **Resume.** The descriptor of `ResumeDevice` is dropped. For a DRM node it is the open file the holder already has,
  master again. For an input node it is a new open file while the holder's stays revoked, so the holder closes input
  devices on `Event::PauseSession` and opens them again on `Event::ActivateSession`, which follows once `Active` is true
  and every paused device is back.
- **Inactive start.** A session that starts inactive reports nothing until it activates, so check `is_active` before
  opening devices.
- **Release.** `close` forgets the device first and closes the descriptor even when `ReleaseDevice` fails. The next `open`
  of that device retries the release before `TakeDevice`.
- **Differences from Smithay's libseat session:** the acknowledgement timing and the strict `close` (see the module docs
  of `session`). `open` ignores its flags, as libseat's does.
- **Signal readers.** One thread reads the session's signals into a calloop channel, logging and skipping a signal that does
  not decode. A second watches `NameOwnerChanged` for logind's name leaving. The notifier returns `Error::SignalsEnded`
  when the bus connection closes or logind exits or restarts.

## Tests

`state::Machine` is driven with message sequences (VT switch away and back, forced pause, gone, release during pause, failed
release, started inactive). Other tests check `dev_t` lookup on `/dev/null` and the bus-name watch against a private
`dbus-daemon` (skipped when missing). Nothing talks to logind, because taking control needs a session of its own.
`cargo run -p cupertino-seat --example print-session` connects to the system bus, finds the session and prints it without
calling `TakeControl`.
