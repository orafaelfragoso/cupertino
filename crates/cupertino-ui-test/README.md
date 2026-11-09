# cupertino-ui-test

CupertinoKit test harness: `TestDriver<Application>`, `HeadlessWindowSystem`, event `Trace` record and replay,
the `Inspector`'s deterministic dumps, and `semantic_gaps`, which lists every node of an accessibility tree a screen
reader could not announce (no name, or a child the tree does not hold). MPL-2.0; no display or session bus is needed.

`TestDriver::new` owns the production runtime and uses the supplied `ManualClock` for both runtime work and trace
receipt timestamps, replacing `RuntimeOptions::clock`. Use `runtime()` to inspect the model, windows, focus and
snapshots, and `window_system_mut()` to queue input, configure windows, control frame callbacks and inspect presentation.
`HeadlessWindowSystem::drags` lists the drags the runtime started (with `Capabilities::drag_and_drop`; without it they
are refused); a start takes the pointer from the window with a leave, and the test enqueues each `PlatformEvent::Drag`
step. A drag from another application is simulated with `drag_enter(window, position, data, actions, mods)` (the data
in each type it offers), `drag_move`, `drag_leave` and `drag_drop`; as a compositor does, the system chooses the action
from each answer (and reports `DropEvent::Action`), drops only a drag whose last answer took a type and an allowed action
(else it leaves), and answers `read_drop` with the data. `incoming_drag(token)` (an `IncomingDrag`) keeps the answers,
the choice, the reads and how the drop was finished; the driver records drop reads as unprotected. `advance_to` rejects backwards time. `run_until_idle(max_turns)` never advances time or waits: it reports waiting work
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
the configure event; after `activate_by_keyboard(window)` a window's configures say it is activated while the last
focus event queued for that window gave it the keyboard, whatever states the test passes, a model of how a layer
surface is reported that does not count its popups' keyboard), and repeat clipboard requests through the driver
before delivering their replies. Raw event injection alone does not recreate outstanding configure or clipboard
requests. Snapshots and presented frames are available through the runtime and window system; the inspector formats
the same snapshots.

## Accessibility audit

`accessibility_audit(tree)` lists what a screen reader user would trip on in one whole tree: a control without a name
(a list item, tree item, cell or tab may be named by its children, a toolbar by its contents), an action on a node that
is not a control, a toggle, slider, spin button or progress without its value or with one out of range, a password
field carrying text, a pop-up button that does not say whether it is open, masked characters in any name, description
or value, a focus the tree does not hold or that the node does not take, a relation to a missing node, a control with
no area, and a list, menu, tree or tab item outside its container, without its place or count, or sharing its place.
`secret_leaks(tree, secrets)` lists every node whose name, description or value reads out a secret, and
`reading_order_breaks(tree, sequence)` lists the steps of a Tab sequence that go backwards in reading order (left on a
row in a left-to-right layout, or up a column).

`TestDriver::accessibility_audit()` audits every open window, an untitled root named by the window's title as the AT-SPI
bridge names it and the focus the one an adapter is told (`FrameSnapshot::accessibility_focus`), and `assert_accessible(what)` fails with every window's outline (`semantic_outlines()`).

`AssistiveTechnology` is a stand-in reader for `RuntimeOptions::accessibility`: it holds each window's tree, put
together from whole and partial updates, records each live region's `Announcement` of its name when it arrives and when
it is renamed (as the AT-SPI bridge announces; a new value or description alone is not said), counts focus moves (so a test can show an announcement did not take the focus), and keeps the window focus and
the session lock. `TestDriver::ask(reader, target, action, data)` asks for an action as a reader does and wakes the
runtime to take it.

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

## Frame timing

`repaint::FrameMeter` steps a driven window one 60 Hz frame period at a time: it moves the clock on, queues the
period's input, answers the frame callbacks until the runtime is idle, and keeps a `FrameSample` (wall time from the
input to the presented frame, damaged and window pixels, rectangles, whether the frame was whole) of each frame it
presents. `Summary::of` gives the nearest-rank p50, p95, p99 and maximum, the frames over `FRAME_BUDGET` (8 ms) and
the whole frames, never an average alone, and `markdown_row` and `csv` write them out.
`HeadlessWindowSystem::set_keep_pixels(false)` stops presented frames from keeping a copy of all their pixels: only
each frame's damage is copied, into one buffer per window, as a client copies a frame into its shared buffer, so a
timing run does not count a whole-window copy that only a test makes. The Files and Settings repaint reports use both
(see their READMEs). Wall time depends on the machine: a test that pins what a frame repaints reads the damage of
`HeadlessWindowSystem::frames`.

A `HeadlessWindowSystem::new` with `Capabilities::raised_layer` takes raised layers: `layers(window)` lists every raised
layer presented (`None` where it was hidden) and `composited(window)` gives the last frame's pixels with the layer
drawn over them, as a compositor shows them without the materials between the two.

The runtime's waker does not dispatch anything in a headless window system, so `run_until_idle` never blocks.
`HeadlessWindowSystem::wait_for_wake(timeout)` blocks until a task finishing on another thread (a listing, a
thumbnail, a file read) wakes it, or the timeout passes, which only guards against a hang. A wake since the last wait
counts, so none is lost between draining the runtime and waiting: a test drains, checks its condition, and waits only
when it does not hold yet.
