# cupertino-ui-test

CupertinoKit test harness: `TestDriver<Application>`, `HeadlessWindowSystem`, event `Trace` record and replay,
the `Inspector`'s deterministic dumps, and `semantic_gaps`, which lists every node of an accessibility tree a screen
reader could not announce (no name, or a child the tree does not hold). MPL-2.0; no display or session bus is needed.

`TestDriver::new` owns the production runtime and uses the supplied `ManualClock` for both runtime work and trace
receipt timestamps, replacing `RuntimeOptions::clock`. Use `runtime()` to inspect the model, windows, focus and
snapshots, and `window_system_mut()` to queue input, configure windows, control frame callbacks and inspect presentation.
`HeadlessWindowSystem::drags` lists the drags the runtime started (with `Capabilities::drag_and_drop`; without it they
are refused); a start takes the pointer from the window with a leave, and the test enqueues each `PlatformEvent::Drag`
step. `advance_to` rejects backwards time. `run_until_idle(max_turns)` never advances time or waits: it reports waiting work
or an exhausted turn budget. Each queued platform event gets one runtime turn, so Tab changes focus before the next
key is recorded even at the same timestamp. Explicit frame callbacks and tasks remain under the fixture's control.

Recording reads protection from core's current focus, including focusable nodes without an accessibility role.
Start clipboard reads through `TestDriver::read_clipboard(window, kind, mime)`: the driver keeps the requesting focus's
protection by token until the reply arrives. A protected request stays redacted after Tab moves focus elsewhere;
a public request stays public if focus later becomes protected. Unknown clipboard tokens fail before their content
is recorded. The low-level `Trace::record` is crate-private; callers supply no arrival-time protection flag.
Core does not yet route clipboard replies to widgets: this API exercises the headless clipboard and trace contract,
not application paste delivery.

`Trace::replay` validates the trace and returns ordered, timestamped events; protected entries require matching
`SecureInputs` fixtures. Advance the driver to each recorded time and enqueue each event. Recreate compositor
lifecycle state through the headless helpers (for example, `configure` registers an acknowledgement before queuing
the configure event), and repeat clipboard requests through the driver before delivering their replies. Raw event
injection alone does not recreate outstanding configure or clipboard requests. Snapshots and presented frames are
available through the runtime and window system; the inspector formats the same snapshots.

## Look checks

`TestDriver::look(window, name)` writes the window as `<name>-<appearance>@<scale>x.png` for a person to compare with
a reference, and does nothing unless `CUPERTINO_LOOK_DIR` names a folder, so an ordinary test run writes no file.
`CUPERTINO_LOOK_SCALE` sets the backing scale (2 when unset) and `CUPERTINO_LOOK_APPEARANCES` the appearances,
comma-separated from `light`, `dark`, `light-hc` and `dark-hc` (`light,dark` when unset). Each picture uses the
Cupertino theme of `data/theme` with the window's own preferences in that appearance; afterwards the window has its
theme back at scale 1. `look_into(dir, window, name, scale, appearances)` does the same without the environment.

The portal's file chooser and screenshot consent, the Files browser window and every Settings pane call it from
their tests. One command writes all of them at 2x, light and dark:

```sh
CUPERTINO_LOOK_DIR=/tmp/look cargo test -p cupertino-portal -p cupertino-files -p cupertino-settings \
  --features cupertino-portal/app,cupertino-files/app,cupertino-settings/app
```

Add `CUPERTINO_LOOK_APPEARANCES=light,dark,light-hc,dark-hc` for the high-contrast pictures too. The pictures use
the test's placeholder icons, not the asset pack's.
