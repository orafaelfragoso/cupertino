# cupertino-kit-shell

The shell's extension to CupertinoKit (`shellkit` layer). Exposes zero Wayland or raw file-descriptor types in its public API.

## Modules

| Module | Contents |
|---|---|
| `context` | Component context (`Context`), role handles, and surface/window registration hooks |
| `outputs` | Display output registry and live geometry snapshots |
| `design` | The menu bar's values from the theme data (shapes, typography, cap height) |
| `session` | Session lock, appearance, and power/session events |
| `host` | Helper threads (`thread`, `Stop`; a helper's panic fails its component) and detached children (`spawn`, reaped on their pidfds) |
| `menu_bar` | Menu bar surface override, wallpaper contrast/luma feed, and active-app/menu state |
| `dock` | Dock tile geometry, running applications, badges, gestures, and switcher hooks |
| `window_manager` | Window/workspace/stage placements and bracketed commits |
| `overview` | Overview transition state and gesture bindings |
| `keyboard_panels` | The input-source switcher's list and the accent picker's candidates the server sends the on-screen displays (`Event::KeyboardPanels`, namespaces of their layer surfaces), and `accent_pick`; the object is asked for once the window system routes the component's events, so a list sent at once is heard |
| `lock` | Session-lock surface bindings |
| `consent` | Desktop-control and portal consent authority handles |
| `capture` | Screen/window capture requests: `windows`, `once`, `follow` |
| `transaction` | Cross-component compositor transaction handles, deadlines, and window/workspace holes |
| `bus` | Host-internal retained pub/sub bus (`Message`, `Sender`), its bridge between hosts (hello/welcome handshake, retained replay, sender checks, `bus_dropped`), the component's session-bus connection with its D20 identity (`session`, `Event::BusIdentity`), and bus-caller resolution (`caller` via `cupertino-peer`) |
| `hosting` | Shell host runtime (`Host`, `Runtime`, `Running`, `EXIT_VERSION`): `Host::wait` returning as a component ends, the component's model kept outside the frames its loop's panic unwinds and dropped after it inside `catch_unwind` (a panic there leaks it), the component's `cupertino_component_v1` (instance, holds, bus-name announcement, `persist`, `ready`), a component refused with `Error::Version` before it starts when a global of `cupertino-protocols` it binds is offered at another version than the crate was built with, or the layer shell older than the components work with, the loop's and the render thread's busy records (`busy_for`, `turns`, `render_busy_for`, `render_done`), a loop or the render thread leaving its wait ending a wait with no timeout after `Host::watch`, a loop failing after its host asked it to stop counted as a stop, a helper that ignores its stop for `HELPER_JOIN_TIMEOUT` asking for the host's restart, the bridges' ends, and `shut` of the Wayland and bus sockets; a failing component's Wayland socket is shut as it fails (a panic on its thread or a helper thread, through a panic hook `Runtime::for_host` chains, or a loop that ends unasked, as the runtime drops its model before its window system), so its teardown sends the server nothing (its bus sockets are shut as the host reads the failure) and the server holds its surfaces from the connection's end, while a component its host stops keeps its orderly teardown; restricted by `cupertino-layers` to `cupertino-shell` only |
| `crash` (private, development builds only) | `org.cupertino.ShellComponentDebug1` at `/org/cupertino/ShellComponentDebug1` on each component's session bus connection (`bus::session`), served before the connection's name is announced, so a caller that finds the component by its accepted name is answered: `Crash(at)` makes the component panic as its loop dispatches, as it paints or on a helper thread, for `cupertino shell crash` |
| `test_clock` | Clocks for component rendering, including the debug build's fixed display clock |

A component (`Entry::of`) and a program of its own (`run`, `run_lock_screen`) run on Kit's UI loop, on the component's own
connection or the one the server gave the program: each window gets the role `Program::surface` names (layer, menu bar,
dock or lock surface, persisted before its first buffer; `ready` once the first view's persistent windows were shown,
the first view being the first for which `Program::showing` holds),
the requests the program queued go out before the loop waits, and what the shell and the host bus say reaches
`Program::event`, with the size the server configured each layer and lock surface to (`Event::Configured`). Captures are `ext_image_copy_capture_v1` sessions read from shared memory on Kit's I/O pool, never on
the UI thread: a frame is cut to the capture's region (logical points of the upright source) and turned upright by the
transform the server sent it in, and a region that covers none of the source ends the capture with an error.

The [shell host specification](../../docs/architecture/shell-host.md#cupertino-kit-shell) describes the full component interface.
