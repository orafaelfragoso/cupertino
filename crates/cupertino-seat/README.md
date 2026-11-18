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
- **Compositor deadlines.** One worker owns the controller connection and makes the calls in order. The loop waits at
  most 2 s per request, including queueing and writing to the bus, and 10 s for the whole startup. A missed deadline
  returns an error; a queued open or VT switch that has already expired is skipped. A release or pause acknowledgement
  still goes out in order. Giving up the notifier waits at most 2 s for `ReleaseControl` and disconnect; if necessary
  the worker finishes teardown after the notifier has returned. Disconnect also relinquishes a `TakeControl` that
  logind accepted without answering.
- **Pause.** The session is reported paused on the first `PauseDevice` of any held device (logind sends those before
  `Active` turns false). `PauseDeviceComplete` for a `"pause"` type goes out after the loop callback returned, and not for
  a device the callback already closed. `"force"` needs no answer. `"gone"` keeps the entry until the holder closes the
  descriptor, which then skips `ReleaseDevice`. If the acknowledgement goes unanswered, the holder is already paused:
  the notifier logs the timeout and continues listening for logind to resume it.
- **Resume.** The descriptor of `ResumeDevice` is dropped. For a DRM node it is the open file the holder already has,
  master again. For an input node it is a new open file while the holder's stays revoked, so the holder closes input
  devices on `Event::PauseSession` and opens them again on `Event::ActivateSession`, which follows once `Active` is true
  and every paused device is back.
- **Inactive start.** A session that starts inactive reports nothing until it activates, so check `is_active` before
  opening devices.
- **Release.** `close` forgets the device first and closes the descriptor even when `ReleaseDevice` fails. The next `open`
  of that device retries the release before `TakeDevice`. If `TakeDevice` itself times out, the worker releases the
  device even if its descriptor never arrives: logind may already have granted it. A failed cleanup is retried before
  the next take; `DeviceNotTaken` confirms that a retried or compensating release has nothing left to do.
- **Differences from Smithay's libseat session:** the acknowledgement timing and the strict `close` (see the module docs
  of `session`). `open` ignores its flags, as libseat's does.
- **Signal readers.** One thread reads the session's signals into a calloop channel, logging and skipping a signal that does
  not decode. A second watches `NameOwnerChanged` for logind's name leaving. The notifier returns `Error::SignalsEnded`
  when the bus connection closes or logind exits or restarts. The session's signals are bound to logind's unique name, which
  zbus and the bus daemon both filter on. A `NameOwnerChanged` counts only when the bus daemon sent it (its sender is
  `org.freedesktop.DBus`, which no peer can send as), checked on each message: the bus delivers a signal sent to the
  compositor's unique name whatever its rules say, and a forged one would make the compositor give up a session that is still
  there. zbus filters the rule's `sender='org.freedesktop.DBus'` too (it parses the daemon's name as a unique name); the check
  does not rely on that.

## Examples

`cargo run -p cupertino-seat --example print-session` connects to the system bus, finds the session and prints it without
calling `TakeControl`.
