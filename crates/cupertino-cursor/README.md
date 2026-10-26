# cupertino-cursor

Builds an **XCursor theme** (and optionally a **hyprcursor source theme**) from per-cursor vector PDFs plus metadata JSON
(hotspots, shadow parameters, animation), or from a built-in **procedural cursor set** drawn with `tiny-skia`, with no proprietary input.

> **Local use only.** A theme built from `--source` derives from a locally extracted *reference cursor set* whose artwork is
> proprietary. The output defaults to the git-ignored, carries a `README.txt` saying
> so, and must never be committed, redistributed or shipped. The procedural set (no `--source`) has no third-party artwork.

## Usage

```
cargo run -p cupertino-cursor --release -- [options]

  --source DIR          directory with <cursor>/cursor.pdf (+ optional info.plist)
  --procedural          built-in cursors only (default without --source)
  --metadata FILE       metadata JSON (default with --source: cursor-metadata.json)
  --output DIR          default: cursor-theme/ (git-ignored)
  --name NAME           theme name (default Cupertino)
  --sizes 24,32,48,64,96   nominal sizes (default; 96 is the largest cursor size, 4x)
  --base-size 24        nominal size that equals 1 pixel per point
  --hotspot unscaled|scaled
  --no-shadow | --no-default-shadow | --no-flip-shadow-y | --no-fill | --default-delay-ms N
  --hyprcursor          also write <name>-hyprcursor/
  --inherits THEME      Inherits= in index.theme
  --pdftocairo PATH / --pdfinfo PATH
  --list-names          print the name mapping
```

Examples:

```
cupertino-cursor --procedural --output /tmp/themes                       # procedural theme
cupertino-cursor --source <reference cursor set (local extraction)>      # PDF theme →
XCURSOR_PATH=$PWD/cursor-theme XCURSOR_THEME=Cupertino <program>
cargo run -p cupertino-cursor --release --example contact_sheet -- <theme dir> sheet.png 48
```

Library:

```rust
use cupertino_cursor::{build_theme, BuildOptions, Source};

let mut o = BuildOptions::new("out");              // or the git-ignored default of the CLI
o.sizes = vec![24, 32, 48, 64];
o.source = Source::Pdf("path/to/reference-cursors".into());
o.metadata = Some("cursor-metadata.json".into);
let report = build_theme(&o)?;                     // Report { theme_dir, files, symlinks, warnings, hypr_dir }
```

`theme::build_cursors` returns the rasterised `Cursor`s (pixmaps, hotspots, delays) without writing,
`xcursor::{write, parse}` is the file format, `shadow::apply` the shadow baker, `procedural::render`
draws one built-in cursor, `names::REFERENCE_MAP` is the name table. Any XCursor loader reads the result; there is no Smithay dependency.

## Design

### Pipeline

1. **Metadata** per cursor: the JSON file (shape of) wins. Otherwise
   the binary `info.plist` next to the PDF is read by the crate's own bounds-checked `bplist00` reader.
   Otherwise defaults.
2. **Page geometry**: `pdfinfo` gives the page size in points. An animation page stacks its frames vertically (frame height =
   page height / `frames`). Without metadata, a page at least twice as tall as wide whose height is a multiple of its width
   is taken as that many square frames (warning, approximate).
3. **Rasterisation**: `pdftocairo -png -transp -scale-to-x W -scale-to-y H` (poppler) renders the page
   at exactly `round(w·s) × frames·round(h·s)` pixels with transparency, `s = size / base_size`
   (24 → 1 px/pt, 32 → 1.33, 48 → 2, 64 → 2.67). `pdftoppm` is not used (no transparency). The sheet is cut into frames.
4. **Shadow** (metadata `shadowcolor`, `shadowoffsetx/y`, `blur`): Gaussian blur of the alpha channel
   with σ = `blur` points × s, tinted with the colour and its alpha, shifted by the offset, composited
   *behind* the cursor. The canvas grows by `ceil(3σ) + |offset| pixels per side. The source's y offset
   points up, so −1 means one point down on screen (`--no-flip-shadow-y` to change). Cursors
   without metadata get the common shadow of the set (`0,0,0,0.65`, blur 1.8, 1 pt down) unless
   `--no-default-shadow`.
5. **Hotspot**: metadata points × s, rounded, plus the shadow padding. The meaning of the `*-scaled`
   keys is unknown; `--hotspot scaled` selects them. Cursors without a hotspot use the frame centre (warning).
6. **XCursor files**: one file per cursor with all sizes (ARGB32 premultiplied, little-endian).
   Frames of one size follow each other with their delay.
7. **Names**: the primary freedesktop name is the file, aliases are relative symlinks
   (`names.rs`): `default left_ptr arrow`, `pointer hand1 hand2`, `text xterm`, `grab`, `grabbing`,
   `not-allowed no-drop crossed_circle`, `crosshair cross`, `copy`, `alias link`, `context-menu`, `help`,
   `progress wait watch left_ptr_watch`, the eight `*-resize` plus `ew/ns/nesw/nwse/col/row-resize` and
   their legacy X names, `zoom-in`, `zoom-out`, `cell`, `move all-scroll fleur`, `vertical-text`. The
   reference set has no plain arrow, horizontal text cursor or spinning wait cursor: `default` and `text` come from the
   procedural set, and `wait`/`progress` fall back to the animated "busy but clickable" cursor. Reference cursors without
   a freedesktop meaning (counting hands, `poof`, one-sided resize arrows, screenshot cursors) keep their own names.
   Missing names are completed procedurally unless `--no-fill`.
8. **Output safety**: an existing `<output>/<name> directory is replaced only if it carries the
   `.generated-by-cupertino-cursor` marker. Anything else is refused.

### hyprcursor source theme (`--hyprcursor`)

`<name>-hyprcursor/manifest.hl` and `hyprcursors/<shape>/{meta.hl, image<side>.png | anim<side>_<n>.png}`
follow the hyprcursor authoring format: square images (content padded at the top-left), `hotspot_x/y` normalised by the
side, `define_override` lines for the aliases, animation frames as repeated `define_size` lines (the image's pixel side)
with a timeout in ms. Compile with `hyprcursor-util --create`.

### Procedural set

31 cursors on a 32 × 32 pt canvas (arrow with badges for copy/alias/context-menu/help/progress, hands, I-beams, crosshair,
cell, move, magnifiers, not-allowed, resize arrows, a 12-frame spinner), black/white with a halo, drawn at the target scale (crisp at every size)
with the common shadow. `output/procedural_cursors.png` is the contact sheet.

## Tests

`cargo test -p cupertino-cursor` (51 tests) covers:

* XCursor writer/parser round trips, shadow maths, the `bplist00` reader (round trip, malformed and cyclic input), metadata parsing (including the repository file) and the name table (no duplicates, required names covered);
* the procedural theme (every required name present and parseable, symlinks, sizes, hotspot scaling, spinner frames, shadow padding, overwrite protection) and the CLI defaults;
* end-to-end PDF tests with PDFs generated in the test, only when poppler is installed.

`CUPERTINO_CURSOR_SOURCE` set to a reference cursor directory additionally parses every `info.plist` and builds the full set
(all 39 cursors, none blank); that run passed locally.

## Open issues

The hyprcursor output is not validated against hyprcursor itself.
