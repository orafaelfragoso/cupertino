# cupertino-dock

The dock process of the Cupertino desktop. It stands in for the design's dock process, and it also hosts the application
switcher panel and the Overview chrome. The crate holds the model (tiles, layout, magnification, bounce,
auto-hide, with no compositor dependency), the CupertinoKit application that draws it (`src/kit.rs`, the model and its
messages, and `src/view/`, one pure view function per window) and the `cupertino-dock` process (feature `app`, `src/app.rs`),
which runs that application on `cupertino-ui-wayland` and gives its windows layer surfaces and the dock role through
`cupertino-shell-client`.

Everything the process shows is drawn through Kit: the bar, the context menus, the tile held by a drag, the Force Quit
dialog, the switcher panel (`switcher_kit`) and the Overview chrome (`overview_kit`). The process paints
nothing itself; `src/icons.rs` only loads the icons the views draw.

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
| Tiles | Pinned and running apps, running indicator, launch / attention bounce state, recents section, separators, folder tiles (stacks), minimized-window tiles, trash. Tiles appear and disappear with an animated presence. |
| Layout | `Dock::layout(screen, now) gives the bar and every tile rectangle for bottom, left and right orientation, including auto-hide offset and magnification. |
| Hit-testing | `Dock::hit_test(pointer, screen, now) on the magnified layout. |
| Minimize target | `Dock::minimize_target(window, app, screen) returns the rectangle a minimizing window flies to. It matches the tile rectangle once the tile has appeared. The process reports the next minimized-window slot, since the protocol has one target. |
| Drag-reorder | `begin_drag` / `drag_move` / `end_drag` with a gap placeholder, section limits, removal when dragged out or dropped on the trash. `Dock::pinned_keys` lists the pins in order for storing. |
| Auto-hide | `AutoHide` state machine and `reveal_zone`. The tile's name fades as the bar hides (its opacity is one less the hidden share), and the name that showed as the pointer left stays over its tile, riding the bar down, until the bar is hidden or the pointer comes back. |

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
a parabola with a 710 ms cycle capped at 120 s and lift `0.2·(len − 8) + 6.7` pt (17.9 at 64); the name over the tile rides it. Attention is three hops of 500/326/212 ms
with amplitudes 1/0.5/0.25, repeating every 2000 ms with lift `len + 1` pt (informational plays it once, critical loops).
`BounceConfig::parametric keeps the old generic arcs (approximate).

**Minimize effect durations**. `MinimizeEffect::duration_ms` gives warp 450, scale 200 and funnel
400 ms into the slot, doubled with the slow-motion modifier; the warp unfolds back out of the slot in 420 ms.

### Bar and tiles

**Geometry.** Everything scales with the tile size; at the default 64 pt:

| Part | Size |
|---|---|
| Tile | 64 pt; its icon is drawn 52 pt square in the middle (`ICON_ARTWORK`), cropped to the icon's drawn shape so every icon's artwork fills that square whatever margin its file leaves (`icons::filled`: an icon with a margin is loaded again larger and cropped, so it stays sharp) |
| Gap between tiles | 4 pt (`TILE_GAP_RATIO`): tiles stand 68 pt apart; a pointer in a gap is on the nearer tile |
| Separator | a 26 pt slot (`SEPARATOR_RATIO`), so the tiles either side stand 34 pt apart, with a 1 pt line 62 pt long in its middle |
| Bar | 84 pt deep: 10.5 pt of padding at its ends, 10 pt between the tiles and its screen-edge side and 11 pt on the other (`BAR_PADDING_RATIO`, `BAR_EDGE_PADDING_RATIO`); continuous corners of 19.6 pt reaching 30 pt along each side (`BAR_RADIUS`); 5 pt from the screen edge (`EDGE_MARGIN`) |
| Running indicator | a 4 pt dot 2 pt below the tile, its middle 6 pt from the bar's edge; in the dark appearance over glass the compositor draws it (`Surface::DockIndicator`), lightening the glass behind it, and otherwise the dot paints itself |
| Badge | an application's count, a 23 pt red circle (a capsule for more than one digit, 6.5 pt either side of the count) with white 13 pt medium figures, its middle 5.75 pt inside the icon's top trailing corner, so a quarter of it overhangs each side (`badge::rect`); counts above 9999 read 9999+ |
| Minimized window | the picture the server drew of the window when it minimized (shell version 22), fitted into 60 × 52 pt across the tile, its middle 1.25 pt above the tile's, and the application's icon over it with a 19.5 pt artwork, 3.25 pt inside the tile's trailing edge and 4.5 pt above its bottom (`minimized.rs`); without a picture, the application's icon at full size |
| Edge and rim | over glass, the glass's own rim light, along the top and the bottom and fading around the corners; where the bar is not glass (Reduce Transparency, no desktop effects), a 0.5 pt dark hairline just inside the outline and inside it a 0.5 pt light rim that is brightest along the top and the bottom and fades out down the sides; Increase Contrast doubles the hairline and draws the rim all round, over glass too |

