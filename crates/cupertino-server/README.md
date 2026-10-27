# cupertino-server

The Smithay 0.7 Wayland compositor of Cupertino, as a library (`cupertino_server`) and a binary (`cupertino-server`). This is its design, for people changing the compositor or the shell processes that talk to it: command line, architecture, configuration files, protocols, tests and open issues.

Window policy comes from `cupertino-wm`. Server-side decorations, window effects and input handling come from `cupertino-decorations`, `cupertino-effects`, `cupertino-input` and `cupertino-keyboard`. Shadows, rounded corners and material passes run as GLES shaders. The overview is driven by `cupertino-overview`. The notifications, menu bar, launcher and dock run as separate Wayland clients that the server starts and restarts (*Shell processes*).

Contents: [Run it](#run-it), [Command line](#command-line), [Architecture](#architecture) (window policy, rendering, material pass, protocols, clipboard, displays, session bus, shell processes, accessibility, keybindings and configuration files, keyboard, launcher, overlays, screenshots, notifications, idle, log out), [Integration points](#integration-points), [Tests](#tests), [Open issues and approximations](#open-issues-and-approximations).

## Run it

```bash
cargo run -p cupertino-server -- --winit                       # nested window (development)
cargo run -p cupertino-server -- --udev                        # TTY: logind + DRM/KMS + evdev
cargo run -p cupertino-server -- --headless --screenshot out.png --exit-after-screenshot
```

The server and every process that draws text or icons read the asset packs of `cupertino-assets`: `CUPERTINO_ASSET_DIR` first, then the user and system packs, then the built-in fallback; a missing pack or file never stops a start. The server's text (title bars, HUD, accent popup) and cursors come from them, and the processes it starts inherit the variable.

The server's text (title bars through `Decorations`, the input-source HUD and the accent popup through `OverlayText`) is drawn with the optical size of the output's scale: both set it from the scale they draw at.

The compositor prints `WAYLAND_DISPLAY=<socket>` once it listens. Start clients with that variable. `Super+Return` starts a terminal (`foot`, `alacritty`, `kitty`, `wezterm` or `xterm`, whichever exists first; configurable with `terminals`).

## Command line

Without a backend flag the server runs nested when `WAYLAND_DISPLAY` or `DISPLAY` is set, else on the TTY. Giving more than one of `--winit`, `--udev` and `--headless` is an error.

| flag | default | effect |
|---|---|---|
| `--winit` / `--udev` / `--headless` | nested if a session display exists, else TTY | backend |
| `--size WxH` | `1280x800` | nested window or headless output size |
| `--scale F` | host scale (nested), 1 otherwise | output scale |
| `--appearance light\|dark` | from the configuration | material and shadow appearance |
| `--spawn "cmd args" (repeat) | | start a client after startup on the compositor's socket |
| `--spawn-delay S` | `0.5` | seconds before the first `--spawn` command |
| `--pointer S:X,Y`, `--click S:X,Y`, `--right-click S:X,Y` (repeat) | | scripted pointer motion, left click or right click at time S (logical coordinates) |
| `--press S:X,Y`, `--release S:X,Y` (repeat) | | scripted left press and release at time S, to script a drag with `--pointer` |
| `--modifier-down S:MOD`, `--modifier-up S:MOD` (repeat) | | hold or let go of `shift`, `ctrl`, `alt` or `super` as a real key event to the focused client (menus that show alternates while a modifier is held) |
| `--time-scale F` | 1 | run model time (animations) at F times the wall clock, for example `0.2` to catch mid-animation frames |
| `--dbus` | off | start the session bus worker also on the headless backend (it always starts on the nested and TTY backends) |
| `--action S:NAME` (repeat) | | run a named action at time S (`overview`, `minimize`, `workspace-right`, `launcher`, `input-source-next`, `notification-center`, ...) |
| `--key S:CHORD` (repeat) | | press and release a chord at time S (`1:Super+space`, `2:Return`). It goes through the HUD / accent / key router, so a scripted power key is a tap, which the login process ignores. A key none of them takes reaches the focused client as real key events. |
| `--type S:TEXT` (repeat) | | type text at time S into the focused client, for example the launcher panel when it is open |
| `--accent S:CHAR` (repeat) | | open the accent picker for a character at time S, as a held key would after its repeat delay |
| `--hold-modifiers` | off | scripted shortcuts behave as if their modifiers stayed held (the input-source HUD stays open) |
| `--screenshot PATH` | | write the rendered output as PNG |
| `--screenshot-after S` | `3.0` | seconds after start at which `--screenshot` is taken |
| `--exit-after-screenshot` | off | exit once the screenshot was written |
| `--shot S:PATH` (repeat) | | several screenshots on a timeline |
| `--run-for S` | | exit after S seconds |
| `--no-menu-bar` | off | hide the menu bar (no menu bar reservation) |

`--screenshot` renders the output offscreen through the normal element tree (cursor included), so it works on every backend. Results of the last run are in `output/` (1280x800 unless noted):

| file | content |
|---|---|
| `desktop-empty.png` | wallpaper, material menu bar with the application name and clock, dock with theme icons (headless; from before the dock became a process) |
| `windows-light.png`, `windows-dark.png` | two decorated terminals (foot): title bar with the window control buttons, rim, active vs inactive, shadow |
| `decorations-hover.png` | pointer over the button group: glyphs on all three buttons |
| `menu-open.png` | the focused client's application menu open over its window (material popup, shortcut column) |
| `minimize-mid.png`, `min-0-rest.png`, `min-1-mid.png`, `min-2-done.png` | warp minimize towards the dock tile (mesh pass); the `min-*` frames come from `tests/headless.rs` at fixed model times |
| `closing-mid.png` | closing animation: the last frame of the window fading out |
| `show-desktop.png` | windows slid off the output |
| `overview.png` | overview with live decorated window textures |
| `launcher-open.png` | the launcher panel with a typed query (top hit, section headers, icons, selection), top edge at one fifth of the height |
| `notification-banners.png` | the newest of three banners sent with `gdbus` to the notification process, below the menu bar |
| `notification-centre.png` | the notification centre list (history cards) after the banners expired |
| `input-switcher.png` | the input-source HUD (most recently used order, selection on the previous source) |
| `accent-picker.png` | the accent popup over a terminal, anchored at the text cursor reported by its text input, focus on the second candidate |
| `nested-winit.png`, `nested-winit-overview.png` | the nested backend inside the host session (tiled to 1900x1034 by the host, exercising `SetOutputRect`) |

Full-desktop screenshots with a fixture home folder come from `scripts/capture.sh OUT.png [flags]`. The overlay screenshots are produced by `scripts/capture-overlays.sh` (deterministic: neutral fixture applications from `tests/fixtures/applications`, a private session bus, the `cupertino-notifications` process and `gdbus`, the sample accent table `tests/fixtures/accents.json`).

## Architecture

```
backend (winit | udev | headless)  ──input──▶  events.rs ─▶ input.rs (KeyRouter, Action, ActionPlan)
        │                                          │                         │
        │ frame                                    ▼                         ▼
        ▼                                     state.rs  ◀── Command/Effect ── cupertino-wm (Wm::handle / tick)
   frame.rs ── build_output_elements ──▶ render/scene.rs ── LayerStack ──▶ [CupertinoElement] ─▶ OutputDamageTracker
                                                 ▲   ▲
                                      shell.rs ──┘   └── render/{shader_rect, clip, material, ui, programs}.rs
```

| module | role |
|---|---|
| `state.rs` | `Cupertino`: Smithay states, `Wm`, outputs, windows, shell; effect application, spawn, hit testing |
| `handlers.rs` | protocol handlers: compositor, xdg-shell, decoration, dialog, layer-shell, seat, selections, dmabuf, activation, foreign toplevel list, fractional scale |
| `clipboard.rs` | the kept clipboard (copy of a client's clipboard, taken over when it exits), `Snapshot`, the data-control globals |
| `policy.rs` | pure conversions between protocol events and model types (states, resize edges); new-window placement (`Cascade`) |
| `input.rs` | key chords, `KeyBindings` (from the `cupertino-input` `ShortcutMap`), `Action`, `translate_action` (action to `Command`s and `ShellOp`s), `KeyRouter` (switcher Escape, overview Escape, suppressed releases) |
| `events.rs` | backend-independent input: keyboard (menus first), pointer (menus, decorations, shell, model interaction, client), axis, touchpad gestures through `GestureRecognizer` |
| `shell.rs`, `shell_ctl.rs` | the overview session (transition, per-window frames), usable area, the dock's overview and workspace requests, named actions, edge reveals, hot-corner forwarding |
| `decor.rs` | server-side decorations: `cupertino-decorations` title bars and rims cached as buffers, hit testing, hover and pressed state, double click |
| `fx.rs` | closing ghosts (last-frame snapshots), stored snapshots, show-desktop slide (geometry from `cupertino-effects`) |
| `kbd.rs` | `keyboard.conf`, repeat info, modifier profile translation, the merge of the verified shortcut table into the `cupertino-input` map, caps-lock debounce, input-source list and HUD model |
| `kbd_ui.rs` | keyboard integration on the compositor state (repeat info, XKB layouts, input-source switching, caps gate, accent picker flow) and the scene functions of the HUD and accent popup |
| `input_method.rs` | in-process `zwp_input_method_v2` client (thread and socket pair) and the `InputMethodHandler` that delivers accent edits to the focused client's `zwp_text_input_v3`; created only with an accent table |
| `shell_v1.rs`, `supervisor.rs`, `restart_policy.rs` | the `cupertino_shell_v1` global and its double-buffered backdrops; starting and restarting the shell processes |
| `overlays.rs` | state shared by the overlays (`Overlays`), the key event type, text drawing, per-frame work |
| `menu_bar.rs` | `cupertino_shell_v1.get_menu_bar` / `cupertino_menu_bar_v1`: bars, the `cupertino-wm::menu_bar` model, menu addresses, event feed, window commands, timers, edge reveal, screen capture state (version 3) |
| `dbus.rs` | session bus worker thread: shell toggles, media keys, lock request, power key |
| `session_lock.rs` | `ext-session-lock-v1`: `LockState`, lock surfaces per output, locked-frame bookkeeping, hit testing |
| `idle.rs` | `ext-idle-notify-v1` and `zwp-idle-inhibit-v1` |
| `display_power.rs` | the idle policy, pure on the model clock: dim, display sleep, lock, system sleep, dim factor and ramps; the fade-in after the session became active |
| `power.rs` | the idle policy on the compositor state: input activity, effects (overlay, DPMS, lock, suspend), the dim overlay element, system bus watchers (UPower `OnBattery`, logind `BlockInhibited` and `PrepareForSleep`, `Suspend`) |
| `quit.rs` | `org.cupertino.Session1`: quitting applications one at a time before a log out; the appearance and accessibility setters |
| `live.rs` | live settings: the watch on `compositor.json` (the system file and the user's), `keyboard.conf` and `power.conf`, and what a change applies |
| `protocols.rs` | KDE appmenu (`org_kde_kwin_appmenu`) and GTK shell (`gtk_shell1`, generated from `protocols/gtk-shell.xml`) globals |
| `dock.rs` | `cupertino_shell_v1.get_dock` / `cupertino_dock_v1`: window feed, `launching`, activation and restore requests, minimize target, the switcher's events (`switcher_output`, `switcher_entry`, `switcher_changed`, `switcher_closed`, `switcher_select`, `switcher_activate`, `switcher_cancel`), `attention`, `toggle_app_grid` |
| `window_manager.rs` | `cupertino_shell_v1.get_window_manager` / `cupertino_window_manager_v1`: arranged-window feed, stage placements and animation (`WindowManagers::look`), strip presses, `toggle_stage` |
| `scroll_v1.rs` | `cupertino_scroll_v1` / `cupertino_pointer_scroll_v1`: the global, the scroll objects per `wl_pointer`, `ScrollObservers::send` (the count goes to the objects of the focused client) |
| `session_v1.rs` | `cupertino_shell_v1.get_session` / `cupertino_session_v1`: observers, `set_appearance` (the one place the appearance changes at run time), lock events |
| `desktop.rs` | `.desktop` entries (the application id of a program the server starts) |
| `displays/ | heads and their state, `displays.conf` (`mod.rs`); `zwlr_output_manager_v1` (`management.rs`), `zwlr_output_power_manager_v1` (`power.rs`), EDID identity (`edid.rs`); see *Displays* |
| `render/scene.rs` | per-output element tree, `plan_window`, decorations, minimize mesh, closing ghosts, workspace slide, layer surfaces, material finalisation |
| `render/image.rs` | cache of CPU-drawn images as memory buffers (re-uploaded when their signature changes) |
| `render/mod.rs` | `CupertinoElement` (render_elements! enum), `SceneLayer`, `LayerStack` |
| `render/mesh.rs` | textured-mesh element (raw GL, scissored to damage) and offscreen `capture` of an element list |
| `render/material.rs` | GLES material pipeline (below) |
| `render/{shader_rect,clip,ui,programs}.rs` | pixel-shader rectangles with stable ids, rounded-corner mask element, rounded-rect constructor, shader sources |
| `backend/* | winit; udev (single GPU, `DrmOutputManager`, one head per connector with EDID identity and every mode; `backend/input.rs` reads evdev devices and feeds touchpads through `cupertino_input::Touchpad`); headless EGL device |
| `config.rs`, `cli.rs`, `cursor.rs`, `frame.rs` | configuration, startup, themed cursors, frame callbacks and presentation feedback |

### Window policy

All policy is delegated to `cupertino-wm`; the compositor only translates.

**New-window placement** (`policy::Cascade`, the visible frame V is the usable area, or the stage frame while a window of the output is in the strip or hidden):
* The first window of an application is centred: `x = V.x + ⌊(V.w − w)/2⌋, a quarter of the vertical slack above it, its top on V's top when it is taller. On a 1920 × 1080 output under a 24 pt menu bar an 800 × 600 window opens at (560, 138).
* Each later window of the application cascades 29 pt right and down from the point its newest window on the same output left (so (589, 167), then (618, 196)). A start that leaves V wraps: a new column at `V.x + 14.5 k`, a new row at `V.y + 14 k`, both back to V's edge once the shifted start no longer fits or (rows) starts within 4 × 29 = 116 pt of the output bottom. The two offsets are kept per application, as the design keeps them per application process.
* Child windows are centred and then placed by the model (sheets, dialogs). The server restores no saved frames.
* the application identity standing for the design's window-controller class, the active output's bottom standing for the main screen's in the 116 pt bound. A stage holding only its centre group is not seen (the protocol carries no stage state), so its new windows are placed in the usable area.

* **Protocol to `Command`**: first buffer commit → `Map` (identity from `app_id`, title, size hints, parent, modal flag from xdg-dialog, placement below). Title / hints / modal / parent updates → `SetTitle` / `SetSizeHints` / `SetModal` / `SetParent`. xdg move / resize requests → `BeginMove` / `BeginResize`, and the pointer is routed to `PointerMotion` / `PointerRelease` (`PointerMode::Model`). Maximize → `ToggleZoom`, fullscreen → `Fullscreen`, minimize → `Minimize` with the dock's minimize target. Activation → `FocusWindow` when the token rule allows it, else `DemandAttention`. Destroy / unmap → `Destroy`. Primary-modifier drags: left = move, right = resize (nearest corner).
* **Window kinds**: the kind (open / close transform, `cupertino-wm::kind`) comes from the protocol facts at map time, in the design's precedence (`window_kind`): modality (xdg-dialog-v1 `set_modal`); Wayland has no utility-window signal. The parent is `xdg_toplevel.set_parent`. The model maps a modal child as a **sheet** (hangs centred from the parent's title bar, slides down 32 pt while fading in over 0.26 s and back up on close), centres a parentless modal panel high on the usable area (cascading by 66 pt from the application's open panel), centres other children on their parent (approximate) and keeps plain windows unanimated. A modal child keeps the focus off its parent (`Wm::blocking_modal`); pointer events still reach the parent's surface.
* **`Effect` to protocol**: `Configure` → xdg configure (size minus insets, model states, activated flag). `RequestClose` → `xdg_toplevel.close`. `Focus` → keyboard focus and activated states. `SwitcherChanged/Closed` → the docks' switcher events. Window state changes → the dock's window feed. Everything else requests a redraw. Stacking, visibility, snap previews and workspace slides are read from the model at render time (`stacking_order`, `presentation`, `workspace_position`, `snap_preview`).
* **Title bar style**: `cupertino_shell_v1.set_titlebar_style` (version 4) lets a first-party client choose `WindowEntry::titlebar` before its window maps. `Unified` gives the window no insets (its surface is the whole frame); the server draws only the window control buttons (a transparent 52 pt strip, `decor.rs`) and the rim, the buttons take the pointer before the surface, and the rest of the strip belongs to the client, which moves the window with `xdg_toplevel.move`. Resize zones and the shadow are the server's. Tested in `tests/unified_titlebar.rs`.
* **Decoration negotiation**: a toplevel that binds xdg-decoration and does not ask for client-side decorations is decorated by the compositor (`WindowEntry::server_side`). Its insets (title bar height from the decoration metrics) are added to the model frame, configures carry the surface size, and content sits below the title bar. A client that requests client-side decorations, or never binds the protocol, keeps its own and gets no insets. With `decorations = false` in the configuration every window gets `ssd_insets` and no decorations.
* **Output changes**: a resized output (nested window, hot-plug mode change) sends `Command::SetOutputRect`, and the model refits full-screen, zoomed, tiled and normal windows of that output.
* **Time**: `Cupertino::now` is the monotonic presentation clock in seconds (`fixed_time` overrides it for tests, `--time-scale` scales it). `tick runs every loop iteration before rendering.
* **Animated frames** are drawn by `plan_window`. At rest the committed surface size wins (no stretch). While the model animates a window the content is scaled to the animated rectangle (frame, then minimize collapse towards the target, then presentation scale about the centre) with `RescaleRenderElement` and a mask in final coordinates.

### Rendering and element order

Bottom to top (`SceneLayer`): wallpaper, layer-shell background, layer-shell bottom, overview backdrop, **windows** (per window, front to back: popups, decoration elements, masked content, shadow; the stage strip's thumbnails below every other window), snap preview, layer-shell top (the menu bar and the dock), notification banners (and the notification centre list), **input-source HUD**, **accent popup**, the extension layer (nothing draws in it), layer-shell overlay, drag icon, cursor. `LayerStack::flatten` yields the front-to-back list the damage tracker expects. The order is unit-tested (`render::tests`) and checked on a real scene (`tests/headless.rs::element_order_...`).

* **Shadows** (`shaders/compositor/shadow.frag`): closed-form Gaussian-blurred rounded box (error function across x, 8-sample integral across y). Parameters come from `cupertino-decorations::ShadowParams` (the verified 32-row table, peak densities 0.40 light / 0.70 dark for active windows) through `ShadowParams::gpu` (`ShadowGpuParams`: σ, vertical offset rounded to device pixels, `ceil(σ·2.8)` reach, density), converted back to points for the shader. Menus use the table's panel row (which class the design uses for menus is unknown). The shadow is cut out under the window shape (`cutout`, approximate design choice). `config::ShadowStyle` remains for the legacy path.
* **Rounded corners** (`clip.frag`): texture shader override on the surface elements (`ClipElement`). Independent of the output transform (winit renders flipped). Corner radius 12 pt, continuous corners with the exponent from `cupertino-geometry::CornerStyle` (approximate expansion); the closed-form SDF is the same formula as `RoundedRect::sdf_approx`.
* **Wallpaper** (`render/wallpaper.rs`): the wallpaper layer draws, per output and appearance, the first image that loads of: the user's `wallpaper` / `wallpaper_dark`, then the asset pack's default picture, then the procedural shader (unchanged). The frame loop never touches the file system and never waits: one worker thread opens, decodes and scales (cover: the longer side cropped evenly, Catmull-Rom, to the output's device pixels) on request, newest request per output wins and older answers are dropped, a file is decoded once however often the output size changes, and the worker looks at the file again every 5 s, decoding again only when its size or modification time changed. The decoded image is kept only while some output asks for its file: when the wallpaper goes back to the procedural one, or an output is not asked about for a minute, it is let go. Until an answer is in, an output keeps what it showed (the procedural wallpaper at the start). The file is opened without blocking and checked to be a regular file through the descriptor; only PNG and JPEG up to 256 MB and 8192 px a side (about 0.5 GB of memory at most) are read, and a missing, unreadable or unsupported file is logged once and the next candidate stands in. The average colour of the image tints the materials in place of the procedural wallpaper's. The lock screen and the power fades stay black. A change of the keys in `compositor.json` (the Settings application writes them) applies without a restart.
* **Material pass** (`render/material.rs`, below): backdrops of shell processes (the menu bar strip, the dock bar, the dock process's switcher panel), the launcher panel (a backdrop of the launcher process, recipe of the appearance), the banners, the HUD and the accent popup.
* **Decorations** (`decor.rs`): title bar (fill, rim along the top, separator, window control buttons with glyphs on group hover, centred title with ellipsis) and the one-pixel rim around the content are rasterised on the CPU (`tiny-skia`, `cosmic-text`) into premultiplied buffers at the output scale. They are cached per window and re-rendered only when width, scale, active state, hover / pressed state, title or appearance changed, and drawn as `MemoryRenderBuffer` elements rescaled with the window.
  * Hit testing (`Hit::{Button, Titlebar, Resize(edge), Content}) runs on the plans of the topmost window first.
  * Buttons act on release over the same button: close sends `RequestClose`, minimize sends `Minimize` towards the dock's slot, zoom sends `ToggleZoom`.
  * A title bar press starts the model move. A double click runs the configured action (zoom by default). Resize zones start `BeginResize` and show the resize cursor.
  * **Input order per window** (`Cupertino::window_target_under`, top down): the window control buttons of a unified bar, the window's popups, the resize zones (3 pt inside to 4 pt outside the frame, corner zones 12 / 15 pt), the client's surface inside the frame, then the title bar and buttons. The resize zones win over the client surface, so the inner band works whatever the client's input region covers. The 4 pt outside part belongs to titled windows only (ours with a title bar, and every client-decorated toplevel, whether or not it draws a header bar, since the server cannot tell); a window of ours without a title bar has the inside band alone. The cursor follows what the frame can do along each axis: the two-way cursor where the edge can grow and shrink, the single arrow where only one way is left (at the minimum width the side edges point outward), and no cursor and no zone where it cannot move, so a fixed-size window (min = max) shows none and its title bar takes the top band (corner rule, `decor::resize_cursor`). The client's size limits reach the window model as frame limits, the insets added (`state::frame_limits`), so an SSD frame is not resized below the client's minimum. A **client-decorated** toplevel gets the same zones from its window geometry and the same compositor-started resize (decision of 2026-10-25); the client's own input region outside the frame (a drawn shadow, GTK's 12 pt) takes no input beyond the band: the pointer passes to what lies below, as the design's shadows are not clickable.
  * **Popup grab** (`popup_grab.rs`): while a client's `xdg_popup.grab` is active, the pointer follows the popup chain of its toplevel (a press on a popup goes to that popup, and a popup that maps or commits under a pointer that does not move takes the pointer focus), a press anywhere else dismisses the whole chain and is consumed (it reaches neither the window nor a shell surface under it; its release is consumed too), and the keyboard stays with the topmost popup until the grab ends, then returns to the toplevel. A popup that has not committed a buffer cannot be hit, so a press at its place counts as outside. Only the popups of the grabbing toplevel (and their subsurfaces) belong to the chain, so a press on that toplevel itself or on an in-window menu bar of the same client is consumed too (two clicks to switch menus), which xdg-shell allows. A session lock ends the grab.
  * Moves and resizes the server starts from a press (title bar, resize zones; there is no modifier drag, a modified click reaches the client as on the design) wait in `PendingDrag` until the pointer leaves the 5 pt box around the press (`cupertino_input::drag_started`), then start anchored at the press.
* **Window effects** (`render/mesh.rs`, `fx.rs`): while the model animates a minimize or restore, the window is captured offscreen at its natural size (decorations and content, `capture_window`) and drawn through the `cupertino-effects::MinimizeEffect` mesh (warp 500 ms, funnel 400 ms or scale 200 ms; `minimize_effect` in the configuration, `plain` keeps the rescale) towards the rectangle the dock process committed with `set_minimize_target` (global through its layer geometry; no mesh target without a dock). The mesh's dock edge is the screen edge nearest to that rectangle. The effect generators carry their own easing, so the model's minimize / restore curves are set to linear over the durations.
  * **Closing**: a snapshot of the window with its shadow is stored (throttled to once per 0.5 s per window, on commits and once when the open animation ends) and refreshed live right before an unmapping commit. When the window disappears the snapshot fades out (`WindowClose`, 0.15 s) as a ghost.
  * **Opening** (scale 0.98 → 1 + fade, spring 0.25 s) and zoom come from the model's presentation. Workspace slide positions use `WorkspaceSlide::offset_x`.
  * **Show desktop** (`F11`, four-finger spread, hot corner) moves every window with `ShowDesktop::placement` (spring with damping ratio 1.0 and response 0.3 s) and is reversed by the same action.
* **Menu bar** (`menu_bar.rs`, `cupertino-wm::menu_bar`): a top-anchored layer surface of the menu bar process with the menu-bar role (`get_menu_bar`; one per output). The server owns its frame as the design window server does. The model keeps per output the height reserved in the usable area (the configured `menu_bar_height`, not the client's exclusive zone, and none while the bar is hidden), the inactive output's dimming (opacity lost 0.4, approximate), auto-hide in full screen and the reveal at the top edge (`EdgeReveal`), kept while a menu is open (`set_menu_open`). The surface is drawn with that presentation's vertical offset and opacity, and pointer hit-testing follows the offset, so a hidden bar takes no input. The feed (`active_app`, one of `menu_none` / `menu_dbusmenu` / `menu_gtk`, `window_state`, `done`) goes to every bar on bind and whenever the key window, its title, its menu address or its window state changes. Menu addresses come from the appmenu and `gtk_shell1` protocols; the bar process reads the menus. `window_command` and `move_to_workspace` run the window-model commands of the fallback menus. The `focus-menu-bar` action sends `engage` to the bar of the active output (the one under the pointer) and gives its surface the keyboard when its layer accepts it.
* **Overview** (`shell.rs`, `cupertino-overview::Transition`): the server opens a session on the pointer's output (`Ctrl+Up`, `Ctrl+Down` for the front application, a dock's `run_action` or `app_command`), sends that output's dock the window set (desktop frames), the workspaces and the usable area, and animates every window from its desktop frame (a minimized one from its dock tile) to the frame the dock answers with (`overview_frame`, the reflow spring for a later change) by the transition's progress (0.25 s). Live window textures rescale (same elements as on the desktop) over a dim backdrop (`overview_dim`). The window set is resent when windows open, close or change state, the workspaces when they are added, removed or reordered. The dock's `overview_select` focuses or restores and leaves, `overview_close` leaves, `switch_workspace` leaves then switches. Without a dock, or with a dead or hung one, the server still leaves on Escape, on a click (a window under the pointer is selected, empty space leaves), on any workspace switch (shortcut or swipe) and when the dock's object or surface goes, and puts the windows back. The chrome (workspaces bar, highlight, titles, keyboard navigation) is the dock's alone.
* **Damage tracking**: `OutputDamageTracker` per output. Elements rebuilt every frame get stable ids and commit counters from `ElementCache` (a hash of their parameters), so nothing is damaged unless a parameter changed. Material elements are damaged whenever anything below them changed (their generation is a hash of the (id, commit, geometry) of everything beneath, assigned by `finalize_materials`).
* **Cursor**: xcursor images by name (cursor-shape protocol) from the theme of the first asset pack that has one (`<pack>/cursors/<theme>`, the theme name from the manifest; no host theme and no environment override); without a theme, or with one that lacks `default` and `left_ptr` (reported on stderr), every cursor is the built-in arrow and the clients keep their own theme; with one, the clients the server starts get `XCURSOR_THEME` = that name and `XCURSOR_PATH` = the pack's `cursors/` directory, so they find no other theme; the image is sampled whole at every output scale (a scale-2 output picks the 48 px image for 24 pt, and the element's source rectangle is the full image); built-in arrow fallback, client cursor surfaces, all drawn by the compositor (the host cursor is hidden in the nested window). Server-drawn cursors use the cursor scale: `cursor_size` (1…4) times the shake-to-locate multiplier, from the smallest theme image at least that size. A client cursor surface takes only the shake multiplier, since clients started by the server get `XCURSOR_SIZE` = nominal size × `cursor_size`. `cursor_fill` / `cursor_outline` (default black / white) redraw the theme images by lightness, the drop shadow kept black (approximate). Shake to locate (`cursor.rs`, `cupertino_input::ShakeDetector`): every pointer motion feeds the detector, a recognised shake grows the cursor 5×, and a 1/60 s timer shrinks it back once the shaking stops.

