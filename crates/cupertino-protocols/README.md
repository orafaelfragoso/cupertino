# cupertino-protocols

The private Wayland protocols between the Cupertino server and its shell processes, with generated bindings:
`protocols/cupertino-shell-v1.xml` and `protocols/cupertino-screenshot-v1.xml`. The public protocols any client may bind
live in `cupertino-app-protocols` (MPL-2.0); `client::{app_effects, scroll}` and `server::{app_effects, scroll}` re-export
them, so the server names every protocol through this crate. Depends on `cupertino-app-protocols`.

**Why the roles stay in `cupertino_shell_v1`.** The private interfaces keep their per-role objects (menu bar, dock, window
manager, session) under one factory instead of a new `cupertino_shell_control_v1`: the objects already separate the
roles, a rename would change every shell process's wire code without adding a check, and the server's authorisation does
not live in the interface name but in the connection (`cupertino-server`, `authorization.rs`). What changed is that the
global is hidden from every connection without a shell role, each `get_*` and each request on a role object checks the
role again, and what applications use moved out to a public protocol of its own.

## Public API

| Interface | Role |
|---|---|
| `cupertino_shell_v1` | the private role factory (version 12; version 12 adds `engage_help` and the `help_search` bound action to the menu bar; version 11 adds `edit` to the menu bar; version 10 adds the window list, the bound chords and window commands 7–15 to the menu bar; version 9 adds `log_out_requested` to the session object; version 8 adds `session_agent` to the menu bar; version 7 adds `lock_requested` to the session object; version 6 adds `session_action` to the menu bar), offered only to the processes the server started for a shell role: `get_menu_bar`, `get_dock`, `get_session` and `get_window_manager` hand out the objects below. Each request needs the role it creates (`ShellRole` in `cupertino-server`); a connection that lacks it gets the error `permission` and an inert object. Version 5 removed `get_backdrop` and `set_titlebar_style`, which moved to the public `cupertino_app_effects_v1` |
| `cupertino_menu_bar_v1` | the menu-bar role of a layer surface: active application and menu feed, `set_menu_open`, `window_command`, `move_to_workspace`; from version 4 `session_action` (an entry of the session menu: sleep, restart, shut down, lock, log out, switch user, refresh capabilities; the server forwards it to the session agent, which answers only the server; an unknown value is the error `invalid_session_action`); from version 3 `screen_capture` (a capture session runs); from version 5 the event `session_agent` (which process is the session agent: none, a process id, or any owner in a development run; the bar reads the session menu's state only from the owner of the agent's bus name when it is that process); from shell version 10 the window list (`app_window`, titles cut to 1024 bytes) and `activate_window` (resolved when it arrives, ignored while the session is locked), the bound chords (`shortcut`, `primary_modifier`) and the `window_command` entries 7–15 (full screen, tiling, bring all to front, cascade, minimize all, zoom all, force quit dialog); from shell version 11 `edit` (undo, redo, cut, copy, paste, select all: taken only while this bar's popup grab is active, sent once to the key window on the first keyboard focus change after the grab ends if it lands there within 500 ms, every guard checked again before sending; a terminal gets only copy and paste as Shift+Ctrl, a first-party or opted-out client the profile's primary modifier, any other Ctrl; never an X11 window, a lock, layer or shell surface, nor during a lock, the overview, the switcher or another grab; an unknown value is the error `invalid_edit_action`); from shell version 12 the event `engage_help` (the Help search shortcut: open the Help menu with its search field focused, taking the keyboard as for `engage`; sent in a group of its own to the bar of the active output, never while the session is locked or a lock is pending) and the `bound_action` entry `help_search`, whose chord comes in the shortcut group |
| `cupertino_dock_v1` | the dock role: window feed, `activate_app`, `restore_window`, `set_minimize_target`; the `toggle_app_grid` event (the dock opens or closes its application grid); from shell version 4 the `unresponsive` and `force_quit_dialog` events and the `force_quit_app` request; the application switcher's `switcher_output`, `switcher_entry`, `switcher_changed` and `switcher_closed` events and the `switcher_select`, `switcher_activate` and `switcher_cancel` requests; the `attention` event for a refused activation; the overview's `overview_window`, `overview_workspace`, `overview_changed` and `overview_closed` events and the `overview_frame`, `overview_select`, `overview_close`, `switch_workspace`, `add_workspace`, `remove_workspace` and `move_workspace` requests; the `hot_corner` and `gesture` events with the `run_action` request; `app_command` for the tile menus |
| `cupertino_session_v1` | the session observer of the shell processes with the Session role (`lock_requested(explicit)` from shell version 7, sent only to the connection that also holds the session lock role, the server's private way to ask the lock screen to lock; `log_out_requested` from shell version 9, the same way for the log out confirmation; the appearance observer plus the lock state): `appearance` (dark or light), `display_options` (version 2: the `display_option` bitfield of reduce motion, reduce transparency, increase contrast and differentiate without colour) and `locked` / `unlocked`, sent on creation and on every change. an object the shell global creates has the version the global was bound at, and the `since` of each of its messages is the shell version that added it (the server sends nothing newer than that) |
| `cupertino_window_manager_v1` | the window manager of an output (`get_window_manager`, no surface): the usable area and the arranged windows, `toggle_stage`, `strip_pressed`; `set_placement` (stage, strip rectangle, hidden) applied together on `commit`, `activate_window`. The server animates the placements and puts every window back when the object goes away |

## How it works

The crate has no code of its own. `build.rs` reruns when the XML changes, because the scanner macros read it untracked.

| Feature | Generates |
|---|---|
| `client` | `cupertino_protocols::client`, used by `cupertino-shell-client`, `cupertino-screenshot` and `cupertino-windowmanager` (which has no surface, so no toolkit) |
| `server` | `cupertino_protocols::server`, used by `cupertino-server` |

## Tests

The crate has no tests of its own. `tests/protocol-flow.rs` in `cupertino-shell-client` and `tests/app_effects_v1.rs`, `tests/shell_roles.rs`,
`tests/menu_bar_v1.rs`, `tests/dock_v1.rs`, `tests/session_v1.rs`, `tests/window_manager_v1.rs` and `tests/scroll_v1.rs` in `cupertino-server`
exercise the bindings:

```
cargo build -p cupertino-protocols --features client,server
cargo test -p cupertino-shell-client
cargo test -p cupertino-server
```
