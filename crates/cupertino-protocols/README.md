# cupertino-protocols

Wayland protocols between the Cupertino server and its clients, with generated bindings: one public
protocol, `protocols/cupertino-app-effects-v1.xml`, and the private ones, `protocols/cupertino-shell-v1.xml`,
`protocols/cupertino-screenshot-v1.xml` and `protocols/cupertino-scroll-v1.xml`. No workspace dependencies.

**Why the roles stay in `cupertino_shell_v1`.** The private interfaces keep their per-role objects (menu bar, dock, window
manager, session) under one factory instead of a new `cupertino_shell_control_v1`: the objects already separate the
roles, a rename would change every shell process's wire code without adding a check, and the server's authorisation does
not live in the interface name but in the connection (`cupertino-server`, `authorization.rs`). What changed is that the
global is hidden from every connection without a shell role, each `get_*` and each request on a role object checks the
role again, and what applications use moved out to a public protocol of its own.

## Public API

| Interface | Role |
|---|---|
| `cupertino_shell_v1` | the private role factory (version 8; version 8 adds `session_agent` to the menu bar; version 7 adds `lock_requested` to the session object; version 6 adds `session_action` to the menu bar), offered only to the processes the server started for a shell role: `get_menu_bar`, `get_dock`, `get_session` and `get_window_manager` hand out the objects below. Each request needs the role it creates (`ShellRole` in `cupertino-server`); a connection that lacks it gets the error `permission` and an inert object. Version 5 removed `get_backdrop` and `set_titlebar_style`, which moved to the public `cupertino_app_effects_v1` |
| `cupertino_app_effects_v1` | the public protocol (`protocols/cupertino-app-effects-v1.xml`, generated in `client::app_effects` and `server::app_effects`), offered to every client: `get_backdrop(surface)` creates a `cupertino_backdrop_v1`, `set_titlebar_style(surface, standard \| unified)` (error `window` for a surface that is not an unmapped application window), `get_appearance` creates a `cupertino_appearance_v1`. Every request names a surface of the client that sends it |
| `cupertino_backdrop_v1` | a server-rendered material below a surface: `set_material` (recipe name), `set_shape`, `set_alpha`; a surface holds at most 64 and a client 128, the next `get_backdrop` is the error `too_many_backdrops` of `cupertino_app_effects_v1` (a destroyed backdrop gives its place back) |
| `cupertino_appearance_v1` | the appearance observer of any client: `appearance` (dark or light) and `display_options` (the bitfield of reduce motion, reduce transparency, increase contrast and differentiate without colour), sent on creation and on every change; no lock state |
| `cupertino_menu_bar_v1` | the menu-bar role of a layer surface: active application and menu feed, `set_menu_open`, `window_command`, `move_to_workspace`; from version 4 `session_action` (an entry of the session menu: sleep, restart, shut down, lock, log out, switch user, refresh capabilities; the server forwards it to the session agent, which answers only the server; an unknown value is the error `invalid_session_action`); from version 3 `screen_capture` (a capture session runs); from version 5 the event `session_agent` (which process is the session agent: none, a process id, or any owner in a development run; the bar reads the session menu's state only from the owner of the agent's bus name when it is that process) |
| `cupertino_dock_v1` | the dock role: window feed, `activate_app`, `restore_window`, `set_minimize_target`; the `toggle_app_grid` event (the dock opens or closes its application grid); from version 2 the `unresponsive` and `force_quit_dialog` events and the `force_quit_app` request; the application switcher's `switcher_output`, `switcher_entry`, `switcher_changed` and `switcher_closed` events and the `switcher_select`, `switcher_activate` and `switcher_cancel` requests; the `attention` event for a refused activation; the overview's `overview_window`, `overview_workspace`, `overview_changed` and `overview_closed` events and the `overview_frame`, `overview_select`, `overview_close`, `switch_workspace`, `add_workspace`, `remove_workspace` and `move_workspace` requests; the `hot_corner` and `gesture` events with the `run_action` request; `app_command` for the tile menus |
| `cupertino_session_v1` | the session observer of the shell processes with the Session role (version 3: `lock_requested(explicit)`, sent only to the connection that also holds the session lock role, the server's private way to ask the lock screen to lock; the appearance observer plus the lock state): `appearance` (dark or light), `display_options` (version 2: the `display_option` bitfield of reduce motion, reduce transparency, increase contrast and differentiate without colour) and `locked` / `unlocked`, sent on creation and on every change. a session object is version 2 and a menu bar version 3 when the shell global is bound at version 3 or newer |
| `cupertino_window_manager_v1` | the window manager of an output (`get_window_manager`, no surface): the usable area and the arranged windows, `toggle_stage`, `strip_pressed`; `set_placement` (stage, strip rectangle, hidden) applied together on `commit`, `activate_window`. The server animates the placements and puts every window back when the object goes away |
| `cupertino_scroll_v1` | the scroll count (`protocols/cupertino-scroll-v1.xml`): the global's `get_scroll(wl_pointer)` creates a `cupertino_pointer_scroll_v1`, whose `scroll_count(axis, count)` event arrives after the `wl_pointer.axis` of that axis and before the `wl_pointer.frame`. The axis is the `wl_pointer.axis` enum; wheels send no event (a count of 0). Generated in `client::scroll` and `server::scroll`, like `screenshot` |

## How it works

The crate has no code of its own. `build.rs` reruns when the XML changes, because the scanner macros read it untracked.

| Feature | Generates |
|---|---|
| `client` | `cupertino_protocols::client`, used by `cupertino-client` and `cupertino-windowmanager` (which has no surface, so no toolkit) |
| `server` | `cupertino_protocols::server`, used by `cupertino-server` |

## Tests

The crate has no tests of its own. `tests/protocol_flow.rs` in `cupertino-client` and `tests/app_effects_v1.rs`, `tests/shell_roles.rs`,
`tests/menu_bar_v1.rs`, `tests/dock_v1.rs`, `tests/session_v1.rs`, `tests/window_manager_v1.rs` and `tests/scroll_v1.rs` in `cupertino-server`
exercise the bindings:

```
cargo build -p cupertino-protocols --features client,server
cargo test -p cupertino-client
cargo test -p cupertino-server
```
