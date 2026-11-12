# cupertino-app-protocols

The public Wayland protocols of the Cupertino server, which any client may bind, with generated bindings:
`protocols/cupertino-app-effects-v1.xml` and `protocols/cupertino-scroll-v1.xml`. MPL-2.0, so applications and
toolkits link it without the private protocols; the XML itself is `MIT OR Apache-2.0`. No workspace dependencies.

## Public API

| Interface | Role |
|---|---|
| `cupertino_app_effects_v1` | the public protocol (`protocols/cupertino-app-effects-v1.xml`, generated in `client::app_effects` and `server::app_effects`), offered to every client: `get_backdrop(surface)` creates a `cupertino_backdrop_v1`, `set_titlebar_style(surface, standard \| unified \| unified_compact \| sidebar)` (the last two from version 6: the compact toolbar row, and the full-height sidebar whose top holds the window control buttons; in every style but `standard` the client draws the row and the server only the buttons; error `window` for a surface that is not an unmapped application window), `get_appearance` creates a `cupertino_appearance_v1`, `get_menu(surface)` (version 2) creates a `cupertino_app_menu_v1` (error `menu_exists` for a second one on the surface), `set_disabled_controls(surface, window_control)` (version 4) disables the minimize and zoom controls of an application window, double-buffered with the surface's commit and changeable after map (error `window` for a surface that is not an xdg toplevel, `invalid_controls` for a bit the object's version does not define). Every request names a surface of the client that sends it. The server applies the set with the commit: the window's buttons, shortcuts and menu items follow it |
| `cupertino_backdrop_v1` | a server-rendered material below a surface: `set_material` (recipe name), `set_shape`, `set_alpha`, `set_glass(style, container, spacing)` (version 6: glass instead of a recipe; glass backdrops of a surface with the same nonzero container join into one shape blended over `spacing` points; error `invalid_glass` for an unknown style or a negative spacing; version 10 adds the style `large_panel`, regular glass with its tones pressed together for a large floating panel; version 12 adds the style `bar`, clear glass optics over a face that barely changes the scene, for a floating bar of icons), the event `tone(appearance | dark | light)` (version 7: whether the face of a small glass backdrop is dark or light, sent only when it changes, past a threshold with hysteresis; `appearance` for a shape that does not adapt or no longer does); a surface holds at most 64 and a client 128, the next `get_backdrop` is the error `too_many_backdrops` of `cupertino_app_effects_v1` (a destroyed backdrop gives its place back) |
| `cupertino_app_menu_v1` | a window's menu announcement (version 2): `set_address` names the unique bus name and object paths of its `org.gtk.Menus` and `org.gtk.Actions` objects (error `invalid_address` for a malformed one), which the menu bar reads; `menu_bar(shown)` on creation and whenever a menu bar starts or stops showing menus; from version 3 `menu_reader(bus_name)`, the unique name of the verified menu bar connection, the only peer the client answers (empty while there is none); destroying it withdraws the address |
| `cupertino_appearance_v1` | the appearance observer of any client: `appearance` (dark or light) and `display_options` (the bitfield of reduce motion, reduce transparency, increase contrast and differentiate without colour, and from version 8 always show scroll bars: tracks beside the content that stay shown, decided by the server from the scroll bar setting and, for an automatic one, whether a mouse is connected; from version 9 steady caret: text fields show the caret without blinking), sent on creation and on every change; from version 5 (the version of the `cupertino_app_effects_v1` that created it) also `session_lock` (1 from the moment the session starts to lock, 0 once it is unlocked), advice to keep quiet for assistive technology behind the lock |
| `cupertino_scroll_v1` | the scroll count (`protocols/cupertino-scroll-v1.xml`): the global's `get_scroll(wl_pointer)` creates a `cupertino_pointer_scroll_v1`, whose `scroll_count(axis, count)` event arrives after the `wl_pointer.axis` of that axis and before the `wl_pointer.frame`. The axis is the `wl_pointer.axis` enum; wheels send no event (a count of 0). Generated in `client::scroll` and `server::scroll` |

## How it works

The crate has no code of its own. `build.rs` reruns when the XML changes, because the scanner macros read it untracked.
`cupertino-protocols` re-exports both modules, so `cupertino-server` names them as before.

| Feature | Generates |
|---|---|
| `client` | `cupertino_app_protocols::client::{app_effects, scroll}`, used by `cupertino-protocol-app` |
| `server` | `cupertino_app_protocols::server::{app_effects, scroll}`, used by `cupertino-server` through `cupertino-protocols` |

## Tests

The crate has no tests of its own. `tests/app_effects_v1.rs` and `tests/scroll_v1.rs` in `cupertino-server` and
`tests/protocol-flow.rs` in `cupertino-protocol-app` exercise the bindings:

```
cargo build -p cupertino-app-protocols --features client,server
```
