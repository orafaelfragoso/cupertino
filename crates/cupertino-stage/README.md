# cupertino-stage

Stage model with no compositor dependency: a centre stage holding the active window group and a left strip of recent
groups. The `cupertino-windowmanager` process runs it, and the server animates what it asks.

## Public API

| Item | Role |
|---|---|
| Groups | Grouping by app or one window at a time (`Grouping`), merge by drag (`add_to_stage`, `drop_group`), split a window out (`drop_window`) |
| Centre and swap | `activate_group`, `window_activated`, `window_opened`. Swaps animate with springs and stay continuous when interrupted (`window_frames` combines each group's blend and slot) |
| `placements` | Every window's resting placement (on the stage, a strip thumbnail rectangle, or hidden beyond the strip's rows), independent of time. It is the target a window manager sends a compositor that runs the animation on its own frame clock |
| Strip layout | Rows clamped between configured minimum and maximum, slot rectangles, icon overlay rectangles, hit-testing |
| Hide / peek | The strip hides when a centre window overlaps it (or always, when configured), leaves a peek width visible, and reveals on edge hover with dwell times |
| Desktop | Desktop items are hidden while the stage is active and revealed by clicking the desktop (`ClickDesktop`) |
| `StageConfig` | Layout defaults |

## How it works

The layout defaults in `StageConfig` are as designed:

- 170 × 170 pt slots, 32 pt spacing, 16 pt strip edge inset, 16 pt gap to the stage (stage inset 202 pt);
- 4–6 rows, slot height cap 160 pt, preferred minimum 96 pt, absolute minimum 20 pt;
- strip corner radius 8 pt, icon overlay 40 pt (offsets −10 / 10, spacing −4 / −22);
- peek width 0, auto-hide off, reveal dwell 0.15 s.

Using the preferred minimum height to choose the row count is.

The swap and slide springs are the window manager's generic frame spring, damping ratio 1.0 and response 0.38 s; a swap's alpha and corner-radius springs (`alpha_curve`,
`corner_curve`) are its alpha and corner-radius property springs, damping ratio 1.0 and responses 0.30 s and 0.20 s. No strip-specific spring exists, so their use for the strip is. The vertical padding and alignment, the thumbnail padding and the hide dwell remain approximate and are
configurable.

## Tests

Integration tests in `tests/stage.rs` cover enabling, grouping, rows, slots, swaps and interruption, closing, drag-add,
strip hide and peek, icons, thumbnails, resting placements, fitting and desktop behaviour.
