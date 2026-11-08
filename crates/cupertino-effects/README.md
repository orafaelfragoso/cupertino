# cupertino-effects

Renderer-agnostic, time-parameterised window effect generators.

Every effect is a pure function of a progress value. [`ProgressAnimator`](src/ease.rs) turns time into progress and is
reversible and interruptible: the new run starts from the current value, and springs keep their velocity. Mesh effects
output a vertex grid with texture coordinates ([`Mesh`](src/mesh.rs)) that any renderer can draw as a triangle list.

## Public API

| Module | Effect |
|---|---|
| `ease` | `ProgressAnimator`, `Ease`, `Driver`, `cosine`: time to progress |
| `mesh` | `Mesh`, `Vertex`: vertex grid with texture coordinates |
| `warp` | warp minimize: 2 × N vertex mesh, two overlapping phases (slide, stretch) with ease `0.5 − 0.5·cos(πx), phase delay from the acos overlap rule, S-curve side offsets, adaptive subdivision (stop below 8 pt, or below 128 pt when the chord error is below 0.6 pt, at most 400 samples) |
| `funnel` | funnel minimize: grid of 70 pt cells, `clamp(int(w/70),1,19)` × `clamp(int(h/70),1,19)` cells (≤ 20 × 20 vertices), each vertex moves `dist − clamp(dist − D·e, 0, D) to its slot point |
| `scale` | scale minimize: one quad, cosine ease |
| `minimize` | the three effects as `MinimizeAnimation`: warp 450 ms into the slot and 420 ms back out of it, funnel 400 ms, scale 200 ms (×2 in slow motion), on a linear progress (the generators ease themselves), minimize / restore / reverse at any time |
| `transitions` | per-`WindowKind` open / close transforms (`TransformKind`): plain windows none, document and app-modal grow-in 0.2 s (scale 0 → 1, alpha), utility and app-modal fade-out 0.15 s, alert pop-in 0.4 s (peak 1.2× at 60 %) and scale-down-and-out 0.15 s (1 → 0.9), pop-out 0.4 s, ×4 in slow motion, `sin²` ease |
| | sheets (`WindowKind::Sheet`, `SheetIn` / `SheetOut`): 0.26 s on linear time, 32 pt slide eased `sin²` (the sample's `dy`), alpha `min(2p, 1)` in and `1 − p` out; slow motion leaves them unchanged |
| | spring pop-in / pop-out (stiffness 385.53142 / 322.2728, damping 31.41593 / 30.51833) |
| | zoom and frame changes: `min(0.35, 181·ln(maxΔ/100 + 1)/750)` s, `sin²` ease (`frame_change_duration`, `WindowZoom`) |
| | full screen 0.5 s, frame bézier (0.25, 0, 0, 1), overlay fade (0, 1, 1, 1) over the first half (`FullScreenTransition`) |
| | overview zoom (0.25 s, 2.0 s slow), show desktop (spring damping ratio 1.0, response 0.3 s), workspace slide (spring 0.4 s) |
| `space_slide` | `SpaceSlideState`: the workspace slide solver, a damped chase of the snap target at a fixed 60 Hz with a rubber band outside the first and last workspace; `point_in_x`, `settle_time`, `SlideStart` |
| `raster` | CPU textured-mesh renderer (2× supersampled), wireframe, test pattern, contact sheet |

## How it works

### Warp landing

The formulas collapse the window onto the near edge of its slot. `Landing::IconRect` (the default, an
extension) continues the near end to the far edge of the slot and blends the side offsets from the stretch phase at the
far end to the slide phase at the near end, so the mesh at `p = 1` is exactly the slot rectangle and at `p = 0` exactly
the window. `Landing::IconEdge` is the literal version. Both are tested.

### Output

`cargo run -p cupertino-effects --release --example render_effects` writes `output/minimize_contact_sheet.png` (warp /
funnel / scale at 11 progress values), `warp_wireframe.png`, `warp_directions.png` (bottom dock below and above the window,
right and left dock), `warp_interrupted_at_270ms.png` (minimize reversed after 270 ms) and `transitions_contact_sheet.png`
(open, workspace slide, overview zoom, show desktop).

## Tests

`cargo test -p cupertino-effects` and `cargo bench -p cupertino-effects`. Tests cover mesh endpoints (window at `t = 0`,
slot at `t = 1` for every kind and direction), monotonic approach of the texture corners, no NaN, vertex budgets (warp ≤
800, funnel ≤ 400, scale 4), reversal and interruption continuity, spring velocity retention, the verified durations, and
rendered-pixel checks through the CPU rasteriser. The bench prints ns per mesh: warp ≈ 2.6 µs, funnel ≈ 2.8–5.3 µs, scale
≈ 20 ns on the development host.
