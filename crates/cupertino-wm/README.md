# cupertino-wm

Pure-logic window-management model for a Wayland compositor, with no compositor dependency. The compositor feeds it
commands with absolute timestamps and maps the returned effects to protocol requests.

## Public API

Entry points are `Wm::handle` (a `Command` in, `Effect`s out: `Configure`, `Place`, `Focus`, `Raise`, `Activate`,
`StartAnimation` and others), `Wm::tick` (advance animations and timers) and `Wm::presentation` (frame, opacity, scale,
shadow, minimize progress).

| Module | Role |
|---|---|
| `lifecycle` | toplevel state machine (`Opening`, `Normal`, `Minimizing`, `Minimized`, `Restoring`, `Zooming`, `Zoomed`, `Unzooming`, `EnterFullScreen`, `FullScreen`, `ExitFullScreen`, `Tiling(k)`, `Tiled(k)`, `Closing`, `Destroyed`); every transient state is interruptible. `ToplevelStates::frame_shape` is the corner policy: `Square` (no corner mask, decorations or shadow) only for a full-screen window, from the start of its enter transition to the start of its exit; `Rounded` for every other state, a zoomed window and one that fills the usable area (the fill tile) included |
| `focus` | app versus window focus, MRU, hide / hide others, app switcher model (steps, hidden counter, quit and hide of the selection), in-app window cycling, modal blocking, attention for refused activations |
| `activation` | the activation-token rule (`token_accepted`, `GRANT_LIFETIME`, `LATE_ACTIVATION`, `TYPING_QUIET`) |
| `pieces` | visible pieces of a window (`visible_pieces`) and the magnetic guides they make (`window_guides`) |
| `kind` | window kind of a new window from its protocol facts (`window_kind(modal, utility)`), where dialogs open (`sheet_frame`, `modal_panel_frame`, `child_frame`); `Map` turns a modal child into a `Sheet` when `place_modal_children_as_sheets` is on |
| `interaction` | move, resize, snap hot zones, option-accelerator preview, edge-dwell workspace switching, resize hit zones, size snapping to the screen edges |
| `magnet` | magnetic edges: moved edges stick to the usable screen rectangle, resized edges to other windows |
| `workspace` | per-output workspaces, sticky and full-screen workspaces, MRU reordering, gesture switching with rubber band, app assignment |
| `zoom` | best-fit sizing from min/max size, content hint and usable area |
| `tiling` | tile kinds, hot zones, tile rectangles with optional margins |
| `wm` | `Wm`: owns apps, windows, outputs, focus, workspaces and the pointer interaction state; `key_window` (the keyboard) and `main_window` (the window the person works in) are separate: a utility panel (floating level) takes the keyboard without becoming main, a sheet or dialog leaves its top-level parent main (`Window::can_be_main`) |
| `window` | `App`, `Window`, `WindowLevel`, `Assignment`, `DisabledControls` (the controls an application disabled on its window: `Window::can_minimize` and `can_zoom`, which also needs a size that is not fixed; `minimize_window` and the zoom leg of `toggle_zoom` refuse them for every caller, `tile_window` refuses the fill tile where zoom is refused (half and quarter tiles stay) and a move does not offer its zone, while an unzoom, a restore and Hide still work; `fullscreen_window` refuses a full screen the person asks for (`Requester::User`: shortcut, menu) where zoom is refused, and keeps the client's own request (`Requester::Client`) and every exit; `toggle_zoom` of a full-screen window leaves full screen, so the zoom button and the Zoom command leave full screen whatever the window's controls) |
| `menu_bar` | menu bar presentation per output: dimming, auto-hide with a reveal dwell, where a full-screen window's title bar shows below a revealed bar (`fullscreen_titlebar`), the area the bar reserves (the `menubar` component draws the content) |
| `command`, `config` | `Command` in, `Effect` out; every tunable value, with its evidence label |
| `stage` | the stage model: a centre stage holding the active window group and a left strip of recent groups. The `windowmanager` component of `cupertino-shell` runs it, and the server animates what it asks. See *Stage* below |
| `overview` | the window overview model: layout, app overview, workspaces bar, selection, keyboard navigation and the gesture-driven transition. See *Overview* below |

## Usage

