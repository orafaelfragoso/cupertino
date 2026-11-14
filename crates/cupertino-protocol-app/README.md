# cupertino-protocol-app

The public protocols of the Cupertino server (`cupertino-app-protocols`) as extensions on the transport's queue
(`cupertino-wayland-client`), for any application. MPL-2.0; it never reaches the private protocols or the shell client
(`cupertino-layers` denies `cupertino-protocols` and `cupertino-shell-client`). Depends on `cupertino-wayland-client`,
`cupertino-app-protocols` (feature `client`), `cupertino-defaults` and `cupertino-geometry`.

## Public API

| Item | Contents |
|---|---|
| `AppEffects` | `cupertino_app_effects_v1`: `bind`, `appearance` (an `Appearance` observer), `backdrop(surface)`, `unify_titlebar(surface)` and `set_window_style(conn, surface, WindowStyle)` (before the surface's first buffer; `WindowStyle::{Titled, Unified, UnifiedCompact, Sidebar}`, the last two from version 6 and, on an older server, the nearest it has: unified for the compact row, titled for the sidebar; `granted_style` tells which), `menu(surface)` (an `AppMenu`; `None` from a version 1 server), `set_disabled_controls(surface, WindowControls)` (version 4; not sent to an older server, which `can_set_disabled_controls` tells), `tells_session_lock` (version 5: the observer sends `AppEvent::SessionLock`), `draws_subsurface_backdrops` (version 11: `backdrop(subsurface)` draws directly below that subsurface, above the rest of its window, and reads what of the window lies below it; before it a subsurface's backdrops draw nothing) |
| `WindowControls` | the minimize and zoom controls of a window, each a `bool`; the close button can't be disabled |
| `AppMenu`, `MenuAddress` | `cupertino_app_menu_v1`: `set_address` announces where a window's menus are on the session bus; dropping it withdraws them |
| `Backdrop` | a server-rendered material below a surface: `set_material`, `set_shape` (rounded to whole points), `set_alpha`, `set_glass(GlassStyle, container, spacing)` (version 6: glass instead of a material; backdrops of a surface with the same nonzero container join into one shape blended over `spacing` points; a version 5 server gets `GlassStyle::fallback_material` instead; `GlassStyle::LargePanel` is sent as regular glass to a server older than version 10 `GlassStyle::Bar` as clear glass to one older than version 12 and `GlassStyle::Dense` and `GlassStyle::Module` as regular glass to one older than version 13, `GlassStyle::Card` as dense glass and `GlassStyle::Track` as clear glass to one older than version 15, `GlassStyle::Popover` as regular glass to one older than version 16, `GlassStyle::Switcher` as regular glass to one older than version 17, `GlassStyle::SubduedModule` as module glass, `GlassStyle::Veiled` as clear glass and `GlassStyle::Sidebar` and `GlassStyle::Snippet` as regular glass to one older than version 18, `GlassStyle::Toolbar` as regular glass to one older than version 19, `GlassStyle::Menu` as dense glass to one older than version 20); `id()` names it in `AppEvent::BackdropTone` and `tells_tone()` whether the server sends it (version 7); dropping it removes it with the next commit |
| `ScrollCounts`, `PointerScroll` | `cupertino_scroll_v1`: `bind`, `watch(pointer)` |
| `Layers`, `Layer`, `LayerId`, `LayerProperty`, `Fixed` | `cupertino_layers_v1` (versions 1 to 3): `bind`, `layer(surface, parent)`, and from version 2 `presentation(surface)`, a layer shell surface's own presentation whose offset, opacity and scale apply to all it draws (`presents_surfaces` tells), from version 3 a popup's too (`presents_popups` tells). `Layer::set_glass_group(group, spacing)` (version 3) joins the glass of the parent's layers of one group into one shape. A `Layer` sets a property at once (`set`), animates it along a `cupertino_animation::LayerCurve` (`animate(serial, property, value, curve, velocity)`) or stops it (`cancel`); what it asks applies with the parent's next commit, and dropping it removes it. Its events arrive as `AppEvent::LayerFinished`, `LayerReplaced` (the value and velocity shown, in wire `Fixed` units) and `LayerSettled` |
| `AppEvent` | what the objects receive, inside the transport's `Event::Extension` and in order with its events: `Appearance`, `DisplayOptions` (`cupertino_defaults::accessibility::DisplayOptions`), `ScrollBars` (version 8, after each display options of an appearance observer: whether scroll views keep their tracks shown beside the content), `SteadyCaret` (version 9, after the `ScrollBars` of each display options: whether text carets stay shown without blinking), `SessionLock` (version 5: whether the session is locked, after the display options and on every change), `BuffersLost` (version 21, `AppEffects::tells_buffers_lost`: the server's GPU was reset or purged and what it showed of the client's buffers is gone), `ScrollCount` (before the frame's scroll), `MenuBar` (whether the menu bar shows a window's menus), `MenuReader` (version 3: the bus connection the window's menus answer, for `cupertino_ui_menus::Exporter::set_reader`), `BackdropTone` (version 7: a glass backdrop's `BackdropTone::{Appearance, Dark, Light}`, which picks the colour of text over it) |
| `report_appearance`, `report_display_options` | report wire values as `AppEvent`s, for another observer of the same state (the shell's session object) |
| `Error` | `InvalidShape`, `InvalidAlpha`, `InvalidGlass`, `InvalidLayerValue` |

An appearance other than 0 or 1, display options with reserved bits, a scroll count of an unknown axis, a menu bar
state other than 0 or 1 and a menu reader that is not a unique bus name fail the connection with a protocol error.

`set_disabled_controls` replaces the window's disabled set, applies with the surface's next commit and may change at any
time. The server applies it with that commit: the buttons are dimmed, and the shortcuts, the Window menu and the client's
own `set_minimized` / `set_maximized` do nothing for a disabled control (unzooming a zoomed window still works).

## Tests

```
cargo test -p cupertino-protocol-app
```

Unit tests cover the backdrop shape. `tests/protocol-flow.rs` runs every object against an in-process server
(`set_disabled_controls` reaches a version 4 server and not a version 3 one), `tests/menu.rs` the menu announcement.
