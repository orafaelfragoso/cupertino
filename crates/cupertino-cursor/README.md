# cupertino-cursor

Builds an **XCursor theme** (and optionally a **hyprcursor source theme**) from the built-in **procedural cursor set**,
vector cursors drawn with `tiny-skia`. The theme contains no third-party artwork. The server draws the same set
when no asset pack provides a cursor theme.

## Usage

```
cargo run -p cupertino-cursor --release -- [options]

  --procedural          accepted for scripts: the procedural cursors are the only source
  --output DIR          default: output/cursor-theme/ of the crate (git-ignored)
  --name NAME           theme name (default Cupertino)
  --sizes 24,32,48,64,96   nominal sizes (default; 96 is the largest cursor size, 4x)
  --base-size 24        nominal size that equals 1 pixel per point
  --no-shadow           do not bake shadows
  --hyprcursor          also write <name>-hyprcursor/
  --inherits THEME      Inherits= in index.theme
  --list-names          print the cursor names and their aliases
```

Examples:

```
cupertino-cursor --output /tmp/themes                                    # the procedural theme
XCURSOR_PATH=/tmp/themes XCURSOR_THEME=Cupertino <program>
cargo run -p cupertino-cursor --release --example contact_sheet -- <theme dir> sheet.png 48
```

The contact sheet puts each cursor whole on a light and a dark tile side by side and marks its hotspot in red.

Library:

```rust
use cupertino_cursor::{build_theme, BuildOptions};

let mut o = BuildOptions::new("out");              // or the git-ignored default of the CLI
o.sizes = vec![24, 32, 48, 64];
let report = build_theme(&o)?;                     // Report { theme_dir, files, symlinks, hypr_dir }
```

`theme::build_cursors` returns the rasterised `Cursor`s (pixmaps, hotspots, delays) without writing,
`xcursor::{write, parse}` is the file format, `shadow::apply` the shadow baker, `procedural::render`
draws one built-in cursor, `names::NAMES` is the name table. Any XCursor loader reads the result; there is no Smithay
dependency.

## Design

### Pipeline

1. **Drawing**: each procedural cursor is drawn at the target scale `s = size / base_size` (24 → 1 px/pt, 32 → 1.33,
   48 → 2, 64 → 2.67), so it is crisp at every size. An animation (the spinner) has one image per frame.
2. **Shadow**: Gaussian blur of the alpha channel with σ = 1.8 points × s, tinted `0,0,0,0.65`, shifted one point
   down, composited *behind* the cursor (`--no-shadow` skips it). The image is cut back to the cursor's canvas, so the
   shadow falls in the canvas's transparent margin and every size keeps the role's canvas.
3. **Hotspot**: the cursor's hotspot in points × s, rounded.
4. **XCursor files**: one file per cursor with all sizes (ARGB32 premultiplied, little-endian). Frames of one size follow
   each other with their delay.
5. **Names**: the primary freedesktop name is the file, aliases are relative symlinks (`names.rs`):
   `default left_ptr arrow`, `pointer hand1 hand2`, `text xterm`, `grab`, `grabbing`, `not-allowed no-drop crossed_circle`,
   `crosshair cross`, `copy`, `alias link`, `context-menu`, `help`, `progress wait watch left_ptr_watch`, the eight
   `*-resize` plus `ew/ns/nesw/nwse/col/row-resize` and their legacy X names, `zoom-in`, `zoom-out`, `cell`,
   `move all-scroll fleur`, `vertical-text`.
6. **Output safety**: an existing `<output>/<name>` directory is replaced only if it carries the
   `.generated-by-cupertino-cursor` marker. Anything else is refused.

### hyprcursor source theme (`--hyprcursor`)

`<name>-hyprcursor/manifest.hl` and `hyprcursors/<shape>/{meta.hl, image<side>.png | anim<side>_<n>.png}`
follow the hyprcursor authoring format: square images (content padded at the top-left), `hotspot_x/y` normalised by the
side, `define_override` lines for the aliases, animation frames as repeated `define_size` lines (the image's pixel side)
with a timeout in ms. Compile with `hyprcursor-util --create`.

### Procedural set

31 cursors (arrow with badges for copy/alias/context-menu/help/progress, hands, I-beams, crosshair,
cell, move, magnifiers, not-allowed, resize arrows, a 12-frame spinner), black/white with a halo, drawn at the target
scale with the common shadow. `output/procedural_cursors.png` is the contact sheet. Each role has its own canvas and
hotspot, in points (transparent margin included):

| Cursor | Canvas | Hotspot |
|---|---:|---:|
| arrow (`default`) and its badged variants | 28 × 40 | 5, 5 |
| I-beam (`text`) | 23 × 22 | 12, 11 |
| pointing hand (`pointer`) | 32 × 32 | 12, 8 |
| open and closed hand (`grab`, `grabbing`) | 32 × 32 | 15, 16 |
| column resize (`col-resize`) | 30 × 24 | 15, 12 |
| row resize (`row-resize`) | 24 × 28 | 12, 14 |
| every other cursor | 32 × 32 | its centre (magnifiers 13, 13) |

At a nominal size `N` an image is the canvas times `N / base_size` and its hotspot the point times the same scale, so
1x, 2x and 4x images (24, 48, 96) put the hotspot on the same logical point.

## Tests

`cargo test -p cupertino-cursor` covers:

* XCursor writer/parser round trips, shadow maths and the name table (no duplicates, required names covered, every
  procedural cursor mapped);
* the procedural theme (every required name present and parseable, symlinks, sizes, hotspot scaling, spinner frames,
  the shadow inside the canvas, overwrite protection) and the CLI options;
* each role's canvas and hotspot at 1x, 2x and 4x, with the shape drawn by its hotspot.

## Open issues

The hyprcursor output is not validated against hyprcursor itself.