```rust
let mut wm = Wm::new(WmConfig::default());
wm.handle(Command::AddOutput { output, rect, usable }, now);
let effects = wm.handle(Command::Map(MapParams::new(window, "org.example.app", geometry)), now);
// Effect::Configure / Place / Focus / Raise / Activate / StartAnimation / ...
let effects = wm.tick(now);             // advance animations and timers
let p = wm.presentation(window, now);   // frame, opacity, scale, shadow, minimize progress
```

## How it works

Behaviour follows. Timings come;
most are approximate, so each is a configuration field. defaults: tiled window spacing 8 pt, hard hot-zone delay 0 s, soft hot zone 20 pt with 0.2 s
delay (the mapping of the two delays onto an edge zone and a surrounding soft zone is; `hot_zone_hit` reports
the delay), dock hot-zone avoidance off, tiling animations on, tiled border off, and the show-desktop spring
(`DesktopPeekConfig`: damping ratio 1.0, response 0.3 s). Restoring always returns the exact pre-state geometry.

### Window transitions and magnetic edges

| Value | Default |
|---|---|
| open / close transform per `MapParams::kind` | plain none; document grow-in 0.2 s; utility fade-out 0.15 s; alert pop-in 0.4 s / scale-down-and-out 0.15 s; app-modal grow-in / fade-out; `sin²` ease |
| kind from protocol facts | modal → app-modal before utility → utility, else plain; documents and alerts have no protocol signal |
| sheet (modal child) | 0.26 s linear time; slides 32 pt down into place eased `sin²`, alpha `min(2p, 1)`; close slides back up, alpha `1 − p` |
| sheet placement | centred on the parent, snapped to whole points; top at the bottom of the parent's title bar (`snap.titlebar_height`, 32 pt, the titled window's bar), or of the toolbar row the parent's client draws when it says so at map (`MapParams.sheet_top`: 52 pt, 40 pt compact) |
| modal panel without a parent | centred horizontally, top a quarter of the free height below the usable top; the next panel of the app 66 pt right and down, kept on screen |
| other child windows | centred on the parent, kept inside the usable area |
| zoom / unzoom | `min(0.35, 181·ln(maxΔ/100 + 1)/750)` s, `sin²` ease |
| full screen enter (exit) | 0.5 s, bézier (0.25, 0, 0, 1) |
| tiling (untiling) | spring, damping ratio 1.0, response 0.38 s |
| corner hot zones | `soft_hot_zone_size` (20 pt) wide; as high, plus `max(0, usable.y − y)` so the top ones reach 20 pt below the usable top; 0.2 s dwell (`edge_dwell`), at once with Alt |
| side and top hot zones with Alt | 96 pt (`snap.option_edge_extent`) |
| tile preview frame | the tile rectangle outset by 15 pt on every side (`snap.preview_outset`, `PreviewPresentation::frame`) |
| tile preview fade | spring, damping ratio 1.0, response 0.2268 s (`snap.preview_opacity_curve`) |
| dwell stillness | 5 pt inclusive (`snap.dwell_stillness`): Euclidean in a corner, \ | on the sides, \ | dx\ | on the top; moving farther starts the dwell again | 5 pt and metric; entry point as reference and the restart |
| snap distance, snap / unsnap speed | 30 pt, 400 / 400 pt/s |
| unsnap pull | `35 + 105·(θ/60°)²`, 140 pt from 60°; instant within 15° pulling away |
| re-snap guard | 55 pt travel or a reversal |
| window gap, overlap margin | 1 pt, 1 pt |
| guides | a move: the usable screen rectangle only, so the window passes other windows one-to-one (`magnet.move_to_windows`, off, adds them); a resize: other windows with opacity ≥ 0.2 and no transform, outside the movement group |
| visible pieces | a window minus the windows in front of it, in bands; pieces under `min(20 pt, window size)` (`magnet.min_piece`) are kept only next to a piece sharing a full edge; a piece guides only along edges on the real frame |
| size snap while resizing | off (`snap.snap_size_to_screen_edges`); once the pointer rests 0.4 s within 50 pt of a screen edge, the dragged edge jumps to the usable edge |
| double click on a resize edge | on (`snap.snap_size_on_double_click`); clicked edges go to the usable edges, Alt adds the opposite ones; size limits kept, not in full screen |
| window levels | normal 0, floating 3 (utility panels), modal panel 8; a child takes its parent's level; new and raised windows stay inside their level |
| sheet dimming of the parent | `Wm::sheet_dimming`: how far the sheet is shown, `p` while it opens and `1 − p` while it closes |

