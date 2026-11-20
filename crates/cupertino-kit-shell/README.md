# cupertino-kit-shell

The shell's extension to CupertinoKit (`shellkit` layer). Exposes zero Wayland or raw file-descriptor types in its public API.

## Modules

| Module | Contents |
|---|---|
| `context` | Component context (`Context`), role handles, and surface/window registration hooks |
| `outputs` | Display output registry and live geometry snapshots |
| `design` | Shared shell design metrics and tokens |
| `session` | Session lock, appearance, and power/session events |
| `host` | Host connection, component registration, bus-identity nonce announcement, liveness reporting, and holds |
| `menu_bar` | Menu bar surface override, wallpaper contrast/luma feed, and active-app/menu state |
| `dock` | Dock tile geometry, running applications, badges, gestures, and switcher hooks |
| `window_manager` | Window/workspace/stage placements and bracketed commits |
| `overview` | Overview transition state and gesture bindings |
| `lock` | Session-lock surface bindings |
| `consent` | Desktop-control and portal consent authority handles |
| `capture` | Screen/window capture helpers |
| `transaction` | Cross-component compositor transaction handles, deadlines, and window/workspace holes |
| `bus` | Host-internal retained pub/sub bus (`Message`, `Sender`) and D20 bus-caller resolution (`caller` via `cupertino-peer`) |
| `hosting` | Shell host runtime (`Host`, `Runtime`, `Running`, `EXIT_VERSION`); restricted by `cupertino-layers` to `cupertino-shell` only |
| `test_clock` | Clocks for component rendering, including the debug build's fixed display clock |

The [shell host specification](../../docs/architecture/shell-host.md#cupertino-kit-shell) describes the full component interface.
