# cupertino-ui-test

CupertinoKit test harness: `TestDriver<Application>`, `HeadlessWindowSystem`, event `Trace` record and replay,
the `Inspector`'s deterministic dumps, and `semantic_gaps`, which lists every node of an accessibility tree a screen
reader could not announce (no name, or a child the tree does not hold). MPL-2.0; no display or session bus is needed.

`TestDriver::new` owns the production runtime and uses the supplied `ManualClock` for both runtime work and trace
receipt timestamps, replacing `RuntimeOptions::clock`. Use `runtime()` to inspect the model, windows, focus and
snapshots, and `window_system_mut()` to queue input, configure windows, control frame callbacks and inspect presentation.
`advance_to` rejects backwards time. `run_until_idle(max_turns)` never advances time or waits: it reports waiting work
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