The bar's material is `view::bar::BAR_SURFACE` (the `Dock` surface): glass in the bar's shape, with Reduce Transparency
the `Dock` body in the colour of the wallpaper under it, and the window's opaque fill where the desktop shows no
material. The stack panels take the same surface. Giving the bar another material is that one line.

The bar is a Kit window (`src/view/bar.rs`) on a layer surface of the dock's display. The surface is at the bottom, left or
right per `Orientation`, full width or height along its edge so magnification and bounces have room. It has the
`BAR_SURFACE` material under the bar's own rectangle only, asked for by the bar's background node (`PaintCx::material`), and takes the dock role (`get_dock`). The
exclusive zone is the bar's thickness, or 0 with auto-hide. The input region is the bar and its tiles, or only the reveal
strip while hidden. Each tile is a node at the rectangle `Dock::layout` gives it and draws its icon; the bar draws the
running indicators and the separators under the tiles.

**Displays.** The dock starts on the first output, the main display. Every other output gets an edge strip: a layer surface
one point deep along the dock's edge (`cupertino-dock-edge`, `src/view/edge.rs`) that draws nothing (`WindowBackground::INPUT_ONLY`,
one transparent pixel the compositor stretches) and holds that output's
dock role. When the pointer rests on a strip for 0.4 s (`kit::EDGE_DWELL`), the bar moves to that output and the output it
left gets a strip. Passing over a strip does not move the bar, and neither does a drag in progress or a locked session. When
the dock's output goes away the bar moves to the first remaining one. The window list is taken from the bar's role only; the
switcher, the Overview, hot corners and gestures are taken from the role of the output they happen on.

**Keyboard.** The Focus Dock shortcut (`focus-dock`: Ctrl+F3 under the `cupertino` profile, Super+F3 under `classic-pc`) makes the server send `engage` to the dock of the
output under the pointer. The bar then takes the keyboard with the first tile focused. The arrows along the bar move the focus,
Home and End go to the ends, Return or Space activates the tile, the Menu key or Shift+F10 opens its menu, and Escape
gives the keyboard back. Activating a tile gives it back too. Every `engage` starts on the first tile, whatever was focused
the last time, with the focus ring shown at once. When the focused tile goes (its application quits, its window is
restored), the focus moves to the tile after it, or else the one before it, never to the bar itself. A press outside the bar
and its menu ends Focus Dock: the server gives the keyboard to the window (see the server README, *Engaged shell layers*),
and the bar, told it lost the keyboard, gives up its claim. The one keyboard leave that keeps Focus Dock is the bar's own
menu taking the keyboard as it opens.

