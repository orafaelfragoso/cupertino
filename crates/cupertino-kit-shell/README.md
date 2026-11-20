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
| `lock` | Session-lock surface bindings |
| `consent` | Desktop-control and portal consent authority handles |
| `capture` | Screen/window capture requests: `windows`, `once`, `follow` |
| `transaction` | Cross-component compositor transaction handles, deadlines, and window/workspace holes |
| `bus` | Host-internal retained pub/sub bus (`Message`, `Sender`), its bridge between hosts (hello/welcome handshake, retained replay, sender checks, `bus_dropped`), the component's session-bus connection with its D20 identity (`session`, `Event::BusIdentity`), and bus-caller resolution (`caller` via `cupertino-peer`) |
| `hosting` | Shell host runtime (`Host`, `Runtime`, `Running`, `EXIT_VERSION`): `Host::wait` returning as a component ends, the component's model kept outside the frames its loop's panic unwinds and dropped after it inside `catch_unwind` (a panic there leaks it), the component's `cupertino_component_v1` (instance, holds, bus-name announcement, `persist`, `ready`), a component refused with `Error::Version` before it starts when a global it binds is older than it needs, the loop's and the render thread's busy records (`busy_for`, `render_busy_for`), a helper that ignores its stop for `HELPER_JOIN_TIMEOUT` asking for the host's restart, the bridges' ends, and `shut` of the Wayland and bus sockets; restricted by `cupertino-layers` to `cupertino-shell` only |
| `test_clock` | Clocks for component rendering, including the debug build's fixed display clock |

A component (`Entry::of`) and a program of its own (`run`, `run_lock_screen`) run on Kit's UI loop, on the component's own
connection or the one the server gave the program: each window gets the role `Program::surface` names (layer, menu bar,
dock or lock surface, persisted before its first buffer; `ready` once the first view's persistent windows were shown),
the requests the program queued go out before the loop waits, and what the shell and the host bus say reaches
`Program::event`. Captures are `ext_image_copy_capture_v1` sessions read from shared memory.

The [shell host specification](../../docs/architecture/shell-host.md#cupertino-kit-shell) describes the full component interface.