### Material pass and the GLES shader port

`render/material.rs` runs inside `MaterialElement::draw` on the renderer's context (`GlesFrame::with_context`), with the target framebuffer bound, and restores every piece of GL state it touches:

1. capture: of the bound framebuffer into a half-size texture (linear filter = 2x2 box average; RGBA16F when renderable, else RGBA8),
2. downsample pyramid (`downsample.frag`) while σ² > 64 texels²,
3. separable Gaussian (`blur.frag`),
4. colour operations (`color.frag`: luminance curve, saturation, monochrome, and the display filter's linear matrix, invert and contrast),
5. composite (`material.vert/frag`): SDF mask, backdrop sample, fill layers with their blend modes, drawn once per damage rectangle with the blend state (SRC_ALPHA, ONE_MINUS_SRC_ALPHA) / (ZERO, ONE).

Recipes come from `cupertino-theme` (`MenuBar` for the strip, `Menu` for the dock bar, and `Switcher`, the switcher panel's code recipe of, for the dock process's panel) through the `cupertino-glass` encoders (`ops`, `blur::plan_blur`); uniform blocks are the std140 structs of `cupertino-glass::ops` unchanged. The shaders in `shaders/` are copies of `cupertino-glass/shaders/gles/`; `color.frag` is kept identical to its original.

**Check against the CPU reference.** The ported shaders compiled and linked unchanged on GLES 3.2 (NVIDIA 615.78) and produce the design result; no shader needed a bug fix. `tests/shell_v1.rs::backdrop_material_pass_matches_the_cpu_reference` renders a backdrop strip over the procedural wallpaper and compares it with `cupertino_glass::cpu::render_regions`:

| recipe | mean abs difference (8-bit) | max |
|---|---|---|
| MenuBar light | 0.322 | 1 |
| MenuBar dark | 0.156 | 1 |
| Sidebar light (fills, darken blend, tint layer) | 0.016 | 1 |
| Menu dark | 0.059 | 1 |

**Changes to the copies** (integration only):

* `material.vert`: the quad corners come from a `u_quad` uniform (output pixels, one draw per damage rectangle) and are mapped through `u_proj` (the frame's projection matrix) instead of dividing by the target size, so the output transform (flipped nested framebuffer, rotated outputs) is honoured; a new varying `v_uv` carries the matching framebuffer-normalised coordinate.
* `material.frag`: the backdrop is sampled with `v_uv` instead of `v_px / size` plus the optional row flip (`Region.info.z` is unused), and the `u_source` branch was removed: recipes without a backdrop filter use an unfiltered full-size copy of the framebuffer as backdrop (the capture pass runs with no pyramid).
* The `Region` / `PassParams` blocks are unchanged; the Smithay renderer exposes no UBOs, so all passes use raw GL through `with_context`.

### Protocols

wl_compositor / subcompositor, wl_shm, linux-dmabuf (v4 feedback when the render node is known), xdg-shell, xdg-decoration (server-side unless the client asks for client-side), xdg-dialog (modal flag → model), wlr-layer-shell (exclusive zones feed the model's usable area; a surface created without an output goes to the output under the pointer), **`zwlr_output_manager_v1`** (v4) and **`zwlr_output_power_manager_v1`** (see *Displays*), xdg-activation (also external tokens for launched applications), presentation-time (feedback at submit, at vblank on the TTY backend), fractional-scale + viewporter, cursor-shape, wl_seat (pointer, keyboard with xkb, repeat), pointer-gestures (forwarded to the client until the recognizer claims the gesture), relative-pointer, **`ext-session-lock-v1`** (below), **`ext-idle-notify-v1`** (every input event is activity, touch included) and **`zwp-idle-inhibit-v1`** (the notifier is held back while a live surface has an inhibitor; the surface need not be visible), wl_data_device + primary-selection with the kept clipboard (*Clipboard* below), **`ext-data-control-v1`** and `zwlr-data-control-v1` (v2, with the primary selection) for clipboard managers, wl_output + xdg-output, ext-foreign-toplevel-list, **`org_kde_kwin_appmenu_manager`** (service name + object path per surface → the window's menu address, announcements before the window maps are applied at map time) and **`gtk_shell1` / `gtk_surface1`** (`set_dbus_properties` → GMenuModel address; `present` / `request_focus` focus the window; the protocol is generated from `protocols/gtk-shell.xml`, a wire description of the interface; the `capabilities` event is not sent, so GTK clients keep drawing their own menu bar as well). `zwp_text_input_manager_v3` and `zwp_input_method_manager_v2` are exported **only when an accent table is configured**: the input-method global is restricted to the compositor's own in-process client (the accent bridge, see *Keyboard*), so an installed input method keeps the seat's single slot otherwise. **`ext-image-capture-source-v1`** (output and foreign-toplevel sources), **`ext-image-copy-capture-v1`** and the private **`cupertino_screenshot_v1`** are offered only to *privileged* clients (see *Screenshots*); a client of the public socket does not see them, so an application cannot read the screen. While a copy session runs (not stopped), menu bars of version 3 get `screen_capture 1` in a group of their own, and `0` when the last one ends; the bar shows the screen capture indicator. Camera, microphone and system-audio indicators need the PipeWire graph, which the server does not read (no C library): not shown. Not implemented: xdg-toplevel-icon, pointer-constraints, drm-syncobj.

**Session lock.** The `lock` shortcut action makes the D-Bus worker call `LockNow` (no arguments; the display-sleep lock calls `Lock`, which differs only in the lock screen's idle hide: 5 s after a lock the person asked for, 30 s otherwise) on `org.cupertino.Login1` at `/org/cupertino/Login1`. The login process answers by taking `ext_session_lock_v1`. Any client can take the lock (the global is not restricted). From the request on, the compositor draws only the lock surface of each output over solid black (no windows, layer surfaces, drag icon or overlays), delivers the pointer and the keyboard only to lock surfaces and ends running grabs and drags. Every shortcut, hot corner and gesture is off except VT switching. `locked` is sent once every output has presented such a frame and has a committed lock surface. After one second without surfaces it is sent with black outputs (approximate). Lock surfaces are configured to the output's logical size.

* A second `lock` while the session is locked or being locked gets `finished`.
* If the lock client disconnects, the session stays locked and black until another client takes the lock (`session locked` is logged again). A client that disappears before `locked` was sent cancels the request.
* Logs: `session locked`, `session unlocked` (info), the disconnect cases (warn).
* Smithay's `unlock` callback does not say which lock object sent `unlock_and_destroy` and also fires for a lock that was refused. The compositor honours it only when the lock object of the current owner is the one that died with the request and the owner's client is still connected. A refused lock's unlock arriving together with the owner's crash leaves the session locked.

### Clipboard

`clipboard.rs` keeps the clipboard when the client that copied exits. When a Wayland client sets the clipboard selection, the server reads the offered types it keeps, one after the other, into memory: UTF-8 text once (the first offered of `text/plain;charset=utf-8`, `UTF8_STRING`, `text/plain`, `TEXT`), then `text/uri-list`, `text/html`, `text/rtf`, `text/tab-separated-values`, `image/png`, `image/tiff`, `application/pdf`, and Latin-1 `STRING` only without UTF-8 text (the Wayland side of the design's pasteboard type table, §7, mapping ). Other types (private formats) are not kept. All types together are capped at `CAPTURE_LIMIT` = 64 MiB (approximate: no limit is known); a type that would pass it is dropped, the ones before it stay. The client stays the owner and serves every paste while it runs. Once it has disconnected (checked after each dispatch of the clients), the copy becomes a server-side selection (its `Snapshot`) offering the text under all four UTF-8 names and every other kept type under its own; pastes are written from memory without blocking the loop. A selection the client cleared before exiting stays empty; the primary selection is not kept (it belongs to its owner by convention; the design has none).

The owner is the client with keyboard focus when the selection is set, which `wl_data_device` requires. A clipboard manager sets the selection through data control without focus; its selection is attributed to the focused client, and the copy takes over if that client exits first (same content; the manager sees its source cancelled). A type still being read when the owner disconnects is incomplete and is not offered; a client that dies in the middle of writing a type and whose end-of-file is seen before its disconnect leaves that type truncated (the two arrive in the same loop iteration in either order). Data control is offered to every client: the server has no sandbox identity (security-context) to deny it to.

### Displays

Every backend reports its displays as **heads** (`displays/mod.rs`): the nested window (one head, `winit`, that cannot be disabled, rotated or given another mode; resizing the window replaces its only mode), each connected DRM connector (named `DP-1`, `HDMI-A-1`, ...; make, model and serial from the EDID, the connector's modes with the preferred one first; refresh in millihertz from the pixel clock and totals), and each offscreen display of the headless backend (any size is a mode). An enabled head has an output (`wl_output` global, model output, layer maps); a disabled one has none but stays listed.

A head starts in the state stored for it, else at its preferred mode, the scale of `cupertino_displays::default_choice` for its pixel and physical size (1 when the size is unknown) and to the right of the other heads; `--scale` beats both. A stored mode the display no longer offers falls back to the preferred one. A display stored as off is turned on when no other display is on, and when the last display that is on goes away, the first remaining one is turned on.

**`displays.conf`** (`cupertino_defaults::displays`; `$XDG_CONFIG_HOME/cupertino/displays.conf`, or `CUPERTINO_DISPLAYS_CONFIG`) keeps one section per display, keyed by make, model and serial (by connector name without a serial): `enabled`, `mode`, `position`, `scale`, `transform`. The TTY session reads it at start and writes it whenever a configuration is applied, keeping the sections of absent displays, so a display gets its arrangement back when it is plugged in again. Nested and offscreen runs keep it in memory only. A file that cannot be read stops the start; a malformed section is logged and ignored.

**Output management** (`management.rs`, `zwlr_output_manager_v1` version 4): each bound manager is sent every head with its name, description, physical size, modes, make / model / serial, enabled state, current mode, position, transform and scale, and adaptive sync as disabled; changes follow as they happen, then `done` with a serial that moves on with every change. A configuration must name every head (else the `unconfigured_head` error); one made against an old serial is `cancelled`; one the backend cannot carry out (disabling the last display that is on, a mode the head does not offer, a rotation of the nested window, adaptive sync) `failed`. `test` checks without applying. Applying turns the heads that go off off first, then brings the others to their state (mode switch through the backend, then position, scale and transform of the output, so windows, layer surfaces, the menu bar and the lock surface follow); a failure part way restores the previous state. Disabling a head closes its layer surfaces.

**Output power** (`power.rs`, `zwlr_output_power_manager_v1`): `off` blanks the output (the TTY backend clears the CRTC) and stops drawing it; `on` draws it again. An output that goes away fails its power objects.

### Session bus

`dbus.rs` runs one worker thread (feature `dbus`, on by default; `CUPERTINO_NO_DBUS=1` disables it). It starts on the nested and TTY backends, and with `--dbus` on headless.

* `ToggleCentre` on `org.cupertino.NotificationCenter1` (the notification process);
* `Toggle` on `org.cupertino.Launcher1` at `/org/cupertino/Launcher1` (the launcher process) and on `org.cupertino.ControlPanel1`;
* the media key steps of `org.cupertino.Osd1`;
* `Lock`, `LockNow`, `RequestLogOut`, `RequestPowerDialog` and `PowerKey(pressed: b)` on `org.cupertino.Login1`. The `lock` action is bound by default to `Super+Ctrl+q`, the `logout` shortcut name (server action `log-out`: the confirmation panel, the applications quit first; it no longer ends the server) to `Super+Shift+q`, the `power-dialog` action (the power panel: Restart, Sleep, Cancel, Shut Down) to `Ctrl+XF86PowerOff` and `Ctrl+XF86Eject`, and the `power-key` action to the power key `XF86PowerOff`. `Ctrl+Alt+BackSpace` still ends the server (`quit-compositor`). Without a login process to ask (no session bus, or nobody owns `org.cupertino.Login1`: a headless or capture run) the `logout` shortcut ends the server as it did before the panel existed. This also holds while `cupertino-login` restarts after a crash (up to a second, the name has no owner): `Super+Shift+Q` then ends the server without the confirmation panel.

The router forwards the power key's press when it is consumed and its release when the key goes up, and swallows the repeats of a held key. The worker awaits these two calls in turn, so the login process sees them in order. The login process times the hold (0.35 s and 1.5 s, `PowerButtonSleepsSystem`) and shows the power-button panel. With the session locked the router forwards the key to the lock screen and sends nothing. The process takes a logind `handle-power-key` block inhibitor so that the key reaches the server instead of powering off. The server receives the key only from a device it has opened as an input device (not checked on hardware). With the greeter nobody owns the name, so each press and release logs the warning below and nothing else happens. A call that fails (no session bus, the process is not running) is logged as a warning naming the process. The application menu registrar and the StatusNotifier watcher belong to the menu bar process.

### Shell processes

The server binds `cupertino_shell_v1` (`render/cupertino-protocols`). A shell process can ask for **backdrops** below its surfaces (`shell_v1.rs`): a material recipe name, a rounded rectangle and an alpha, double-buffered with the surface's commit and drawn by the material pass directly below that surface. The surface may be a window's toplevel or a popup of a window or layer surface (a context menu): a popup with a backdrop is drawn over its material with the 1 device pixel rim and the panel row of the shadow table, which menu windows use, cut out under the popup (`popup_backdrop_elements`; `tests/window_popup_backdrop.rs`).

The `"shell"` key of the configuration lists the processes to start (`[{"name": "notifications", "command": ["cupertino-notifications"]}]`, empty by default). The supervisor (`supervisor.rs`, `restart_policy.rs`) restarts a process that exits with failure, at most once per second, and leaves a process that exits successfully stopped. The children get the server's `WAYLAND_DISPLAY` and `XDG_CURRENT_DESKTOP=Cupertino` (also every client the server starts), so the session's desktop-specific files (`cupertino-mimeapps.list`, `OnlyShowIn=Cupertino`) apply to them. The `notification-center` and `launcher` actions go through the D-Bus worker (*Session bus*). A layer surface above the windows that commits exclusive keyboard interactivity takes the keyboard (the launcher panel while it is shown) and gives it back to the key window when it stops asking or goes away.

**Menu-bar role** (`get_menu_bar`, `menu_bar.rs`; see *Rendering*): the server reserves the bar's height, applies hide / reveal / dimming, feeds it the active application and menu addresses, and runs its window commands. A surface that is not a layer surface of the output, already has a shell role, or an output that already has a bar is a `role` protocol error.

**Dock role** (`get_dock`, `dock.rs`): the dock process draws the bar, the switcher's panel, the overview chrome and the context menus, and lays out the overview. The server supplies the window feed, minimize target, the switcher's list and selection, the overview's window set and workspaces, and maps hot corners and gestures to named actions. the client owns the dock's presentation and its reserved area (the layer surface's exclusive zone, the standard layer-shell usable-area path). The server sends every window (`window`, `window_closed`, `done`-delimited; the first group on bind) and `launching` when it starts a program. It takes `activate_app` (the window model's `ActivateApp`), `restore_window` (unminimize, animating out of the tile) and the double-buffered `set_minimize_target`. The same `role` error applies (one dock per output).

**Application switcher.** The dock draws the panel. The server keeps the keys and the choice (`input.rs::switcher_key` and `cupertino-wm`'s `focus.rs`). While the switch is open every key goes to it and none to a client:

* Tab / Right / Shift+Tab / `` ` `` / Left step. A fresh press wraps, a key repeat stops at the ends, and Shift pressed or released mid-repeat turns it round.
* Home / Page Up and End / Page Down jump to the ends.
* Return / Space commit. Escape /. cancel.
* Up / Down / `1` / `! commit and open the application overview.
* Q asks every window of the selected application to close. H hides or shows it.
* The release of the primary modifier commits. For the first 150 ms only the count of steps is kept (the hidden counter).

Only applications with a window are listed, and an application whose last window closes leaves the list. The group for a change goes to one dock, the one on the output under the pointer when the switch opened (else the first dock): `switcher_output` (that dock client's object for the output, its own output when it has none), `switcher_entry` per application, most recently used first, then `switcher_changed` with the selection. The end (`switcher_closed`) reaches every dock. A dock that binds mid-switch on the panel's output gets the state at once. `switcher_select` (the pointer over or on a tile) moves the selection, `switcher_activate` (the left button released on a tile) selects and commits, `switcher_cancel` (released on the panel off the tiles) cancels. A press anywhere but the panel (layer namespace `cupertino-dock-switcher`) is the server's and its release cancels the switch. Without a dock, or with a dead one, the switcher opens, steps and commits the focus unchanged and is invisible. `cupertino-dock` creates an overlay surface on the named output when the switch opens and destroys it when it ends.

**Activation** (`handlers.rs`, `cupertino-wm::activation`): every xdg-activation token is kept with its creation time and whether it may activate (made by the server for a program it starts, or by a client for an input event while it had the keyboard focus). A token never expires and is used up by its first request. A window's request brings it forward when its token is valid, newer than its application's last activation, and no key or button press came more than 100 ms after the token. Otherwise the request is refused, logged (`activation refused`) and, for an application not in front, sent to every dock as `attention` (the dock bounces the tile once).

**Application grid.** It is the dock's alone: the `app-grid` shortcut (and the dock's own mapping of a corner or gesture) ends in `ShellOp::ToggleAppGrid`, which sends every dock `toggle_app_grid`. The server keeps no grid state, so without a dock the action does nothing. The dock opens the grid on a keyboard-exclusive overlay surface and closes it on a second action, Escape, a click on empty space, a launch or `locked`.

**Window-manager role** (`get_window_manager`, `window_manager.rs`; one per output, else the `role` error): the process owns the stage policy and the server keeps the frame clock.

* The server sends the usable area and the arranged windows (mapped, unminimized, parentless toplevels of the output's current workspace whose application is not hidden: `window` with application id, frame and active state, `window_closed` when one leaves the set; `done`-delimited, the first group on bind), `toggle_stage` on the `toggle-stage` action (no default chord), and `strip_pressed` for a left press on a strip thumbnail with no window or top or overlay layer surface above it (the press reaches no client).
* The process answers with `set_placement` (stage, strip rectangle or hidden) per window, applied together on `commit`, and `activate_window` (`Command::FocusWindow`).
* On commit every window whose placement changed is retargeted from where it is drawn now, on three tracks with the stage model's springs, all critically damped (springs, for the strip): the frame with response 0.38 s, the alpha with 0.30 s and the corner radius with 0.20 s, from the window's own radius (12 pt) to the strip's 8 pt and back. Each track keeps its value and velocity when retargeted; the frame is one spring on x, y, width and height, headed for the window's own frame; when that frame changes during the swap (the client resized or the window was moved) the track turns towards the new one, keeping its value and velocity. The mask's shape is part of a masked element's commit, so a corner-radius change alone damages the window. A strip window is its live content and decorations scaled into its rectangle (the overview's path, not a capture), its content, title bar and rim masked to the animated radius and its shadow drawn with it, below every stage window, with no hit testing of its own. A window placed hidden keeps its frame and fades out (alpha 1 to 0), then is not drawn; it comes back in its new place fading in, with that place's corners. The alpha values per placement (1 on the stage and in the strip, 0 hidden) are approximate: gives the alpha spring, not the values the design's modifiers write.
* When the object goes away, also when the process dies, every window is back at its own frame at once. A window leaving the arranged set loses its placement the same way. With the stage off the process sends no placement.

**Hot corners and gestures** are detected in the server (every corner arms; dwell 0.0 s, `hot_corner_dwell` and `hot_corner_size` in `compositor.json`; the recognizer's swipe up / down, pinch and spread) and sent to the dock of the output as `hot_corner` and `gesture`. The dock maps them (`dock.conf`) and answers with `run_action` (a named action of the shortcut file, run as its chord would: `overview`, `app-overview`, `show-desktop`, `launcher`, `lock`, ...), its own grid or a program. Without a dock a corner or gesture does nothing, except that a committed gesture still leaves an open overview. `app_command` serves the dock's tile menus: `show_windows` opens the application overview, `hide` and `quit` run the application's commands.

**Scroll count** (`cupertino_scroll_v1`, `scroll_v1.rs`): `wl_pointer` cannot carry the scroll count, so a client binds the global, calls `get_scroll(wl_pointer)` and gets `scroll_count(axis, count)`. For every touchpad scroll event that sends an axis value, `Cupertino::pointer_axis` sends one `scroll_count` per moving axis to the scroll objects of the client that has the pointer focus, after the `wl_pointer.axis` events and before the `wl_pointer.frame`; events the coalescing holds back and the gesture-ending stop send nothing. The count comes from `cupertino_input::ScrollCounter` per touchpad (`PointerDevice::scroll_count`): the first event of a gesture is `Began`, the next ones `Changed`, the stop `Ended`. A count of 0 means no series has started. Wheels have no gesture phases, so the design gives them no count either: their frames carry no `scroll_count`, which a client reads as 0.

**Session observer** (`get_session`, `session_v1.rs`): every shell process creates one and follows the session from it, instead of a `--dark` flag. Right after creation it gets `appearance` (1 dark, 0 light) and `locked` or `unlocked`, then one event per change.

* The appearance changes at run time through `Cupertino::set_appearance` (the `toggle-appearance` action, bindable in `shortcuts.conf` or `compositor.json`, no default chord).
* `locked` is sent when the lock request is accepted, before the lock screen has its first frame, and `unlocked` when the session opens again. A lock whose client crashed stays locked and sends nothing, and a client taking it over does not send `locked` a second time.
* Like `get_backdrop`, the request needs no surface, so any client of the socket may observe.
* A version 2 observer (`cupertino_shell_v1` version 2) also gets `display_options` (a bitfield of reduce motion, reduce transparency, increase contrast and differentiate without colour) after `appearance` at creation and once per change. A version 1 observer never does.

### Accessibility display settings

`accessibility.conf` (`cupertino-defaults::accessibility`, `$CUPERTINO_ACCESSIBILITY_CONF` overrides the path) is read at start in the winit and udev modes and written on every change; offscreen runs keep the settings in memory. The server is its only reader and writer (`accessibility.rs`): it applies the settings to its own drawing and tells the shell processes the four display options. Actions `toggle-reduce-motion`, `toggle-reduce-transparency`, `toggle-increase-contrast` and `toggle-differentiate-without-color` (no default chords; bindable in `compositor.json`'s `keybindings`, not in `shortcuts.conf`, whose parser knows only the shared shortcut table's actions) flip one option; `Cupertino::set_accessibility` replaces them all.

| Setting | Server drawing |
|---|---|
| Reduce Transparency | materials take their `-Opaque` variants (`RenderResources::recipe`, cached per appearance and option) |
| Increase Contrast | the high-contrast catalogs (`Cupertino::appearance`), so materials are opaque too; decorations take the high-contrast palette and the contrast half of the shadow table |
| Reduce Motion | `render::motion`: the workspace switch cross-fades in place instead of sliding; full screen draws the new frame at once (no frame zoom, the workspace cross-fade carries the change); a closing sheet disappears at once |
| Colour filter, invert, display contrast | a `Full` material element over the whole output in `SceneLayer::DisplayFilter` (below the pointer): backdrop at scale 1 without blur, then the colour chain of `cupertino_glass::display_filter` |
| Differentiate without colour | broadcast only |

The design filters the pointer too; here it stays above the filter pass so pointer motion does not repaint the whole output. The colour filter and invert are read from the file only: no action changes them at run time. Without the material pipeline (no GLES 3) the filter pass is not drawn; the pipeline failure is logged once at start.

### Keybindings and configuration files

Shortcuts are the `cupertino-input` `ShortcutMap` (defaults approximate, `Super` = primary modifier) with the free rows of the `cupertino-keyboard` reference assignment table merged in (see *Keyboard*), translated into compositor actions, plus a few compositor-only defaults.

**Configuration files**

| file | override | read by | content |
|---|---|---|---|
| `~/.config/cupertino/shortcuts.conf` | `$CUPERTINO_SHORTCUTS` | server | chord to action lines, applied on top of the defaults: `Super+x = exec:foot`, `F5 = show-desktop`, `Super+Tab = none`. `# starts a comment. Invalid lines are reported and skipped. |
| `~/.config/cupertino/compositor.json` | `$CUPERTINO_SERVER_CONFIG` | server | the keys below. Overrides single chords after `shortcuts.conf`. `/etc/cupertino/compositor.json` (installed by the packages: the `shell` list of the whole desktop) is applied first; the user file overrides its keys. A file named by the variable replaces both. Both files are watched: a change of either applies live on top of the other, and a key removed from the user file goes back to the system file's value (the default when that has none). |
| `~/.config/cupertino/keyboard.conf` | `$CUPERTINO_KEYBOARD_CONFIG` | server | see *Keyboard* |
| `~/.config/cupertino/dock.conf` | | dock process | what a corner or gesture does |
| `~/.local/share/cupertino/launcher.json` | | launcher process | its history |

**`compositor.json` keys** (all optional; read and written by `cupertino_defaults::compositor`, which checks each key on its own: a wrong value, effect name or colour is logged with its key and leaves that key's default, the other keys apply; an unknown key action or chord is logged by the server the same way). A `shell` entry looks like `{"name": "notifications", "command": ["cupertino-notifications"]}`. What a corner does is the dock's setting (`dock.conf`, `corner_bottom_right = show-desktop`).


| key | type | default | effect |
|---|---|---|---|
| `keybindings` | object | `{} | chord to action name, for example `{"Super+t": "spawn-terminal"} |
| `appearance` | `"light"` / `"dark" | `light` | appearance (`--appearance` beats it) |
| `terminals` | string array | `["foot", "alacritty", "kitty", "wezterm", "xterm"] | candidates for `Super+Return`, first existing wins |
| `corner_radius` | number (pt) | `12` | window corner radius |
| `decorations` | bool | `true` | server-side decorations |
| `menu_bar_height` | number (pt) | `cupertino-theme` menu bar height | height reserved in the usable area |
| `show_menu_bar` | bool | `true` | reserve the menu bar's height (`--no-menu-bar` sets it to false) |
| `minimize_effect` | `"warp"` / `"funnel"` / `"scale"` / `"plain"` (`"none") | `warp` | minimize effect; `plain` keeps the rescale |
| `closing_animation` | bool | `false` | fade plain windows out when they close |
| `hot_corner_dwell` | number (s) | `0.0` | hot corner dwell |
| `hot_corner_size` | number | `2` | hot corner size |
| `mouse_speed`, `trackpad_speed` | number | `0.6875` | tracking speed |
| `mouse_dpi` | object | `{}` | resolution per mouse in dpi (50…32000), key `vendor:product:name` in hex ids; applies live |
| `mouse_acceleration` | bool | `true` | pointer acceleration |
| `natural_scrolling` | bool | `true` | natural scrolling of wheels and touchpads; applies live, to the next event of every device |
| `wallpaper`, `wallpaper_dark` | absolute path | none | the wallpaper image (PNG or JPEG) and the one of the dark appearance (`wallpaper` serves both when only it is set); applies live |
| `cursor_size` | number | `1.0` | cursor scale, clamped to 1 to 4 |
| `cursor_fill`, `cursor_outline` | `#rrggbb` or `#rrggbbaa` | `#000000`, `#ffffffff` | cursor colours |
| `shake_to_locate` | bool | `true` | shake to locate |
| `shell` | array of `{name, command} | ` | processes the supervisor starts |
| `screenshot` | string array | `["cupertino-screenshot"] | screenshot process (non-empty) |

Precedence of shortcut sources, highest first: `compositor.json` `keybindings` › `shortcuts.conf` › `cupertino-input` default map › `cupertino-keyboard` reference table rows. Table rows are added before `shortcuts.conf` is applied (so `chord = none` removes them too), only when their chord is free. A row whose chord is bound to another action is a *conflict*, reported in the log and dropped, never doubled (the default merge has none).

**Default chords**

| keys | action |
|---|---|
| `Super+Tab` / `Super+Shift+Tab` | application switcher (releasing `Super` commits, `Escape` cancels) |
| `Super+grave` | cycle windows of the application |
| `Super+Q` / `W` / `M` / `H` / `Alt+H` | quit application, close window, minimize, hide application, hide others |
| `Super+Ctrl+F` / `Z` / `Left` / `Right` / `Up` | fullscreen, zoom, tile left / right / fill |
| `Ctrl+Up` / `Ctrl+Down` | overview / application overview (`Escape` leaves; arrows, `Tab`, `Return` and `Space` go to the dock's overview surface) |
| `Ctrl+Left` / `Ctrl+Right`, `Ctrl+1..9` | previous / next / numbered workspace (`Super+Shift+Left/Right` moves the window, `Ctrl+Alt+N` adds one) |
| `Super+Space` (`Super+Alt+Space`) | show / hide the launcher panel (a `Toggle` call to the launcher process) |
| `F11`, `F4` | show desktop, application grid (the dock process draws the grid; see *Shell processes*) |
| `Ctrl+Space` / `Ctrl+Alt+Space` | input-source switcher: previously used source / next source in menu order (HUD while `Ctrl` is held; needs two or more `input_sources`) |
| `Super+Shift+F` | fullscreen (alias of `Super+Ctrl+F`, from the design table) |
| `notification-center` action | notification centre list (a click on the menu bar clock does the same from the menu bar process; `Escape` or a click elsewhere closes it) |
| `Ctrl+F2` | focus the menu bar |
| `Super+Return` | terminal |
| `Super+Shift+3` / `4` / `5`, with `Ctrl` for `3` / `4` | start the screenshot process: screen, selection, toolbar; `Ctrl` sends to the clipboard (see *Screenshots*) |
| `Ctrl+Alt+Backspace` | quit the compositor; `Ctrl+Alt+F1..F12` switch VT (TTY) |

Without a counterpart here (logged): focus dock, toggle dock auto-hide, move to next output. While a menu is open, arrows, `Return`, `Space`, `Escape`, `Home`, `End` and typed characters go to the menu; while the launcher panel is shown it is the keyboard-focused client and receives the keys the compositor's shortcuts do not take.

**Touchpad** (swipe and pinch events from the touchpad model → `GestureRecognizer`, all distances approximate): three- or four-finger horizontal swipe → workspace switch with the content following the fingers (the server's); vertical swipe up / down, four-finger pinch and spread → the dock of the output as `gesture` (its defaults: overview, application overview, application grid, show desktop). Events are forwarded to the focused client until the recognizer claims the gesture, then the client's gesture is cancelled. **Hot corners** fire on arrival (dwell 0.0 s) and go to the dock as `hot_corner` (none does anything by default); the **edge reveal** drives the full-screen menu bar (delays unknown).

### Keyboard

`~/.config/cupertino/keyboard.conf` (`$CUPERTINO_KEYBOARD_CONFIG`; read and written by `cupertino_defaults::keyboard`, applied over the keyboard model's defaults by `KeyboardSettings::apply_file`; `#` comments, unknown or invalid lines are logged with their numbers and leave those keys' defaults):

```ini
repeat_delay = 500          # ms before the first repeat, 0 = repeat off              (default)
repeat_interval = 83        # ms between repeats                                      (default)
modifier_profile = cupertino    # or classic-pc
input_sources = us, de, fr(nodeadkeys)  # xkb layout[(variant)], menu order
caps_lock_delay = 75        # ms, 0 = off                                             (default)
accent_picker = on
accent_table = ~/.config/cupertino/accents.json
accent_keyboards = default, en
accent_disabled_apps = foot, alacritty
client_rewrite = on         # Super and Alt chords for third-party clients under cupertino (approximate)
rewrite_terminals = foot, kitty, Alacritty, org.wezfurlong.wezterm, com.mitchellh.ghostty, org.gnome.Terminal, org.kde.konsole, xterm   # app ids, exact and case-insensitive (approximate)
rewrite_off = firefox       # app ids that never get a rewrite
fn_key = change-input-source  # tap action of the fn key: do-nothing, emoji, dictation
caps_lock_switch = off      # on: a caps-lock tap switches the input source, a hold turns caps lock on
caps_lock_hold = 500        # ms a caps-lock press must last to count as a hold, 1-1500 (default)
layout_keyboard = ansi      # key arrangement of `decoded:` sources: ansi, iso, jis
layout_level3 = right-alt   # level-3 keys of `decoded:` sources: both-alt, right-alt, none
slow_keys = off             # a key counts only once held for the delay
slow_keys_delay = 250       # ms                                                      (default)
sticky_keys = off           # modifiers latch for the next key, a second tap locks
sticky_keys_shortcut = off  # five Shift presses within 30 s toggle sticky keys
mouse_keys = off            # the keypad moves the pointer
mouse_keys_shortcut = off   # five Alt presses within 30 s toggle mouse keys
mouse_keys_delay = 1000     # ms before a held direction key repeats                  (default)
mouse_keys_max_speed = 11   # points per 50 ms tick                                   (default)
```

System layouts stay the groups of one keymap and come back when a system source is chosen. approximate defaults (the values is not known): both shortcut toggles off, `caps_lock_switch` off, level 3 of decoded sources on the right Alt only (both Alt keys would take Alt away from the shortcuts).

* **Repeat**: `wl_keyboard.repeat_info` advertises `RepeatConfig::repeat_info`: 500 ms / 83 ms becomes 12 characters per second with a 500 ms delay (tested with a real client); with slow keys on both become 420 ms. Clients do the repeating. The compositor repeats, with the same values, only keys it consumes itself: one `KeyRepeater` (`kbd_filters.rs`, polled from the overlay tick) replays a held key through the HUD / accent / shortcut router while the key and the same modifiers stay down. Stepping actions repeat (switcher open and cycle, window cycling, relative workspace switch, overview navigation, input-source step, volume and brightness steps; approximate), as do the HUD's Up / Down and the popup's Tab / arrows. Toggles (mute, overview open, launcher ...) never repeat.
* **Filter chain** (the design keyboard filter's order): device keys pass slow keys, the caps-lock delay (skipped while slow keys are on), sticky keys, mouse keys, then the router.
  * Slow keys: a press is delivered once held for the delay, an early release drops both events, a second key while one is pending is rejected.
  * Sticky keys: a lone modifier tap latches it for the next key (its release is held back and sent after that key), a second tap locks it, a third unlocks. With the shortcut on, five Shift releases within 30 s toggle the feature.
  * Mouse keys: keypad 8 / 2 / 4 / 6 and the diagonals move the pointer one point, then after the delay every 50 ms with the verified ramp up to the maximum speed. Keypad 5 clicks, 0 holds the button,. releases it (keys ). With the shortcut on, five Alt presses within 30 s toggle it.
  * A key the TTY backend reports as already held when it reads a device (open after a session switch, `SYN_DROPPED`) skips the filters until its release, so the Ctrl and Alt of a `Ctrl+Alt+F<n> switch do not arrive latched in the new session.
* **Fn key**: a tap (held no longer than the repeat delay, no other key in between) runs `fn_key`. `change-input-source` steps to the next source; `emoji` and `dictation` are logged as unavailable.
* **Modifier profile**: physical modifiers go through `ModifierProfile::translate` before any chord is matched. `cupertino`: `Super` is the primary modifier, `Ctrl` stays `Ctrl`. `classic-pc`: `Ctrl` is the primary modifier and `Super` unused. All chords in the tables are written with the primary modifier as `Super`.
* **Input sources**: the list becomes the XKB layouts of the seat keyboard (`XkbConfig` layout / variant lists), so clients get one keymap with several groups. `Ctrl+Space` (previously used source) and `Ctrl+Alt+Space` (next source in menu order) are the design assignments. The first press opens the HUD in most-recently-used order and moves the selection, further presses step, releasing the modifiers commits (`Return` commits, `Escape` cancels), and a chord without held modifiers switches at once. `Up` / `Down` step the open HUD. HUD visuals: radius 24 pt, minimum width 167 pt, padding 10 pt, rows of 40 pt, white tint at alpha 0.35, 0.3 s fade-out. The material (`Menu`), the placement (centre of the primary output) and the source icons (two-letter placeholders) are approximate / unknown. Row labels are the XKB layout names.
* **Caps lock**: the press is held back and delivered only if the key is still down after the delay (a calloop timer; a brushed key vanishes completely),. With `caps_lock_switch = on` and two or more sources the delay is `caps_lock_hold`: a shorter press switches to the next input source (the design toggles between the last two ABC / non-ABC sources; next source is the adaptation), a longer one turns caps lock on.
* **Accent picker**: hold a letter key. On its first repeat the popup opens next to the text cursor. `Tab` / arrows move the focus (shown as marked text), `1`-`9` pick directly, `Return` commits, `Escape` restores the base character, any other key closes it and is delivered. A click on a candidate commits it, a click elsewhere in the popup is taken, a click outside closes it (approximate). A held letter with candidates reaches the client as a press and an immediate release (the input method gets the repeats), and the compositor's own repeater opens the popup, so no extra base character is inserted; the physical release is swallowed.
  * Text reaches the application through `zwp_text_input_v3`. Smithay serves text input only while an input method is connected, so with an accent table the compositor connects an in-process `zwp_input_method_v2` client (a thread behind a socket pair). It learns which application has an active text input, receives its cursor rectangle through an input popup surface, and sends `delete_surrounding_text`, `set_preedit_string`, `commit_string` and `commit`. Applications without text-input support (plain terminals) never activate the bridge, so the picker does not start for them. Verified end to end with a real client (a terminal committed the chosen character).
  * The candidate lists are user data (nothing embedded; `tests/fixtures/accents.json` is a small authored sample).

### Launcher

The panel is drawn by the `cupertino-launcher` process (see its README); the server renders its backdrop and toggles it. `Super+Space` (`launcher` action, hot corner) calls `org.cupertino.Launcher1.Toggle` through the D-Bus worker. The process holds one overlay layer surface, created without an output each time the panel opens so that the server puts it on the output under the pointer. While the panel is shown it takes the keyboard (see *Shell processes*). The supervisor starts and restarts it through the `shell` key, for example `{"name": "launcher", "command": ["cupertino-launcher", "--terminal", "foot"]}`; without that entry `Super+Space` only logs a failed call. The `--no-launcher-files` flag and the `launcher` / `launcher_files` keys of `compositor.json` no longer exist (the parser ignores unknown keys): pass `--no-files` to the process, or leave it out of `shell`.

### Volume and brightness overlay

The volume, mute, display-brightness and keyboard-backlight keys go to the `cupertino-osd` process (see its README), which changes the level and shows the overlay. The default bindings (`XF86AudioRaiseVolume` / `LowerVolume`, `XF86AudioMute`, `XF86MonBrightnessUp` / `Down`, `XF86KbdBrightnessUp` / `Down`, plus Shift+Alt for the fine step) call `org.cupertino.Osd1.VolumeStep` / `ToggleMute` / `BrightnessStep` / `KeyboardBrightnessStep` through the D-Bus worker; they are bound only where the shortcut table leaves the chord free, and rebindable as `volume-up`, `volume-up-fine`, `volume-mute`, `brightness-down-fine`, `keyboard-brightness-up` and so on. Start the process with the `shell` entry `{"name": "osd", "command": ["cupertino-osd"]}`; without it a key press only logs a failed call. The compositor repeats held stepping keys (see *Keyboard*), so holding a key steps repeatedly.

### Screenshots

The five screenshot shortcuts are the actions below. Each starts the program of the `screenshot` configuration key (default `["cupertino-screenshot"], see its README) with the mode appended. One process runs per capture; the server does not supervise it.

| Action | Default chord | Arguments appended |
|---|---|---|
| `screenshot` | `Super+Shift+3` (and `Super+Shift+S`) | `--mode screen` |
| `screenshot-clipboard` | `Super+Ctrl+Shift+3` | `--mode screen --clipboard` |
| `screenshot-area` | `Super+Shift+4` | `--mode selection` |
| `screenshot-area-clipboard` | `Super+Ctrl+Shift+4` | `--mode selection --clipboard` |
| `screenshot-toolbar` | `Super+Shift+5` | `--mode toolbar` |

The process gets a connection of its own and runs on it as a *privileged* client:

- `Cupertino::privileged_client` inserts one end of a socket pair as a client marked `ClientState::privileged`;
- the other end is passed as `WAYLAND_SOCKET`, and `WAYLAND_DISPLAY` is removed.

Only privileged clients see the capture globals. While a layer surface of namespace `cupertino-screenshot` (the overlay or the toolbar) is mapped, a further shortcut starts nothing. The log has `screenshot process started` and `screenshot process exited` with the pid and the exit status.

Capture (`src/capture/`):

- **Sources.** An output source renders the output once more offscreen at its mode size, through the normal element tree. The pointer is drawn only when the session asks for it (`paint_cursors`). A toplevel source renders the window at its natural size like the window snapshots: frame and decorations, no shadow.
- **Buffers.** shm only, ARGB8888 and XRGB8888. The session announces the buffer size of the source's current placement.
- **When frames are copied.** After the source's output was rendered.
- **Damage.** A frame reports the rectangle that changed since the session's previous frame; the first frame is all damage.
- **Unchanged source.** A frame of an unchanged source waits for a change.
- **Sources that go away.** A session whose output or window is gone is stopped; its waiting frames fail with `stopped`.
- **Cursor sessions.** These are stopped at once: the cursor is not offered as a separate image.

`cupertino_screenshot_v1` sends the windows on screen front to back for the window pick:

- the foreign-toplevel identifier;
- the frame with its decorations, in global points, where the workspace slide puts it.

It sends them on bind and again whenever the list changes.

### Notifications

Banners and the notification centre list are drawn by the `cupertino-notifications` process (see its README); the server renders their backdrops and toggles the list.

### Display power and idle

`display_power.rs` decides, `power.rs` carries it out. The settings are `power.conf` (`cupertino-defaults::power`, read at start and on every change (*Live settings*); `$CUPERTINO_POWER_CONF` overrides the path; an invalid line is logged and skipped): display and system sleep per power source, `dim_before_display_sleep`, `require_password_after`, `lock_screen_display_sleep`. The defaults are the design's (display sleep 10 min on mains power and 2 min on battery, system sleep 10 min;: password after 300 s).

| Step | When (T = display sleep of the current power source) |
|---|---|
| dim | T - 15 s after the last input, to brightness factor 0.01 in 0.26667 s |
| lengthened countdown | input within 20 s of the dim undims (0.26667 s) and sets T to max(min(2T, 600 s), T) until the display sleeps |
| display sleep | T after the last input; instant. DRM: the CRTC is disabled (DPMS off) and no frame is queued; elsewhere the dim overlay goes fully black |
| lock | the display has slept for `require_password_after` (`never` disables it): `Lock` on `org.cupertino.Login1` |
| system sleep | the system sleep timer after the last input: logind `Suspend(false) |
| wake | any input event (keys, pointer, touch, gestures): the display is on at once, a full redraw, the ext-idle notifier gets the activity too |
| fade-in on activation | logind makes this session the active one again (a console switch back, a fast user switch): a black overlay over every output fades out over 0.25 s with `e = 0.5 (1 - cos(pi p))`, alpha `1 - e`; logged as `fading in from black` |

An idle inhibitor (`zwp-idle-inhibit-v1` on a live surface, or a logind `idle` inhibitor in `BlockInhibited`) holds the countdown at its start without lengthening it. The power source follows UPower's `OnBattery` (`cupertino-powerd`); without UPower the mains timers apply and that is logged. Resume from sleep (`PrepareForSleep(false)`) counts as input. The dim is a black overlay above everything on every output, with alpha `1 - encode(f)` where `f` is the brightness factor and `encode` the sRGB transfer, so that the dimmed light is `f` of the full (the design scales the backlight); the ramp is a raised cosine (approximate, the curve). Log lines: `display power state=dimmed|asleep|on`, `the display slept for the password delay: locking the session`, `idle: putting the system to
sleep`, `power source on_battery=…`, `logind idle inhibitor blocked=…`, `the system woke from sleep`.

### Live settings

`live.rs` watches `compositor.json`, `keyboard.conf` and `power.conf` (`cupertino_defaults::watch`, from start in every
mode) and applies a change while the server runs. Only the changed file is read again, with the last values read as the
last good ones: a wrong line or key keeps its value and is logged (`invalid configuration: the key keeps its value`). Only
the keys whose value changed in the file apply, so a setting changed another way (the `toggle-appearance` action) stays
until its own key changes; a key removed from the file goes back to its default. Log line: `configuration reloaded`
(`keyboard settings reloaded`, `power settings reloaded`).

| File | Applied at once | At the next start |
|---|---|---|
| `compositor.json` | appearance, tracking speeds, acceleration (the pointer devices start over with the new curve), scroll direction, cursor size, colours (the cursors are recoloured) and shake, minimize effect and closing animation (with the window model's minimize timing), hot corner size and dwell, corner radius, key bindings, terminals, screenshot command | `decorations` (windows that open from now on follow it), `menu_bar_height`, `show_menu_bar`, `shell` |
| `keyboard.conf` | every key; input sources and the layout keys rebuild the keymap, repeat changes are sent to the clients | — |
| `power.conf` | every key; the countdown in progress continues with the new timers | — |

### Quitting the applications before a log out

`quit.rs` serves `org.cupertino.Session1` at `/org/cupertino/Session1` on the session bus (from its own thread, started with the D-Bus worker); the login process's session agent calls it between the confirmed log out and logind (`shell/cupertino-login`):

| Member | What it does |
|---|---|
| `QuitNextApplication -> (s app_id, u windows) | activates the most recently used application that has toplevels and sends `close` to each (`xdg_toplevel.close`); `("", 0) when none has a window |
| `ForceQuitApplication(s app_id) -> u` | SIGKILL to the processes of the application's toplevels (Wayland socket credentials); returns how many |
| `ApplicationQuit(s app_id) | signal: the last toplevel of the application being quit went away |
| `SetAppearance(s appearance) | `light` or `dark`: stored as `appearance` in `compositor.json` (only that key changes), then applied as the `toggle-appearance` action applies it; `auto` is `NotSupported` (no time-of-day or location source), any other value `InvalidArgs`, a failed store, or no settings directory, `Failed` and nothing changes |
| `SetAccessibility(s key, s value) | one key of `accessibility.conf` with a value as the file writes it (`reduce_motion` `on`, `color_filter` `tint 0.5`): applied and stored like the accessibility shortcuts; an unknown key or bad value is `InvalidArgs` |
| `Appearance` (property `s`), `Accessibility` (property `a{ss}) | the session's appearance and every key of `accessibility.conf` with its value; `PropertiesChanged` on every change, by any path |
| `Mice` (property `a(ssub)`, read-only) | the connected mice, not the touchpads, each as `(key, name, dpi, stored)`: the `mouse_dpi` key of `compositor.json` (`vendor:product:name`, ids in hex), the device name, the resolution in use (stored, else the hardware database's `MOUSE_DPI`, else 1000) and whether it is the stored one; one entry per key; a mouse without a bus id (the winit backend's pointer) has no key, so it is not listed and cannot get a resolution; `PropertiesChanged` when a mouse is plugged or removed and when `mouse_dpi` changes |

The server stays the only writer of `accessibility.conf` and of the appearance: the settings application uses these members. The setters trust the same-user session bus and check nothing further (decision of 2026-10-25): any client of that bus, a sandboxed application with bus access included, can change the appearance and the accessibility settings. The upgrade path is a check of the caller's uid (`MessageHeader::sender` and `GetConnectionUnixUser`) against the session's.

The 45 s timeout, the 0.1 s pause and the interrupt panel are the agent's. The order is approximate (most recently used first). Log lines: `log out: asking the application to quit`, `log out: the application quit`, `log out: force quit`.

## Integration points

Everything above is built in. The `ShellExtension` trait is gone; the shell processes take its place.

* **Decorations**: with `decorations = false`, `Config::window.ssd_insets` reserves the space and nothing is drawn in it.
* **Pointer buttons**: a press the server consumes (overview, decoration, `Super` move or resize) also consumes that button's release (`consumed_buttons`), so a client never gets a release without its press; a client's own press and release go through the seat's implicit grab, and a model move the client asked for still ends with the client getting its release.
* **Element types**: `CupertinoElement` has variants for surfaces, shader rectangles, materials, CPU-drawn images (`Image`), rescaled decorations (`Decoration`), and mesh textures (`Mesh`); `render::mesh::capture` renders any element list into a texture, `MeshElement` draws it through a `cupertino_effects::Mesh`; `ElementCache` gives stable ids.
* **Input**: `Cupertino::process_input_event` accepts any Smithay `InputBackend` event (winit, evdev); `events.rs` documents the order: menus → decorations → shell → model interaction → client. Scripted input: `Cupertino::pointer_moved_to`, `pointer_button`, `gesture`. Relative motion of every pointer device goes through a per-device `PointerDevice`: the device's counts through the design curve family (`cupertino_input::PointerAccel`; touchpads use their own family and report rate). Wheel detents scroll 10 points per line, unaccelerated: a wheel, high-resolution or not, has no scroll resolution. Touchpad finger scrolling goes through `cupertino_input::ScrollAxisAccel` per axis (trackpad scroll curves, resolution 400, 67 Hz, setting 0.3125) and its lines become 10 points each. Momentum stays with the clients (they run kinetic scrolling after the finger source's axis stop; a server momentum would double it). Natural scrolling follows `natural_scrolling` (on by default). * **Stage**: the policy is the `cupertino-windowmanager` process's (on `cupertino-stage`); the server only applies its placements (`window_manager.rs`, see *Shell processes*).

## Tests

`cargo test -p cupertino-server` runs the unit tests in `src/` and these files in `tests/`: `clipboard.rs`, `headless.rs`, `layer_input.rs`, `pointer_buttons.rs`, `overlays.rs`, `session_lock.rs`, `shell_v1.rs`, `shell_processes.rs`, `menu_bar_v1.rs`, `dock_v1.rs`, `session_v1.rs`, `window_manager_v1.rs`, `dbus.rs`, `window_kinds.rs`, `output_management.rs`, `pointer_output.rs`, `cursor.rs`, `capture.rs`, `scroll_v1.rs`, `live_settings.rs`. `cargo clippy --workspace --all-targets --all-features` is clean.

Unless the harness column says otherwise, an integration test runs the compositor in-process on an offscreen EGL device with real Wayland clients and skips (pass) without an EGL device. `CUPERTINO_ARTIFACTS=dir` saves screenshots as PNG.

Crates touched, with test counts: `cupertino-wm` 161 (`SetOutputRect`, the `menu_bar` presentation model), `cupertino-dock` 119 with `--features app` (`Dock::set_revealed`), `cupertino-keyboard` 62 (`Switcher::press_menu`), `cupertino-input` 83 (input-source actions), `cupertino-theme` 53 (`Easing` with the cosine ease).

### Unit tests

| module | proves |
|---|---|
| general | chord parsing, default and shortcut-map bindings, action to command translation, switcher, the overview's Escape, overview session (frames between origin and target, hit test, window-set replacement), event to state mapping, placement, configuration (JSON, hot-corner dwell and size, effects), cursor frames, layer order, `plan_window`, workspace slide, clip opaque cross, element cache, decorations (insets, cached buffers, double click, hit testing), show desktop and closing fade, desktop entries (parsing, `Exec` splitting, identity matching), icon theme lookup, PNG and SVG loading |
| `kbd` | `keyboard.conf` (repeat info, the newer keys and their errors), modifier profiles, shortcut merge without conflicts and with `none` overrides, caps-lock debounce and hold-to-switch, the 420 ms slow-keys repeat, decoded source groups, input-source MRU and HUD flow and fade |
| `input` | which plans repeat |
| `kbd_ui` | accent picker model: opens on first repeat, preview, commit, restore, per-application switch, popup placement |
| `restart_policy`, `supervisor`, `render::image` | restart rules, supervision, cache signatures |
| `display_power` | timers per power source, the lengthened countdown, inhibitors, the lock after the password delay, system sleep once, the overlay alpha |
| `session_lock` | `a_locked_session_keeps_only_virtual_terminal_switching` covers the shortcuts |

### Integration tests

| file | harness | proves |
|---|---|---|
| `tests/headless.rs` | | Mapped client styled and closed, element order, workspace slide and overview, server-side insets, minimize frames, capture orientation and mesh drawing. **Decorations**: button hover, title bar drag, edge resize, double click zoom, close button, client-side fallback, a double click on a resize edge moves that edge to the usable edge. **Closing ghost** visible, removed after the fade. **`SetOutputRect`**: full screen follows a resized output. **Gestures**: a three-finger swipe switches the workspace, a four-finger spread without a dock only leaves an open overview. **Accessibility**: Reduce Motion draws full screen at once; the display filter pass turns pure red into 0.3 of linear white under grayscale, inverts the output, and is gone with the settings off. |
| `tests/layer_input.rs` | one client, two overlapping overlay surfaces | A layer surface without input at the point lets the one below take the pointer. |
| `tests/pointer_buttons.rs` | | A consumed press also consumes its release. A title-bar press with 2 pt of jitter is a click; the move starts only past 5 pt and catches up with the pointer. |
| `tests/pointer_output.rs` | two displays, a `cupertino-client` client making the launcher's panel | A layer surface without an output maps on the output under the pointer, and the client is told that output. |
| `tests/cursor.rs` | | A line shake grows the cursor five times and it shrinks back once the pointer is still. `cursor_size` 2 doubles it and the shake multiplies that. `shake_to_locate` off ignores shakes. |
| `tests/window_kinds.rs` | a parent and a child toplevel | A child with xdg-dialog `set_modal` maps as a **sheet**: hung centred from the parent's title bar, 32 pt above and transparent when it maps, in place after 0.26 s. The blocked parent's focus goes to it. Its closing ghost slides 16 pt up at half opacity half way, and the parent gets the focus back. The parent dims as far as the sheet is shown; a press on its content is the server's and leaves the focus on the sheet. A plain child opens and closes at once, centred on its parent. |
| `tests/clipboard.rs` | two clients | Text and an image copied in a client that then exits are pasted in another from the server's copy: the text under `text/plain` and `UTF8_STRING`, the private type gone. A cleared clipboard stays empty. A type over `CAPTURE_LIMIT` is dropped and the text kept. |
| `tests/dock_v1.rs` | fake dock | **Overview**: the group on open (windows in stacking order, workspaces, area), `overview_frame` gliding a window, `overview_select` focusing and closing, a dead dock closing the overview with the windows back, workspace add / move / switch / remove, `run_action`, `app_command`, a hot corner and a spread reaching the dock. **Switcher without a dock**: open, step and release commit the focus; End, Home, Right and H work; a held Tab stops at the last entry instead of wrapping (the cause of the old QEMU flake); a click off the panel cancels and focuses nothing. **Switcher with a dock**: list and selection groups in order, `switcher_select` moving the selection and a wrong index ignored, `switcher_activate`, `switcher_cancel`, the state a dock gets when it binds mid-switch, Escape's `switcher_closed(0)`, the panel sent to the dock of the pointer's output only. **Activation**: a token older than the application's last activation only bounces its tile, a newer one activates after 30 s, a token works once, attention for an inactive application only. Also the **role error**, the first window-feed group, the dock's rectangle and exclusive zone, the minimize target and animation, **`activate_app`**, **`restore_window`** (ignored for a window that is not minimized) and **`launching`**. |
| `tests/overlays.rs` | | **Scripted keys**: `--key` / `--type` reach the focused client as evdev key events. **Keyboard**: `repeat_info` 12 / 500, configuration override, the HUD commits on modifier release and fades in 0.3 s, `Escape` cancels, MRU order and the menu-order shortcut, caps-lock delay with real xkb state. **Accent bridge**: a held letter key opens the picker on its first repeat and its release is swallowed, a tapped key never does. The popup anchors at the text input's cursor rectangle, a number key commits `é` after `delete_surrounding_text(1, 0)`, preview is marked text, `Escape` restores. No bridge without a table. A click on a candidate commits it, a click outside closes the popup. **Key filters**: a tapped Shift latches for the next key only, five Shift presses toggle sticky keys, a double tap locks. Slow keys accept a key only after the delay. Mouse keys move the pointer with the verified ramp. The HUD arrow repeats at the configured delay and interval. A fn tap switches the input source, a hold or chord does not. Caps-lock taps switch, holds turn caps lock on. A `decoded:` source replaces the keymap while current. QEMU check `19-keyboard-a11y` proves sticky keys from `keyboard.conf` through the TTY server into a real terminal. |
| `tests/shell_v1.rs` | GLES | `cupertino_shell_v1` backdrops (placement, radius, alpha), double buffering on commit, the three protocol errors, the GLES material pass against the CPU reference (table in *Material pass and the GLES shader port*). Under Reduce Transparency and Increase Contrast a menu bar strip is one uniform opaque colour, and the translucent strip returns when both are off. A layer surface's exclusive keyboard focus is taken on commit and returned when it stops asking. |
| `tests/popup-grab.rs` | GLES, a raw client with a grabbing popup and a subsurface | A press on the popup's subsurface reaches it and keeps the grab; a press outside dismisses the popup and never reaches the toplevel; a session lock ends the grab and the first press after it reaches the lock surface |
| `tests/settings-popup.rs` | GLES, the real Settings binary on a private bus | The popup grab policy: an outside press dismisses the choice menu and is consumed (grabs released), a pane or appearance change removes it, a press on a row selects it, a popup under a resting pointer takes the focus |
| `tests/scroll_v1.rs` | GLES, a client with a scroll object | A touchpad flick reaches the client as `wl_pointer.axis`, `scroll_count`, `wl_pointer.frame` (counts 1 then 2 for a second flick within the gap, none outside that order); a wheel scroll has no `scroll_count`. Scrolls are injected through `Cupertino::pointer_axis`. |
| `tests/session_v1.rs` | observers, a lock client | Appearance and lock state on bind, then changes, `locked` and `unlocked` in order (setting the current appearance sends nothing). A lock whose client crashed and was taken over sends a single `locked`. Every display option change reaches a version 2 observer and no version 1 observer. |
| `tests/window_manager_v1.rs` | a window manager client, two application clients | The first group (usable area, arranged windows, active state) and the `role` error of a second window manager. A strip placement starts where the window was drawn, shrinks on the frame clock and ends on the committed rectangle with the strip's 8 pt corners. A hidden placement fades out before the window is no longer drawn. The window's own frame takes no pointer. A click on the thumbnail sends `strip_pressed` without focusing. `activate_window` focuses and the window returns to the stage. A disconnected window manager leaves every window at its own frame at once. The `toggle-stage` event. Unknown ids ignored. |
| `tests/shell_processes.rs` | supervisor with stand-in scripts, then the real `cupertino-dock`, `cupertino-menubar`, `cupertino-notifications`, `cupertino-windowmanager` and `cupertino-screenshot` binaries | The processes register their roles and own their bus names. The menu bar's labels turn from dark to light when the appearance flips while it runs. The dock maps `cupertino-dock-overview` and every window gets a frame off its desktop one; `SIGKILL` to the dock closes the overview and the windows return. The dock draws a panel-sized buffer on its switcher overlay surface after Super+Tab, gone after the release. The window manager with `stage = on` puts the older of two applications in the strip; killing it (SIGKILL) puts it back at once and the restarted process places it again. Screenshot process: the selection action maps its overlay and a drag writes a 200 x 150 PNG. The toolbar maps at (186, 584), 628 x 104, and Return in the remembered screen mode writes a PNG of the output's size. The clipboard action offers an `image/png` selection, which pasted is a PNG of the output's size. A `dock.conf` written while the dock runs (tile size 64 to 36) shrinks the reserved dock zone by 28 pt and resizes the dock surface. |
| `tests/capture.rs` | clients only | A client of the public socket sees no capture global. A privileged client captures the output at its mode size, with full damage on the first frame and the cursor only on request. A second frame waits for a change and reports only the changed area. A toplevel is captured at its frame size and listed in the window feed. |
| `tests/menu_bar_v1.rs` | | `get_menu_bar`: the first group, the reserved usable area, `role` error, active app / menu address / window state groups, `window_command` (minimize), `move_to_workspace`, `engage` by the shortcut, full-screen hide, reveal by an open menu or the top edge, no input on a hidden bar. A version 3 bar's `screen_capture` groups while a privileged client's output capture session runs and after it ends. |
| `tests/dbus.rs` | private `dbus-daemon`, fake notification, launcher, overlay and login processes | Each call reaches its own process. |
| `tests/display_power.rs` | headless, `fixed_time` | The screen dims (pixels darken), goes black at the display sleep, and an input event restores full brightness. Input while dimmed ramps back up. |
| `tests/quit.rs` | headless, two clients | The most recently used application is asked first and only it gets `close`. One that closes its toplevel is reported quit. One that ignores the request stays the one being quit until its client goes. With no toplevel left the answer is `("", 0). |
| `tests/session_lock.rs` | a toplevel client, a lock client | A lock puts the lock surface under the pointer and on the keyboard and the pixels where the toplevel was. A lock client that disconnects leaves the session locked, black and without focus until a new client takes the lock. A second lock request gets `finished`, and its `unlock_and_destroy` in the same dispatch as the holder's crash leaves the session locked and black. `unlock_and_destroy` gives the toplevel the pointer, keyboard and screen back. Not covered: idle notification (needs a synthetic input backend event) and the TTY backend's locked-frame tracking (QEMU). |
| `tests/output_management.rs` | two displays, a `zwlr_output_manager_v1` client | Both heads listed with modes and positions. Rearranged with a scale change (the model's output rectangles follow). One turned off and on again returns where it was. Turning off the last one fails, in a test and in an apply. A configuration with an old serial is cancelled. A hot-plugged head is announced right of the others and finished when unplugged. A plugged-in display takes its section of `displays.conf`, and an applied arrangement is written to the file and restored on the next plug. |
| `tests/unified_titlebar.rs` | two toolkit clients, one per title bar style | The unified window has no insets and fills its frame; its close button takes the pointer over the surface, the rest of its strip goes to the surface; the standard window keeps its 28 pt server title bar. |
| `tests/wallpaper.rs` | GLES | An image covers the output (cover crop checked on a two-colour picture), the dark appearance takes its own image, a settings change applies live and removing it restores the procedural wallpaper, an unreadable or missing image leaves the procedural wallpaper |
| `tests/live_settings.rs` | headless compositor watching files in a scratch directory; a private `dbus-daemon` for the bus test | Changes to `compositor.json` (appearance, tracking speed, minimize effect, corner radius), `keyboard.conf` (repeat, input sources) and `power.conf` apply while the server runs; a wrong value keeps the last good one; a toggled appearance survives an edit of another key and a removed key returns to its default. `SetAppearance` stores only its key and applies once (the server's own write changes nothing more), `auto` is `NotSupported`; `SetAccessibility` takes the file's keys and values and refuses bad ones; over the bus the properties follow the setters. |
| `tests/mice-property.rs` | GLES, a private session bus | The `Mice` property lists two fake mice (not the touchpad) with the resolution in use and the stored flag, cannot be set, and follows a change of `mouse_dpi` in `compositor.json` |
| `tests/files_window.rs` | real `cupertino-files` (built with its `app` feature), scratch home | The window maps server-side decorated at 920 x 436 titled with the folder, redraws when a file is created, follows Super+Up and a sidebar click in its title, and exits with success on the close shortcut (not restarted). |

## Open issues and approximations

* **Session desktop name.** `XDG_CURRENT_DESKTOP=Cupertino` reaches only the processes the server starts (clients and shell processes); applications started by D-Bus activation or as systemd user units get the environment of the bus or the user manager, so the session's `cupertino-mimeapps.list` and `OnlyShowIn=Cupertino` do not apply to them.

* **TTY backend.** Run only in QEMU (virgl, one connected output), never on real hardware or with two DRM displays; logind refuses control of a session that has a controller, so it cannot run inside the desktop session. Single GPU, hot-plug by rescanning, no mirroring, adaptive sync or custom modes. Output management and output power are proved headless only. Depends on `cupertino-seat`, `cupertino-devices`, `cupertino-evdev`.
* **Menu bar.** The reserved height is the configured `menu_bar_height`, and the bar surface is expected to have that height. The active output follows the pointer. Keyboard focus after a menu closes is the bar process's job (the server hands the keyboard to the bar on `engage` only). A click on a revealed bar overlaying a window also reaches the window's decoration under it.
* **Dock** (`shell/cupertino-dock`). One minimize slot in the protocol, so the next-slot target is used for every window (no `minimize_to_app`). The dock's rectangle (where a minimized window flies in from in the overview) is its committed backdrop. The overview goes to the dock of its output only.
* **Windows.**
  * The kind is fixed at map. A later `set_modal` or parent change updates blocking and placement links, not the transforms. Documents and alerts have no protocol signal, so `Document` and `Alert` are never chosen.
  * A sheet hangs from the model's `titlebar_height` (approximate 28 pt), which fits the server-side title bar but not a client-side header bar (a GTK 4 Wayland dialog overlaps its parent's header bar). Sheets keep the server-side title bar the design's sheets lack.
  * A sheet's parent is dimmed with the `SheetUnderlay` material at the sheet's shown fraction, blended as an ordinary material layer, not the design's behind-window blend (approximate). Its content takes no clicks (the press brings the sheet forward, the title bar still moves the parent).
  * Non-sheet children centre on their parent (approximate). `closing_animation` opts plain windows into a fade. Utility panels float above normal windows and modal panels above both.
  * Every newly mapped window takes the focus, because a client without an activation token (most terminals) would otherwise never get it.
  * The switcher does not step on the scroll wheel, does not mirror the arrows for right-to-left layouts and has no alert sound or file-manager exception for Q. The label's compositing over the panel is approximate (see the dock's README).
* **Stage** (`shell/cupertino-windowmanager`). The alpha values a placement fades to are approximate (1 shown, 0 hidden; recovers the springs only), and the shadow keeps the window's active or inactive parameters instead of moving them with the 0.2 s corner spring as the design does. Thumbnails are flat (no 3D tilt, no app-icon overlay). Dialogs and other child windows of a strip window stay where they are. The strip does not hide when a stage window overlaps it (no edge-dwell path).
* **Effects.** Minimize captures the window each frame (a full offscreen render, fine for a handful of windows). The closing ghost is drawn above all windows. Snapshots are throttled (0.5 s), so a window that closes soon after its last repaint fades out with a slightly older image. The overview does not use the effect meshes (placed frames rescale).
* **Material backdrop.** The capture reads the framebuffer, so material elements drawn earlier in the same frame appear in a later one's backdrop (blur of blur where they overlap). The damage generation keeps this stable.
* **Pointer.** A mouse's delta is taken as raw counts. Touchpad motion and finger scrolling arrive in trackpad counts from `cupertino_input::Touchpad` (400 per inch). Mice use the default acceleration table on the speed of the hand in inches per second: counts over the device's resolution over real time, from the event timestamps summed over 8 ms, so the same hand movement gives the same cursor travel at any dpi and report rate. The resolution is the user's stored `mouse_dpi` entry for the device (key `vendor:product:name`, e.g. `046d:c077:USB Optical Mouse`), else the default entry of the hardware database's `MOUSE_DPI`, else 1000; a mouse's own dpi button is invisible to the host, so the stored value is how the user tells the server. The first report after a pause of a quarter second takes one period of the nominal 67 Hz. With `mouse_acceleration` off the gain is linear, per count of a 400 dpi mouse. The touchpad's scroll is accelerated and then coalesced as the design's event translator does (events are held back until one exceeds 8 counts or 24 counts have summed up, and the sum goes out in whole points); `wl_pointer.axis` carries only the coalesced value. Wheels are neither accelerated nor coalesced (no scroll resolution on the design either); a notch (120 v120 units) scrolls 48 points, three lines of 16, in logical points whatever the output scale, and the v120 value goes along for toolkits that scroll by it. The touchpad's scroll count (the acceleration of repeated flicks) goes to clients through `cupertino_scroll_v1` (see Scroll count above). approximate: whether the design counts before or after the coalescing is not known, so the count is fed the points that are sent, and the events the coalescing holds back are skipped; the count is computed per device, one for both axes. The winit backend sees only absolute host positions, so acceleration is exercised on the TTY backend and in unit tests only.
* **Keyboard.**
  * Third-party clients get Super and Alt chords rewritten by the table §10 (`cupertino_keyboard::rewrite::decide`, applied in `keyboard_event_now`, `forward_rewritten` in `kbd_filters.rs`): Super+letter, digit, punctuation or Return becomes the Ctrl chord (Ctrl+Shift+C / V in terminals), and with an active `zwp_text_input_v3` Super+arrows become Home / End (with Ctrl for Up and Down) and Alt+Left / Right / Backspace / Delete the Ctrl word chords. Ctrl chords, claimed keys, function keys, Tab, Space and `cupertino-` clients are never rewritten. The client gets a `modifiers` event with the rewritten masks, then the key, then the real modifiers after the release. Open: Cmd+Backspace and Option+Up / Down have no toolkit-neutral spelling; the rewrite needs a runtime pass against GTK, Qt, Firefox and Chromium. `Resolver` stays with the first-party clients.
  * Decoded sources: Compose sequences for private dead-key states need the generated table installed as the clients' compose file. The control and command key tables are dropped, the keypad is the generic pc block, and each decoded keymap has a single group (switching swaps the whole keymap).
  * Sticky, slow and mouse keys have no indicator or sounds. The mouse-keys alternate letter keys are not implemented. HUD mouse selection is not wired. The fn key's `emoji` / `dictation` actions have nothing to open.
  * The accent picker needs a client with `zwp_text_input_v3`, and the popup falls back to the window centre without a cursor rectangle. Candidate lists are user data. The bridge occupies the seat's single input-method slot whenever an accent table is configured.
* **Session lock.** `locked` waits for a presented locked frame per output (page flip on the TTY backend, swap elsewhere), but a frame already queued when the request arrives is not cancelled, so one old frame may reach the screen before the black one. Smithay never clears its list of locked outputs after a lock client died; harmless, because the list is keyed by per-client output objects. Lock surfaces are drawn from the request on, before `locked`, which the protocol allows. Presentation feedback and frame callbacks of hidden clients wait for the unlock. Touch and tablet events are not routed, locked or not.
* **Launcher and notifications** (`shell/cupertino-launcher`, `shell/cupertino-notifications`). The server only renders their backdrops. The launcher panel opens on the output under the pointer.
* **approximate or unknown values.** The overview's spring form (its duration is as designed), gesture distances and finger counts, hot corner size, edge reveal delays of the menu bar, bar and popup colours and radius, popup shadow class, status icon size, snapshot interval, hidden-window frame callback throttle (1 s), wallpaper colours and the tint layer colour (wallpaper average), continuous corner expansion.