### Close, minimize, zoom and full screen

They are distinct operations, and each keeps the frame to go back to: a minimized window restores to the state and
frame it left (zoomed included), unzoom returns to the frame before the zoom, and leaving full screen returns to the
state before it (a zoomed window comes back zoomed, then unzooms to its first frame). A refused full screen (the person
asking for a window that cannot zoom, or any request for a minimized window) emits nothing and leaves the state, the
frame, the workspaces and the key window as they were. Closing is a request to the client (`Effect::RequestClose`),
which may refuse; when its last window goes, the application stays registered and in front with no key window, and
whether it then quits is the application's own policy (the toolkit's resident flag), never the window manager's.
When an output goes away its windows move to the remaining output with every frame they go back to (normal, zoomed,
tiled, full screen) fitted to it; a full-screen window leaves full screen there, since its full-screen workspace went with
the output. A window being dragged on it keeps being dragged: the drag goes on on the remaining output, where the server
brings the pointer, and a cancelled drag leaves it there. A resize on it ends with the frame the window was given on the
remaining output. When the last output goes, its windows wait without one and the next output added takes them, fitted
the same way. `Command::MoveWindowToOutput` puts a window on another output's current workspace with its normal frame
at a given place fitted into that output's usable area, its zoomed and tiled frames fitted there too (the server uses it to
bring windows back to a display that returns); a full-screen or sticky window stays, and one already on that output takes
the frame in its own workspace.

Decisions where the behaviour is not known: the resize hit zones (3 pt inside, 4 pt outside, 12 pt corners, 15 pt
bottom-right) and the title bar's double click (zoom) stay configurable defaults; the drag-to-edge thresholds and the
snapping preview timing are the values of the table above; a window that refuses to move is not modelled.

### Focus on map

A window that maps (`MapParams.activation`) takes the keyboard when the compositor asks for it unconditionally (`Activation::Always`: shell windows, tests) or, with `Activation::Policy(MapFocus)`, when: nothing is focused, it is a sheet or dialog of the key window, its application is the front one, its client presented a valid grant (`Grant`: made by the compositor or for an input event of the focused client) at most 10 s old that passes `token_accepted`, or it is the first window of a client the person has not left since it connected and is not typing (`first_of_client`, not `input_elsewhere`, and no key or button press in the last `TYPING_QUIET` = 1 s). Any other window maps just below the key window and `Effect::AppAttention` bounces its application's tile.

### Application switcher and activation

| Value | Default |
|---|---|
| entries | applications with at least one window, most recently used first |
| steps | Next / Previous wrap on a fresh press; a key repeat stops at the ends; First / Last jump |
| hidden counter | for 150 ms after the switch opens (`SWITCHER_SHOW_DELAY`) steps only count; the count picks `min(c, n−1)`, from the end `n + c` (0 past the start), and 0 follows the opening direction |
| Q / H | Q asks every window of the selected application to close (nothing with a single entry); H toggles its hidden state; the switch stays open |
| activation token | refused when older than the app's last activation, and in every case when a user event came more than 100 ms after it; for a window that is already mapped also refused when more than `LATE_ACTIVATION` = 2 s old; single use; a refused activation asks for attention (`Effect::AppAttention`) when the app is not in front |
| catch-up after a jump with the pointer outside the window | 0.15 s, ease-out (0, 0, 0.58, 1) |
| Alt disables snapping | on |
| resize hit zones | 3 pt inside, 4 pt outside, corners 12 pt, bottom-right 15 pt |
| resize snapping | dragged edges snap to other windows, not for child windows |

### Stage

