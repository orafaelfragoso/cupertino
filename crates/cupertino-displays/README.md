# cupertino-displays

Display mode model: the scaled "looks like" sizes a display offers and the scale a new display starts at. Pure functions,
no I/O, no dependencies., §1a. Layer: `models`.

Used by `cupertino-server` (`displays::default_scale`: the scale of a display without stored settings).

## Public API

| Item | Contents |
|---|---|
| `looks_like(panel, modes) | the design display settings' trimmed mode list: buckets of mode heights around the height that gives 110 points per inch (half-width target / 12, step anchor height / 6, the near-floor rule with divisor 5), one representative mode per bucket (the mode filling the native buffer, then the higher scale, the rate, the nearer height), at most five choices, 1x modes added below four HiDPI choices; nothing below 640 points (times the aspect ratio when rotated), nothing at or above 180 points per inch, modes of another aspect ratio (beyond 1 %) left out |
| `Panel::from_physical` | a display from its pixel size and physical size in millimetres; `None` when the physical height is unknown (virtual outputs, no size in the EDID); `precise_rate` is the display's request for precise refresh rates |
| `candidates(pixels, refresh_mhz) | the modes this compositor offers instead of an installed scaled-mode table: one per scale from 1 to 3 in eighths (exact in the fractional-scale protocol's 1/120 units), each drawn from a buffer of the next whole scale |
| `default_choice(panel, refresh_mhz) | the scale a new display starts at: the anchor of its list; `None` (unscaled) when the native height is at or below the floor |
| `scaled_choices(panel, refresh_mhz) | the "looks like" choices among the candidates, from the largest text to the most space |

## Tests

`cargo test -p cupertino-displays`. A dense laptop panel defaults to twice the pixels per point, a 4K desktop monitor
looks like 1440 points, a low-density monitor stays unscaled and offers larger text, a panel at the floor has no list, a
rotated panel raises the floor by its aspect ratio, and the tests cover the tie-breaking order, modes of another aspect
ratio, and the candidate scales.
