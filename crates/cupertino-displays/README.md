# cupertino-displays

Display mode model: the scaled "looks like" sizes a display offers, the scale a new display starts at, and an output's
quantities (mode, logical extent, scale, rasters) kept apart. Pure functions,
no I/O, no dependencies., §1a. Layer: `models`.

Used by `cupertino-server` (`displays::default_scale`: the scale of a display without stored settings; `displays` and
`displays::recovery`: the arrangement checks, a returning display's place, windows and the pointer kept reachable).

## Public API

| Item | Contents |
|---|---|
| `looks_like(panel, modes) | the design display settings' trimmed mode list: buckets of mode heights around the height that gives 110 points per inch (half-width target / 12, step anchor height / 6, the near-floor rule with divisor 5), one representative mode per bucket (the mode filling the native buffer, then the higher scale, the rate, the nearer height), at most five choices, 1x modes added below four HiDPI choices; nothing below 640 points (times the aspect ratio when rotated), nothing at or above 180 points per inch, modes of another aspect ratio (beyond 1 %) left out |
| `Panel::from_physical` | a display from its pixel size and physical size in millimetres; `None` when the physical height is unknown (virtual outputs, no size in the EDID); `precise_rate` is the display's request for precise refresh rates |
| `candidates(pixels, refresh_mhz) | the modes this compositor offers instead of an installed scaled-mode table: one per scale from 1 to 3 in eighths (exact in the fractional-scale protocol's 1/120 units), each drawn from a buffer of the next whole scale |
| `catalogue(panel, refresh_mhz)`, `Catalogue` | the "looks like" choices from the largest text to the most space and the index of the default. A built-in panel (`Panel::built_in`) offers its ladder: `BUILT_IN_STEPS` (1024, 1147, 1352, 1512, 1800) in 1512ths of its default width, the default being the size at `BUILT_IN_POINTS_PER_INCH` (127), at a whole scale when that is within `WHOLE_SCALE_SNAP` (5 %); heights follow the panel's aspect ratio, sizes below the floor are left out, and a size with more space than the panel has is the panel's own, unscaled, once. Each scaled size is drawn from a raster of twice its points (`Candidate::backing`, `raster`); its effective scale is the panel's width over its width, so the output shows exactly its points. Any other display gets the buckets of `looks_like` over `candidates`, starting at the anchor |
| `default_choice(panel, refresh_mhz) | the scale a new display starts at: a built-in panel's default size, an external display's anchor; `None` (unscaled) when the native height is at or below the floor |
| `scaled_choices(panel, refresh_mhz) | `catalogue(..).choices` |
| `is_built_in(connector)` | whether a connector name (`eDP-1`, `LVDS-1`, `DSI-1`) is the machine's own panel |
| `arrangement`: `Area`, `check`, `ArrangementError`, `free_place`, `relocate`, `reachable_point` | the displays that are on as one arrangement in the global logical space: `check` refuses an empty set, an area that is empty or not finite, one beyond `COORDINATE_LIMIT` (the 32-bit range positions travel in) and two that overlap (touching along an edge is fine; there is no mirroring); `free_place` keeps a returning display's place when it is free, else puts it right of the others level with the highest; `relocate` keeps a frame's distance from its display's top-left corner on another display; `reachable_point` brings a point no display covers onto the nearest point of the nearest display |
| `geometry`: `Extent`, `OutputGeometry`, `GeometryError`, `raster`, `scale_in_120ths` | the quantities of an output kept apart: the mode in panel pixels (not turned), the logical extent in points (the turned mode over the scale, rounded), the effective scale, and buffer rasters for a logical extent at a scale in 120ths (halves rounded up, as the fractional-scale protocol does; `whole_raster` at the next whole scale). `to_points` / `to_pixels` map panel pixels and points by the scale the output is drawn with; `snap` and `snap_outward` put edges on panel pixel edges. A zero extent, a scale that is not finite and positive, or a side that overflows fails with `GeometryError` |

## Tests

`cargo test -p cupertino-displays`. A dense laptop panel defaults to twice the pixels per point, a 4K desktop monitor
looks like 1440 points, a low-density monitor stays unscaled and offers larger text, a panel at the floor has no list, a
rotated panel raises the floor by its aspect ratio, and the tests cover the tie-breaking order, modes of another aspect
ratio, and the candidate scales. A 14-inch 3024 × 1964 built-in panel offers 1024 × 665, 1147 × 745, 1352 × 878,
1512 × 982 (default, scale 2) and 1800 × 1169, rasters twice their points; the buckets of `looks_like` over that panel's
whole table of 22 sizes pick the same five; eight built-in panels from 1280 × 800 to 3840 × 2400 get ordered ladders
with their default, the panel's aspect ratio and outputs of exactly their points; connectors tell the built-in panel.
`geometry` tests: logical extents, rasters and their rounding, refusal of empty, invalid and overflowing sizes,
pixel-point mapping at 1, 1.25, 1.5, 1.75, 2 and 2.953125, and snapped edges on pixel edges. `arrangement` tests: side by side and
stacked displays pass, overlapping and identical ones are refused, empty, non-finite and out-of-range areas are refused, a
returning display keeps a free place or goes right of the others, a frame keeps its offset across displays, and a lost
point comes onto the nearest display.

Decision: the built-in ladder is used only for a connector of the machine's own panel; an external display keeps the
bucket list, so the default of external monitors does not change.