| Item | Role |
|---|---|
| Groups | Grouping by app or one window at a time (`Grouping`), merge by drag (`add_to_stage`, `drop_group`), split a window out (`drop_window`) |
| Centre and swap | `activate_group`, `window_activated`, `window_opened`. Swaps animate with springs and stay continuous when interrupted (`window_frames` combines each group's blend and slot) |
| `placements` | Every window's resting placement (on the stage, a strip thumbnail rectangle, or hidden beyond the strip's rows), independent of time. It is the target a window manager sends a compositor that runs the animation on its own frame clock |
| Strip layout | Rows clamped between configured minimum and maximum, slot rectangles, icon overlay rectangles, hit-testing |
| Hide / peek | The strip hides when a centre window overlaps it (or always, when configured), leaves a peek width visible, and reveals on edge hover with dwell times |
| Desktop | Desktop items are hidden while the stage is active and revealed by clicking the desktop (`ClickDesktop`) |
| `StageConfig` | Layout defaults |

The layout defaults in `StageConfig` are as designed:

- 170 × 170 pt slots, 32 pt spacing, 16 pt strip edge inset, 16 pt gap to the stage (stage inset 202 pt);
- 4–6 rows, slot height cap 160 pt, preferred minimum 96 pt, absolute minimum 20 pt;
- strip corner radius 8 pt, icon overlay 40 pt (offsets −10 / 10, spacing −4 / −22);
- peek width 0, auto-hide off, reveal dwell 0.15 s.

Using the preferred minimum height to choose the row count is.

The swap and slide springs are the window manager's generic frame spring, damping ratio 1.0 and response 0.38 s; a swap's alpha and corner-radius springs (`alpha_curve`,
`corner_curve`) are its alpha and corner-radius property springs, damping ratio 1.0 and responses 0.30 s and 0.20 s. No strip-specific spring exists, so their use for the strip is. The vertical padding and alignment, the thumbnail padding and the hide dwell remain approximate and are
configurable.

### Overview

| Item | Role |
|---|---|
| `pack` / `layout` | The design's iterative relaxation. See *Overview layout* below |
| `layout_row`, `reading_order` | The minimized row; keyboard order |
| `OverviewMode::Overview` | All windows of the workspace plus the workspaces bar |
| `OverviewMode::AppOverview(app) | One app's windows, minimized windows in a lower row |
| `Transition` | Gesture-driven progress with rubber band and velocity projection, spring completion. Also used on its own by the display server, which owns the open and close animation. `Overview` interpolates per-window frames from it, and windows reflow smoothly when the set or the mode changes |
| `WorkspaceBar` | The full-width desktops band: names when closed, pictures and names when open on hover, add button, hit-testing |
| Selection | Hover selection, animated highlight, keyboard navigation (directional keys, next/previous, activate, cancel), click actions |
| `TitleFade` | Title fade schedule (approximate: after progress 0.8) |
| `OverviewConfig` | Every number is configurable through it |

#### Overview layout

The display rectangle is inset by 16 pt. Windows are placed at their scaled centres 20 pt up, pushed apart along the line
through their centres, and shrunk by `s·(r + 0.9·(1 − r))`, where `r` is the room left over the room used. The pass repeats
until no pair overlaps by more than 2 pt and the largest move is under 2 pt, or 499 passes have run. 14 pt is kept between
windows. A packing that ends at the pass limit with windows still closer than 2 pt falls back to a grid of the work area in
the same reading order, so thumbnails never overlap. The arrangement is then moved as a whole so its bounding box is centred
across the work area and sits the lift above its middle: it is balanced whatever the windows' desktop places were. `LayoutParams::title_space`
reserves space under every window for a title; the overview uses 0, as only the highlighted window shows its name, on a
plate centred over the window.

In the all-windows mode the work area starts below the desktops band: 39 pt (`BarConfig::layout_gap`) under the band's
bottom, 72 pt closed and 165 pt open, so windows start 111 pt from the top (`BarConfig::windows_top`). The band opens
while the pointer is over it and the windows lay out again below it; it closes when the pointer leaves.

- **** the inset, the gap value, the lift, the tolerance, the shrink constant and the pass limit.
- **** the gap's role, the start scale from the windows' bounding box, scaling about the work area's centre and the
  repulsion direction.

#### Overview transition and bar numbers

The transition is a critically damped spring of response 0.25 s, or 2.0 s while the slow-motion modifier is held
(`Overview::set_slow_motion`). A gesture released faster than 5 progress/s settles on a spring of response × 0.6. The
band is 72 pt closed and 165 pt open, with 146×95 pt pictures 20 pt below its top, 24 pt name pills and a 20 pt add button
whose centre is 28 pt from the right and 35 pt from the top. The gap between pictures and the side margin are
approximate.
