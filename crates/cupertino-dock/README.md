# cupertino-dock

The dock process of the Cupertino desktop. It stands in for the design's dock process, and it also hosts the application
switcher panel, the application grid and the Overview chrome. The crate holds the model (tiles, layout, magnification, bounce,
auto-hide, with no compositor dependency) and the `cupertino-dock` process (feature `app`), a client of the server on `cupertino-wayland-client`,
`cupertino-shell-client` (layer surfaces and the dock role) and `cupertino-protocol-app` (backdrops), with a
`cupertino-ui-wayland` presenter per surface.

## Running

Build with `cargo build -p cupertino-dock --features app` (`--features app`). The server starts the process from the `"shell"`
key of `compositor.json` and restarts it after a failure.

Command line: `cupertino-dock [--terminal PROGRAM]`.

`--terminal` names the terminal program that runs `Terminal=true` entries. The dock also tries it first when it picks default
pins.

## Force Quit

The Force Quit Applications dialog is a centred overlay layer surface of this process. The server opens it with the `force_quit_dialog` event (`Super+Alt+Escape`). The list holds the running applications by application id, alphabetical without regard to case with the file manager last; the first one the server reported as unresponsive (`unresponsive` event) is selected and shows `(not responding)` after its name. Click selects a row, Ctrl or Super toggles it, Shift takes a range, and Shift with the arrows grows the selection. Force Quit (Return) needs at least one selected row and opens the second step, a smaller panel that asks about one application or about the N selected ones; its Force Quit sends one `force_quit_app` per application, and Cancel is the default. An unresponsive application's tile menu starts with a disabled "Application Not Responding" row and offers Force Quit in place of Quit; it opens the dialog on the second step.

When `CUPERTINO_DOCK_FIXTURE_UNRESPONSIVE=id,id` is set, `capture.sh` builds a separate dock with `--cfg cupertino_capture_fixtures` (a `RUSTFLAGS` cfg, not a cargo feature, so `--all-features` builds never contain it) that marks those application ids unresponsive at startup.

| Setting | Effect |
|---|---|
| `~/.config/cupertino/dock.conf` | The configuration file, read and written by `cupertino_defaults::dock` (with the triggers and actions, the size defaults and their validation) and applied by `src/conf.rs`. `$CUPERTINO_DOCK_CONFIG` overrides the path. The dock never rewrites it (the settings application does). A line that is not understood is reported on stderr with its number and leaves that key's default. |
| `~/.config/cupertino/dock-pins` | The pins as the user left them in the dock, one key per line, written atomically after a drag-reorder, a drag-out removal, Keep in Dock and Remove from Dock. When it exists it replaces the pins of `dock.conf` (an empty file is no pins). `$CUPERTINO_DOCK_PINS` overrides the path. |
| bare lines, `pin = ...`, `pinned = ... | Pinned applications, named by desktop file ID, `app_id` or command. Without pins the dock pins up to four installed applications after the file manager: the terminal, then a candidate list (approximate). |
| `tile_size`, `large_size` | Tile and magnified tile size (defaults below). |
| `magnification`, `autohide`, `recents` | Flags. |
| `position` | `bottom`, `left` or `right`. |
| `corner_top_left`, `corner_top_right`, `corner_bottom_left`, `corner_bottom_right`, `swipe_up`, `swipe_down`, `pinch`, `spread` | Hot-corner and gesture actions: `none`, `app-grid`, `exec:<command>`, or a server action name such as `overview`, `app-overview`, `show-desktop`, `launcher`, `lock`. |
| `XDG_ACTIVATION_TOKEN`, `DESKTOP_STARTUP_ID` | Set by the dock, to the xdg-activation token, in the environment of every application it launches (the command and the detached start are `cupertino_apps::launch`, shared with the menu bar). |

Preference defaults: tile size 64 (52 for displays at most 800 pt tall,
`DockConfig::for_display_height`), large size 128, magnification off, launch animation on, auto-hide off, auto-hide delay
0.15 s, time modifier 1.0. Validation is `sanitize_tile_size`, `sanitize_large_size` and `DockConfig::from_preferences`.

## How it works

### Model

| Part | Contents |
|---|---|
| Tiles | Pinned and running apps, running indicator, launch / attention bounce state, recents section, separators, minimized-window tiles, trash. Tiles appear and disappear with an animated presence. |
| Layout | `Dock::layout(screen, now) gives the bar and every tile rectangle for bottom, left and right orientation, including auto-hide offset and magnification. |
| Hit-testing | `Dock::hit_test(pointer, screen, now) on the magnified layout. |
| Minimize target | `Dock::minimize_target(window, app, screen) returns the rectangle a minimizing window flies to. It matches the tile rectangle once the tile has appeared. The process reports the next minimized-window slot, since the protocol has one target. |
| Drag-reorder | `begin_drag` / `drag_move` / `end_drag` with a gap placeholder, section limits, removal when dragged out or dropped on the trash. `Dock::pinned_keys` lists the pins in order for storing. |
| Auto-hide | `AutoHide` state machine and `reveal_zone`. |

