# cupertino-overview

Window overview model with no compositor dependency.

## Public API

| Item | Role |
|---|---|
| `pack` / `layout` | The design's iterative relaxation. See *Layout* below |
| `layout_row`, `reading_order` | The minimized row; keyboard order |
| `OverviewMode::Overview` | All windows of the workspace plus the workspaces bar |
| `OverviewMode::AppOverview(app) | One app's windows, minimized windows in a lower row |
| `Transition` | Gesture-driven progress with rubber band and velocity projection, spring completion. Also used on its own by the display server, which owns the open and close animation. `Overview` interpolates per-window frames from it, and windows reflow smoothly when the set or the mode changes |
| `WorkspaceBar` | The full-width desktops band: names when closed, pictures and names when open on hover, add button, hit-testing |
| Selection | Hover selection, animated highlight, keyboard navigation (directional keys, next/previous, activate, cancel), click actions |
| `TitleFade` | Title fade schedule (approximate: after progress 0.8) |
| `OverviewConfig` | Every number is configurable through it |

## How it works

### Layout

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

### Transition and bar numbers

The transition is a critically damped spring of response 0.25 s, or 2.0 s while the slow-motion modifier is held
(`Overview::set_slow_motion`). A gesture released faster than 5 progress/s settles on a spring of response × 0.6. The
band is 72 pt closed and 165 pt open, with 146×95 pt pictures 20 pt below its top, 24 pt name pills and a 20 pt add button
whose centre is 28 pt from the right and 35 pt from the top. The gap between pictures and the side margin are
approximate.

## Tests

Unit tests cover packing (containment, the gap, one common scale, the lift, the pass limit, the shrink constant idle while
room is left, determinism), the transition and the bar. `tests/overview.rs` covers gestures, modes, reflow, selection,
keyboard navigation and title fading, and
`three_windows_stay_below_the_strip_and_apart_at_every_size_and_scale` checks three windows of different sizes (one small)
in four arrangements on six output modes at scales 1, 1.25, 1.5 and 2, edges snapped to device pixels: every thumbnail at
least 39 pt below the band, none over a slot or another thumbnail, the arrangement centred, and hit testing on each frame.
`adversarial_window_sets_keep_thumbnails_and_titles_apart_and_below_the_strip` checks 3000 seeded cases (up to sixteen
windows, tiny, very wide and very tall ones and windows on one place, outputs from 640×360 pt to 4K at scales 1 to 2): no
thumbnail or title over the strip, another window or another title, or outside the area, also snapped to device pixels.
