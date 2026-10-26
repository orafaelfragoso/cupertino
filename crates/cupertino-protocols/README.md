# cupertino-protocols

Private Wayland protocols between the Cupertino server and its shell processes, with generated bindings.
Three protocols: `protocols/cupertino-shell-v1.xml`, `protocols/cupertino-screenshot-v1.xml` and
`protocols/cupertino-scroll-v1.xml`. No workspace dependencies.

## Public API

| Interface | Role |
|---|---|
| `cupertino_shell_v1` | the global; `get_backdrop`, `get_menu_bar`, `get_dock`, `get_session` and `get_window_manager` hand out the objects below; version 4 adds `set_titlebar_style(surface, standard \| unified)`: a unified window's surface fills the whole window and the server draws only the window control buttons over it (error `window` for a surface that is not an unmapped application window) |
| `cupertino_backdrop_v1` | a server-rendered material below a surface: `set_material` (recipe name), `set_shape`, `set_alpha` |
| `cupertino_menu_bar_v1` | the menu-bar role of a layer surface: active application and menu feed, `set_menu_open`, `window_command`, `move_to_workspace`; from version 3 `screen_capture` (a capture session runs) |
| `cupertino_dock_v1` | the dock role: window feed, `activate_app`, `restore_window`, `set_minimize_target`; the `toggle_app_grid` event (the dock opens or closes its application grid); the application switcher's `switcher_output`, `switcher_entry`, `switcher_changed` and `switcher_closed` events and the `switcher_select`, `switcher_activate` and `switcher_cancel` requests; the `attention` event for a refused activation; the overview's `overview_window`, `overview_workspace`, `overview_changed` and `overview_closed` events and the `overview_frame`, `overview_select`, `overview_close`, `switch_workspace`, `add_workspace`, `remove_workspace` and `move_workspace` requests; the `hot_corner` and `gesture` events with the `run_action` request; `app_command` for the tile menus |
| `cupertino_session_v1` | the session observer: `appearance` (dark or light), `display_options` (version 2: the `display_option` bitfield of reduce motion, reduce transparency, increase contrast and differentiate without colour) and `locked` / `unlocked`, sent on creation and on every change. `cupertino_shell_v1` is at version 3 so that a session object can be version 2 and a menu bar version 3 |
| `cupertino_window_manager_v1` | the window manager of an output (`get_window_manager`, no surface): the usable area and the arranged windows, `toggle_stage`, `strip_pressed`; `set_placement` (stage, strip rectangle, hidden) applied together on `commit`, `activate_window`. The server animates the placements and puts every window back when the object goes away |
| `cupertino_scroll_v1` | the scroll count (`protocols/cupertino-scroll-v1.xml`): the global's `get_scroll(wl_pointer)` creates a `cupertino_pointer_scroll_v1`, whose `scroll_count(axis, count)` event arrives after the `wl_pointer.axis` of that axis and before the `wl_pointer.frame`. The axis is the `wl_pointer.axis` enum; wheels send no event (a count of 0). Generated in `client::scroll` and `server::scroll`, like `screenshot` |

## How it works

The crate has no code of its own. `build.rs` reruns when the XML changes, because the scanner macros read it untracked.

| Feature | Generates |
|---|---|
| `client` | `cupertino_protocols::client`, used by `cupertino-client` and `cupertino-windowmanager` (which has no surface, so no toolkit) |
| `server` | `cupertino_protocols::server`, used by `cupertino-server` |

## Tests

The crate has no tests of its own. `tests/protocol_flow.rs` in `cupertino-client` and `tests/shell_v1.rs`,
`tests/menu_bar_v1.rs`, `tests/dock_v1.rs`, `tests/session_v1.rs`, `tests/window_manager_v1.rs` and `tests/scroll_v1.rs` in `cupertino-server`
exercise the bindings:

```
cargo build -p cupertino-protocols --features client,server
cargo test -p cupertino-client
cargo test -p cupertino-server
```