**Magnification.** The default profile is the half-sine fisheye, `FisheyeProfile`.
Both edges of every tile move by `real_scale · sin(π·d / (2·fish_size))`, clamped to ±`real_scale`, with
`fish_size = 3 · tile` and `real_scale = (large − tile)/2 / sin(π·tile / (4·fish_size)) · fish`. The tile under the pointer
centre is exactly `large_size` wide (tested for several sizes). Neighbours at ±1/±2/±3 tile widths are 1.87/1.5/1.07 × the
tile size for 64/128, and the dock grows by `2·real_scale` (247 pt for 64/128). The magnification amount ramps with the cosine
ease `0.5 − 0.5·cos(πt)` over the duration formula. The raised-cosine `CosineProfile` stays
available through `Dock::set_profile` as an alternative. Custom curves implement `MagnificationProfile`; a profile that
displaces edges overrides `layout`.

**Auto-hide duration**. `autohide_hide_ms` / `autohide_show_ms` implement
`floor(ln(dist·0.5 + 1) + 1) · 60 · time_modifier` ms: 240 ms for the default dock, 300 ms with the magnification open.

**Bounce.** `Bounce` / `BounceConfig`. The default `BounceModel::Verified` is the curve; what starts and stops it is (§7.1). Launch is
a parabola with a 700 ms cycle capped at 120 s and lift `0.2·(len − 8) + 6` pt. Attention is three hops of 500/326/212 ms
with amplitudes 1/0.5/0.25, repeating every 2000 ms with lift `len + 1` pt (informational plays it once, critical loops).
`BounceConfig::parametric keeps the old generic arcs (approximate).

**Minimize effect durations**. `MinimizeEffect::duration_ms` gives warp 500, scale 200 and funnel
400 ms, doubled with the slow-motion modifier.

### Bar and tiles

The process (`src/app.rs`, `render.rs`) draws the bar and tiles on a layer surface of the first output. The surface is at the
bottom, left or right per `Orientation`, full width or height along its edge so magnification and bounces have room. It uses
the `Menu` material as a server-rendered backdrop and takes the dock role (`get_dock`). The exclusive zone is the bar's
thickness, or 0 with auto-hide. The input region is the bar and its tiles, or only the reveal strip while hidden.

- A click on a running application's tile calls `activate_app`.
- A click on a pinned application that is not running launches its desktop entry with an xdg-activation token and bounces.
- A press on an application or recents tile acts on release. Past 5 pt on either axis from the press point
  (`tile_drag::DRAG_THRESHOLD`) the press is a drag-reorder; held in place for 0.5 s it opens the tile's
  menu instead (`HOLD_MENU_DELAY`). A drag follows the design rules of §7.2:
  - The icon keeps the place it was grabbed at, at the size it had, and follows the pointer without lag. It, its
    "Remove" label and the removal puff are drawn on a surface over the whole output (`cupertino-dock-drag`, no input), so
    they can leave the bar's surface; the server keeps the pointer on the bar's surface until the release.
  - The gap is the slot of the tile under the pointer: before its middle the gap goes before it, from its middle on after it.
    It is open only within one tile of the screen edge, before the trash and over the tile's own section; otherwise the
    neighbours close up and a release puts the tile back. Neighbours move to their new places in 270 ms on the
    cosine ease.
  - A release settles the icon into its slot in 350 ms on the cosine ease, its running indicator fading back in.
  - A release removes the tile (unpins it, stores the pins) when the pointer is more than one bar thickness beyond the bar,
    across the bar, and at least 0.5 s have passed since the drag began, or when it is on the trash. Past that
    point the icon dims to 0.35 and the "Remove" label shows, over 0.25 s ease-in-ease-out; a removal fades the
    icon and label out over 0.333 s on the curve. Removed running applications keep an unpinned tile.
  - approximate: the look of the "Remove" pill (fill, padding, gap to the icon), the removal puff (a ring of soft circles, as
    long as the fade; the design's Dock draws none and the source of its cloud, if any, is outside it), magnification is
    released for the drag where the design keeps the fisheye following the pointer, and the menu bar and window tiles
    are not special cases of the removal rule.
- A click on a minimized-window tile calls `restore_window` with its rectangle.
- A click on the trash (and its menu's Open) opens `trash:///` in the session's file manager, `cupertino-files`, through
  the `%U` of its desktop entry, with an xdg-activation token, and bounces its tile like a launch; without that entry
  `xdg-open trash:/// asks the session's default handler.
- `set_minimize_target` follows the slot the next minimized window takes.
- `launching` from the server makes a tile bounce until a window of that application appears. The server sends it when it
  starts a program that has a desktop entry and when an `xdg-activation-v1` token names an application (`set_app_id`: the
  launcher, Open With, GTK's launch context). An application that appears with no such signal (started from a terminal)
  bounces for one cycle at its first window; the windows listed when the dock starts do not bounce; a second signal for a
  bounce already running does not restart it.
- `attention` (the server refused an application's activation) bounces its tile once (informational attention).
- An application's own icon comes from `cupertino-icons` (its desktop entry's `Icon=`: an absolute path or `hicolor` only); the trash is the asset `trash.empty` or `trash.full` (the home trash folder has an entry or not, looked at once a second).

**Session.** The indicators and separators follow the appearance the server announces over `cupertino_session_v1`, read at
start and redrawn on every change. The grid redraws too. `locked` removes an open grid. The bar and the switcher panel ignore it.

### Application switcher

Code: `src/switcher.rs`, `render::paint_switcher`, the overlay in `app.rs`. The server keeps the keys and the choice and sends
the panel's output, the list and the selection (`switcher_output`, `switcher_changed`, `switcher_closed`). The keys (Q / H among
them) are the server's.

The dock draws the panel on an overlay layer surface (namespace `cupertino-dock-switcher`) on that output. That is the output
under the pointer when the switch opened, not necessarily the dock's. The panel is centred whatever the dock bar and the menu
bar reserve (exclusive zone -1) and uses the `Switcher` material as a server-rendered backdrop.

- The panel appears 150 ms after the switch opens, so a quick tap never shows it. It fades out in 0.25 s
  ease-in-out when it closes. A stepping change after the fade began cancels the fade.
- Tiles, selection highlight and label follow. The label shows the desktop entry's name.
- The pointer moving over a tile (after a 3 pt box of hover slack) or pressing it sends `switcher_select`.
- Releasing the left button on a tile sends `switcher_activate`, and the switch commits at once. Releasing off the tiles sends
  `switcher_cancel`. A press outside the panel is the server's and cancels there.
- An application with no icon of its own (a running app without a desktop entry, for example) is drawn with the asset `application.generic` in the dock, the switcher and the app grid; an asset directory without that icon (or without `trash.empty` / `trash.full`) is an error at startup (`Icons::new`), never a stand-in drawn later.
- A running application lights the pinned tile of the same desktop entry (`Session::key_of`: tile id, entry id, `StartupWMClass`, aliases of the same entry). An application with no desktop entry cannot be matched to any pin and gets its own tile; installing a desktop entry for it (`Exec`, `Icon`, `StartupWMClass` = its `app_id`) is what merges it with a pin.
- The label is drawn without the plus-darker blend of the real panel, so it is faint over a dark backdrop (approximate).

### Application grid

Code: `src/app_grid.rs`, `render::paint_app_grid`, the `Grid` in `app.rs`. The server forwards the app-grid action (shortcut,
hot corner, four-finger pinch) as `toggle_app_grid` and keeps nothing else.

**Surface.** The dock opens the grid on an overlay layer surface (namespace `cupertino-dock-app-grid`) anchored to every edge of
its output. The surface starts 30 pt below the top (the design's top band, which keeps the menu bar visible) and
sits inside the dock bar's exclusive zone, so the bar stays visible. It takes exclusive keyboard focus and the `AppGrid`
material as a server-rendered backdrop (blur 30, saturate 1.8, gain 0.55 / bias 0.05292).

**Content and geometry.** Every desktop entry without `NoDisplay` is listed, sorted by name (the design keeps the user's
arrangement: approximate), on pages of 7 × 5. The geometry is the design's layout routine on the surface as the
layer. The dock bar's zone stands for the design's dock extent, an mapping.

- Side margins are `max(90, m/2)` widened by the zoom factor, with 35 pt top and bottom minimums.
- Cells are `trunc(page / 7 | 5)`, and the icon is `min(cell) − 46` through the icon rule.
- The label is 13 pt medium white, 22 pt tall, with its black 0.2 shadow (blur not drawn).
- The page dots are 15 × 15 at pitch 20 (white, others at 0.2).
- The search field is 250 × 25, 15 pt above the page, with a light 16 pt text.

**Motion.** The grid fades and scales in from 1.05 over 0.25 s ease-in, and out the reverse way. A page flip takes
0.5 s on the curve. Under Reduce Motion (the session's display options) the grid keeps its opacity
animation and skips the scale step.

**Input.**

- Typing filters with the search rule of: title, word, camel-case and initials prefixes, the space-stripped title,
  then the desktop file ID as words. Diacritics are not folded (approximate).
- Arrows move the selection and follow it across pages. Page Up / Down and Home / End flip. Return launches the selection (or
  the first result of a search). Escape closes.
- The wheel accumulates twice the dominant delta and flips one page 75 ms after the burst (positive flips forward,
  ).
- A click on an item launches it through the tiles' path (desktop entry, activation token, bounce) and closes the grid. A click
  on a dot flips the page. A click on the search field does nothing. A click anywhere else closes the grid.
- The grid also closes when the keyboard leaves it, and goes at once, without the exit animation, on `locked`.

approximate chrome: the keyboard selection (white 0.2, radius 8), the search field's fill, stroke and inset, the placeholder's
alpha, "No Results" as white at gray 0.204, end-of-text truncation instead of the middle.

### Overview

Code: `src/overview_ui.rs`, `render::paint_overview`, the `OverviewSurface` in `app.rs`. The server owns the session: the
transition, the windows' frames and their animation, Escape, the click fallback, and closing on a workspace switch and when
the dock dies. It sends the dock of its output the window set with desktop frames, the workspaces and the usable area
(`overview_changed`).

The dock lays the windows out with the design packing (`cupertino-overview`,: inset 20, gap 8, lift 20, tolerance 4,
shrink 0.9, 499 passes, constants; the application mode puts minimized windows in a lower row) and answers one
`overview_frame` per window. A changed set is laid out again and resent.

The chrome is a keyboard-exclusive overlay surface over the whole output (namespace `cupertino-dock-overview`, exclusive zone
-1, so surface and output coordinates agree). The server draws the dim and the windows. The chrome has:

- the workspaces bar: slots with "Desktop n" labels and a border on the current one, and an add slot with a plus. It stays
  expanded for the whole overview, where the design collapses it until hovered (approximate);
- a remove button on the hovered slot;
- the selection highlight, from hover or the keys (arrows, `Tab` / `Shift+Tab`, `Return` / `Space` select, `Escape` leaves);
- a 13 pt title under every window (colours and sizes approximate).

The chrome fades out over 0.25 s on `overview_closed`, and then the surface goes.

### Hot corners and gestures

Code: `src/actions.rs` (the triggers, actions and defaults are `cupertino_defaults::dock`). The server detects them and sends `hot_corner` / `gesture`. The dock maps them with the eight trigger
keys of `dock.conf` and runs the result: nothing, its own grid, a program through `sh -c`, or `run_action`. Defaults: no corner
does anything. Swipe up is overview, swipe down is application overview, pinch is application grid and
spread is show desktop (roles).

### Context menus

Code: `src/menu.rs`, `render::paint_menu`, the `Menu` in `app.rs`. A right click on a tile or the separator opens a popup of
the bar surface with a grab (`Menu` material).

- **The file manager tile.** The session file manager (`cupertino-files` when installed, else the first installed of
  Nautilus, Dolphin, Thunar; the list is approximate) is built into the dock as its first tile, as the design's is: that
  tile is none of the 20 `persistent-apps` of its `default.plist` and its bundle path is a literal of the dock binary
  (that it makes this tile ). It is always present, also while the file manager does not run, never written to
  `dock-pins` and never removed: its menus have an empty Options submenu and no Quit, so it offers Show
  All Windows and Hide while it runs and Open while it does not; the design's Relaunch is not offered (approximate). It
  does not drag, and nothing drops ahead of it (first place and the fixed position ). A pin of the same application
  in an older `dock-pins` (or `dock.conf`) is skipped, and drops out of `dock-pins` the next time the dock writes it.
- **Tiles.** Keep in Dock / Remove from Dock (pin and unpin for the session; `dock.conf` is not rewritten), Show in
  File Manager (the desktop entry's directory, in `cupertino-files` like the trash, else `xdg-open`), then Show All Windows (`app_command show_windows`, disabled while
  the application does not run), Hide and Quit (`app_command`), or Open (launch). The trash offers Open. Separators and
  minimized-window tiles have no menu.
- **Separator.** Turn Hiding On / Off, Turn Magnification On / Off, and the position as disabled radio items. The bar
  is anchored at start, and the position is `dock.conf`'s.

### Live settings

The dock watches `dock.conf` (`cupertino_defaults::watch`) and applies a change at once: the file is read again with the
current values as the last good ones (a bad line is reported and keeps its value), and only the keys whose value changed
apply; a key removed from the file goes back to its default. Tile size, magnified size, magnification, position and
recents change the layout, the surface (`LayerSurface::set_placement`; the separator menu's magnification item resizes it the same
way) and the exclusive zone; auto-hide switches as the
separator menu switches it; the corner and gesture actions take effect at the next trigger. The pins of `dock.conf` apply
at the next start (the dock keeps the pins the user changes in `dock-pins`). A setting the user changed in the separator
menu stays until its own key changes in the file.

## Protocols / D-Bus it speaks

The process speaks the server's dock protocol and `cupertino_session_v1`, and has no D-Bus dependency. The messages are named
where they are used above. In the Overview chrome a press on a window sends `overview_select` and on empty space
`overview_close`. A press on a slot sends `switch_workspace` when released in place, or `move_workspace` when dragged past 4 pt
to another slot (approximate slack). The add slot sends `add_workspace` and the remove button `remove_workspace`.

**Text.** The `ui` font is a variable font whose optical size follows the point size, so every `TextRenderer` owner calls `set_scale` with the output scale before it draws and measures (`LabelText::width` measures in points at scale 1, the painters draw at `scale`): text is laid out in points at the same optical size it is drawn with, at every output scale.

## Open issues

- **Not wired.** A recents source, window thumbnails in minimized tiles and in the
  workspaces bar, a key to toggle auto-hide, and changing the position at run time.
- **Switcher.** The all-displays preference, scroll, and drag and drop.
- **Application grid.** Folders, edit mode and drag, the pinch spring, mouse-drag paging and the rubber band, and
  the Shift slow-motion durations.
- **Overview.** Thumbnails of the workspaces are not drawn, and the slots are plain (approximate).
- **Context menus.** Deviations (approximate): the Options submenu is flattened. There is no Open at Login (no login-items
  store). Show Recents, Dock Settings, Minimize Using and the Option-key alternates are absent. Metrics and colours are the
  menu bar's. The desktop background menu has no surface to live on and is not done.

## Tests

```
cargo test -p cupertino-dock --features app
```

`cargo test -p cupertino-dock --features app` runs 188 tests; without the feature the process code is skipped.

| Test | Proves |
|---|---|
| App grid unit tests | The layout against the computed table of the app-grid (§5.5), the icon rule, item and dot hit-testing, the search rule, typing and the ID fallback, arrows and page keys, the flip curve, the wheel burst, clicks, the entry and exit visuals, and the painted tiles, dots, selection and search field. |
| Switcher unit tests | Layout, hit-testing, delay and fade timing, hover slack, the painted highlight and fade. |
| Magnification unit tests | The ramp, and the fisheye: exact large size under the pointer, neighbour sizes, growth, monotonic fall-off, continuity. |
| Animation unit tests | The bounce curves, the cosine ease and the auto-hide durations. |
| Tile drag unit tests | The press that becomes a drag past the threshold, the held press, and the removal puff's timing. |
| `tests/dock.rs` | Sections, orientations, hit-testing, minimize targets and drag-reorder: the grab, the gap rule, the slide, the strip and section limits, the removal gate, the dim, the settle and the removal fade. |
| `tests/session.rs` | Windows and tiles, the launch bounce (announced, not doubled, unannounced, not for the first list) and the refused-activation hop. |
