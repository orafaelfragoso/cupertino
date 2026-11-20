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
| `hosting` | Shell host runtime (`Host`, `Runtime`, `Running`, `EXIT_VERSION`): the component's `cupertino_component_v1` (instance, holds, bus-name announcement), the bridges' ends, and `shut` of the Wayland and bus sockets; restricted by `cupertino-layers` to `cupertino-shell` only |
| `test_clock` | Clocks for component rendering, including the debug build's fixed display clock |

The [shell host specification](../../docs/architecture/shell-host.md#cupertino-kit-shell) describes the full component interface.
