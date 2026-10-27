# cupertino-client

Client toolkit of the shell processes (notifications, menu bar, launcher, dock) and of the application windows (file
manager). One `Client` connects to the server, binds the globals every process needs and owns its surfaces. It runs on the
transport, `cupertino-wayland-client`: the standard protocols are the transport's, the public ones of the desktop are
`cupertino-protocol-app`'s, the privileged ones (layer shell, session lock, idle notify, the shell roles) are
`cupertino-shell-client`'s, and every surface draws through a `cupertino-ui-wayland` presenter. This public API is an
adapter onto those crates, to be deleted once the shell processes use them directly; `cupertino-osd`,
`cupertino-notifications` and `cupertino-controlcenter` already do. Depends on them,
`cupertino-geometry`, `cupertino-defaults` and `tiny-skia`.

## Public API

| Item | Contents |
|---|---|
| `Client`, `LayerSurfaceConfig`, `Margin`, `SurfaceId`, `Waker` | connection, layer surfaces on a chosen output (`set_layer_placement` moves or resizes one, `set_exclusive_zone` changes its zone; both apply with the next draw), popups of them with a seat grab |
| `Client::toplevel`, `ToplevelConfig`, `Client::{set_title, toplevel_activated} | an xdg toplevel (application window) with server-side decorations: app id, title, initial and minimum size; a configure resizes it, `Event::Closed` is the close request, scrolling over it is `PointerEvent::Scroll` as on any surface |
| `Client::{set_clipboard_text, clipboard_text}` | the text clipboard (`wl_data_device`, optional at connect): copy offers `text/plain;charset=utf-8`, `text/plain` and `UTF8_STRING` with the serial of the latest input event, served by the transport; a paste returns the client's own copy directly or reads another client's selection within the transport's limits, blocking the caller for up to 1 s (`None` when the other client does not finish in time) |
| `Event`, `PointerEvent`, `Button`, `key`, `Modifiers`, `ActivationRequest` | pointer and key events, `Client::modifiers` for the held modifiers; `PointerEvent::Scroll` carries `count`, the touchpad's scroll count from `cupertino_scroll_v1` (optional at connect; 0 for a wheel, a slow scroll or a server without the global). With the global bound, scroll events wait for the `wl_pointer.frame` that brings their count and are queued then, in order; the transport translates the keymap and runs key repeat |
| `CursorLook`, `Client::set_cursor`, `Client::input_serial` | the pointer's look over the surface it entered last: hidden (the process draws its own) or the server's arrow (`wp_cursor_shape_v1`, optional at connect); the serial of the latest input event |
| `OutputInfo`, `OutputGeometry`, `OutputId`, `Scale` | outputs with logical geometry and the buffer scale; `Client::surface_outputs` lists the outputs the server entered a surface into, which is how a layer surface created without an output learns the one the server chose; `OutputId::registry_name` binds the same output on another event queue |
| `Backdrop` | a server-rendered material below a surface (`cupertino_backdrop_v1` of the public `cupertino_app_effects_v1`) |
| `ToplevelConfig::titlebar`, `TitlebarStyle`, `Client::toplevel_move`, `Client::toplevel_toggle_maximized` | the title bar style of an application window (`cupertino_app_effects_v1.set_titlebar_style`): with `Unified` the client draws the toolbar strip and title and moves / zooms the window itself |
| `Client::connect_shell`, `Client::from_shell_connection` | the connection of a shell process the server started (`WAYLAND_SOCKET`): binds the private `cupertino_shell_v1` (version 5) in addition. `Client::menu_bar` and `Client::dock` need it and fail with `ClientError::MissingGlobal("cupertino_shell_v1")` on a `Client::connect` connection, which binds no private object; the server also gives the roles only to a process whose `"roles"` entry lists them |
| `MenuBar`, `MenuBarUpdate`, `ActiveApp`, `WindowCommand`, `WindowMenu`, `WindowState`, `GtkMenu` | the menu-bar role, with `done`-grouped updates (from version 3 also the screen capture state, `MenuBarUpdate::screen_capture`) |
| `Dock`, `DockUpdate`, `DockChange`, `DockWindow`, `OverviewMode`, `OverviewWindow`, `OverviewWorkspace`, `Corner`, `Gesture`, `AppCommand` | the dock role: window feed, activation, restore, minimize target, the app-grid action (`DockChange::ToggleAppGrid`), the application switcher (`DockChange::SwitcherChanged` with the panel's `OutputId` / `SwitcherClosed`, `Dock::switcher_select`, `switcher_activate`, `switcher_cancel`), attention (`DockChange::Attention`), the overview (`DockChange::OverviewChanged` / `OverviewClosed`; `Dock::overview_frame`, `overview_select`, `overview_close`), workspaces (`switch_workspace`, `add_workspace`, `remove_workspace`, `move_workspace`), hot corners and gestures (`DockChange::HotCorner` / `Gesture`, `Dock::run_action`) and tile menu commands (`Dock::app_command`) |
| `Client::is_dark`, `Client::display_options`, `Client::session_locked`, `Event::Appearance`, `Event::DisplayOptions`, `Event::SessionLock` | the session state, read before `connect` returns: on a public connection from `cupertino_appearance_v1` (appearance and display options; `session_locked` stays false), on a shell connection (`Client::connect_shell`) from `cupertino_session_v1` version 2, which adds the lock state. The state at connect is read before `connect` returns, changes arrive as events. `DisplayOptions` is `cupertino_defaults::accessibility::DisplayOptions`; options with reserved bits are a protocol error |
| `Client::from_connection`, `Connection` | the client over an established `wayland_client::Connection`, which the process may share with event queues of its own |
| `rgba_to_argb8888`, `ClientError` | the pixel conversion into shm buffers, and errors |

## How it works

Drawing is tiny-skia on a `Pixmap` at the surface's device scale, presented through the transport's buffer leases and
paced by frame callbacks by the surface's `cupertino_ui_wayland::ShmPresenter`; `Event::Draw` waits for a free buffer. `Client::draw` paints a whole frame;
`Client::draw_damage` repaints only rectangles of the last one (cleared for the paint, rounded out to device pixels):
only they and what the leased buffer missed of the frames shown since it last held one (`BufferLease::missed`) are
converted into it, and only they are reported with `damage_buffer`, so the server uploads only them. Rectangles that add
up to the buffer convert it once, whole. A frame at another size or scale is drawn whole. A process runs one loop around `Client::wait` and reacts to
the returned `Event`s. Other threads wake the loop with a `Waker`.

A missing required global is an error at `Client::connect`, never a degraded mode. The required globals are layer shell,
`cupertino_app_effects_v1` (`cupertino_shell_v1` too on `Client::connect_shell`), `xdg_activation_v1`, `zxdg_output_manager_v1`, shm and a seat. Fractional scale is used with
viewporter when offered. `zxdg_decoration_manager_v1` is optional at connect, but `Client::toplevel` errors without it, and
a server that refuses server-side decorations fails the client with `ClientError::Protocol` (the toolkit draws no title bar).

## Tests

```
cargo test -p cupertino-client
```

`tests/protocol_flow.rs` runs a minimal protocol server in-process, so no display is needed. It covers layer surfaces,
popups, the roles, the session observer and a decorated toplevel (configure, resize, scroll with its count, close).
`cupertino-server`'s `tests/files_window.rs` runs a toplevel client (the file manager) against the real server.
