# cupertino-ui-atspi

Publishes a CupertinoKit application's accessibility tree to assistive technology (screen readers) over AT-SPI, and
hands their requests back to the application's loop. MPL-2.0. It implements `cupertino-ui-core`'s
`AccessibilityAdapter` on AccessKit's Unix adapter (`accesskit` 0.25, `accesskit_unix` 0.24 with its default features:
zbus 5 on async-io, no tokio). AccessKit's types stay inside the crate.

## Public API

| Item | Contents |
|---|---|
| `AtspiAdapter` | `new(waker)` takes the window system's `Waker` (`WindowSystem::waker()`); the adapter goes into `RuntimeOptions.accessibility`. `muted(waker)` starts muted until the first `session_locked`, for an application that may start while the session is locked |

```rust
let options = RuntimeOptions {
    accessibility: Some(Box::new(AtspiAdapter::new(system.waker()))),
    ..options
};
let runtime = Runtime::new(system, options);
```

## How it works

- **Windows.** Each window is a top-level AT-SPI object with its own AccessKit adapter, created the first time core
  names the window (its first focus or update). The application's name on the bus is its process name.
- **Activation.** Nothing is exported until an assistive technology turns accessibility on (`org.a11y.Status`
  `IsEnabled`); until then `is_active` is false and core builds no tree. When a window activates, the adapter wakes the
  application so core sends every window whole; a window that activates after core sent its tree gets it from the copy
  the adapter keeps, never by calling into the application from AccessKit's thread. When the last window deactivates,
  those copies are no longer handed out: core stops sending while nothing listens.
- **Nodes.** Roles, names, descriptions, states, bounds in window points, relations, children and values (text, number
  with its range and step, toggle) map one to one. `Separator` and `Splitter` are both AccessKit's splitter, ARIA's
  separator.
- **Secrets.** A `PasswordInput` node, or any node whose value is `Protected`, carries no value and offers neither
  `SetValue` nor a selection, whatever core sent: a second defence after core's.
- **Session lock.** While `session_locked(true)` holds, every node is sent without its live politeness, so nothing is
  announced. AccessKit announces only as a live node is added or renamed, from the node it is sent then, so neither
  the lock nor the unlock announces anything, and the first change after the unlock speaks as usual.
- **Actions.** A request from an assistive technology becomes core's `ActionRequest` (`Click`, `Focus`, `Increment`,
  `Decrement`, `Expand`, `Collapse`, `ShowMenu`, `ScrollIntoView`, `SetValue` with text or a number). It is kept only
  when its target advertised that action in its last update and it doesn't set a secret's value. It waits in a queue
  that `drain_actions` empties on the application's loop, and the waker wakes the loop for it. A request without a
  core action, or with data the action doesn't take, is dropped.

## Limitations

- **Text caret and selection.** AccessKit carries them in text runs, which the crate doesn't build yet. A text field's
  value is read, but its caret and selection are not, and `SetSelection` is not offered.
- **Window position.** A Wayland client doesn't know where its window is, so bounds are window-relative.
- **Closed windows.** Core's adapter interface has no call for a closed window, so its AT-SPI object stays until the
  application exits.

## Tests

```
cargo test -p cupertino-ui-atspi
```

Unit tests cover the node and action mapping, the password and protected-value defences, the action filter, and
activation and deactivation, including a tree from before the last deactivation never being handed out.
`tests/atspi.rs` starts a private `dbus-daemon` (skipped when it is not installed) that serves a fake
`org.a11y.Status` and AT-SPI registry. The application runs in a copy of the test binary, started with the bus
addresses AccessKit reads from its environment. Acting as an assistive technology, the test reads the window and its
nodes with their roles and names, checks that the password's text is empty, tries to type into it, and clicks OK: only
the Click reaches the application. With accessibility off, the application never embeds its root.
