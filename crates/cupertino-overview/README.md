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
| `WorkspaceBar` | Thumbnail slots, expansion on hover, add slot, hit-testing |
| Selection | Hover selection, animated highlight, keyboard navigation (directional keys, next/previous, activate, cancel), click actions |
| `TitleFade` | Title fade schedule (approximate: after progress 0.8) |
| `OverviewConfig` | Every number is configurable through it |

## How it works

### Layout

The display rectangle is inset by 20 pt. Windows are placed at their scaled centres 20 pt up, pushed apart along the line
through their centres, and shrunk by `s·(r + 0.9·(1 − r))`, where `r` is the room left over the room used. The pass repeats
until no pair overlaps by more than 4 pt and the largest move is under 4 pt, or 499 passes have run. 8 pt is kept between
windows.

- **** the inset, the gap value, the lift, the tolerance, the shrink constant and the pass limit.
- **** the gap's role, the start scale from the windows' bounding box, scaling about the work area's centre and the
  repulsion direction.

### Transition and bar numbers

the transition duration is 0.25 s, or 2.0 s while the
slow-motion modifier is held (`Overview::set_slow_motion`), and the workspaces bar band is 90 pt tall. The packing constants
are as designed. The bar's inner metrics (padding / thumbnail split, expanded size) and the spring shape are
approximate or unknown.

## Tests

Unit tests cover packing (containment, the gap, one common scale, the lift, the pass limit, the shrink constant idle while
room is left, determinism), the transition and the bar. `tests/overview.rs` covers gestures, modes, reflow, selection,
keyboard navigation and title fading.
