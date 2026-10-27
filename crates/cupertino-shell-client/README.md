# cupertino-shell-client

The privileged protocols of the shell processes as extensions on the transport's queue (`cupertino-wayland-client`).
GPL-3.0-or-later. Depends on `cupertino-wayland-client`, `cupertino-protocol-app`, `cupertino-protocols` (feature
`client`), `cupertino-geometry`, `wayland-protocols` and `wayland-protocols-wlr`.

## Public API

| Item | Contents |
|---|---|
| `LayerShell`, `LayerSurfaceConfig`, `Margin`, `LayerSurface`, `Layer`, `Anchor`, `KeyboardInteractivity` | `zwlr_layer_shell_v1`: `bind`, `layer_surface(config)` returns the transport's surface and its layer surface; popups of it work through the transport |
| `SessionLockManager`, `SessionLock` | `ext_session_lock_v1`: `lock` numbers each lock (`generation`), `lock_surface(output)`, `unlock` once confirmed, else `destroy` |
| `IdleNotifier`, `IdleWatch` | `ext_idle_notify_v1`: `watch(seat, millis)`; dropping it stops the events |
| `Shell`, `Session` | `cupertino_shell_v1` (version 9): `session` observes the session, `menu_bar` and `dock` give a layer surface the role of an output |
| `MenuBar`, `MenuBarUpdate`, `ActiveApp`, `WindowCommand`, `WindowMenu`, `WindowState`, `GtkMenu`, `SessionAction`, `SessionAgent` | the menu-bar role, with `done`-grouped updates |
| `Dock`, `DockUpdate`, `DockChange`, `DockWindow`, `OverviewMode`, `OverviewWindow`, `OverviewWorkspace`, `Corner`, `Gesture`, `AppCommand` | the dock role, with `done`-grouped updates; the switcher's output arrives as the transport's `OutputId` |
| `ShellEvent` | what the objects receive, inside the transport's `Event::Extension` and in order with its events: configures of layer and lock surfaces (acknowledged by the transport), `Closed`, `Lock`, `Idle`, `SessionLock`, `LockRequested`, `LogOutRequested`, `MenuBar`, `Dock`. The session's appearance and display options arrive as `cupertino_protocol_app::AppEvent` |
| `Error` | `Transport`, `UnknownOutput`, `InvalidShape` |

## Tests

```
cargo test -p cupertino-shell-client
```

Unit tests cover the menu-bar and dock groups. `tests/protocol_flow.rs` in `cupertino-client` runs every role against
an in-process server.
