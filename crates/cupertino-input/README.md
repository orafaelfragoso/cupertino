# cupertino-input

Input model for the Cupertino compositor. Pure logic, with no windowing, input-library or Smithay dependency. The
compositor feeds events and timestamps (absolute seconds) and acts on the returned events.

## Public API

| Module | Contents |
|---|---|
| `gesture` | `GestureRecognizer`: libinput-like swipe / pinch / hold events → `Begin` / `Update { progress, velocity }` / `End { Commit \| Cancel, target, direction, steps }. 3 or 4 fingers horizontal → workspace switch, vertical up → overview, vertical down → app overview, pinch in → app grid, spread → show desktop. Axis lock, cancel and reverse, velocity projection, configurable finger counts and distances |
| `hotcorner` | `HotCorners`: four corners, per-corner action and modifier requirement, dwell, once-per-visit firing, no traps on seams between outputs |
| `edge` | `EdgeReveal`: screen-edge dwell with hold region for dock or fullscreen menu bar reveal |
| `shortcuts`, `keysym` | `ShortcutMap`: parse `Super+Tab`, `<Super>Tab`; xkbcommon keysym names; `Action` enum for windows, overview, workspaces, launcher and input sources (`input-source-previous` / `input-source-next`); text configuration (physical modifiers, `Primary` as the profile's command key, `[profile]` sections, `Super = action` for the Super key tapped alone); the default maps of the `cupertino` and `classic-pc` profiles |
| `scroll` | `KineticScroller`: drag with rubber band, fling with the default quartic momentum model, elastic overshoot and return, hold-to-stop |
| `pointer` | `PointerAccel`: per-event pointer acceleration on device counts: speed = counts per event at a nominal 67 Hz over the resolution (400 dpi fallback), polynomial → linear tangent → square-root curve scaled by 96/67, one ratio for both axes; optional report-rate time factor; linear mode. `ParametricCurve` selects and interpolates a curve from a family (`MOUSE_CURVES`, `TRACKPAD_CURVES`) by tracking speed (`TRACKING_SPEED_STOPS`, 0…3); `TRACKPAD_SCROLL_CURVES` is the trackpads' scroll family. `double_click_space(t)` = `min(6t + 2, 14)` px. `POINTS_PER_LINE` = 10, `scroll_point_delta` = `trunc(10 × lines)`. `PointerAccel::with_curve` runs the same per-event logic on any `AccelCurve` (parametric or table). Fresh-account defaults: `DEFAULT_TRACKING_SPEED` 0.6875, `DEFAULT_SCROLLING_SPEED` 0.3125, `DEFAULT_DOUBLE_CLICK_TIME` 0.5 s, `DEFAULT_ACCELERATION` on |
| `mouse_accel` | `MouseAccel`: mouse acceleration on physical speed: counts over the device's dpi over the time of the event timestamps (8 ms window) in inches per second, times `DESIGN_SPEED_PER_HAND_SPEED` (2.5: the design reads every count as one of a 400 dpi mouse, so one inch of a typical 1000 dpi mouse is 2.5 of its inches) through the curve (`AccelCurve`), gain applied to the counts; linear mode scales per count of a 1000 dpi device. `parse_mouse_dpi` reads the hardware database's `MOUSE_DPI` (marked `*` entry, else first); `resolve_dpi`: stored value, else database, else 1000 |
| `table` | `AccelTable::parse`: big-endian table (16.16 scale, `@200` / `USB*` signature, entries of 16.16 points). `TableCurve::new` blends the two entries bracketing the setting (first point per x wins) or scales one by `(a+1)/(acc+1)`; piecewise linear from the origin, extrapolated past the last point. `DEFAULT_TABLE` / `AccelTable::default_table: the embedded fallback a generic mouse gets |
| `scroll_accel` | `ScrollAxisAccel`: per-axis 9-slot history (reset on direction change or a gap above 500 ms), speed polynomial, `√n` boost for table curves, output scale 6554/65536, momentum scaled by dispatch rate / 60; 67 Hz report-rate fallback; output in lines. `ScrollConsume` is the translator's coalescing (`ScrollAxisAccel::scroll_lines`): lines add up until one event exceeds `u = trunc(2·res/100)` counts or `3u` counts have summed up, then go out truncated to 0.1 line, the truncated part kept for the next emission in the same direction; `u = 0` passes through. `TouchpadSettings::scroll_accel` builds one axis for the touchpad model's scroll counts: trackpad scroll curves, resolution 400 (`TRACKPAD_SCROLL_RESOLUTION`), 67 Hz, setting `scrolling_speed` (fresh 0.3125) |
| `cursor` | `clamp_cursor_scale` (user scale 1…4, display-server limit 16). `ShakeDetector`: velocity filter (`0.2·v + 0.8·v_prev` while the signs agree and the samples are at most 0.2 s apart, samples under 1 ms dropped), reversals of the smoothed velocity in x or y recorded in a 10-slot table, reset 0.2 s after the last reversal; at ten reversals a circular test (every point 32 pt or more from the centroid, more than three quadrant steps one way) or a line test (scaled slope variance ≤ 0.2, more than four points outside); multiplier `clamp(0.5·n, 1, 5)`, `n ← trunc(0.8·n) per evaluation once the last reversal is 0.1 s old, evaluated every 1/60 s |
| `scroll_count` | `ScrollCounter`: the scroll count of repeated flicks, `update(phase, x, y, ms)` per scroll event with `ScrollPhase::{Began, Changed, Ended, Cancelled}` and the event's movement in points. Starts at 1 when a scroll's speed squared reaches 900 (30 points), a gesture that begins within 600 ms of the previous one's end raises it by one (once per gesture), from the third on each `Changed` adds `speed / 2.5` up to 2000; it drops to 0 on a vertical direction change (a horizontal-dominant event never sets the direction, as in the design), on a begin after the 600 ms, and when no event exceeds speed squared 400 for 250 ms. A wheel has no phases and gets no count |
| `velocity` | `VelocityTracker`: least-squares velocity over a sliding window |


## Usage

```rust
use cupertino_input::{GestureConfig, GestureEvent, GestureRecognizer, GestureOutput};

let mut r = GestureRecognizer::new(GestureConfig::default());
r.handle(GestureEvent::SwipeBegin { time: 0.0, fingers: 3 });
for out in r.handle(GestureEvent::SwipeUpdate { time: 0.016, dx: 0.0, dy: -30.0 }) {
    if let GestureOutput::Update { progress, .. } = out { /* drive the overview */ }
}
```

## How it works

Feed `GestureRecognizer::handle` a `GestureEvent` (for example `SwipeBegin { time, fingers }`, then `SwipeUpdate { time, dx, dy }`) and
act on the returned `GestureOutput`s (`Update { progress, .. }` drives the overview). Until the recognizer reports
`Disposition::Claimed` or `Disposition::Ignored`, the compositor keeps forwarding swipe events to the focused client.
`Begin` is where the compositor takes them over.

## Tests

```
cargo test -p cupertino-input
cargo clippy -p cupertino-input --all-targets
```