**Accessibility.** The bar is a toolbar named "Dock". Every tile is a button named after its application (the desktop
entry's name), "Trash", or the minimized window's title, with the actions Click and Show Menu. Its description says what
the tile shows: "Running", "Opening", "Not responding" or "Needs attention" for an application, "Minimized window" for a window, and "Contains items" for a trash with something in it. Separators
are separators. The menus are Kit menus, and the Force Quit rows and buttons are named
nodes with a Click action. The Force Quit dialog reads its heading, its footer and its empty or unsaved-changes notes
as text; its rows are a list named "Applications" whose items say their place and count, and the selected row takes
the keyboard focus, so a screen reader follows the arrows. The switcher's entries say their place and count. The
edge strips are groups named "Dock edge".

**Reduce Motion, Reduce Transparency and Increase Contrast.** These follow the session as in the other Kit shells. With
Reduce Transparency or Increase Contrast the theme gives the bar, the menus and the dialog an opaque fill instead
of a material, and Increase Contrast draws the running indicators fully opaque and the separators at twice their alpha.
The bar follows Reduce Motion too:

- Magnification follows the pointer at once instead of ramping in and out.
- A launch or attention bounce dims the tile in place: its opacity dips to half at the top of each hop, so the tile does
  not rise.
- The launch hop of an application that appears unannounced becomes the same dim.
- The server's minimize effect becomes the scale effect (server README, *Minimize*).

- A click on a running application's tile calls `activate_app`; a click on a resident one without windows asks it for a
  window (see *Residency*).
- A click on a pinned application that is not running launches its desktop entry with an xdg-activation token and bounces.
  **Launching without a token** (decision): when Kit gets no token (the bar's window is gone, or the compositor offers
  none), the program still starts, without `XDG_ACTIVATION_TOKEN`, and the tile still bounces. The token decides whether
  the new window takes the focus, not whether the program may start: the server maps a window without a valid grant
  below the key window and bounces its tile (server README, *Activation*), so the launch only loses its focus, and the
  click is never dropped.
- **Name label.** The tile under the pointer shows its name over it at once (`src/view/label.rs`): a popup of the bar
  that takes neither the pointer nor the keyboard, a 25 pt capsule with the name at 13 pt and 13 pt either side, and a
  16 pt wide, 6.5 pt deep pointer under its middle whose tip stands 3 pt off the tile (beside the tile, without the
  pointer, when the dock is on a side). It is a translucent fill with a 0.5 pt outline, light or dark with the
  appearance, and follows the pointer from tile to tile. It does not show over a separator, with a menu or a stack
  open, during a press or a drag, or while the session is locked. For assistive technology it is a tooltip.
- **Recents.** With `recents` on (off by default), the bar has a section of up to three recent applications
  (`RECENTS_MAX`) after the applications, between two separators, before the minimized windows and the trash. It holds
  the applications that quit since the dock started, the most recent first, then the launcher's frecency history, each
  once, each by its desktop entry, and none with a tile among the applications (pinned or running). It is worked out
  again, and the history read again, only when an application gets or loses its tile or one quits. A recent tile opens
  and starts its application as a pinned one does, and is named after its entry. **Decision:** an unpinned application
  that runs keeps its tile among the applications, after the pinned ones, rather than in the recents section; when it
  quits it comes into the recents.
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
  bounce already running does not restart it; an application that already shows a window does not bounce (the launcher
  asks for a token before the server brings the application forward, and no first window would end the bounce).
- `attention` (the server refused an application's activation) bounces its tile once (informational attention).
- An application's own icon comes from `cupertino-icons` (its desktop entry's `Icon=`: an absolute path or `hicolor` only); the trash is the asset `trash.empty` or `trash.full` (the home trash folder has an entry or not, looked at once a second).

**Residency.** From shell version 19 the server tells the dock which applications are resident
(`app_resident`): a process of the application that showed a window is still connected, with or without windows now. The
running dot follows it:

- An application whose last window closes while its process runs keeps its dot. It does not quit, so it does not bounce again
  when its next window comes.
- Opening it (a click on its tile, its menu's Open) asks that process for a new window. When the desktop entry
  says `DBusActivatable=true`, the dock calls `org.freedesktop.Application.Activate` on the bus name of the desktop file ID
  (`cupertino_apps::activate`, on a thread of its own; the launcher shares it). The activation token goes in the platform data as `activation-token` and
  `desktop-startup-id`. Otherwise, or when the call fails or is not answered within 3 s (`ACTIVATE_TIMEOUT`), the dock
  starts the entry again with an activation token, and a running instance opens a window. Neither bounces the tile: the application is already running.
- Only an application the desktop shows stays without windows: a process with no desktop entry, or with a `NoDisplay`
  one (a portal's dialogs, a helper), is in the dock while it shows a window, as before.
- The dot goes when the server reports the application no longer resident (its process exited or crashed) and it has no
  window left.
- No duplicate launches. A second ask for the same tile within 5 s (`OPEN_GUARD`) is dropped while no window has come,
  and so is one whose activation token is still on the way. The guard ends early when the started process fails (an
  unsuccessful exit status or a signal) before a window comes, so asking again starts it again; one that exits
  successfully keeps it, as a launcher that starts the application in the background or hands the open to a running
  instance does before its window comes. An earlier launch's exit leaves a later launch's guard alone. The wait for the activation token is bounded: after 2 s (`TOKEN_TIMEOUT`) the program starts
  without one, and a token that comes later is dropped.
- No false dots. A new bar role (the dock moved to another display, or the server restarted) gets every resident
  application in its first group; one the dock still held that is not listed has gone.
- With a server older than version 19 there is no residency: the dot goes with the last window, as before.

**Animation and repaints.** What moves by itself (bounces, slides, auto-hide, the drag's settle,
the switcher's fade, the overview's glide and fade) runs on the frame callbacks of the window that shows it: the window's
root asks for the next frame while its view moves, and every frame wakes the model (`Input::Frame`). A slow wake every
0.1 s (`kit::FRAME_FALLBACK`) keeps it going if the frames stop (a surface the compositor does not show). Delays are
timers: the switcher's, the held press and the edge dwell. Reduce Motion applies as before. A window repaints
only the nodes whose view changed: the bar's material, marks and tiles are separate nodes, so the pointer moving over the
bar with magnification off repaints nothing, and a desktop entry's name is looked up once per application. The bar does
present again when its surface is given a new placement or keyboard interactivity, which apply with the surface's next
commit (a magnification change alters the surface's depth, not what the bar draws).

**Session.** The indicators and separators follow the appearance the server announces over `cupertino_session_v1`, read at
start and redrawn on every change. `locked` removes the Force Quit dialog, a menu, a folder's stack and the edge strips at once, and the bar gives the keyboard back. The bar and the switcher panel stay.

### Folder stacks

Code: `src/stack.rs` (entries, layout, entry motion), `src/view/stack.rs` (the window) and the stack part of `src/kit.rs`.
The user's downloads folder (`XDG_DOWNLOAD_DIR` of `user-dirs.dirs`, else `~/Downloads`, when it exists) is a tile after
the applications and the recents, before the minimized windows and the trash, drawn with the theme's `folder-download`
icon and named after the folder. A click on it opens its stack, and a second click, Escape or a press outside closes it.
Its menu has one item, Open "Downloads", which opens the folder in the file manager.

The stack is a popup of the bar with a grab (`cupertino-dock`'s bar window is its parent), centred over the tile and
standing 8 pt off it (beside the tile when the dock is on a side), on the bar's material (`BAR_SURFACE`) with the bar's
edge and rim. It holds the folder's name along the top (13 pt semibold), a grid of the newest entries and a capsule
button, "Open in Files", that opens the folder and closes the stack. The grid has up to 5 columns and 3 rows of 96 × 96 pt
cells, each a 56 pt icon (the type's generic theme icon, or `folder`) over its name (11 pt, one line, ellipsized); the
panel is 16 pt in from its edges with a 20 pt corner radius. Entries are listed when the stack opens, on a thread of
their own, and the stack shows once they are (a click on the tile meanwhile closes it): hidden files, names that are not
UTF-8 and links that point nowhere are left out, the most recently changed first, at most 15, and only those shown have
their type looked up; the button opens the folder for the rest. An empty folder says
"No Items". A click on an entry, on its release, opens it with the session's opener and closes the stack.

The stack comes in over 0.25 s, easing out: it fades in and scales up from 0.9 towards the tile (`stack::entry`). Under
Reduce Motion it only fades. It goes at once. It closes when its tile goes, when a menu opens, and on `locked`; an
auto-hidden bar stays shown while it is open.

For assistive technology the stack is a dialog named after the folder; its entries and the button are buttons with a
Click action, and the name and an empty folder's text are static text.

**Fan.** A folder of up to 10 entries opens as a fan (`src/fan.rs` for the geometry, `src/view/fan.rs` for the window):
rows rise from the tile in an arc, each a 56 pt icon with its name on a tilted pill to its left and an "Open in Files"
button on top. It opens over 0.244 s (ease in and out) and closes over 0.178 s (ease in); the tile shows a down chevron
while it is open. Arrow keys move the selection, Return or a click opens the entry and closes the fan, Escape or a press
outside closes it. The `stack` key of `dock.conf` (`auto`, `fan`, `grid`) chooses: `auto` is the fan for small folders and
the grid for large ones or when the bar is not at the bottom. The rest of this section describes the grid.

**Decisions** (conservative, recorded here): the grid is the fallback of the fan, and there is no list; it orders by last
change, not by date added; it closes without an exit animation; its menu has no sorting or display options; only the
downloads folder is shown, and folders cannot be added by dragging.

### Application switcher

Code: `src/switcher.rs`, `src/switcher_kit.rs` and the switcher part of `src/kit.rs`. The server keeps the keys and the choice and sends
the panel's output, the list and the selection (`switcher_output`, `switcher_changed`, `switcher_closed`). The keys (Q / H among
them) are the server's.

The dock draws the panel on an overlay layer surface (namespace `cupertino-dock-switcher`) on that output. That is the output
under the pointer when the switch opened, not necessarily the dock's. The panel is centred whatever the dock bar and the menu
bar reserve (exclusive zone -1) and lies on the compositor's switcher glass (`SwitcherGlass`).

- The panel appears 150 ms after the switch opens, so a quick tap never shows it. It fades out in 0.25 s
  ease-in-out when it closes. A stepping change after the fade began cancels the fade.
- Tiles, selection highlight and label follow. The label shows the desktop entry's name.
- The pointer moving over a tile (after a 3 pt box of hover slack) or pressing it sends `switcher_select`.
- Releasing the left button on a tile sends `switcher_activate`, and the switch commits at once. Releasing off the tiles sends
  `switcher_cancel`. A press outside the panel is the server's and cancels there.
- An application with no icon of its own (a running app without a desktop entry, for example) is drawn with the asset `application.generic` in the dock and the switcher; an asset directory without that icon (or without `trash.empty` / `trash.full`) is an error at startup (`Icons::new`), never a stand-in drawn later.
- A running application lights the pinned tile of the same desktop entry (`Session::key_of`: tile id, entry id, `StartupWMClass`, aliases of the same entry). An application with no desktop entry cannot be matched to any pin and gets its own tile; installing a desktop entry for it (`Exec`, `Icon`, `StartupWMClass` = its `app_id`) is what merges it with a pin.
- The label is drawn without the plus-darker blend of the real panel, so it is faint over a dark backdrop (approximate).

**On Kit.** `src/switcher_kit.rs` is the same panel as a Kit window (`SwitcherSurface`, app id `cupertino-dock-switcher`):
the model above, the `Switcher` surface (the compositor's `SwitcherGlass`) as the window background, one list item per application and the label,
with the delay, the fade and the pointer rules unchanged. The panel is a list named "Application Switcher". Each entry is a
list item that a screen reader can press (`switcher_activate`), and the label is a polite live region, so the selection is
read as the keys step it (the panel never takes the keyboard). Under Reduce Transparency and Increase Contrast the
material is opaque, and the selection and label get stronger colours. The dock's Kit model keeps one `SwitcherSurface`
per output, opens the panel on the output the server names (window key and app id carry the output), and draws each
application with the dock's icon for it, or the generic application icon.

### Application browser

The dock has no application grid of its own: the launcher's application browser is the one (see `cupertino-launcher`).
A hot corner or gesture the dock maps to `app-grid` (by default the four-finger pinch) is sent to the server as
`run_action("app-grid")`, the server's `app-grid` action, which asks the launcher on the session bus
(`org.cupertino.Launcher1.ToggleApplications`). The dock opens no surface for it and takes no keyboard, and a
`toggle_app_grid` from an older server is ignored.

**Decision** (recorded here): one application browser. The dock's grid was removed rather than kept beside the
launcher's. Choosing an application in the browser (or in the launcher's search) opens it as a click on its tile does: the
launcher asks the server (`cupertino_session_v1.activate_app`, the same server path as this dock's `activate_app`), which
brings a running application forward, and only otherwise asks a resident one for a window or starts the entry with an
activation token (see the launcher's README, *Opening an application*).

### Overview

Code: `src/overview_ui.rs`, `src/overview_kit.rs` and the overview part of `src/kit.rs`. The server owns the session: the
transition, the windows' frames and their animation, Escape, the click fallback, and closing on a workspace switch and when
the dock dies. It sends the dock of its output the window set with desktop frames, the workspaces and the usable area
(`overview_changed`).

The dock lays the windows out with the design packing (`cupertino-overview`,: inset 20, gap 8, lift 20, tolerance 4,
shrink 0.9, 499 passes, constants; the application mode puts minimized windows in a lower row) and answers one
`overview_frame` per window. A changed set is laid out again and resent. The windows start 20 pt (`BarConfig::layout_gap`)
below the expanded workspaces bar, and the arrangement is centred in the area left (see the `cupertino-overview` README).

The chrome is a keyboard-exclusive overlay surface over the whole output (namespace `cupertino-dock-overview`, exclusive zone
-1, so surface and output coordinates agree). The server draws the dim and the windows. The chrome has:

- the workspaces bar: slots with "Desktop n" labels and a border on the current one, and an add slot with a plus. It stays
  expanded for the whole overview, where the design collapses it until hovered (approximate);
- a remove button on the hovered slot;
- the selection highlight, from hover or the keys (arrows, `Tab` / `Shift+Tab`, `Return` / `Space` select, `Escape` leaves);
- a 13 pt title under every window (colours and sizes approximate).

The chrome fades out over 0.25 s on `overview_closed`, and then the surface goes.

**On Kit.** `src/overview_kit.rs` is the same chrome as a Kit window (`OverviewSurface`, app id `cupertino-dock-overview`,
no background), with the model of `overview_ui.rs` and its layout. It is a group named "Overview" holding one button per
slot ("Desktop n", selected when current), the hovered slot's "Remove Desktop n", "Add Desktop" and one button per window
named by its title ("Untitled Window" without one). A screen reader presses them as the pointer would. The selected window
takes the keyboard focus, so its title is read as the keys move the selection. Under Reduce Motion the highlight goes
straight to the selection and the windows' frames are their final ones. Under Reduce Transparency the slots are opaque, and
under Increase Contrast the slots get an outline, the borders and highlight are wider, and the titles sit on a plate.
The dock's Kit model keeps one `OverviewSurface` per output, sends its `overview_frame` answers and requests, passes
Reduce Motion, and gives its surface the keyboard exclusively while `wants_keyboard` holds (before the bar).

### Hot corners and gestures

Code: `src/actions.rs` (the triggers, actions and defaults are `cupertino_defaults::dock`). The server detects them and sends `hot_corner` / `gesture`. The dock maps them with the eight trigger
keys of `dock.conf` and runs the result: nothing, the launcher's application browser (`run_action("app-grid")`), a
program through `sh -c`, or `run_action`. Defaults: no corner
does anything. Swipe up is overview, swipe down is application overview, pinch is the application browser and
spread is show desktop (roles).

### Context menus

Code: `src/menu.rs` (the items), `src/view/menu.rs`. A right click on a tile or the separator opens a Kit menu, a popup
of the bar surface with a grab (`Menu` material).

- **The file manager tile.** The session file manager (`cupertino-files` when installed, else the first installed of
  Nautilus, Dolphin, Thunar; the list is approximate) is built into the dock as its first tile, as the design's is: that
  tile is not one of the default pins. It is always present, also while the file manager does not run, never written to
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

Minimising into the application's tile, the launch bounce and the running indicators are keys of `dock.conf` too and apply at once; the folder stack style is read from the file each time a stack opens.

## Protocols / D-Bus it speaks

The process speaks the server's dock protocol (version 22) and `cupertino_session_v1`. On the session bus it only calls
`org.freedesktop.Application.Activate` to ask a resident D-Bus-activatable application for a window (see *Residency*). It listens for the launcher entry signal (`com.canonical.Unity.LauncherEntry.Update`): an application names its
desktop file as `application://ID.desktop` and sets `count` and `count-visible`, and a running application's tile shows
a visible count above zero on its badge (`badge.rs`). The messages are named
where they are used above. In the Overview chrome a press on a window sends `overview_select` and on empty space
`overview_close`. A press on a slot sends `switch_workspace` when released in place, or `move_workspace` when dragged past 4 pt
to another slot (approximate slack). The add slot sends `add_workspace` and the remove button `remove_workspace`.


## Open issues

- **Not wired.** Window thumbnails in minimized tiles and in the
  workspaces bar, a key to toggle auto-hide, and changing the position at run time.
- **Switcher.** The all-displays preference, scroll, and drag and drop.
- **Overview.** Thumbnails of the workspaces are not drawn, and the slots are plain (approximate).
- **Context menus.** Deviations (approximate): the Options submenu is flattened. There is no Open at Login (no login-items
  store). Show Recents, Dock Settings, Minimize Using and the Option-key alternates are absent. Metrics and colours are the
  menu bar's. The desktop background menu has no surface to live on and is not done.

## Tests

```
cargo test -p cupertino-dock --features app
```

`cargo test -p cupertino-dock --features app` runs 265 tests; without the feature the process code is skipped.

| Test | Proves |
|---|---|
| Activation unit tests (`cupertino-apps`, `src/activate.rs`) | The object path of a desktop file ID and the token in the platform data. |
| `tests/activate.rs` | `Activate` reaches a fake `org.freedesktop.Application` on a private session bus (`cupertino-test-bus`) at the desktop file ID's path, with the token in its platform data; a name nobody owns fails. |
| Stack unit tests (`src/stack.rs`) | The grid growing to five columns then up to three rows, cells in reading order, presses on entries and the button, the quarter-second entry that only fades under Reduce Motion, the newest-first order without hidden files, a real folder listed without hidden files, dangling links or names that are not UTF-8 and with only the shown entries' icons looked up, and the downloads folder from `user-dirs.dirs`. |
| Switcher unit tests | Layout, hit-testing, delay and fade timing, hover slack, colours. |
| Icon unit tests (`src/icons.rs`) | The generic application icon, the trash icons, the placeholder, a window's own image, averaging down a large image, a sharp edge at the drawn size. |
| Magnification unit tests | The ramp, and the fisheye: exact large size under the pointer, neighbour sizes, growth, monotonic fall-off, continuity. |
| Animation unit tests | The bounce curves, the cosine ease and the auto-hide durations. |
| Tile drag unit tests | The press that becomes a drag past the threshold, the held press, and the removal puff's timing. |
| `tests/dock.rs` | Sections, orientations, hit-testing, minimize targets and drag-reorder: the grab, the gap rule, the slide, the strip and section limits, the removal gate, the dim, the settle and the removal fade. |
| `tests/wm_surfaces.rs` | The overview and switcher on Kit, headless: three windows of different sizes clear the workspaces bar by 20 pt and each other on four outputs at scales 1, 1.25, 1.5 and 2, and hit-test to themselves; the keys, the bar's slots, add and remove, screen-reader reading and presses, the closing fade, reflow, Reduce Motion, opaque fills under Reduce Transparency and Increase Contrast; the switcher's delay, pointer and screen-reader activation, live label, step, commit and cancel fades, and a lost panel resumed. With `CUPERTINO_LOOK_DIR` set, `look_captures` writes light and dark pictures at 1× and 2×. |
| `tests/session.rs` | Windows and tiles, the launch bounce (announced, not doubled, unannounced, not for the first list, not for an application that shows a window) and the refused-activation hop. |
| `tests/kit.rs` | The dock as CupertinoKit runs it, headless (`cupertino-ui-test`), behind a fake host: the bar's accessible tree, a click that activates or launches with an activation token, a drag off the bar that removes a pin, a tile menu whose item reaches the server, Focus Dock with the arrows, Return and Escape, the move to another display by resting on its edge strip, the window list from the bar's role only and a gesture from any, the bar's material as one region of the window (none under Reduce Transparency), the Force Quit dialog from the keyboard, the app-grid trigger sent to the server as its action with no window of its own, and the lock. Also the running indicators under their tiles, an attention bounce and a launch bounce run on frames with no timer faster than the fallback and back to rest, the tile menu as a grabbing popup anchored to its tile, every Focus Dock starting on the first tile with the ring shown, the focus moving to a neighbour when its tile goes, a keyboard leave ending Focus Dock except the menu's own, no new frame for pointer motion that changes nothing, a new frame for every change the bar shows (an application starting or quitting, a bounce, a new scale, Focus Dock, and a `dock.conf` turning magnification on, which changes only the surface's depth, or changing the tile size, position or auto-hide) and for the strip of a plugged-in output and the bar moved off an unplugged one, the switcher on the output the server names with desktop names and a click that switches, and the overview taking the keyboard, framing three windows and closing. Residency: the dot kept after the last window, a resident application asked over D-Bus with a token once and without a bounce, started again with a token without the bus or when the call fails, no second launch from the bar, and the dot dropped when the process goes or a new role does not list it. Reduce Motion: a launch that dims in place and magnification at once. Launches: a token that never comes lets the launch go ahead without one after 2 s, a program that fails at once can be opened again straight away, one that exits successfully before its window stays guarded, an earlier launch's exit leaves a later guard alone, and icons loading in the background (the bar's magnified side) with one left out loaded on the spot. Recents: an application that quits comes into the recents between two separators and opens from there, the quit ones then the launcher's history at most three, and none with `recents = off`. The name label over the tile under the pointer, following it, and gone with a menu open or the pointer away. Stacks: the downloads tile before the trash, its stack opening on a click with its files and button, a file opening and closing it, a second click, Escape and the button closing it, and a stack that shows only once its folder is listed and stays closed when closed meanwhile. With `CUPERTINO_LOOK_DIR` set, `look_captures` writes the bar, a tile's menu, the switcher, the overview's chrome over three windows, light and dark at 1× and 2×. |

### Label, menu and trash

The name label (`src/view/label.rs`) is a translucent 26 pt pill with a tail 3 pt over the bar; it stays through a click
and hides on a drag, a menu or leaving the tile. An application that runs without windows adds a dimmed second line and
a dimmed running indicator. A pressed tile shows its icon darkened until release. The running indicator fades in over
0.28 s. In the light appearance the bar keeps the dark clear material (`Surface::DockBar`), the indicator is near black
and the separator darker. The context menu at the bottom has a tail and a 14.5 pt corner. The trash's menu has "Empty
Trash", enabled while the trash holds something, which asks first (`src/view/confirm.rs`) and then empties it through
`cupertino-trash`.
