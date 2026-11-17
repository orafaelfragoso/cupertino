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
