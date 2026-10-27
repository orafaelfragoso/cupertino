# cupertino-protocol-app

The public protocols of the Cupertino server (`cupertino-app-protocols`) as extensions on the transport's queue
(`cupertino-wayland-client`), for any application. MPL-2.0; it never reaches the private protocols or the shell client
(`cupertino-layers` denies `cupertino-protocols` and `cupertino-shell-client`). Depends on `cupertino-wayland-client`,
`cupertino-app-protocols` (feature `client`), `cupertino-defaults` and `cupertino-geometry`.

## Public API

| Item | Contents |
|---|---|
| `AppEffects` | `cupertino_app_effects_v1`: `bind`, `appearance` (an `Appearance` observer), `backdrop(surface)`, `unify_titlebar(surface)` (before the surface's first buffer), `menu(surface)` (an `AppMenu`; `None` from a version 1 server) |
| `AppMenu`, `MenuAddress` | `cupertino_app_menu_v1`: `set_address` announces where a window's menus are on the session bus; dropping it withdraws them |
| `Backdrop` | a server-rendered material below a surface: `set_material`, `set_shape` (rounded to whole points), `set_alpha`; dropping it removes it with the next commit |
| `ScrollCounts`, `PointerScroll` | `cupertino_scroll_v1`: `bind`, `watch(pointer)` |
| `AppEvent` | what the objects receive, inside the transport's `Event::Extension` and in order with its events: `Appearance`, `DisplayOptions` (`cupertino_defaults::accessibility::DisplayOptions`), `ScrollCount` (before the frame's scroll), `MenuBar` (whether the menu bar shows a window's menus) |
| `report_appearance`, `report_display_options` | report wire values as `AppEvent`s, for another observer of the same state (the shell's session object) |
| `Error` | `InvalidShape`, `InvalidAlpha` |

An appearance other than 0 or 1, display options with reserved bits, a scroll count of an unknown axis and a menu bar
state other than 0 or 1 fail the connection with a protocol error.

## Tests

```
cargo test -p cupertino-protocol-app
```

Unit tests cover the backdrop shape. `tests/protocol-flow.rs` runs every object against an in-process server,
`tests/menu.rs` the menu announcement.
