# Plan: GPU rendering

The design is in `docs/src/gpu-rendering.md` (in the book: "GPU rendering") and the rules every change keeps are in
`docs/architecture/rendering.md`. This file is the working plan: every step, what it needs, and when it counts as
done. Tick a box only when the item is on `main`; tick a step's gate only when its "done when" holds. Record
measurements under "Measurements" with the commit they were taken at.

Target machine for every measured gate: NVIDIA (proprietary driver), 3840 x 2160 at 60 Hz, session scale 1.5.
Headless numbers are also recorded at scale 2 for comparison.

## Rules for every step

- The CPU path stays green: `scripts/done` and `scripts/test --slow` pass at every commit.
- After the look freeze (step 1), goldens are re-recorded only by a change that names the pixels it changes, with a
  before/after sheet.
- A step that adds a GPU path keeps the fallback rule working and is covered by the local GPU tier
  (`CUPERTINO_GPU_REQUIRED=1`).
- No step ships if it misses a budget of its own; it is fixed or reverted.
- An item too large for one reviewable change is split in this file before work on it starts.

## Step 0: instruments, baseline and probes

Nothing is optimised before it is measured on the target machine, and no gate may rely on a tool that does not exist.

Instruments:

- [x] `scripts/dev/bench` takes size and scale as arguments.
- [x] Bench scenarios: idle, window drag over glass, title-bar hover, live resize, Control Center open, launcher
      typing (the Files two-finger scroll exists).
- [x] Toolkit processes write per-phase frame timings (update, view, layout, paint, raster, copy, commit) to a file
      named by an environment variable; bench collects them per process.
- [x] Bench counts client frames longer than one refresh, per process, besides the compositor's missed refreshes.
- [x] `scripts/dev/measure` reports private dirty memory besides Pss and Rss.
- [x] Counters in the frame statistics for decoration and HUD rasters and uploads, and for blur passes.
- [x] The DRM backend logs, on request, which presentation-time branch it takes (`flip-timestamp` or `own-clock`), missed
      flips, plane assignment (cursor plane, direct scanout) and input-to-flip latency.
- [x] Every timer that wakes a process on an idle desktop recorded with its reason; each is kept (and listed under
      "Measurements") or removed.

Baseline:

- [x] Every bench scenario at scale 1.5 and 2: frame time p50/p99, missed refreshes, slow client frames, compositor CPU
      and GPU time, per-phase client split, per-process CPU and private memory.
- [x] Each scenario run five times; the run-to-run spread of every figure recorded as its noise.
- [x] One measured table for Files replacing the two figures in the docs (full repaint at 3840 x 2160, two-finger scroll
      p99 at scales 1, 1.5 and 2).
- [ ] Presentation-time branch and plane assignment on the target driver recorded. BLOCKED (owner action): it needs
      the session server on the display ("Measurements").

Probes (small programs outside the product; results recorded here):

- [x] Context creation, first program, later programs and teardown: time and private memory, from a Rust binary.
- [x] Render-target texture cost in total GPU memory, the first one in a process and later ones.
- [x] A saved program binary as the first program of a fresh process: time and memory against compiling.
- [x] Native fence soak: resident-memory slope with and without the fence over 10 million frames.
- [ ] 10-bit (`Abgr2101010`) gradient on the primary plane with the driver's dithering off, against 8-bit. BLOCKED
      (owner action): it needs the display to itself ("Measurements").

Gate:

- [x] All of the above recorded under "Measurements" (the two owner-action items as BLOCKED); if update, view and
      layout exceed 4 ms in a Files scroll frame, step 4 gains the incremental view and layout item. They do in the
      worst frame of a full-screen scroll (5.5–6.1 ms at scales 1 and 1.5), so it does.
- [x] Each budget in the design confirmed or revised once against the baseline, in a reviewed change, with the reason
      recorded ("Measurements", "Budgets against the baseline").

## Step 1: shared maths and the look freeze

One owner review, one re-recording of the goldens.

- [x] A graphics layer in `cupertino-layers` that the toolkit, the render layer and the compositor may depend on,
      and that depends on the foundation layer only; `layers.dot`, the architecture page and the layer tests updated.
- [x] A crate in it with the definitions as plain Rust and test vectors: rectangle, rounded-rectangle and
      continuous-corner distance and coverage; the Gaussian kernel with its pyramid levels, edge rule, premultiplied
      alpha and intermediate precision; drop and inner shadow; gradient interpolation (at most eight stops);
      position-keyed dithering.
- [x] The blur planner moves from `cupertino-glass` into it; the compositor uses it from there.
- [x] CPU renderer: rectangle, rounded-rectangle, stroke and clip coverage from the distance function; nested clips
      multiply coverage.
- [x] CPU renderer: blur and shadows from the shared kernel.
- [x] CPU renderer: gradients per pixel from the shared rule.
- [x] CPU renderer: scroll offsets rounded to device pixels when drawn.
- [x] `cupertino-decorations` shapes (title bar, window buttons) from the shared distance function, so the later move
      to GPU pipelines changes no pixel.
- [x] Compositor dithering from the shared definition (blur, material, glass and shadow outputs), tested against the
      vectors. On from the look freeze (see "The definitions and where the renderers disagree").
- [ ] Before/after sheet of every golden at scales 1, 1.5 and 2, reviewed by the owner. The sheet is made (see "The
      look freeze"); the owner's review is pending.
- [x] One re-recording of every golden.

### The definitions and where the renderers disagree

The graphics layer holds `cupertino-raster-math` (MPL-2.0, so the toolkit may link it): `coverage`, `blur` (the
planner, moved from the glass crate, with the kernel, edge rule, precision and an `f32` implementation), `shadow`,
`gradient` and `dither`, each in `f32` as the shaders compute it. Its test vectors are plain text in
`crates/cupertino-raster-math/vectors/` (columns named in each file's first line), checked by its tests; a change to a
definition fails there and `CUPERTINO_WRITE_VECTORS=1` writes them again. Each definition was taken from what the code
does; where the compositor and the CPU renderer disagree, the definition is the compositor's (the glass tests and the
compositor's half of the goldens rely on it), and the CPU renderer moves to it at the freeze. For the look-freeze
review:

- **Coverage.** Compositor (shadow cut-out, rounded rectangles, clip, material, glass): the closed-form first-order
  distance, coverage `clamp(0.5 − d · px, 0, 1)`. CPU renderer: the shape's cubic path filled by its rasterizer's
  anti-aliasing (supersampled), strokes by its stroker, clips as masks filled the same way. Definition: the distance.
  The glass program keeps a copy of its own (`rect_sdf`): it holds the extent to half the size, treats exponents up to
  2.0 (not 2.001) as circular and writes the straight parts as one expression; the values are the same in practice.
- **Blur.** Compositor: 2× box halvings to the backdrop scale and while the variance is above 64 texels², then a
  Gaussian (radius `ceil(3σ)`, at most 35) on the last level; edges clamped; levels in half floats where the device
  renders to them, 8 bits otherwise. CPU renderer (`Effect::Blur`): three box passes per axis of radius
  `round(radius · scale / 2)`, transparent edges, every pass rounded to 8 bits. Definition: the compositor's pyramid
  and kernel, σ = radius / 2 for a toolkit radius, edges clamped for a backdrop and transparent for a layer's own
  effects, half-float levels with 8 bits as the fallback.
- **Drop shadow.** Compositor (window shadows): the blurred rounded box in closed form, two error functions across x
  and an 8-step integral over ±3σ across y, times a density, the window's own area optionally cut out. CPU renderer:
  the content's alpha moved by its offset rounded to whole device pixels, blurred by the three box passes in 8 bits,
  tinted and drawn under. Definition: the closed form for rounded rectangles, offsets not rounded. The closed form
  peaks a little under one (`blur_mass`, 0.9979 for σ = 4) since the tails past 3σ are left out.
- **Inner shadow.** CPU renderer only: the inverted alpha moved and blurred as above, kept inside the content and drawn
  over it. Definition: one minus the closed form divided by `blur_mass` (so a large caster's middle stays clear), times
  the caster's coverage.
- **Gradients.** CPU renderer: its rasterizer's gradients, straight stops interpolated in encoded sRGB and
  premultiplied after, any number of stops, at the precision its pipeline picks. Compositor: only the wallpaper's
  procedural mix of opaque colours. Definition: two to eight stops in order, straight encoded sRGB interpolated
  linearly in `f32`, premultiplied after, end colours held past the ends, evaluated at pixel centres; more stops are
  refused.
- **Dithering.** Neither dithers today. Definition: before an 8-bit store, `v + t / 255` with
  `t = (m + 0.5) / 64 − 0.5` from the 8 × 8 ordered matrix entry `m` of the framebuffer pixel; exact 8-bit values are
  kept. Colour channels of straight-alpha outputs (blur, material, glass), alpha of the shadow.
- **8-bit stores.** The definition rounds to nearest; a driver may round a value within a few hundredths of a step of
  the half step either way (the local driver does), so a shader test compares such stores within one step and the
  matrix itself exactly.

Dithering changes pixels, so it waited for the look freeze: `cupertino_raster_math::dither::BY_DEFAULT` is now
`true`, and the compositor compiles its blur (into 8-bit levels only), material, glass and shadow programs with it;
`CUPERTINO_DITHER=0` turns it off and `=1` on, to compare. The GL tests (`dither::` in the server's suite, slow tier)
draw the shader's matrix and dithered stores and the dithered shadow against the definition.

### The look freeze

What each renderer now computes, and what it was tested against (rule 4):

- **CPU renderer coverage.** Fills and strokes of rectangles and rounded rectangles, and clips of those shapes, take
  each pixel's coverage from its centre's distance to the outline under the node's transform (distance in the shape's
  units times the transform's scale); a pixel's coverage depends on its position only, so the span path, the scratch
  path and any damage give the same bytes. Strokes stay centred on the outline: the shared stroke of the outline moved
  out by half the width (a rectangle's corners stay square). Nested clips multiply. Paths (continuous corners in the
  toolkit are paths), images and glyphs keep their rasterizers (rule 7). Each row is read in from both ends to the
  fully covered (or hollow) run, which is written at once. Tested against `coverage.txt`.
- **CPU renderer gradients.** Per pixel at its centre in node coordinates, by the shared rule, on every shape and on
  paths (their coverage as a mask). Offsets are held to 0 to 1 and made to increase; one stop is that colour; **more
  than eight stops are refused and the paint draws nothing**. Tested against `gradient.txt`.
- **CPU renderer blur and shadows.** A layer's blur and shadows take the shared plan with no capture halving (2x
  halvings while the variance is above 64, the Gaussian on the last level), transparent edges, levels in `f32` (finer
  than the half floats the definition allows), the last level read back linearly at every pixel, moved by a shadow's
  offset unrounded. The inner shadow is one minus the blurred silhouette, times the caster's coverage. Tested against
  `blur.txt` and, for a rounded rectangle, the closed form of `shadow.txt`'s definitions (within 0.012; the closed form
  leaves the tails past 3σ out and approximates the error function).
- **Dithering.** Blur colour channels and shadow alpha, as the definition names; the definition now also names a
  gradient's colour channels (premultiplied, held at its alpha, which is rounded), so the CPU renderer and the
  decorations dither gradients. Keyed to the window's (or buffer's) pixel.
- **Scroll offsets.** A scroll container places its content by its offset rounded to the window's device pixels; its
  content size and scroll events read the same drawn offset.
- **Decorations.** The title bar's fill, the rim inside the frame (now the shared inside stroke of the frame's
  outline, its continuous corners from the outline's extents and exponent), the buttons' discs and their two edge
  rings (inside strokes) and the button faces' and rings' gradients come from the shared functions. The buttons'
  glyphs stay paths.

The freeze was taken pending the owner's review of the before/after sheet: every golden image (before: the base
commit's, after: re-recorded), the preview scenes at scales 1, 1.5 and 2 in both appearances, and the review sheet's
windows, corners, dock, menus and Control Center in both appearances, side by side with the changed pixels and
enlarged crops of the most changed places.

The one re-recording changed 67 golden images: 46 of the kit's 52 control fixtures (and the text box's semantics dump, whose scrolled text now sits on a whole point), 5 of
Control Center's, Files' icon view, the menu bar's 3, 5 of the renderer's 10 (its chrome and clip scenes) and 7 of the
compositor's 8 (all but the empty desktop; the launcher's drawn region is one pixel taller). Edge pixels of
rectangles and rounded rectangles move by up to about 30/255, fully covered and empty pixels not at all; shadows and
blurs move by the Gaussian against three box passes; gradients and the compositor's blurred and glass outputs move by
the dither, a step at most.

Two server goldens already failed before the freeze, for changes of their own, and the re-recording takes those up too;
the sheet names them apart from the shared maths: the overview no longer dims the desktop behind it (561 thousand of its
pixels are lighter), and the launcher's large panel is 177 rows instead of 176, with its dark edge and bevel.
Step 2's work merged before the re-recording (reduced copies and the minification filter of shrunk windows, partial
blurs again) changes no golden past the comparison's tolerance. A gradient with more than eight stops draws nothing, so the scroll
edge's veil, the one gradient that had nine, now has eight; its opacity is linear down the band, so it draws the same.
Revealing a scrolled target (the caret, a focused row) measures it by the offset itself, not by its whole-pixel place.
The GL test `coverage_gl::` (server suite, slow tier) draws the compositor's rounded rectangles, a continuous corner
and an inside border at scales 1, 1.5 and 2 and compares every pixel with the shared coverage.

Gate:

- [ ] The owner approved the look, the goldens are re-recorded, and no bench scenario's client CPU p99 is more than
      10% above its baseline. The goldens are re-recorded and the bench holds (see "Step 1: the look freeze" under
      Measurements); the owner's review of the before/after sheet is pending.

## Step 2: compositor wins

- [x] Non-glass materials use the overlap-limited backdrop generation glass uses.
- [x] The minimize effect reuses one captured texture.
- [x] Per-frame waste removed: HUD signature string, cursor image clone, wallpaper path bookkeeping.
- [x] Reduced copies per window for windows drawn below half their size (overview, switcher, minimize, zoom), as a
      hand-written texture program with the external-sampler variant.
- [x] A better minification filter for integer-scale clients shown at a fractional scale.
- [x] Objective checks: a reduced window against a box-filtered reference (shimmer); run-length check of a captured
      gradient under glass (banding).

Gate:

- [ ] Window drag over glass meets its budgets (p99 CPU time to compose and submit a frame under 2 ms, p99 GPU time
      under 4 ms) with fewer texels written by blur passes per frame than the baseline; the shimmer and banding
      checks pass. (Revised from "fewer blur passes": every frame of a drag uncovers something, so every frame needs
      a blur; the work the blur does is what can fall. The budget was then split into its CPU and GPU halves, each
      measured on its own. See Measurements, step 2.)

## Step 3: vertical slice of the GPU path

The risk is the handoff, so it is proven first with the smallest renderer possible.

- [x] The headless backend offers `linux-drm-syncobj-v1` when its device has a render node, with a flag to withhold it.
- [x] A test client renders solid rectangles with GLES into GBM buffer objects on the feedback device, with the
      offered modifiers, never adding linear on a hardware driver.
- [x] It attaches them through `zwp_linux_dmabuf_v1` v4.
- [x] Acquire through native fence → syncobj timeline point; release waited through a syncobj eventfd.
- [x] The syncobj surface object is destroyed before a shared-memory attach and re-created on return to dmabuf.
- [x] Without the protocol: wait for the own fence before commit; reuse after release plus one frame.
- [x] The client counts missed refreshes from presentation-feedback sequence gaps.
- [x] Overdraw test: many layers of drawing ending in one flat colour; every capture must be uniform.
- [x] The compositor imports the client's buffers with the driver's default modifier (recorded).
- [x] Probe: the slice on llvmpipe in the goldens image, allocating by texture export; result recorded.
- [ ] Runs in a headless session and in the live session on the target machine. Headless: done
      (`crates/cupertino-server/tests/gpu_slice.rs`). Live: BLOCKED (owner action), one command ("Measurements",
      "Step 3").

Gate:

- [ ] One hour in the live session on the target machine beside a `wl_shm` control client of the same size: no more
      missed refreshes than the control, and every overdraw capture uniform. BLOCKED (owner action): the command is
      under "Measurements", "Step 3".
- [x] The slice's offscreen loop of 10 million frames meets the fence budget.

## Step 4: retained layers in the toolkit

Works with the CPU renderer; no GPU needed.

- [x] The paint tree marks layer boundaries: scroll content (`Layer::Scroll` with its drawn offset, the scroll bars
      beside it) and opacity groups (`Layer::Opacity`, fades included).
- [x] The paint tree marks animated subtrees: a node whose transform animates (`Layer::Transform`, a popover's roots
      while it grows out of its anchor or shrinks into it;
      `a_growing_popover_marks_its_roots_as_layers_whose_transform_animates`).
- [x] The CPU renderer keeps scroll layers as images between frames and composites them, within one frame's bytes and
      8 MiB more (`kept_layers_stay_within_their_cap_and_can_be_dropped`).
- [x] Scrolling moves a layer and paints only the exposed strip
      (`a_scroll_paints_only_the_rows_that_come_into_view`).
- [x] Opacity animations change layer properties, not pixels: a group marked as fading or moving is kept as the image
      of its subtree within the same cap and laid over from it at each opacity while its subtree is the same and lies
      where it was painted (`a_fading_group_is_laid_over_from_its_kept_image_and_a_moved_one_painted_again`).
- [ ] Transform animations change layer properties, not pixels. On the CPU path a kept image laid at another place or
      scale does not have the bytes of its subtree painted there (dithering follows the pixel, paths are rasterized at
      their place), so a group whose transform changed is painted again and the pixels stay those of painting in
      place. Laying the kept image over at its transform goes with the GPU renderer's layers (step 6); on the CPU it
      needs the owner to accept a moving popover's pixels differing from painting in place.
- [x] A window told it is suspended drops its renderers' layers and keeps its frame
      (`a_window_told_it_is_suspended_lets_its_layers_go`).
- [x] The server tells minimized and fully covered windows they are suspended (`xdg_toplevel` state `suspended`), so
      they drop their layers: minimized, of a hidden application or of a workspace not shown, or every point of it
      under the opaque region of windows at rest above it, corners cut; the state goes as soon as it shows again
      (`a_minimized_window_is_told_it_is_suspended_and_no_longer_once_restored`,
      `a_window_covered_by_an_opaque_one_is_suspended_until_it_shows_again`,
      `a_window_under_covers_that_leave_no_point_of_it_does_not_show`, `a_rounded_window_covers_all_but_its_corners`).
- [ ] Kit windows declare their opaque region (the window less its material and glass regions and its corners), so a
      Kit window covering another suspends it: today only clients that declare one cover.
- [x] Test: a scissored repaint equals a full repaint, with effects crossing the damage edge, at scales 1, 1.5 and 2
      (`a_frame_drawn_over_the_last_has_the_bytes_of_one_drawn_whole` at 1, 1.25, 1.5 and 2, and, through kept
      layers, `a_scrolled_layer_has_the_bytes_of_one_drawn_whole_at_its_offset` at 1, 1.5 and 2).
- [x] Incremental layout for the Files listing: the icon layout is kept between views while the width and the number
      of items stay, and the grid takes it without comparing its cells.
- [x] Files view under 4 ms in the worst frame of a full-screen scroll: rows are made only as they come into view and
      the rest kept (`Element::kept`; the grid's cells keep their nodes while the window only scrolls, and an item's
      icon is made only for a cell that is built). View p99 2.4–2.8 ms at scale 1 and 2.3–2.5 ms at 1.5, from 6.3 and
      2.9–4.5 (`a_kept_element_of_the_same_mark_keeps_its_node_unbuilt_and_a_new_mark_builds_it_again`,
      `a_cell_is_kept_while_the_grid_only_scrolls_and_built_again_when_its_state_or_the_revision_changes`,
      `a_grid_of_thousands_of_items_makes_icons_only_for_the_cells_it_builds`).
- [x] Maps keyed by node and paint node ids hash an id with a rotate and one multiply, not the keyed hash of the
      standard maps: the layout cache and result, the window's placements, the painter's and the damage tracker's
      maps (`ids_handed_out_in_order_hash_apart_and_an_id_map_finds_each`). Every phase that looks nodes up gets
      cheaper with it ("Measurements", "Step 4", "Third round").
- [ ] Files layout under 4 ms in the worst frame of a full-screen scroll. The layout pass walks the whole tree every
      frame however little changed; timed inside the client at scale 1 (about 3700 nodes, load about 4), the cached
      solve and the placement of every node take 0.8 ms p50, the window's placements 0.5 and its semantics 0.6, the
      worst frames those that lay out a new row's names. Split in two:
  - [ ] Placements and semantics kept for a subtree that did not change: a kept node of the same subtree revision
        moves its subtree's placements by its own offset and keeps its semantics nodes, as paint does.
  - [ ] The names of a row measured before it comes into view, so the frame that brings a row in does not shape its
        text.
- [x] The toolkit's paint phase made incremental: a kept node whose subtree did not change (its subtree revision, the
      ground it is painted on and its nodes in paint order are the same) has its paint nodes laid in as they were,
      without walking its subtree, and the list is the one a full walk paints
      (`a_kept_subtree_that_did_not_change_is_laid_in_as_it_was_unwalked_and_the_list_is_the_one_painted_whole`:
      scrolls at whole and fractional offsets walk 2 of 42 nodes, a changed row walks its own, an inactive window all;
      list, own pixels and blank equal a new painter's). A Files scroll frame at scale 1 walks about 60 of 3700 nodes;
      the walk is 0.95 ms p50 from 1.20 at load about 4 ("Measurements", "Step 4", "Third round").
- [ ] The toolkit's damage phase made incremental: comparing the paint tree with the last frame's is 0.7 ms p50 at
      scale 1 (load about 4). It needs the comparison skipped for a kept run laid in unchanged, whose nodes moved only
      by the scroll's offset inside a layer.
- [x] A kept opaque scroll layer laid over by copy rather than by blending, with the background behind it baked in: the
      layer is laid over the background colour pending under it in one pass, its empty runs writing the colour and
      its opaque runs copied (`a_layer_laid_over_its_background_in_one_pass_has_the_bytes_of_the_background_written_and_laid_over`,
      `a_layer_scrolled_over_its_background_has_the_bytes_of_the_general_path`). Raster p50 5.9–7.2 ms at scale 1, from
      7.5.
- [x] The copy into the shared-memory buffer converts each pixel once: a frame's damage and what the buffer missed are
      cut into rectangles that do not overlap, where a scroll converted the moved viewport once for each
      (`overlapping_rectangles_become_ones_that_hold_each_pixel_of_their_union_once`). Copy p50 3.2–4.6 ms at scale 1,
      from 8.9.
- [ ] The frame reaches the compositor without the copy into the shared-memory buffer: the copy's p99 stays at
      20.3–20.6 ms, the frames that convert a whole buffer (a buffer new to the window, or one whose pixels are
      unknown). Rendering straight into the buffer needs the buffer in the renderer's byte order (`abgr8888`, which
      the window system offers when the compositor's renderer takes it; the toolkit asks for `argb8888` and swaps the
      channels as it copies), the renderer drawing into a buffer it borrows rather than its own pixmap, and what the
      buffer missed repainted by its age, not copied from the last frame; or the GPU path of step 6. On the GPU path
      (`CUPERTINO_RENDERER=gpu`) the frame is copied from the renderer's texture into the dmabuf on the GPU: 0.07–0.12
      ms p50 and 0.6–0.75 ms p99 at 1.5 ("Measurements", "Step 6: Files on the GPU path"); the CPU path keeps its
      copy until step 7 turns the GPU on.
- [ ] The CPU renderer's kept image of content with a blur or shadow is its picture in place: drawn whole at offset
      0, scene 910 of `cupertino-ui-gpu`'s `parity::scroll_layers_drawn_whole_match_the_cpu` parts from the same list
      drawn without the layer by up to 6/255 on up to 30 pixels at the viewport's first column and rows, where the GPU
      renderer's kept texture and both renderers' pictures in place agree within 1/255. That test runs scenes 900 to
      909 until this is found. Narrowed: the scene's parting comes from two blur groups, one inside the other (radii
      5.1 and 5.6), over a gradient fill that lies just outside the viewport's start edge (window x 0.1 to 3.1, the
      viewport starting at 4). Moving the fill inside the viewport or dropping either group removes it; one blur
      group alone across the edge does not part. Still to find: how the CPU layer's image bounds the inner group's
      blur source where it crosses the layer's edge, against the outer group's.

Gate:

- [ ] Files two-finger scroll meets its budget at scale 1 and 1.5 on the CPU path, and paint and raster of a scroll frame
      cover only the exposed strip. Not met on either path. CPU: client frame p99 34.9–37.5 ms at scale 1 and
      36.6–41.0 ms at 1.5 before the third round, 39.3–67.5 and 41.3–64.8 after it under a heavier load, against 8 ms
      (79.3 and 126.4 ms at step 0); painting covers the exposed strip, compositing and copying the whole viewport. GPU
      (`CUPERTINO_RENDERER=gpu`): p99 8.4–10.6 ms at scale 1 and 21.6–35.3 at 1.5 before, 11.4–17.8 and 10.8–14.8 after,
      with frame p50 4.0–4.1 ms at 1.5 from 6.6 ("Measurements", "Step 4", "Third round"). The unticked items above are
      what remains of it; on the CPU path the copy's p99 alone is over the budget.

## Step 5: `cupertino-gpu` and the shader toolchain

- [x] API: devices, buffers, textures with external memory, immutable pipelines, explicit passes, bind groups,
      fences; borrowed-context mode with a state-restore contract.
- [x] GLES backend over EGL, with EGL and GBM loaded at run time.
- [x] Allocators: GBM buffer objects; texture export (`EGL_MESA_image_dma_buf_export`) where there is no GBM.
- [x] Robustness and purge attributes on every context; reset and purge detection.
- [x] Shader build step: shared module concatenated, `naga` generating GLSL ES 3.00 and SPIR-V, coordinate adjustment
      off, blocks and samplers bound by name, uniform blocks only, no `mat2x2` in blocks.
- [x] Generated shaders committed; a test regenerates them and fails on a difference.
- [x] Shaders tested against the shared definitions' vectors.
- [x] Atlases (8-bit coverage, RGBA colour glyphs, path masks) with eviction and caps; pooled render targets with caps.
- [x] Tests per backend: pipelines, orientation, atlas eviction, pool reuse, export and import, fences, context loss.

Gate:

- [x] CI: the crate's tests pass on llvmpipe. In the CI image (the goldens image, no device passed in) and on the
      host's llvmpipe ("Measurements", "Step 5"); the first CI run on `main` after the merge confirms it there.
- [x] Local tier: the crate's tests pass on the NVIDIA GPU.

## Step 6: the GPU renderer and presenter

`GpuRenderer` (`crates/cupertino-ui-gpu`) implements `UiRenderer` over layers. Each primitive lands with its golden
subset passing on both renderers and its generated scenes (`tests/parity.rs`, one test per group) matching the CPU
renderer at scales 1, 1.5 and 2 on odd sizes:

- [x] Rectangles and rounded rectangles (fill and stroke).
- [x] Gradients.
- [x] Glyph runs.
- [x] Images and icons.
- [x] Path masks.
- [x] Clips: analytic, nested by multiplied coverage, path clips through the mask atlas, stencil only for
      pixel-aligned rectangles. A rectangle on whole pixels is the scissor (the stencil is not needed), up to four
      rectangles or rounded rectangles are multiplied in the program, the rest go into one clip mask.
- [x] Layers with opacity: groups and operation layers as targets from the pool, ready to carry step 4's layer
      boundaries.
- [x] Blur, drop shadow, inner shadow.

Presenter and process:

- [x] dmabuf presenter in `cupertino-ui-wayland` with the step 3 synchronisation, size-stepped allocation for live
      resize, two buffers with release points and three without.
- [x] Fallback rule in full: `CUPERTINO_RENDERER` (`gpu` overriding software and missing feedback), missing libraries,
      three losses, purges not counted, per-surface fallback to shared memory on import or allocation failure.
- [x] "Buffers lost" after a reset or purge: protocol version bump, contract entry in
      `docs/architecture/contracts.yaml`, compositor event, client repaint, tests.
- [x] First frame on the CPU while the context and programs warm on a worker thread; after the switch the CPU
      renderer's frame, layers and caches are freed.
- [x] `cupertino-diagnose` and the frame statistics report the renderer per process.
- [x] Local GPU tier, presenter: dmabuf import, syncobj, injected loss and purge, CPU↔GPU switch, failed-import
      fallback, resize storm, fence soak on the NVIDIA GPU.
- [x] Local GPU tier: goldens on the NVIDIA GPU (with `GpuRenderer`): `cupertino-ui-gpu`'s suite, slow tier.
- [x] Renderer selection wired with every process on the CPU by default: the single switch `GPU_BY_DEFAULT`
      (`crates/cupertino-ui-wayland/src/renderer.rs`), which step 7 turns on.
- [x] Parity at scales 1, 1.5 and 2 with odd sizes, whole frames and frames repainting damage.
- [x] The presenter draws through `GpuRenderer` when the process is on the GPU, so `CUPERTINO_RENDERER=gpu` gives a
      GPU-drawn window: the kit's renderer factory makes a `GpuRenderer` on the presenter's device (`GpuDevice` holds
      it in an `Rc`), its frame carries the window texture to `GpuSurface::present`, which draws it into the buffer
      instead of uploading pixels (the blit reading texels by position, the buffer being allocated in size steps),
      and a surface that falls back to shared memory reads the texture back; a headless test in `gpu_presenter`
      (`the_presenter_shows_what_the_gpu_renderer_drew_from_its_texture_in_dmabufs`).
- [x] A whole frame's recording on the CPU under 4 ms at 1.5 (13 ms for the timed window, "Measurements",
      "Step 6: the renderer"). The time was not the draws: 117 of the frame's 125 offscreen targets were made and
      destroyed every frame, the pool's cap (eight 2 MiB granules) holding fewer than the frame had alive. A target
      the frame has finished with now gives its texture to the next of its size and format: 2.5 ms, no target made
      (`timing::a_large_windows_damaged_frame_stays_within_the_gpu_budget`, which holds both budgets). Batching
      draws is not needed for the budget.
- [x] `GpuRenderer` keeps scroll layers (`Layer::Scroll`) as textures between frames and moves them, painting only the
      rows that come into view, as the CPU renderer keeps its images (the cap, a layer new to a frame laid over only
      where damaged). Without it a scroll frame's damage, which leaves out what only moved with the layer, made the
      GPU renderer paint the whole viewport in place (raster 5.9 ms p50 at 1.5). Each layer has two textures the size
      of its viewport: a scroll by whole device pixels draws what stays in view into the other, moved, and paints the
      rows or columns that come into view and the damage; the content is painted under none of the layer's clips and
      dithered by its own pixel, so it keeps its bytes as it moves, and the window lays the texture over where the
      clips let pixels through whole and paints the content in place along their edges. Tests:
      `parity::scroll_layers_scrolled_frame_by_frame_are_the_frames_drawn_whole` (scrolls down, up, sideways, past the
      viewport and by half a pixel, with damage), `parity::scroll_layers_drawn_whole_match_the_cpu` and
      `parity::a_kept_scroll_layer_paints_only_the_rows_that_come_into_view`. Raster 0.7–0.8 ms p50 at 1.5.
- [x] The raised layer's subsurface presented from GPU buffers too: its frame was read back into shared memory, a
      wait for the GPU of 4–6 ms in every third Files scroll frame; it now has dmabufs of its own and is copied from
      the renderer's texture as its window is.

Gate:

- [x] Every golden passes on both renderers, the local tier passes on the target machine, client GPU time and the
      first-frame budget are met. At `c30025ef`: the fast tier (7393 tests) and the slow tier (226) pass with and
      without `CUPERTINO_GPU_REQUIRED=1`; the GPU timing test's whole-frame budget failed once at 5.7 ms while other
      builds ran (the CPU renderer's frame beside it took 38 ms against its usual 11) and passes at 2.5 ms on its own.
      The GPU's share of a frame is under 0.1 ms; the first frame goes out on the CPU in 30 ms while the device warms.

## Step 7: turn it on

- [ ] Files windows on the GPU; live resize meets its budget.
- [ ] Settings on the GPU.
- [ ] Third-party toolkit applications and the C ABI by the size and repaint rule; sandboxed apps without render-node
      access take the fallback.
- [ ] Dock (and overview), launcher and Control Center measured on both renderers with layers; each assigned to the
      class that meets its budgets; numbers recorded.
- [ ] CPU classes confirmed in a full session: OSD, banners, menu bar, prompts, portal overlays and chooser, screenshot
      toolbar and overlay, lock screen, login, the Files desktop instance.
- [ ] A week of daily use on the target machine without a fallback.

Gate:

- [ ] The resize, animation-frame, memory and idle budgets hold across a full session.

## Step 8: the compositor's own drawing on the GPU pipelines

The way in is step 10's: a device borrowed from the compositor's context draws into the framebuffer Smithay has bound
(`Device::begin_owner_pass`, `render::passes`). Two facts shape the split. The layer rules let the compositor (core)
depend on the graphics layer but not on the toolkit, so it reaches the toolkit renderer's pipelines where they live,
in `cupertino-gpu` (the `ui_fill` and `ui_texture` programs, the coverage and path mask atlases), not through
`cupertino-ui-gpu`, which records paint trees. And `ui_fill` draws circular corners only, while title bars and
buttons have continuous corners, per-corner radii (the bar's top corners alone), linear gradients (button faces and
rings) and strokes (the rim, the rings). Split (open at the end of step 10's third session):

- [ ] Title bars and window buttons through the renderer's rectangle and glyph pipelines in borrowed-context mode.
      Split:
  - [ ] `cupertino-decorations` records a title bar, its buttons and the rim as a list of draws (an outline with
        per-corner extents and an exponent, a solid or linear shade, a stroke width; a coverage image of a glyph run
        or a button glyph) and its CPU path paints that list, the same pixels as today (its tests and the server
        goldens unchanged).
  - [ ] The `ui_fill` block carries the corner exponent in a lane unused today (zero, circular, for every draw the
        toolkit records), so a continuous corner is the shared distance on both renderers; per-corner extents by a
        shape taller than the strip, held to it by the scissor.
  - [ ] The block layout of `ui_fill` and `ui_texture` as one Rust type in `cupertino-gpu`, which
        `cupertino-ui-gpu` and the compositor both fill, so neither keeps a copy.
  - [ ] The compositor draws a title bar's list into a texture of its own through those pipelines on the borrowed
        device (title glyphs and button glyphs rasterized once into the coverage and path mask atlases, kept across
        frames), when its state changes, and lays the texture over the window as the uploaded buffer was; the frame
        statistics count GPU draws apart from CPU rasters.
- [ ] Keyboard HUDs the same way (the input-source HUD, the accent popup, the shortcut notice: rounded fills and
      centred text).
- [ ] The procedural cursor last, kept a cursor-plane element: on the GPU into the plane's buffer where the plane
      takes a GPU buffer, else the CPU image kept for the plane, the choice recorded here.
- [ ] The rectangle passes of a frame (shadows, outlines, rounded rectangles, title bars) drawn inside one enter and
      leave of the borrowed device instead of one each, so the state is read and put back once a frame
      ("Measurements", step 10: 0.2 to 0.3 ms of compose work at p50 today).
- [ ] `scripts/dev/bench`'s hover finds the window buttons in every run: in two of three runs, with either build,
      it reports that the hover drew no title bar (the window's place differs between runs), so its median counts
      none. The baseline from the run that hit them, before step 8: 36 decoration rasters and 36 uploads, 12 MiB.

Gate:

- [ ] The hover scenario shows no CPU raster and no upload.

## Step 9: layers and animation in the compositor

- [x] Protocol XML (new, versioned): fixed-point geometry atomic with the parent commit, glass attached to layers,
      blend roles, timed and spring animations on the compositor clock, retargeting with value and velocity,
      `finished` and `replaced`, input and semantics against target values, clipping, per-client limits, the
      late-content rule.
- [x] Server global, with its entry in `docs/architecture/contracts.yaml`.
- [x] Protocol tests (atomicity, retargeting, limits, late content, time scale and fixed time).
- [x] Compositor animator.
- Toolkit support; shell and control transitions use it. Split:
  - [x] A layer shell surface's own presentation (version 2, `get_presentation`): its offset, opacity and scale apply
        to all it draws, backdrops, popups and layers included, on the server's clock, with its entry in
        `docs/architecture/contracts.yaml`
        (`a_presentation_fades_moves_and_scales_all_its_surface_draws_on_the_server_clock`,
        `a_presentation_is_a_layer_shell_surface_s_own_and_has_four_properties`).
  - [x] The client side of the protocol on the transport (`cupertino-protocol-app`: `Layers`, `Layer`, the events as
        `AppEvent`s) and the window system's presentations: `WindowRequest::Present` with
        `Capabilities::present_animation`, committed without a buffer, or with the first frame of a window the update
        opens (`a_request_for_a_window_the_view_opens_goes_before_its_first_frame`); the animations followed through
        `finished`, `replaced` and `settled`. Without the global, or on another compositor, the capability is off and
        the application draws its transitions as before. The server retargets from the value and velocity shown,
        applies the time scale and ends spatial motion under Reduce Motion; input and semantics stay with the layout,
        the target.
  - [x] Control Center's open and close run as the card's presentation on the curves it draws itself: the
        critically damped rise as a spring of the same rate, exact up to the 0.23 s where the drawn curve jumps its
        last hundredth, and the 0.18 s fall as a cubic Bezier within a two-thousandth of it
        (`the_compositor_runs_the_same_fades`,
        `the_compositor_runs_the_card_s_fades_and_the_card_asks_for_no_frame`); no fade is handed over under Reduce
        Motion, where the card appears and goes at once.
  - Paint-tree subtrees as layers of their own surface: a subtree marked `Layer::Opacity` or `Layer::Transform`
    (or a mark of its own for the launcher's parts) drawn into a child surface the server places and animates, in both
    presenters, its animations sent to the compositor instead of repainted, hit-testing and semantics on the target
    values, and the client-side drawing kept where the server lacks the protocol. The presenters draw the raised layer
    into a subsurface of its own already (`present_layer`); this generalises it to a list of layer surfaces, each
    `cupertino_layer_v1` with its own frame. Too large for one reviewable change; split:
    - [ ] The mark and the runtime: a widget marks its subtree a surface layer with a key, its look (glass style,
          glass group and spacing, corner radius, masks to bounds) and the changes of its properties (`x`, `y`,
          `width`, `height`, `opacity`, `scale`, `corner_radius`, `content_blur`, each set or animated along a
          `LayerCurve`, as `WindowRequest::Present` carries a window's). The painter leaves a marked subtree out of the
          window's list and paints it into a list of its own, at its layout size whatever its animated values; the
          runtime keeps a damage tracker and a renderer per layer key (as `LayerState` does for the raised layer) and
          hands the window system the layers' frames, bounds and changes with the window's frame
          (`WindowSystem::present_surface_layers`, `Capabilities::surface_layers`; unsupported by default, where the
          subtree is painted in place with its values applied by the toolkit, as today). Hit-testing and semantics keep
          the layout, the target values. Tests over the headless window system: a marked subtree's pixels are in its
          layer's frame and not the window's, a change of its animated values alone presents no frame, and without the
          capability the window's frame is the one drawn today. New modules beside `runtime/handover.rs`, to stay apart
          from the layout and damage work in `cupertino-ui-core`.
    - [ ] The shared-memory presenter: each layer a `wl_surface` made a `cupertino_layer_v1` of the window's surface,
          its frame copied into buffers of its own as the raised layer's is, its changes sent as the layer's requests
          before the window's commit (so they apply with it), its events (`finished`, `replaced`, `settled`) back to the
          runtime as a presentation's are; a layer the server lacks the version for is painted in place.
    - [ ] The GPU presenter: the layers' frames drawn from the GPU renderer's textures into dmabufs of their own, as
          the raised layer's subsurface is, and the GPU renderer's frame split by layer as the CPU renderer's is; the
          local GPU tier covers it.
  - The launcher's open and close as compositor layers, keeping its look (the owner's choice: the look as it is drawn
    now, run by the compositor). Split:
    - [x] Glass groups (version 3, `set_glass_group`): the layers of one group draw their glass as one container of
          their shapes as shown each frame, blended over the group's spacing, so a button grows out of the field's
          glass as the layers move (`a_glass_group_joins_the_glass_of_its_layers_into_one_shape_as_they_move`).
    - [x] Content blur (version 4, `content_blur`): an animatable property that blurs a layer's own content (not its
          glass nor what lies behind it) by a Gaussian in the layer's points, 0 to 64, ended at its target under Reduce
          Motion. The compositor draws each surface of the layer into a texture with room for three deviations around
          it, blurs it by the backdrop passes' pyramid and separable Gaussian and draws it in the content's place through
          the layer's mask, scale and opacity (`render/content_blur.rs`), keeping a blur until the surface, the deviation
          or the scale changes. The backdrop passes wrote an opaque result; under a new pass flag (`PASS_KEEP_ALPHA`)
          the downsample and the blur keep the premultiplied alpha of what they blur, the colour within it, and a
          backdrop's passes are as they were. A vibrant layer draws its content sharp. Across a straight edge the blurred
          content is within 3/255 of the Gaussian's integral
          (`a_layer_s_content_blurs_past_its_edges_by_the_gaussian_on_the_server_clock_and_the_parent_stays_sharp`,
          `content_blur_is_a_property_from_version_4_from_sharp_to_its_widest_and_reduced_motion_ends_it`); at scale
          1.5 a deviation of 4 points is 6 pixels, and a layer scaled to half blurs half as wide about the edge its
          content is drawn at (`a_layer_s_content_blur_is_in_points_and_scales_with_the_layer_at_a_fractional_scale`);
          the client side is `LayerProperty::ContentBlur`.
    - The launcher as layers: the enclosure a layer whose width, height and corner animate with its glass, its
      content a sibling that does not scale, the search field's layer animating its width, each mode button a layer on
      its own spring in a glass group with the field, and the close animating the content blur; its look at every
      moment that of the launcher's drawing within the goldens' tolerance, and no application buffer for the open or the
      close (counted as Control Center's is, a test like
      `shell_processes::the_control_panel_fades_on_the_compositor_s_clock_without_a_new_buffer`). It needs the subtree
      layers above. Not every value the launcher draws is one curve of one property: where the search field opens with
      its mode buttons, the enclosure's drawn width is the opening spring's scale (response 0.30, damping 0.74, from
      1.1) times the field's width drawing back on a spring of its own (0.305, 0.68), a button's centre is that scale
      times its place along the split spring (0.575, 0.69), and a button's size is a clamped ramp of the split spring
      (`modes::button_rect`, `bud_size`); its corner radius is the radius times the smaller of the two axes' scales. A
      timed curve or a spring per property cannot carry a product or a clamp of two curves, so these need the
      protocol to carry them first. Split:
      - [ ] Curves the protocol carries as samples: an animation given as values at even steps of its duration (the
            client sampling its own motion at, say, 240 a second), which the server interpolates, retargeting from the
            value and the slope shown as other animations do; `LayerCurve` gains it, the server's animator and the
            transport with it. Then every value the launcher draws is carried exactly, on the compositor's clock.
      - [ ] The close: the enclosure's opacity on the ease-out over 0.1 s, its width and height growing to 1.02 of
            the shape on the same curve (a timed curve per property, exact), and the content's blur to 4 points on it
            (`CLOSE_BLUR` is a blur radius of 8 points, a deviation of 4); no buffer for it.
      - [ ] A scope's open and folding (the browser, the files scope): the enclosure's width, height, corner and
            place on the opening and shape springs, which are linear in the springs' values (exact); the content layer
            unscaled, its fade on the content's curve; the files scope's width dip as samples.
      - [ ] The search field's open with its mode buttons: the field's layer and each button's on sampled curves of
            the products above, in one glass group with the field, each button's symbol sharpening as its layer's
            content blur goes from its starting blur to 0 on `GLYPH_SHARPEN` after `GLYPH_DELAY`.
      - [ ] The test that the open and the close make no new application buffer, and the look at several points of
            each within the goldens' tolerance of the launcher's drawing.
  - [x] Control transitions in popups: version 3 takes a popup's surface in `get_presentation`, its scale about the
        centre of its window geometry, after its window's rescale or its layer surface's presentation
        (`a_popup_s_presentation_scales_and_fades_it_on_the_server_clock`). The runtime hands a popover's growth out of
        its anchor and its shrinking back and a closing menu's fade to it under `Capabilities::present_popups`, on the
        curves it draws: the opening spring, `(1 - t)²` and `(1 - t)³` as cubic Béziers, the anchor kept by an offset on
        the same curve (`the_compositor_grows_a_popover_on_the_spring_it_draws_and_out_of_the_same_point`,
        `the_compositor_shrinks_a_popover_and_fades_a_menu_on_the_curves_they_draw`); the popup presents no frame for
        them (`a_popover_grows_and_shrinks_and_a_menu_fades_on_the_compositor_s_clock_without_a_frame`). Under Reduce
        Motion nothing is handed over, as nothing animates.
  - [x] Desktop control reads whether every presentation and layer is settled: an observation waits until they show
        their targets, a second of model time at most, so what it reads is what is shown
        (`an_observation_waits_for_what_is_shown_to_settle_and_no_longer_than_its_wait`,
        `desktop_control_reads_the_clients_layouts_once_their_layers_show_their_targets`).
- [x] Vibrant foreground as a blend-role layer.
- [x] Lifted thumbs and lenses as glass in a raised layer.
- [x] Pointer-lit highlight and variable blur.

Gate:

- [ ] Opening Control Center and the launcher costs the application no repaint per frame, and each material effect is
      reviewed by eye in both appearances. Control Center holds: its card draws no frame of its fade in or out (a single
      frame after its first as it opens)
      (`shell_processes::the_control_panel_fades_on_the_compositor_s_clock_without_a_new_buffer`, "Measurements",
      "Step 9"). Popovers and menus hold: a popover's growth and shrinking and a menu's fade present no frame. The
      launcher waits on its split items above. The review by eye is pending the owner's review of the sheet.
- [ ] The before and after sheet of Control Center's fades and the launcher's open and close, mid-way and settled,
      light and dark, from a headless server, for that review: "before" drawn by the application, "after" by the
      compositor. It goes with the launcher's layers, whose "after" it shows; Control Center's "before" is the card's
      own fade from before its presentation. Control Center's half is made ("Measurements", "Step 9", "The sheet");
      the launcher's pairs are in it with both sides drawn by the application until its layers land.

## Step 10: retire the duplicates

- [x] Shadow, rounded-rectangle and wallpaper passes move to borrowed-context passes generated from WGSL. Split:
  - [x] A borrowed device's pass can draw into the framebuffer its owner has bound, with its viewport and scissor
        (the element's damage), under the state-restore contract: `Device::begin_owner_pass` keeps the owner's
        framebuffer and viewport, sets a pass's fixed state, and `leave` puts everything back
        (`gles::tests::a_borrowed_device_s_pass_draws_into_the_framebuffer_its_owner_bound_at_the_owner_s_viewport`).
  - [x] The shadow pass, dithered as before: a flag of its placement block instead of a second program text.
  - [x] The rounded-rectangle pass.
  - [x] The wallpaper pass. The three are a third source set (`crates/cupertino-server/shaders/passes/`, one
        placement block and vertex stage included by each, one quad per damage rectangle through the frame's
        projection), drawn by `render::passes::Passes` on a device borrowed from the renderer's context; the
        hand-written GLSL ES 1.00 copies are gone, and the tests that held them to the shared definitions
        (`dither::`, `coverage_gl::`) hold the generated passes now.
- [x] The hand-written GLSL ES 3.00 programs of the glass crate and the compositor replaced by generated ones; the
      tests that pinned the copies pin the generated output. Split:
  - [x] The build step generates a second set, `cupertino-glass`'s programs (the shared module ahead of each, include
        files, and the assembled WGSL for the wgpu renderer); the backdrop passes (downsample, blur, colour, blit) are
        generated, the compositor and the wgpu renderer draw them, their hand-written copies in both crates are gone,
        and the blur's dither for 8-bit levels is a flag of its block instead of a second program.
  - [x] The material region program (the glass crate's and the compositor's, which differ in how the region is placed
        and sampled): one program, placed by a block of its own (the frame's projection, the quad and the dither
        flag), the compositor's placement for both.
  - [x] The vibrant program.
  - [x] The glass program.
  - [x] The compositor's mesh program of the minimize and close effects (GLSL ES 1.00, compiled by the compositor
        itself, not through Smithay's interface).
- [x] `wgpu` removed from `cupertino-glass`; the headless glass renderer and its tests run on `cupertino-gpu`.

Gate:

- [x] One WGSL source for every program of ours; the texture programs compiled through Smithay's shader interface
      (clip, its vibrant variant, reduced copy, minification) stay GLSL ES 1.00 and are listed by name in the design.
      The one hand-written program text left is `crates/cupertino-server/shaders/compositor/clip.frag` with its
      distance (`sdf.glsl`), compiled as those four (`render::programs::tests`).

## Step 11: output timing and depth on the target display

- [x] Frame scheduling latches late only where presentation timestamps are real, from the recent maximum render time
      plus a margin, never past half a period (`crates/cupertino-server/src/latch.rs`; `CUPERTINO_LATE_LATCH=0` turns
      it off; "Measurements").
- [x] Pointer-to-screen latency reported by the DRM backend's log (`pointer-to-flip`; the bench reports the headless
      figure, `pointer_latency_p50_ms` and `_p95_ms`, `ptr95` in its table).
- [x] 10-bit primary plane (`Abgr2101010`) behind `output_depth` in `compositor.json`, 8 bits by default, falling back
      to 8 bits on a plane without a 10-bit format; capture paths never read the plane and stay 8-bit
      (`output_depth::tests`).
- [ ] 10 bits by default if the step 0 probe showed a visible gain. OWNER DECISION: the probe's result decides
      ("Measurements", "Turning 10 bits on").

Gate:

- [ ] In the live session over an hour: input-to-flip latency below the baseline and no more missed flips than the
      baseline, from the DRM backend's log; the 10-bit decision recorded. BLOCKED (owner action): it needs the
      desktop session; the one command is under "Measurements", "The step 11 gate".

## Step 12: Vulkan backend for the toolkit (gated)

- [ ] Entry: a named budget the GLES backend cannot meet, or a driver problem Vulkan avoids, recorded here.
- [ ] The `wgpu` decision re-checked against the cost of a second hand-kept backend.
- [ ] Vulkan backend: DRM format modifiers, dmabuf export, syncobj semaphores, SPIR-V from the generated shaders.

Gate:

- [ ] Same goldens and budgets as GLES, and the entry budget met.

## Step 13: Vulkan renderer for the compositor (gated)

- [ ] Entry: as step 12, for the compositor.
- [ ] Smithay's renderer traits implemented for Vulkan; `DrmCompositor` and damage tracking unchanged.
- [ ] Material, glass and mesh passes on `cupertino-gpu`.

Gate:

- [ ] Goldens, GPU reset tests and the daily-driver checks pass on Vulkan, and the entry budget met.

## Documentation

- [x] Design page in the book, linked from `SUMMARY.md`.
- [x] Rules in `docs/architecture/rendering.md`.
- [x] Rendering paragraph of `docs/src/architecture.md`, the roadmap section and `docs/src/displays-and-gpus.md` point
      here.
- [ ] Each step that changes behaviour updates the pages it affects (`apps-on-kit.md`, `kit-getting-started.md`,
      `displays-and-gpus.md`, crate READMEs) in the same change.

## Measurements

Filled in as steps complete.

### Step 0: instruments, probes and the idle desktop

Taken at `624dab3b` on the target machine's GPU (GeForce RTX 3060, proprietary driver 615.78.08), with the
desktop session running beside the runs.

**Where the instruments are.**

- `scripts/dev/bench` (headless sessions, every process measured; its tests: `python3 -I scripts/dev/test_bench.py`).
- `CUPERTINO_FRAME_TIMING=PATH`: every toolkit process appends one JSON line a presented frame with its phases
  (update, view, layout, paint, raster, copy, commit, total). Without the variable no clock is read
  (`runtime::tests::a_presented_frame_is_timed_phase_by_phase_and_nothing_is_timed_without_a_sink`).
- The frame statistics (`--frame-stats`, headless) end with the GPU time of the frame (timer queries), title bar and
  HUD rasters and uploads with their bytes, and blur passes; a `# start_monotonic_ns` line lines other processes'
  times up with them.
- `CUPERTINO_DRM_TRACE=1` on the session server logs, under the `cupertino_server::drm_trace` target: the
  presentation-time branch when it changes (`flip-timestamp` or `own-clock`), every missed flip with how many refresh
  periods late, the plane assignment when it changes (direct scanout, cursor plane, overlays) and the input-to-flip
  latency of each output (keys and pointer alike, an input whose flip was dropped reported at the output's next flip).
  Without the variable nothing is kept.
- `scripts/dev/measure` reports private dirty memory beside Pss and Rss.
- The probes live in `tools/gpu-probes`, a crate with a workspace of its own: never built by the workspace, never in
  the packages. Build: `cargo build --release --offline --manifest-path tools/gpu-probes/Cargo.toml --target-dir
  target/gpu-probes`; `target/gpu-probes/release/gpu-probes --help` lists the probes. Each probe runs its
  measurements in fresh processes, with the driver's on-disk program cache off unless the probe says otherwise.

**The baseline in one command.** The baseline runs (every scenario five times at scale 1.5 and at scale 2, on the
target display's size) are:

```sh
scripts/dev/bench --size 3840x2160 --scale 1.5 --runs 5 --idle-seconds 60 --out baseline-1.5.json
scripts/dev/bench --size 3840x2160 --scale 2 --runs 5 --idle-seconds 60 --out baseline-2.json
```

Each JSON holds every run and, per scenario, the median of the runs with their spread (max - min, the noise):
compositor frame interval p50/p99, missed refreshes, work and GPU time p50/p99, compositor CPU time, slow client frames
per program, the per-phase client split (p50 and p99 of each phase), and per-program CPU and private memory. The
scenarios: idle, drag (Settings over Files and its glass), hover (along the window buttons), resize (Files' corner),
control-center, launcher-typing and files-scroll.

**Smoke run** (one run each, 1280 x 800 at scale 1, 60 Hz; it only shows that each scenario drives what it should):

| Scenario | Frames | Missed | Work p50/p99 ms | GPU p99 ms | Server CPU ms/s | Blur passes/frame | Title bar rasters (upload) | Slow client frames | Client frame p99 ms |
|---|---|---|---|---|---|---|---|---|---|
| idle (15 s) | 2 | 0 | 0.75 / 1.35 | 0.61 | 1.3 | 0 | 0 | 0 | 0.38 (menu bar clock) |
| drag | 180 | 0 | 0.76 / 2.19 | 1.72 | 60 | 7.6 | 0 | 0 | 0.48 |
| hover | 288 | 0 | 0.52 / 0.83 | 0.30 | 42 | 0 | 36 (5.3 MiB) | 0 | 1.88 |
| resize | 181 | 0 | 2.91 / 4.20 | 2.91 | 213 | 9.2 | 319 (284 MiB) | 0 | 5.43 |
| control-center | 86 | 0 | 1.13 / 2.30 | 1.64 | 22 | 6.8 | 0 | 0 | 8.24 |
| launcher-typing | 240 | 0 | 0.91 / 2.26 | 1.11 | 45 | 3.5 | 0 | 0 | 2.99 |
| files-scroll | 182 | 0 | 0.88 / 1.32 | 0.67 | 69 | 7.0 | 0 | 0 | 3.27 |

The resize already shows what step 8 removes: a title bar raster and upload on every frame.

**The baseline** (`96034ac4`, release build; 3840 x 2160 at 60 Hz, five runs a scenario, `--idle-seconds 60`; no
build ran during the runs). Each figure is the median of the five runs, with the run-to-run spread (max - min, the
noise) in brackets where it is not zero. "Compositor work" is the CPU time of a compositor frame, "Client" the
program whose frames the scenario drives, "Slow client frames" those of every program longer than a refresh. No
scenario missed a refresh at either scale.

Scale 1.5:

| Scenario | Frames | Missed | Compositor work p50 / p99 ms | Compositor GPU p50 / p99 ms | Server CPU ms/s | Slow client frames | Client | Client frame p50 / p99 ms |
|---|---|---|---|---|---|---|---|---|
| idle | 1 | 0 | 1.28 (0.13) / 1.28 (0.13) | 0.74 (0.01) / 0.74 (0.01) | 0.0 | 0 | menu bar (1 frame) | 0.46 (0.10) / 0.46 (0.10) |
| drag | 180 | 0 | 1.32 (0.04) / 2.38 (0.28) | 1.01 / 2.07 (1.13) | 90.0 (3.3) | 0 | Settings (2 frames) | 0.59 (0.06) / 0.64 (0.06) |
| hover | 288 | 0 | 1.14 (0.05) / 1.54 (0.03) | 0.69 / 0.78 | 77.1 (4.2) | 0 | Settings (12 frames) | 0.49 (0.05) / 1.09 (0.16) |
| resize | 181 | 0 | 4.33 (0.15) / 7.41 (0.36) | 2.83 (0.20) / 4.35 (0.42) | 332.3 (12.9) | 2 (4) | Files (203 frames) | 9.63 (0.46) / 16.44 (0.83) |
| control-center | 85 (2) | 0 | 1.95 (0.02) / 2.60 (0.55) | 1.51 / 1.83 (0.21) | 31.7 (3.3) | 0 | Control Center (102 frames) | 12.84 (0.86) / 14.55 (0.54) |
| launcher-typing | 242 (1) | 0 | 1.69 (0.06) / 2.98 (0.25) | 1.17 (0.04) / 1.64 (0.02) | 72.3 (1.5) | 0 | launcher (372 frames) | 1.67 (0.13) / 6.44 (0.05) |
| files-scroll | 182 | 0 | 1.57 (0.01) / 2.47 (0.11) | 1.19 / 1.51 (0.01) | 105.5 (6.6) | 0 | Files (184 frames) | 5.44 (0.05) / 6.23 (0.40) |

| Scenario | Client | update | view | layout | paint | raster | copy | commit |
|---|---|---|---|---|---|---|---|---|
| idle | menu bar | 0.17 / 0.17 | 0.02 / 0.02 | 0.03 / 0.03 | 0.04 / 0.04 | 0.15 / 0.15 | 0.03 / 0.03 | 0.02 / 0.02 |
| drag | Settings | 0.01 / 0.02 | 0.00 / 0.00 | 0.09 / 0.09 | 0.18 / 0.19 | 0.26 / 0.26 | 0.02 / 0.07 | 0.02 / 0.02 |
| hover | Settings | 0.02 / 0.02 | 0.00 / 0.00 | 0.09 / 0.13 | 0.16 / 0.24 | 0.18 / 0.22 | 0.01 / 0.58 | 0.02 / 0.03 |
| resize | Files | 0.02 / 0.03 | 0.39 / 0.61 | 0.22 / 0.41 | 0.20 / 0.38 | 4.67 / 8.59 | 3.95 / 6.49 | 0.27 / 0.85 |
| control-center | Control Center | 0.01 / 0.02 | 0.03 / 0.11 | 0.02 / 0.11 | 0.07 / 0.35 | 12.09 / 13.05 | 0.61 / 1.75 | 0.01 / 0.05 |
| launcher-typing | launcher | 0.01 / 0.11 | 0.01 / 0.07 | 0.03 / 0.10 | 0.03 / 0.21 | 1.18 / 6.19 | 0.28 / 2.20 | 0.01 / 0.28 |
| files-scroll | Files | 0.00 / 0.01 | 0.00 / 0.38 | 0.08 / 0.19 | 0.20 / 0.30 | 4.66 / 5.02 | 0.47 / 0.61 | 0.01 / 0.02 |

| Program | idle | drag | hover | resize | control-center | launcher-typing | files-scroll | private MiB | private dirty MiB |
|---|---|---|---|---|---|---|---|---|---|
| Control Center | 0 | 0 | 0 | 0 | 20.7 (0.3) | 0 | 0 | 13.3 (0.4) | 4.3 |
| dock | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 52.0 (1.6) | 41.2 (1.6) |
| Files | 0 | 0 | 0 | 67.7 (1.6) | 0 | 0 | 37.6 (0.7) | 36.9 (3.6) | 33.8 (3.6) |
| launcher | 0 | 0 | 0 | 0 | 0 | 12.0 (0.5) | 0 | 15.3 (0.2) | 4.9 |
| menu bar | 0 | 0 | 0 | 0 | 0.2 (0.2) | 0 | 0 | 20.1 (0.1) | 5.9 |
| notifications | 0 | 0 | 0 | 0 | 0.0 (0.2) | 0 | 0 | 13.6 (0.2) | 4.2 |
| OSD | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 12.9 (0.3) | 4.3 (0.3) |
| server | 0 | 9.0 (0.3) | 7.7 (0.4) | 33.2 (1.3) | 3.2 (0.3) | 7.2 (0.2) | 10.5 (0.7) | 179.3 (5.7) | 118.1 (6.0) |
| Settings | 0 | 0.3 (0.7) | 0.4 (0.4) | 0 | 0 | 0 | 0 | 38.6 (5.6) | 22.4 (5.7) |
| window manager | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1.0 | 0.2 |
| bus daemon | 0 | 0 | 0 | 0 | 0.0 (0.2) | 0 | 0 | 0.8 | 0.6 |

Scale 2:

| Scenario | Frames | Missed | Compositor work p50 / p99 ms | Compositor GPU p50 / p99 ms | Server CPU ms/s | Slow client frames | Client | Client frame p50 / p99 ms |
|---|---|---|---|---|---|---|---|---|
| idle | 1 | 0 | 1.89 (0.19) / 1.89 (0.19) | 1.30 (0.07) / 1.30 (0.07) | 0.0 (0.2) | 0 | menu bar (1 frame) | 0.57 (0.06) / 0.57 (0.06) |
| drag | 180 | 0 | 1.60 (0.02) / 1.97 (0.40) | 1.37 (0.01) / 1.43 (0.63) | 103.3 (3.3) | 0 | Settings (2 frames) | 0.62 (0.10) / 0.77 (0.22) |
| hover | 288 | 0 | 1.35 (0.11) / 1.96 (0.11) | 1.03 / 1.14 (0.14) | 93.8 | 0 | Settings (12 frames) | 0.53 (0.03) / 1.44 (0.35) |
| resize | 181 | 0 | 5.90 (0.44) / 10.48 (0.41) | 4.02 (0.12) / 6.80 (0.26) | 483.9 (12.9) | 61 (3) | Files (175 frames) | 13.55 (0.33) / 22.14 (1.27) |
| control-center | 81 (5) | 0 | 2.74 (0.04) / 3.73 (1.41) | 2.18 / 2.63 (0.64) | 40.0 (3.3) | 64 (57) | Control Center (76 frames) | 17.34 (1.20) / 19.58 (1.28) |
| launcher-typing | 240 | 0 | 2.32 (0.06) / 5.15 (0.29) | 1.78 (0.04) / 2.83 (0.09) | 97.9 | 0 | launcher (362 frames) | 2.49 (0.13) / 10.94 (0.08) |
| files-scroll | 182 | 0 | 2.25 (0.04) / 3.30 (0.09) | 1.77 / 2.05 (0.37) | 141.8 (9.9) | 0 | Files (184 frames) | 5.78 (0.17) / 6.98 (0.68) |

| Scenario | Client | update | view | layout | paint | raster | copy | commit |
|---|---|---|---|---|---|---|---|---|
| idle | menu bar | 0.18 / 0.18 | 0.02 / 0.02 | 0.03 / 0.03 | 0.03 / 0.03 | 0.24 / 0.24 | 0.05 / 0.05 | 0.02 / 0.02 |
| drag | Settings | 0.01 / 0.01 | 0.00 / 0.00 | 0.06 / 0.08 | 0.13 / 0.17 | 0.31 / 0.35 | 0.03 / 0.14 | 0.01 / 0.02 |
| hover | Settings | 0.02 / 0.03 | 0.00 / 0.00 | 0.09 / 0.11 | 0.16 / 0.17 | 0.21 / 0.24 | 0.01 / 0.97 | 0.02 / 0.03 |
| resize | Files | 0.02 / 0.04 | 0.41 / 0.63 | 0.22 / 0.44 | 0.21 / 0.35 | 5.27 / 8.89 | 7.08 / 11.62 | 0.43 / 1.22 |
| control-center | Control Center | 0.02 / 0.03 | 0.05 / 0.11 | 0.03 / 0.11 | 0.10 / 0.41 | 16.05 / 17.03 | 1.08 / 2.89 | 0.02 / 0.05 |
| launcher-typing | launcher | 0.01 / 0.11 | 0.01 / 0.07 | 0.03 / 0.10 | 0.03 / 0.13 | 1.66 / 10.56 | 0.49 / 3.95 | 0.01 / 0.32 |
| files-scroll | Files | 0.00 / 0.01 | 0.00 / 0.38 | 0.09 / 0.18 | 0.24 / 0.35 | 4.56 / 5.08 | 0.86 / 0.97 | 0.01 / 0.03 |

| Program | idle | drag | hover | resize | control-center | launcher-typing | files-scroll | private MiB | private dirty MiB |
|---|---|---|---|---|---|---|---|---|---|
| Control Center | 0 | 0 | 0 | 0 | 21.2 (0.3) | 0 | 0 | 13.2 (0.2) | 4.3 |
| dock | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 54.4 (0.7) | 43.8 (0.7) |
| Files | 0 | 0 | 0 | 83.2 (1.3) | 0 | 0 | 39.5 (2.0) | 44.7 (2.2) | 41.7 (1.8) |
| launcher | 0 | 0 | 0 | 0 | 0 | 16.3 (0.3) | 0 | 15.4 (0.2) | 4.9 |
| menu bar | 0 | 0 | 0 | 0 | 0.2 (0.2) | 0 | 0 | 20.2 (0.3) | 6.1 |
| notifications | 0 | 0 | 0 | 0 | 0.0 (0.2) | 0 | 0 | 13.6 (0.1) | 4.2 |
| OSD | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 12.8 (0.1) | 4.3 |
| server | 0 | 10.3 (0.3) | 9.4 | 48.4 (1.3) | 4.0 (0.3) | 9.8 | 14.2 (1.0) | 191.8 (8.3) | 130.7 (8.1) |
| Settings | 0 | 0.3 (0.3) | 0.4 (0.2) | 0 | 0 | 0 | 0 | 40.8 (1.2) | 24.6 (1.1) |
| window manager | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 1.0 | 0.2 |
| bus daemon | 0 | 0 | 0 | 0 | 0.0 (0.2) | 0 | 0 | 0.8 | 0.6 |

The tables above are, in order: the frames; the per-phase client split (p50 / p99 ms) of the scenario's client; each
program's CPU (% of one core over the scenario) and its private memory at the end of the idle scenario.

What the baseline shows:

- **Control Center at scale 2** (slow frames 64, spread 57): its frames take 17.3 ms at p50, at the refresh's edge, so
  the count swings between runs. Nearly all of it is raster (16.1 / 17.0 ms; 12.1 / 13.1 at 1.5): the card and its
  soft shadow, redrawn whole on every frame of the opening. Its full-output click catcher is not repainted. No plain bug:
  this is the work layers (step 4), the shared shadow (step 1) and the compositor's layers (step 9) remove.
- **Resize at scale 2** (61 slow Files frames): raster 5.3 / 8.9 ms and copy 7.1 / 11.6 ms. The copy costs more than
  the raster because every frame is a buffer of a new size, copied whole. Steps 6 and 7 remove it (size-stepped GPU
  buffers, no copy).
- **The drag** let go 7.5 pt from where it began at 3840 x 2160: the server anchored a move a client asks for after
  its drag-start box at the pointer's position then, not at the press. A move or resize asked for past the drag
  start is now anchored at the press
  (`a_move_and_a_resize_asked_for_past_the_drag_start_keep_the_pressed_point_under_the_pointer`).
- **A scroll of a full-screen grid** damaged 641–673 separate rectangles a frame at scale 1, each walking the whole
  display list: 530–600 ms frames. More than 16 separate rectangles are now merged down to 16, the closest pairs first
  (`a_scrolled_grid_of_separate_items_is_repainted_in_a_few_rows_not_one_rectangle_around_them`); the worst frame went
  to about 80 ms.

**Files at 3840 x 2160** (`--scenario files-full-repaint,files-full-scroll,files-scroll`, five runs a scale;
`96034ac4`). Full screen: Files fills the output (`toggle-fullscreen`) on a folder of 3000 files; the repaint is
three appearance switches, each repainting the whole window; the scroll is a two-finger (touchpad) scroll over the grid, as the bench sends it (`--scroll`). Default size:
the baseline's `files-scroll` (a 400-file folder, the window at its default size). Client frame p50 / p99 ms, spread
in brackets:

| Scale | Full repaint, full screen | Raster / copy p50 | Two-finger scroll, full screen | Frames over a refresh | Two-finger scroll, default size |
|---|---|---|---|---|---|
| 1 | 82.5 (1.4) / 106.5 (1.4) | 52.0 / 13.6 | 56.7 (1.4) / 79.2 (5.1) | all (41 of 41) | 2.26 / 3.25 (0.32) |
| 1.5 | 81.0 (2.0) / 106.2 (3.3) | 56.6 / 16.0 | 59.1 (6.9) / 77.9 (8.9) | all (42 of 42) | 5.44 / 6.23 (0.40) |
| 2 | 57.4 (1.0) / 83.7 (2.1) | 36.0 / 18.5 | 34.0 (0.2) / 53.1 (0.6) | all (60 of 60) | 5.78 / 6.98 (0.68) |

The full-screen scroll is raster (45 / 50 / 26 ms at p50) and copy (6.4 ms at p50, 19–25 ms at p99). A frame's cost
follows the items in view, not the pixels: scale 2 shows a quarter of the items scale 1 does on the same pixels and is
the fastest. The full-screen frames take so long that Files draws one frame in three refreshes (41 frames in 3 s).

**Gate: update, view and layout in a Files scroll frame** (summed per frame, `logic_p50_ms` / `logic_p99_ms` of the
bench; `scroll-*.json`, five runs a scale, the runs the machine disturbed left out):

| Scale | Default size p50 / p99 ms | Full screen p50 / p99 ms |
|---|---|---|
| 1 | 0.09 / 0.48–0.96 | 1.0–1.2 / 5.6–6.1 |
| 1.5 | 0.09 / 0.78–1.21 | 0.6–0.7 / 5.5–5.8 |
| 2 | 0.09 / 0.57–0.72 | 0.33–0.36 / 0.43–2.53 |

At the default size the three phases take 0.1 ms at p50 and under 1.3 ms at p99 (the 0.6 ms read earlier holds). With
Files full screen on 3000 files the worst frame of the scroll spends 5.5–6.1 ms in them at scales 1 and 1.5 (view and
layout about 3 ms each): above 4 ms, so step 4 gains the incremental view and layout item.

**Budgets against the baseline** (the design's budgets table, each checked once):

| Budget | Baseline | Verdict |
|---|---|---|
| Any frame inside one refresh; no missed refreshes or slow client frames | no missed refresh in any scenario; slow client frames only in resize and Control Center at scale 2 and in full-screen Files | confirmed: the scenarios that miss are the ones the owning steps fix |
| Files two-finger scroll, p99 client CPU under 8 ms at scales 1 and 1.5 | default size 3.25 / 6.23 ms; full screen 79.2 / 77.9 ms | revised: the budget now names the window filling the output. At its default size Files meets it today, so only the full-screen scroll measures what step 4 must do |
| Files live resize: client CPU excluding raster under 8 ms, GPU under 4 ms | p99 at 1.5: raster 8.6 ms, copy 6.5 ms, the other phases 2.3 ms together | confirmed: the copy is what the GPU path removes; the rest is a quarter of the budget |
| Toolkit animation frame under 4 ms client CPU | Control Center 12.8 / 14.6 ms (raster 12.1 / 13.1); launcher p99 6.4 ms (raster 6.2) | confirmed: outside raster the phases' p99s add to 2.4 ms (Control Center) and 3.0 ms (launcher) at 1.5, most of it the copy the GPU path removes |
| Client GPU time per frame under 4 ms | no client draws on the GPU yet; the compositor's whole output takes 0.7–6.8 ms of GPU at p99 | confirmed, checked from step 6 |
| Compositor frame during a drag over glass: under 4 ms GPU, under 2 ms CPU | GPU p99 2.07 ms (1.5) / 1.43 ms (2); CPU p50 1.32 / 1.60 ms, p99 2.38 / 1.97 ms | confirmed: the CPU p99 at 1.5 is over, which step 2 fixes; the GPU room is for the compositor's own drawing that steps 8 and 9 move there. Revised in step 2: the CPU half is the time to compose and submit, without the wait for the GPU, each half against its own budget (Measurements, step 2) |
| First frame of a GPU process; switch within 150 ms on a warm cache | context 57 ms, first program 0.4 ms from the driver's cache | confirmed |
| Added private memory per GPU process under 80 MiB | 68.8 MiB after a context and a compiled first program; 21 MiB with the driver's cache | confirmed |
| GPU memory per GPU window | a process's first 3840 x 2160 target adds 63.5 MiB for 31.6 MiB of pixels and keeps it | revised: the driver reserves one more target's size with a process's first one; the 32 MiB of atlases and pools cannot hold that at 4K, so the budget names it |
| Client CPU after the look freeze: no scenario's client p99 more than 10% above its baseline | client p99 spreads: 1–10% for resize, Control Center, the launcher and the scrolls; 9–29% for the sub-millisecond drag and hover frames | revised: 10% or the recorded spread, whichever is larger, since 10% of a 0.6 ms frame is inside its noise |
| Fence overhead under 1 byte a frame | -0.25 bytes a frame | confirmed |
| Idle desktop: no frames, no GPU work; listed timers only | one menu bar frame and its composite a minute (the clock); wakeups as listed below | revised: the clock's one frame a minute is named as the exception |
| Pointer motion to screen below the baseline; cursor on the cursor plane | BLOCKED (owner action): the traced live session gives the baseline | confirmed as written; its baseline comes with the owner's trace |

**Context, first program, later programs, teardown** (`gpu-probes context --runs 5`, median of five fresh processes,
spread in brackets; private memory is clean + dirty, after each stage):

| Stage | GBM: time | GBM: private after | EGL device: time | EGL device: private after |
|---|---|---|---|---|
| process start | | 0.3 MiB | | 0.3 MiB |
| context (GLES 3, surfaceless) | 57.3 ms (17.4) | 20.0 MiB | 59.2 ms (6.1) | 20.0 MiB |
| first program | 31.5 ms (0.8) | 68.8 MiB | 31.2 ms (1.2) | 68.8 MiB |
| each later program (8) | 5.84 ms (0.05) | 68.9 MiB | 5.85 ms (0.03) | 68.9 MiB |
| teardown | 13.0 ms (0.5) | 61.7 MiB | 12.9 ms (0.8) | 61.7 MiB |

The first program costs 48.8 MiB of private memory (39 MiB of it dirty), the compiler loading; later programs cost
almost nothing. Teardown gives back 7 MiB: a process that had a context keeps about 61 MiB until it exits. Both
platforms are the same.

**Render targets** (`gpu-probes texture --size 3840x2160 --count 4 --runs 5`, an RGBA8 texture and its framebuffer,
total GPU memory from NVML; identical in all five runs):

| Stage | Total GPU memory added | Process GPU memory |
|---|---|---|
| context and one program | 11.4 MiB | 2.7 MiB |
| first 3840 x 2160 target (31.6 MiB of pixels) | 63.5 MiB | 66.2 MiB |
| each later target | 32.0 MiB | +32.0 MiB |
| all four deleted | 63.5 MiB stays | 66.2 MiB |

The first target of a process costs twice its size: the driver reserves a pool the size of one more target, and keeps
it when the targets are deleted. Later targets cost their size rounded up to 2 MiB.

**A saved program binary** (`gpu-probes binary --runs 5`: the program saved once with `glGetProgramBinary` (9.6 KiB),
then loaded as the first program of fresh processes; median of five):

| First program of a fresh process | Time | Private memory added |
|---|---|---|
| compiled, driver's disk cache off | 32.5 ms | 46.7 MiB (38.6 MiB dirty) |
| loaded from the saved binary, disk cache off | 4.95 ms | 6.2 MiB (0.6 MiB dirty) |
| compiled, driver's disk cache on and filled | 0.40 ms | 0.7 MiB (0.2 MiB dirty) |

**Native fence soak** (`gpu-probes fence --frames 10000000`, GBM, one small draw a frame, resident memory sampled and
fitted by least squares after the first tenth):

| Per frame | Resident growth | Run time |
|---|---|---|
| native fence: create, flush, export the fd, wait on it, close, destroy | 15.89 bytes | 305 s |
| no fence (`glFinish`) | 16.15 bytes | 132 s |

The fence adds nothing (-0.25 bytes a frame, inside the 1 byte a frame budget). Both grow about 16 bytes a frame,
15.5 MiB a million frames, linearly: any draw without a present grows the driver's memory this way (no draw: no growth;
the same on the EGL device platform and with the driver's threaded optimisations off). Step 3's slice presents every
frame and must show whether presenting stops it; until then a GPU toolkit process that draws without presenting
(offscreen work) is assumed to grow.

**10-bit gradient on the primary plane.** BLOCKED (owner action): it needs the display to itself. Tested as far as it
goes without the display (`gpu-probes depth --dry-run`): the primary plane of the connected DisplayPort output takes
`AB30` and `XR24`, both 3840 x 2160 framebuffers are made, the connector offers `dithering mode` (auto now; the probe
sets off) and no `max bpc`; the CRTC has 1024-entry gamma and degamma tables. To run it: log out, switch to a text
console (Ctrl+Alt+F3), log in, and from the checkout run `tools/gpu-probes/depth.sh`; then note for each band whether
the 10-bit half is smoother than the 8-bit half, beside `target/gpu-probes/depth.jsonl`.

**Presentation-time branch and plane assignment on the target driver.** BLOCKED (owner restart): it needs the session
server on the display. With a build that has the trace installed: log out, switch to a text console, log in, run
`CUPERTINO_DRM_TRACE=1 CUPERTINO_SESSION_LIMIT=300 cupertino-session`, use the desktop for a few minutes (move and
resize a window, scroll in Files, play a video full screen), let it end, then
`grep drm_trace ~/.local/state/cupertino/server.log`. The lines name the branch, missed flips, planes and latencies.

**Timers that wake an idle desktop.** The bench's idle scenario counts, per thread, its voluntary context switches,
samples where it sleeps (the call and its timeout) and reads each timerfd's period and count. Headless, Files and
Settings open (Settings on its first pane, Account), nobody touching the desktop for 60 s. Before this step's changes
(1280 x 800, 30 s): every process with a bus connection woke twice a second in its pool thread (120 a minute each,
10 processes), the server's main loop woke 242 times a minute, Settings drew a frame a second.

Removed:

- The pool thread that bus connections run their blocking calls on stayed for good after its first call and woke twice
  a second, in every process with a bus connection: its 1.7.0 release never ends an idle thread. Held at 1.6.2, which
  ends it after half a second without work, by a normal dependency of `cupertino-peer`
  (`tests::the_blocking_pool_ends_its_threads_once_idle` there; the gate fails on any other locked version).
- Hung-application pings: the server pinged every client with a mapped window once a second, and each woke to answer.
  Pings now go out only after input since the last one (the first input after a rest pings at once), so a desktop
  nobody touches pings no one (`hung::tests`, `hung_apps::a_desktop_nobody_uses_pings_no_one_and_the_first_input_pings_at_once`).
- The main loops' one-second backstop (session and headless): every time-driven piece of work names its deadline (the
  tick's calls are audited), so the backstop is now a minute (`a_session_at_rest_sleeps_and_wakes_its_loop_once_a_minute_at_most`).
- Settings drew a frame every second on a pane of readings (Account, 15 frames in 15 s): its rows all share one
  control, so they shared one key and were mounted again at every refresh. Each row now has a key of its own and a
  refresh that changes no reading draws nothing (`the_rows_of_a_section_have_keys_of_their_own_though_readings_share_their_control`,
  `a_turn_that_changes_no_reading_presents_nothing_on_a_pane_of_readings`).
- Settings re-read every pane once a second, whatever it showed: the appearance from the session service over the
  bus (which woke the server and the bus daemon each time), and on a machine without a system bus (the headless runs)
  a connect attempt and a listing of the bus's names a second. Settings now listens: one thread a bus asks the bus
  daemon for the signals of the services it reads (the session and notification services' owner and property
  changes; for the network, Bluetooth, power and power-profile services on the system bus, their owner and their
  signals on the objects the panes read only, so an access point's strength or a single power device's update never
  reaches it) and reads them from one queue it drains while it subscribes, so a burst at start cannot stall the
  connection. A signal the shown pane does not read is dropped in that thread and never wakes the window; one it
  reads wakes it once a burst. The appearance is read when the session service starts or says it changed. A device
  plugged in or removed (input, power supply, network, sound, display, Bluetooth) is heard from udev's
  announcements and reads the hardware again whatever the pane; a watch that ends with its connection starts again
  on a new one, then after 1 s, doubling up to a minute. A pane whose readings all have a change signal, or that
  shows none, arms no timer (`a_pane_whose_readings_have_change_signals_or_that_shows_none_arms_no_timer_and_a_polling_pane_one`,
  `the_watch_tells_a_service_starting_its_property_changes_and_a_system_service_signal`,
  `a_signal_the_shown_pane_does_not_read_never_reaches_the_window`,
  `a_burst_of_signals_while_the_watch_starts_stalls_neither_the_watch_nor_the_connection`,
  `a_device_plugged_in_adds_its_row_whatever_pane_shows`,
  `a_watch_that_ends_watches_the_bus_again_and_keeps_trying_while_the_bus_is_gone`,
  `a_failure_while_the_window_reads_the_background_queue_keeps_its_wait`).
- The settings file watch of every process woke whenever any process opened or read a file beside the settings or in
  the folder above them (`~/.config`, `/etc`; `/tmp` in the headless runs): its library always asks the kernel for
  open events and drops them only after waking. The watch now asks only for changes, and the folder above only for
  entries appearing and going (`opening_and_reading_files_in_the_watched_directories_wakes_nothing`).

At 3840 x 2160 before these last two changes, a minute of rest woke Settings 480–1140 times over its threads (its
async-io thread 240–780, its main thread 180, its bus thread 60–180), the server's async-io thread 145–216 times and
its bus thread 60, and the bus daemon 129. After them (`idle-1.5.json`, `idle-2.json`, five runs a scale): Settings 0,
the server 1, the bus daemon 9, and no settings watch thread in any run. Those runs have no system bus; with only
Settings given the machine's own (NetworkManager, BlueZ, UPower, the power and power-profile services running, Account
shown), its idle minute is the same: Settings 0 over all its threads, its two bus watch threads and the hot-plug
thread included (the idle scenario at 60 s, and each thread's voluntary context switches over a further 60 s).

Kept, with what they cost per minute after the changes (Settings open on Account, 60 s, both scales):

| Who | Every | Why | Wakeups a minute |
|---|---|---|---|
| Settings, while Sound, Displays, Wi-Fi, Network or Notifications shows | 1 s (100 ms while the input level meter shows) | readings without a change signal: the sound server's volume and outputs, the display list (a fresh connection to the display server), the kernel's interface addresses (Wi-Fi also scans every 10 s), a new notification's application | Settings one refresh a period; no other pane arms a timer |
| Settings, while Focus or Battery shows | 60 s | the durations offered follow the time of day; the charge history | Settings 1 |
| menu bar clock | at each minute | the clock; it reads do-not-disturb and the control panel's state with it | menu bar 21–26 over its threads, notifications 8–17, Control Center 2, bus daemon 9 |
| session and headless main loop | 60 s | backstop for a deadline left unnamed | server 1 |
| Files' folder watches and the dock's trash watch | on activity only | their file watch library asks for open events too; they wake only when a process opens a file in the shown folder or the trash, which nothing does on an idle desktop | 0 |

The whole desktop at rest wakes about 50 times a minute, all of it the clock's minute and the backstop. Two timerfds
stay armed without firing in a minute (one in the server, one in the menu bar): long deadlines, not periods. The
headless sessions keep the server's settings in a folder of their own, as a machine does (`/etc/cupertino`): beside
`/tmp` every file another process made there woke the settings watch.

### Step 3: the vertical slice

Taken at `7f54f6b8` on the target machine's GPU (GeForce RTX 3060, proprietary driver 615.78.08), headless.

**Where it is.** `cupertino-gpu-slice` (`crates/cupertino-gpu-slice`, a development binary, never packaged): a window
drawing solid rectangles with GLES (scissored clears) into dmabufs. It reads the compositor's default dmabuf feedback,
opens the main device's render node, makes a GLES 3 context on its GBM device (or, without one, on the surfaceless
platform with texture export), and allocates ARGB8888 buffers with the modifiers both the feedback offers and the
driver renders to (`model::usable_modifiers`: linear only when both list it). With `linux-drm-syncobj-v1` it keeps two
buffers, each with a timeline: the frame's native fence is imported into a binary syncobj and transferred to the
acquire point, the release point is waited for through an eventfd in its poll loop. Without the protocol it keeps
three, waits for its own fence before each commit (on a driver without native fences, for the GPU on the CPU), and
reuses a buffer only after its release and one further commit. `--shm-every K` draws every K-th frame from shared
memory, destroying the syncobj surface object first and making it again on the next dmabuf frame. Presentation
feedback counts missed refreshes from sequence gaps (or from the times over the refresh period when the compositor
does not number its refreshes); the headless backend now numbers them (refreshes since its clock started). Every
frame lays 24 overlapping rectangles in colours never shown at rest, then one flat colour over the window.

The headless session offers explicit sync when its device has a render node and the kernel waits on timeline points
through an eventfd; `--no-explicit-sync` withholds it.

**Headless session tests** (`gpu_slice::`, the slow tier but the first two; on the NVIDIA GPU with
`CUPERTINO_GPU_REQUIRED=1`, which turns every skip into a failure):

| Test | What it shows |
|---|---|
| `a_headless_session_offers_explicit_sync_with_a_render_node_and_withholds_it_when_told` | offered with the render node, withheld with the flag |
| `a_headless_presentation_numbers_its_refresh` | the sequence a headless presentation carries |
| `the_slice_attaches_dmabufs_with_acquire_and_release_points_in_the_driver_s_layout` | 180 dmabuf frames, two buffers, no CPU wait, every buffer but the last two back through its release point; the compositor's buffer has the client's modifier, `0x0300000000e08014` (the driver's default) on the NVIDIA GPU, never linear |
| `the_slice_destroys_its_syncobj_surface_before_a_shared_memory_frame_and_makes_it_again_after` | 6 shared-memory frames in 60, the object destroyed 6 times and made 6 times; the session never shows a shared-memory buffer on a surface that has the object |
| `the_slice_without_explicit_sync_waits_for_its_own_fence_and_keeps_three_buffers` | 180 commits, 180 fence waits, three buffers, captures uniform |
| `the_slice_counts_no_missed_refresh_when_the_session_keeps_up_and_every_overdraw_capture_is_flat` | 8 captures of the window, each one flat colour; missed refreshes counted |

Missed refreshes headless: none in 59 frames when nothing holds the loop up; the captures (each a whole-output render
and a PNG written by the session) hold it for a few refreshes each (15 in 107 frames with 8 captures).

**The driver's default modifier.** The compositor imports the slice's buffers in `0x0300000000e08014`, the layout the
driver picks among those both sides list (linear is import-only on this driver, so never offered to the allocator).

**The fence budget** (`cupertino-gpu-slice --offscreen 10000000`, two GBM buffers, one full-buffer draw a frame,
resident memory sampled 200 times and fitted after the first tenth):

| Per frame | Resident growth | Run time |
|---|---|---|
| the window's fence path: native fence, import into a syncobj, transfer to a timeline point, eventfd wait | 15.95 bytes | 443 s |
| no fence (`glFinish`) | 15.99 bytes | 262 s |

The fence path adds nothing (-0.04 bytes a frame; the budget is under 1). Both grow about 16 bytes a frame, as the
step 0 probe found for drawing without presenting.

**llvmpipe and texture export.** On the host's llvmpipe (`LIBGL_ALWAYS_SOFTWARE=1`, the Mesa EGL vendor forced, the
headless server on the surfaceless platform): the compositor offers dmabuf version 3 without feedback, with linear
and the implicit layout; the slice allocates by texture export (linear), and 120 of 120 frames were dmabufs the
compositor imported, each waited for by its fence. In the goldens image (`cupertino-goldens:7153b7a8…`, no device
passed in): llvmpipe has neither native fences nor texture export, so the slice falls back to shared memory for its
surface (120 shared-memory frames). With `/dev/udmabuf` passed into the same image (`docker run --device
/dev/udmabuf`), texture export works: 120 of 120 dmabuf frames, linear, imported. So the dmabuf presenter can run
in CI on llvmpipe only where the runner gives the container `/dev/udmabuf`.

**The live session and the one-hour gate.** BLOCKED (owner action): it needs the desktop session. From a terminal in
the session, in the checkout:

```sh
cargo build --release -p cupertino-gpu-slice
target/release/cupertino-gpu-slice --minutes 60 --control-shm --size 1280x800 --report ~/gpu-slice-hour.txt
```

Two windows open side by side (place them so neither covers the other): the slice and a shared-memory control
window of the same size, drawing a frame every refresh for an hour. At the end both reports are printed (the control's
lines start `control_`) with `comparison gpu_missed=N control_missed=M pass|fail`; pass is the gate's first half.
For the second, take a few screenshots of the slice window while it runs: inside its rounded corners each must be one
flat colour (blue or orange). The report also names the modifier and whether explicit sync was used.

### Step 5: `cupertino-gpu` and the shader toolchain

Taken at `0c12d235` on the target machine's GPU (GeForce RTX 3060, proprietary driver 615.78.08) and on llvmpipe.

**Where it is.** `crates/cupertino-gpu` (graphics layer, MPL-2.0, depends on `glow` and `libloading` only):

- `Device`: made on a render node through GBM (`Platform::RenderNode`) or on the surfaceless platform
  (`Platform::Surfaceless`), or borrowed from the context current on the thread (`Device::borrow`, unsafe: the owner
  keeps it alive and current). `DeviceInfo` says what it is: renderer, software or not, allocator (GBM, texture
  export or none), native fences, fence import, robustness, purge notification, float targets.
- Resources from descriptions, destroyed explicitly: `Buffer` (uniform, vertex, index), `Texture` (`R8`, `Rgba8`,
  `Rgba16Float`, `Rgba32Float`; sampled and render targets), `Sampler`, `BindGroupLayout`, `BindGroup`, `Pipeline`
  (program, layouts, vertex buffers, blending, topology, all fixed at creation), `Fence`.
- External memory: `allocate_dmabuf` (a GBM buffer object with an offered modifier, or a texture exported in a layout
  the offer holds) gives an `ExternalImage` (its texture and its `Dmabuf`); `import_dmabuf` makes a texture of
  another device's or process's dmabuf.
- Passes: `begin_pass(target, LoadOp)` gives a `Pass` (`set_pipeline`, `set_bind_group`, `set_vertex_buffer`,
  `set_index_buffer`, `set_scissor`, `draw`, `draw_indexed`, `end`). `submit` gives a fence, `submit_exported` a fence
  and its sync file, `wait_sync_file` makes later work wait on another's sync file on the GPU.
- Loss: `status` reads the driver's reset status (`Loss::Reset { guilty }` or `Loss::Purge`; a purge does not count
  toward the CPU fallback); a lost device refuses every call with `Error::Lost`. `inject_loss` is the test hook.
  Every missing library, platform or device is `Error::Unavailable`, never a failed start.
- Borrowed contexts: `enter` saves and `leave` restores the state listed in `gles::state` (program, framebuffers,
  viewport, scissor, blending, colour mask, clear colour, depth, stencil and cull enables, the first 16 texture units'
  textures and samplers, array and uniform buffer bindings and the first 16 uniform ranges, the vertex array, pack and
  unpack alignment and row length); the device draws with a vertex array of its own.
- `atlas`: coverage, colour and path mask atlases (shelf-packed pages with a one-pixel gap, a page used least
  recently emptied when an entry fits nowhere at the cap, never one used in the current frame). `pool`: render
  targets kept and handed out again by size and format, with a cap.
- `shaders`: the generated programs (`shape`, `blit`, `blur_halve`, `blur_gauss`, `dither_fill`, and the test
  programs `probe_coverage`, `probe_shadow`, `probe_gradient`, `probe_dither`). Sources in
  `crates/cupertino-gpu/shaders/*.wgsl`, the shared module in `shared.wgsl`; `cargo run -p cupertino-gpu-shaders`
  generates `generated/` (GLSL ES 3.00 for each stage, SPIR-V, the binding names), `--check` compares.
  `cupertino-gpu-shaders` uses `naga` 24, the version already locked for the glass crate.

**Caps** (each a multiple of the 2 MiB the driver rounds a render target up to, step 0's "Render targets"):

| What | Size | Cap |
|---|---|---|
| Coverage atlas | 2048 x 1024 8-bit pages (2 MiB) | 2 pages, 4 MiB |
| Colour glyph atlas | 1024 x 1024 RGBA pages (4 MiB) | 2 pages, 8 MiB |
| Path mask atlas | 2048 x 1024 8-bit pages (2 MiB) | 2 pages, 4 MiB |
| Render target pool | each target its bytes rounded up to 2 MiB | 16 MiB in use and free together |

Together 32 MiB, the design's "32 MiB of atlases and pools" for a GPU window. The window's buffers and its
window-sized layer are outside the pool, and a process's first render target makes the driver reserve one more
target's size beside it, which no cap can hold (step 0); the budget names both separately. A target the pool cannot
hold (one 3840 x 2160 RGBA target alone is 32 MiB) is refused with `Error::OverBudget`, so a full-window intermediate
is the renderer's own, not the pool's.

**Tests.** The device tests (`cupertino-gpu::suite` and the borrowed-context test in the library) run in the slow
tier; the models of the atlases, the pool and the binding names, and the generator's tests, in the fast tier.

| Where | What ran | Result |
|---|---|---|
| NVIDIA, GBM on the render node, `CUPERTINO_GPU_REQUIRED=1` | both crates, both tiers (`--profile ci`) | 35 of 35 passed, none skipped |
| host llvmpipe (`LIBGL_ALWAYS_SOFTWARE=1`, the Mesa EGL vendor forced, surfaceless, texture export), `CUPERTINO_EGL_REQUIRED=1` | the same | 35 of 35 passed, none skipped |
| the CI image (the goldens image, no device passed in), `CUPERTINO_EGL_REQUIRED=1`, `cargo test` | the same | 35 passed; the two dmabuf tests and the native fence test skipped: llvmpipe there has neither texture export nor native fences, as step 3 found |

What the tests show: every generated program compiles and links; the shape program's rectangles, circular and
continuous corners match the shared coverage within 2/255 at every pixel; premultiplied blending and the scissor;
a shape at the top left lands in the first rows, a texture's first row on the target's first row, and dithering is
keyed to the pixel counted from the first row; the coverage, shadow, gradient and dither programs match their vectors
(distances and shadows within 2·10⁻³, gradients within 10⁻⁵, the dither matrix exactly and its stores within a step);
the blur's halving and Gaussian programs match the blur vectors at both edge rules in half floats (within 2·10⁻³) and
8 bits (within a step); a dmabuf drawn on one device is imported and sampled on a second after its sync file, in the
driver's modifier, never linear on the NVIDIA GPU; fences and sync files signal; an injected reset of each kind and a
purge are reported and the lost device refuses work; a borrowed device leaves its owner's state exactly as it found
it; atlas eviction reuses a page's texture and the cap holds within a frame; a released pooled target comes back.

### Step 6: the presenter and the process

Taken on the target machine's GPU (GeForce RTX 3060, proprietary driver 615.78.08), headless sessions, scale 1.

**Where it is.**

- `cupertino-ui-wayland::renderer`: the fallback rule as functions of what the process found (`start`, `settle`,
  `LossHistory`), the switch `GPU_BY_DEFAULT` (off), `CUPERTINO_RENDERER`, the process's current renderer and why
  (`active`), logged as `renderer: <cpu|gpu> (<reason>)` on every change and recorded in
  `$XDG_RUNTIME_DIR/cupertino/renderers/<pid>` for `cupertino-diagnose` (`gpu.client_renderers`). The frame
  statistics carry `renderer` on every line and `scripts/dev/bench` lists each program's renderers.
- `cupertino-ui-wayland::gpu_path`: the warm-up thread (device on the feedback's render node, saved programs, the blit
  program; the context released and taken over by the main thread), the switch (`PlatformEvent::RendererChanged`,
  on which the runtime drops each window's renderer and draws whole frames with a new one), losses (every surface's
  buffers freed, a new warm-up, the CPU for good after three resets within a minute; purges never counted), the
  per-surface fallback to shared memory and `buffers_lost`.
- `cupertino-ui-wayland::gpu_present`: one surface's buffers. A frame's damage is uploaded into a staging texture and
  drawn into a dmabuf allocated in 128-pixel steps (a buffer serves a frame that fits and is less than two steps
  smaller; the viewport crops it), in a modifier both the compositor offers and the driver renders to. With
  `linux-drm-syncobj-v1` the frame's native fence becomes the acquire point of a timeline per buffer and its release
  point is waited for through an eventfd, two buffers; without, the commit waits for the own fence and a buffer is
  reused one commit after its release, three buffers.
- Transport (`cupertino-wayland-client`): dmabuf feedback (format table, main device, tranches), buffers the
  compositor may refuse (`create`, not `create_immed`), syncobj timelines and points, frames cropped to their size; the
  syncobj surface object is destroyed before any shared-memory or one-colour commit.
- `cupertino-gpu`: syncobj timelines on the render node (`syncobj`), the saved program binaries (`program_cache`), and
  a device made on one thread and used on another (`Device::detach`, `DetachedDevice::attach`).
- `cupertino_app_effects_v1` version 21: `cupertino_appearance_v1.buffers_lost`, sent by the server after its own
  reset or purge to every observer of version 21 or later. It goes on the appearance observer because every toolkit
  process, third-party ones included, already holds that surface-less object for session-wide events that need a
  repaint, so no new global is needed and `cupertino_shell_v1` (shell processes only) is not the place.

**First frame and warm-up** (the check window, 320 x 240, three runs each; the first frame went out on the CPU every
time):

| Program cache | First frame (shared memory) | GPU ready after the start | Context | Programs |
|---|---|---|---|---|
| empty (compiled, then saved) | 29.7–30.2 ms | 99–103 ms | 62.9–66.5 ms | 35.9–36.7 ms |
| filled by the run before (loaded) | 28.9–29.3 ms | 65–73 ms | 64.6–72.1 ms | 0.3 ms |

With the renderer's programs made in the warm-up too (`118b31cd`, the presenter's tests' window, 3 runs): first frame
29.8–30.5 ms; empty cache, GPU ready after 233–237 ms (context 64–69 ms, programs 168–169 ms, once per driver and
cache); filled cache 69–78 ms (context 65–74 ms, programs 4 ms).

**Tests.** The presenter's GPU tier is `cupertino-server`'s `gpu_presenter` (slow tier) with
`cupertino-ui-wayland`'s `gpu-window` as its client; the rule, the size steps, the reuse, the records and the switch
in the runtime are fast tests.

| Test | What it shows on the NVIDIA GPU (`CUPERTINO_GPU_REQUIRED=1`) |
|---|---|
| warm-up and syncobj | first frames in shared memory, then 2 buffers in the driver's tiled modifier (`0x0300000000e08014`) imported as drawn, every buffer back through its release point, no CPU fence wait, one syncobj surface object, the record says `gpu` |
| program cache | the second start loads the program the first compiled and compiles none |
| without explicit sync | 3 buffers, one fence wait a frame, no syncobj object |
| resets | a new device after each, the CPU for good after the third (reason "the context was lost 3 times within 60 s"), the syncobj object destroyed before every shared-memory frame |
| purges | four purges, five devices, the process stays on the GPU and the later devices load the saved program |
| refused import | the surface moves to shared memory with its syncobj object destroyed first; the process stays on the GPU |
| resize storm | 183 sizes in 233 frames from 13 buffers |
| short soak | 900 frames, 2 buffers, every release point signalled, no fence wait |

8 of 8 passed; `gpu_slice` (6 of 6) and `gpu_reset` (3 of 3) still pass beside them.

### Step 6: Files on the GPU path

Taken at `999c46f2` on the target machine (GeForce RTX 3060, proprietary driver), release builds,
`CUPERTINO_RENDERER=gpu scripts/dev/bench --size 3840x2160 --scale S --scenario files-full-scroll --runs 3`, other
checkouts building beside it (load 5 to 9). Every Files frame counted was drawn by `GpuRenderer` (the frame lines'
`renderer`). Medians of the three runs, the range of the runs after them; the phases are the Files client's own. The
CPU column is the CPU path in the same hour at `68ff5660` (one run, load over 10).

| Files two-finger scroll, client frame ms | GPU, scale 1.5 | GPU, scale 1 | CPU, scale 1.5 |
|---|---|---|---|
| Frames drawn in the scroll | 184 | 158 | 80 |
| Frame p50 / p99 | 2.8 / 7.7 (7.2–10.8) | 6.3 / 37.5 (25.9–39.5) | 32.0 / 52.5 |
| Update + view + layout p99 | 4.6 | 25.4 | |
| Paint p50 / p99 | 1.5 / 2.0 | 4.0 / 12.3 | |
| Raster p50 / p99 | 0.8 / 2.2 | 1.1 / 4.6 | 11.3 / 16.1 |
| Copy p50 / p99 | 0.09 / 0.65 | 0.11 / 0.59 | 13.3 / 29.8 |
| Commit p99 | 0.0 | 0.0 | |

How it came there, at 1.5:

| Commit | Frame p50 / p99 | Raster p50 / p99 | Copy p50 / p99 | Commit p99 |
|---|---|---|---|---|
| `68ff5660` (the content painted in place) | 7.8 / 15.0 | 5.9 / 7.4 | 0.07 / 0.53 | |
| kept scroll layers on the GPU | 4.1 / 14.6 | 0.9 / 2.2 | 0.14 / 0.73 | 5.6 |
| the raised layer from GPU buffers (`06898193`) and `999c46f2` | 2.8 / 7.7 | 0.8 / 2.2 | 0.09 / 0.65 | 0.0 |

The scroll's budget (8 ms p99) is met at 1.5 in two runs of three. What is left of a frame is the toolkit's: at scale
1 the frames that show other rows make the view again (view and layout together 16–26 ms p99, paint 8.5–12.5 ms
p99), step 4's open items. The renderer's own part of a frame (raster and copy) stays under 6 ms p99 at both scales.

### Step 6: the renderer

Taken at `27c8c02c` on the target machine's GPU (GeForce RTX 3060, proprietary driver) and on the host's llvmpipe
(`LIBGL_ALWAYS_SOFTWARE=1`, the Mesa EGL vendor forced, surfaceless), release builds.

**Where it is.** `cupertino-ui-gpu`: `GpuRenderer` records a frame's draws (`record`) and then runs them (`exec`): its
uniforms and instances in one buffer each, a pass per target. Three programs, written in WGSL and generated like the
others: `ui_fill` (a rectangle's or rounded rectangle's distance, or a coverage image, under up to four analytic clips
and a clip mask, in solid or gradient paint, all in the CPU renderer's byte arithmetic), `ui_texture` (colour glyphs,
layers, images) and `ui_effect` (a blurred level resolved into a blur, drop shadow or inner shadow). Glyphs go into a
coverage and a colour atlas, path masks and clip masks into a mask atlas, images into textures kept by revision under a
byte budget; layers and blur levels (32-bit floats where the device renders to them) come from the target pool. A frame
repaints the rectangles every renderer repaints (`cupertino_ui_render::damage_rects`). `ui-render` gained the shared
request check and damage rectangles, its golden scenes in their own file for both suites, and one fix found by the
comparison: under a stretch (different horizontal and vertical scales) its span path laid pixels at full coverage
that its general path covers a little less, so a stretched node keeps its scratch image.

**Goldens** (`goldens::every_golden_scene_matches_its_baseline_on_the_gpu`, the CPU renderer's 13 scenes against its
baselines, tolerance 2/255): all within 1/255 on the NVIDIA GPU (`CUPERTINO_GPU_REQUIRED=1`) and on llvmpipe
(`CUPERTINO_EGL_REQUIRED=1`); the incremental clip frames included.

**Parity** (`parity::*`, 9 tests, 76 seeds, 3 scales, 3 frames each, the last two repainting one to three damaged
rectangles after a change): every pixel within 2/255 of the CPU renderer, but for at most 4 pixels a frame at 3/255
(`STRAY_PIXELS`). Those few come from what the CPU renderer lays through its rasterizer's own pipeline, an operation's
layer (`PushLayer`) and a solid stroke along a path, whose products round each piece apart; the GPU renderer lays the
stroke drawn by the same stroker and the layer with exact products. The same on both devices.

**A large window** (`timing::a_large_windows_damaged_frame_stays_within_the_gpu_budget`: 1600 x 1000 points at 1.5,
2400 x 1500 pixels, tiled with the golden scenes, 325 nodes; medians; a GPU frame timed from the request to the
device finishing it):

| | NVIDIA GPU | llvmpipe | CPU renderer |
|---|---|---|---|
| making the renderer (its programs) | 16.7 ms | 12.6 ms | |
| first frame (caches and atlases filled) | 16.4 ms | 75.3 ms | 14.7 ms |
| whole frame | 13.4 ms, of which 13.3 ms before the request returns (2.5 ms at `88f056ec`) | 42.3 ms | 10.8 ms |
| a control's worth of damage | 0.24 ms, of which 0.17 ms before the request returns | 1.7 ms | 0.51 ms |

The GPU's own share of a frame is under a tenth of a millisecond after the request returns: client GPU time is far
under its 4 ms. The whole frame's time is the recording and issuing of its draws on the CPU, one draw and bind group
per operation, and above all the 117 of its 125 offscreen targets made and destroyed every frame: with a finished
target's texture serving the next of its size (`88f056ec`) the whole frame takes 2.5 ms, 2.3 ms before the request
returns, no target made; a control's worth of damage 0.24 ms. The first frame still goes out on the CPU while the device
warms (step 6's presenter), so the first-frame budget holds; a switch with a warm cache costs making the renderer and
its first frame, about 33 ms on the NVIDIA GPU, within the 150 ms. Under heavy load from other builds the GPU
renderer's whole frame once took 420 ms on the CPU side, the driver's threads starved; the CPU renderer's did not
move.

### Step 9: layers and animation in the compositor

Headless server, `cupertino-controlcenter --fake-backends` on a private bus, debug builds, counted by
`shell_processes::the_control_panel_fades_on_the_compositor_s_clock_without_a_new_buffer` (its trace with
`--no-capture`): the buffers the card committed between its first frame and the end of each fade, against the server
steps that showed the fade part way.

| Control Center | Server steps part way | New buffers of the card |
|---|---|---|
| Fade in, with the compositor's presentation | 58 | 1 |
| Fade out, with the compositor's presentation | 27 | 0 |

The one buffer of the fade in is a single frame the card draws after its first as it opens; the fade's steps draw
none, where without the presentation each step is a frame. Taken at the branch's commit, on the machine shared with other builds.

Without the presentation the card draws one frame per frame callback through each fade
(`the_fade_steps_at_each_frame_of_the_card_s_window_until_it_ends`): about 14 at 60 Hz for the 0.23 s fade in and 11
for the 0.18 s fade out.

Popups, counted by `a_popover_grows_and_shrinks_and_a_menu_fades_on_the_compositor_s_clock_without_a_frame` (the
toolkit's runtime over its in-memory window system, frame callbacks answered every 16 ms):

| Popup | Frames answered | Frames presented, handed over | Without the handover |
|---|---|---|---|
| Popover growing (0.35 s) | 25 | 0 | one per frame until it rests |
| Popover shrinking (0.22 s) | 14 | 0 | one per frame until it goes |
| Menu fading (0.236 s) | 14 | 0 | one per frame until it goes |

**The sheet.** Headless server at 1280 x 720 points, scale 1.5, release builds, recorded at 60 frames a second:
"before" from the commit before Control Center's presentation (the card drawing its own fade), "after" from this step's
branch (the compositor fading the card). Each pair is taken the same number of frames after the first frame that
changes, the after frame the closest of that frame and its two neighbours, as the two runs start up to a frame apart;
the number is the largest difference of any channel in the crop, out of 255.

| Control Center | Mid-way pairs (frames after the start) | Largest mid-way | Settled |
|---|---|---|---|
| Open, dark | 1, 2, 4, 7, 10 | 7 | 1 |
| Close, dark | 1, 3, 5, 8 | 14 | 0 |
| Open, light | 1, 2, 4, 7, 10 | 10 | 0 |
| Close, light | 1, 3, 5, 8 | 19 | 0 |

Settled, the card is the same within a step of one channel. Mid-way, the two fades sit at slightly different moments:
each run's frames land at their own phase of the fade's clock (the card's own frames before, the compositor's after),
so a pair compares two moments up to a frame apart, and the differences fall to 2 or 3 as the fade slows. The
launcher's pairs (open at 1, 2, 3, 5, 8, 14 and 24 frames, close at 1 to 4, settled) are drawn by the application on
both sides: settled within 2, mid-way up to 255 at the moving edge of the field, as the launcher's own frames land at
different moments in the two runs.

### Step 1: the look freeze

`scripts/dev/bench --size 3840x2160 --scale 1.5 --runs 3 --idle-seconds 30`, the base commit's programs and then the
freeze's, one after the other on the same machine (other builds ran beside them; drag and idle were measured again
after a run the machine disturbed, whose spread was 0.6 and 0.8 ms). Client frame p99 ms, median of the three runs
(spread):

| Scenario | Client | Before | After | After / before |
|---|---|---|---|---|
| idle | menu bar | 0.49 (0.07) | 0.44 (0.09) | 0.90 |
| drag | Settings | 0.75 (0.03) | 0.77 (0.26) | 1.02 |
| hover | Settings | 1.27 (0.05) | 0.96 (0.00) | 0.76 |
| resize | Files | 17.61 (6.16) | 17.05 (2.61) | 0.97 |
| control-center | Control Center | 14.99 (18.08) | 16.12 (5.88) | 1.08 |
| launcher-typing | launcher | 6.77 (0.46) | 5.99 (0.55) | 0.88 |
| files-scroll | Files | 7.64 (6.06) | 8.03 (2.20) | 1.05 |

No scenario is more than 10% above its before. The first run of the freeze put launcher-typing at 8.51 ms (1.26): its
open and close blur the whole panel as a group, and the Gaussian's first version split, read back and rounded every
pixel through the general paths (46 ms for a 1000 x 640 layer at the widest blur, against the box blur's 21). Rounding a
store without the library call, splitting the planes in one pass and reading back and dithering a row at a time, with
the same bytes, brought it to 21 ms at the widest and less below it, and a drop shadow from 15 to 3 ms.

### Step 2: compositor wins

Taken on the target machine's GPU, release builds, `scripts/dev/bench --scenario drag --size 3840x2160 --scale 1.5
--runs 3`, no build running. Before is `2b119fa4` with only the counter of texels the blurs write added; after is the
step's last commit. Medians of the three runs, spread in brackets where it is not zero.

| Drag over glass | Before | After |
|---|---|---|
| Blur passes per frame | 9.36 | 9.33 |
| Texels the blurs write per frame | 1 221 263 | 561 017 |
| Backdrops blurred again / reused per frame | 4.69 / 4.31 | 2.79 / 6.21 |
| Compositor work p50 / p99 ms | 1.25 (0.02) / 2.33 (0.11) | 1.39 (1.12) / 2.42 (0.24) |
| Compositor GPU p50 / p99 ms | 1.01 / 2.09 (1.03) | 1.06 (0.37) / 1.49 (0.76) |
| Server CPU ms/s | 86.7 (3.3) | 93.3 (53.3) |
| Missed refreshes | 0 | 0 |

The GPU p99 is not the overlap-limited generation's. It moves between runs of one build by more than any change of
the step: `2b119fa4` measured 0.94 ms one session, 1.53 to 5.55 ms and 1.16 to 2.19 ms on two later ones, and a build
of the step without the overlap-limited generation 2.11 to 5.89 ms. The frames past 2 ms are one to three a run,
where the blur passes alone take 1.65 to 1.8 ms of GPU time against 0.23 ms in the frames around them, with the same
passes, crops and backdrops made again, at full clocks, in every build. The GPU is shared with the desktop session
while the bench runs, and the slow frames come with it, not with a build.

Every frame of the drag uncovers something under the backdrops the window passes or carries, so every frame runs the
passes of a blur for them: 9.33 passes a frame is the drag's own, and making a backdrop again in parts adds passes
(a capture and a pyramid a part) while each covers less. What the step cuts is how much the passes cover: a
backdrop whose region moved, or whose scene changed in part, keeps its pyramid's last level and makes again only the
texels whose squares changed or newly lie under it, then filters the whole level as after a whole capture. Since
each texel of the last level is the mean of its own square of pixels, the result is the whole capture's texel for
texel (`material::tests::a_backdrop_made_again_where_it_changed_or_newly_reaches_is_the_blur_of_the_whole`, and on the
GPU `window_popup_backdrop::a_sidebar_dragged_over_a_window_is_blurred_again_in_part_and_looks_as_if_blurred_whole`,
worst difference 1 of 255). A window's shadow and outline draw nothing under the window, so their move changes only
the ring around it. The texels written fall by 54 %; the glass containers, whose passes are most of the count, still
blur their whole sampled area.

The gate is not met as written: the passes are inherent to the drag, and the compositor work p99 stays over the
2 ms budget (2.33 ms before, 2.42 ms after, which includes waiting for the GPU). Proposed revision of the gate, for
the lead:

- Old: "Window drag over glass meets its budget with fewer blur passes than the baseline; the shimmer and banding
  checks pass."
- New: "Window drag over glass meets its budget with fewer texels written by blur passes per frame than the baseline;
  the shimmer and banding checks pass."
- Reason: each frame of a drag uncovers new backdrop, so the pass count is set by how many backdrops the window
  touches, not by how they are made; the texels written are what a cheaper blur saves, and are measured per frame by
  the bench (`blurKt/f`).

Even revised, the budget half stays open: the work p99 at 1.5 is over 2 ms before and after.

The revision was accepted. A second round on main as of `8603ae73`, five runs, other checkouts building and testing
throughout (8 to 19 compilers and test runners beside the bench, no other headless session):

| Drag over glass, 5 runs | Before (3 runs, quiet) | After: median (spread) |
|---|---|---|
| Compositor work p99 ms | 2.33 (0.11) | 7.08 (6.34); the two runs with the least beside it 2.65 and 2.95 |
| Compositor GPU p99 ms | 2.09 (1.03) | 5.40 (5.69); 2.32 and 2.58 |
| Texels the blurs write per frame | 1 221 263 | 561 233 (3 425) |
| Blur passes per frame | 9.36 | 9.42 (0.11) |

What remains of the work is waiting for the GPU. The compose is 40 µs a frame; the render is the GPU's frame plus
0.5 ms (p50, 1.6 ms p99 with the compilers beside it). The frames past 2 ms are those whose blur passes take
1.6 to 2 ms of GPU time instead of 0.3 ms for the same ten passes.

The glass containers do not take the partial re-blur. In this drag every glass backdrop made again belongs to the
dragged window: its sidebar (492 by 1266 pixels), its toolbar and its title area. Each moves with the window, and its
capture is its own area at half scale, with the window's edge repeated past it. A move shifts each texel's place under
the screen by a fraction of a texel and moves the repeated edge with it, so no texel of the last blur is one of the
new blur, and there is nothing to keep. The same holds for the stationary dock while it resizes.

Next steps, as proposed then:

1. Capture a window's glass on a grid fixed to the output, with a margin as the materials have, and repeat the
   window's edge in the composite rather than in the capture. A dragged window's glass then keeps every texel away
   from the edges it uncovers, as its material does. This changes pixels at the glass edges, so it re-records the
   goldens that show window glass.
2. Measure the work without waiting for the GPU: CPU time to submit, GPU time from timestamps, each against its own
   budget. The budget then holds unless the GPU itself is over.
3. Bench on a quiet machine before ticking the gate.

**Third round: the budget split, and why the grid capture waits.**

The budget is split, as accepted by the lead. The frame line gains `submit_us`, the time from the start of the
frame's drawing until its commands are issued, before the wait for the GPU; the bench reports the CPU's part of a
frame as compose plus submit (`submit_p50_ms`, `submit_p99_ms`, table column `cpu99`) beside the GPU time from
timestamps.

- Old budget: "Compositor frame while a window is dragged over glass: under 4 ms GPU, under 2 ms CPU", checked as the
  compositor's work p99 (compose and render, the render waiting for the GPU to finish) against 2 ms.
- New budget: "p99 GPU time per frame (timestamps) under 4 ms; p99 CPU time to compose and submit a frame, not
  counting the wait for the GPU, under 2 ms".
- Reason: the work time holds the GPU's own time, so a shared or busy GPU put it over the CPU budget while the CPU did
  no more work (the second round's 7.08 ms work p99 were 5.40 ms of GPU time). Each half is now measured against its
  own budget, and the GPU half has the 4 ms it was given.

Five runs on main as of `383368e8` with the split measured (4K, 1.5), on a quiet machine: load average 1.3 to 2.6,
no compiler or test runner, no headless session but the bench's own. A second five runs after the change below,
with other checkouts building and testing beside it (load average up to 9.2, up to 5 compilers, 2 test runners and
one other headless session):

| Drag over glass, 5 runs: median (spread) | Quiet | Parts in kept targets, busy machine |
|---|---|---|
| CPU time to compose and submit, p50 / p99 ms (budget 2) | 0.61 (0.19) / 2.34 (0.43) | 0.59 (0.41) / 2.35 (0.69) |
| GPU time, p50 / p99 ms (budget 4) | 1.06 (0.00) / 2.20 (0.87) | 1.06 (0.01) / 2.11 (0.58) |
| Work p99 ms (compose and render, with the wait) | 2.49 (0.25) | 2.42 (1.25) |
| Targets created per frame | 0.49 | 0.04 |
| Texels the blurs write per frame | 561 017 | 561 017 |
| Blur passes per frame | 9.33 | 9.33 |
| Missed refreshes | 0 | 0 |

The GPU half holds with room. The CPU half is over at p99: 2.34 ms against 2 ms. The frames that pass 2 ms are
those whose GPU time does as well, and their CPU time is the drawing (lowering the scene takes 0.1 ms of them), so a
GL call during the drawing waits for the GPU. An earlier cause of the same frames is gone: a part of a backdrop made
again had a pyramid of its own size, and the parts change size every frame, so each frame created and deleted
textures (13 to 27 in the slowest frames, 1 to 3 ms of CPU time). The parts' levels are now made at the corner of
targets whose sides are rounded up (to a multiple of 64 texels, or of a quarter of the power of two below), which
later frames find again (`material::tests::the_parts_of_a_drag_are_made_in_targets_of_few_sizes_that_hold_them`; the
drag still matches a whole blur within 1 of 255). Targets created per frame fall by 92 %; the CPU p99 does not move,
so the wait for the GPU inside the drawing is what holds it over.

A last run of the committed build (one run; the machine is shared all night, so its times are taken under load:
load average 2.7 to 3.2, no compiler at that moment): 561 017 texels written by the blurs per frame against the
baseline's 1 221 263 (54 % fewer), 9.33 blur passes per frame, 0.04 targets created per frame, CPU p99 2.31 ms,
GPU p99 2.10 ms, no missed refresh. The texel count does not depend on the load.

The gate stays open. Next: find which call in the drawing waits for the GPU (timing the glass and material passes'
GL calls on the CPU, as was done for the textures) and remove the wait, then bench again on a quiet machine.

**Fourth round: no call waits for the GPU.** Taken at `4e1bf610` with a temporary build that timed, on the CPU, each
part of the drawing (lowering, binding, every element kind, every blit by its place, every pass's binding, uniform
upload and draw, target creation and deletion, the tone readback, the composite) and, around the whole submit, the
render thread's own CPU time (`CLOCK_THREAD_CPUTIME_ID`), its user and system time, page faults and voluntary and
involuntary context switches. The probe was removed afterwards; nothing of it is committed. One run each (`--runs 1`,
4K, scale 1.5), the frames of the scenario's span only (180), other checkouts building beside it; load average at
start and end of each run:

| Run | Condition | Load | CPU p99 ms | Frames with one call over 1 ms | CPU p99 without them ms |
|---|---|---|---|---|---|
| 1 | as committed | 1.05, 0.81 | 2.37 | 3 | 1.39 |
| 2 | as committed | 4.44, 5.10 | 2.54 | 5 | 1.85 |
| 3 | the driver told to sleep, not spin, while it waits (`__GL_YIELD=USLEEP`) | 6.00, 5.31 | 2.36 | 3 | 1.51 |
| 4 | no timer queries at all (frame timestamps and pass timers off) | 2.48, 1.91 | 2.35 | 3 | 1.34 |
| 5 | as committed | 3.49, 2.86 | 2.40 | 2 | 1.33 |
| 6 | the driver's threaded dispatch on (`__GL_THREADED_OPTIMIZATIONS=1`) | 2.66, 2.31 | 2.42 | 2 | 1.46 |

What holds the p99 over 2 ms is two to five frames a run in which one single GL call takes 1.7 to 2.0 ms of CPU,
where the same call takes 5 to 60 µs in every other frame: a framebuffer blit (the first level of a material's
pyramid, a glass capture or one of its edge strips), once the tone readback, once a glass composite, once a call of
the renderer's own outside our passes. In those frames:

- the render thread's CPU time equals the submit's wall time within 1 %, almost all of it user time, with no
  voluntary and at most one involuntary context switch and no page faults: the thread is not sleeping, waiting in the
  kernel or preempted;
- with the driver told to sleep while it waits, the frames are the same and still switch no context, so the time is
  not the driver waiting on a fence or the GPU;
- turning every timer query off changes nothing, nor does the driver's threaded dispatch;
- the blit's sizes and filter were each seen in earlier frames, no target is created or deleted in or around those
  frames, and the frames before and after draw the same passes in the usual time;
- they come about once a second (in four runs at 9.24, 10.19 and 11.25 s of the session, give or take one frame), and
  the server's other threads use no CPU meanwhile.

So no call of ours waits for the GPU. Each frame's GPU time rises with them because the GPU has nothing to do while
the CPU is held: the timestamps span the gap, and the GPU p99 (2.2 ms) is these frames too. The earlier reading, that a
call in the drawing waits for the GPU because the CPU time follows the GPU time, had cause and effect reversed.
Without those frames, the CPU p99 is 1.33 to 1.51 ms on a lightly loaded machine (1.85 ms at a load of 5) against the
2 ms budget, and the GPU p99 under 1.2 ms. At 60 Hz, 180 frames and a stall a second, three frames are enough to
own the p99, whatever the drawing costs.

The stall is driver CPU work on the calling thread at a period of its own, and nothing in what the drawing issues
moves it. The gate is not ticked: BLOCKED until the cause inside the driver is found or the gate's p99 is taken over a
span long enough to measure a once-a-second event as such (the drag scenario's 180 frames cannot), which is the
lead's call. The texel count, the shimmer and the banding checks pass as before.

The grid capture for window glass (next step 1) is not built: it would change the glass edges and keep nothing in the
drag it is for. A dragged window's glass reads the window under it, and the window's own surface is one of the
elements below the glass. A moved element changes its whole place, where it was and where it is
(`backdrop_change`), so every frame of the drag marks the whole area of each of the window's glass as changed, and a
capture on a fixed grid would make all of its texels again, as today. It would keep texels only where the moved
surface draws nothing, and the compositor does not know where that is: the sidebar window paints nothing under its
sidebar, whose glass therefore shows the windows and the wallpaper behind it, but its buffer has an alpha channel and
no region says so. (A window's shadow and outline have known holes only because the compositor draws them.)

What would let the drag keep the glass's texels, for the lead to choose:

1. The application says where it paints nothing: with its glass request, the rectangle under the glass its window
   leaves empty. The compositor gives the window's surface that rectangle as a hole, as it does for shadows, so a move
   changes only the ring the hole does not share; the grid capture with a margin then keeps everything inside it.
   A wrong rectangle can only spoil the look of that application's own glass, which reads only its own window.
2. The compositor finds the empty rectangle itself, from the alpha of a shared-memory buffer where a commit damaged
   it. No change to the protocol; a scan of each damaged region on commit, and nothing for buffers on the GPU.

Either is a change of its own, then the grid capture and its goldens. No golden changes in this round, so none is
listed for re-recording and there is no before and after sheet.

Goldens (`ci/goldens.sh`): the overview (561 027 pixels over tolerance, the dimmed wallpaper behind it lighter) and
the launcher's structure (its panel 176 to 177 rows) fail, and fail the same way at `2b119fa4`; no golden shows a
window drawn below half its size. Nothing is re-recorded.

**Checks of our own output** (headless, GPU tier):

- A window of one-pixel stripes shown at 0.27 of its size in the overview, against its box-filtered picture computed in
  Rust: mean difference 6.0, worst 10.7 of 255 (plain filtered lookups: 37.5 and 112). While it moves into place, the
  stripes stray 0.5 of 255 from their average on every frame (plain lookups: 63), so nothing shimmers
  (`headless::a_window_shown_well_below_its_size_matches_its_box_filtered_picture_and_does_not_shimmer`). The copy is
  made once and kept while the window does not commit.
- A scale 2 window on a scale 1.5 output against its box-filtered picture: mean 0.29, worst 0.5 of 255 (plain lookups:
  9.4 and 20.8) (`headless::a_scale_2_window_on_a_scale_1_5_output_matches_its_box_filtered_picture`).
- A grey gradient (64 to 96 over the output's height) under a large glass panel: down the panel the value steps one
  level at a time, the longest run 35 rows where its 3 levels over 90 rows give 30 a level; the bare wall beside it
  runs 16 rows over 6 levels (`backdrop_tone::a_gradient_under_glass_shows_no_bands`). The check passes without
  dithering: the glass adds no bands of its own. The glass halves the gradient's contrast, so its 8-bit steps are
  twice as tall as the wall's; dithering is what breaks those up.
- One snapshot a minimize and one a restore, whatever the frames in between
  (`headless::a_minimize_captures_the_window_once_and_a_restore_once_more`); a steady frame copies no cursor image
  (`scene_cost::a_steady_frame_copies_no_cursor_image`), an overlay's signature allocates nothing
  (`scene_cost::an_overlay_signature_allocates_nothing`) and a frame that changes nothing copies no wallpaper path
  (`wallpaper::tests::a_frame_that_changes_nothing_copies_no_wallpaper_path`).

### Step 10: retire the duplicates

**The backdrop passes generated.** Pixels, on the target machine's GPU: a headless shot of Settings alone and of Files
alone, 1920 x 1200 at scale 1.5, dark and light, drawn by the compositor with the hand-written passes and with the
generated ones (the same build otherwise): no pixel differs by more than 2 of 255 outside the menu bar's clock, whose
minute changed between the shots. The server's material, glass, tone and dither tests and the glass crate's GPU
tests (its colour pass within one level of the CPU chain, PSNR 60 dB and more) pass unchanged. The blur's 8-bit
dither is now a flag of `PassParams` (`flags[0]`) set by the compositor when its levels have 8 bits, instead of a
second program text; the levels have half floats on the target machine, so the flag is off there, as the dither
was.

The assembled WGSL leaves out every function its entry points do not reach: the Vulkan driver of the target machine
crashed (a segmentation fault in its SPIR-V compiler, inside pipeline creation) on the backdrop passes with the whole
shared module in them, and no longer does once the unused gradient function, which takes arrays by value, is left out.
A Vulkan backend (step 12) compiling `cupertino-gpu`'s own SPIR-V, which keeps every function, will meet the same.

**The region programs generated, `wgpu` gone.** The material, glass, vibrant and mesh programs are generated from
WGSL; the compositor links them and binds by the generated names, and the headless glass renderer draws the same
material and glass programs on `cupertino-gpu` (Cargo.lock: 378 lines and every `wgpu` crate gone). Material and
glass are placed by one block (`ops::Placement`: the projection, the quad, the dither flag), so the compositor's
dithered and plain variants are one program each instead of two texts. The GLSL writer named a second local `gl` of
the glass program `gl_1`, which GLSL reserves: the target machine's driver refused it and the compositor fell back
to plain fills with no error a test saw; the generator now refuses such a name. Pixels: the
server's headless goldens pass unchanged in the pinned image, and every preview scenario, shot headless on the target
machine's GPU with a build of the commit before the step and with this one (118 shots, 1, 1.5 and 2 times, light and
dark), is unchanged except where two runs of the same build differ too: which of two windows started together holds
the focus in the overview scenarios (the set of changed shots moves between runs), and the caret's blink in the
launcher.

**The rectangle passes generated.** The shadow, outline, rounded rectangle and wallpaper are drawn by three generated
passes on a device borrowed from the compositor's context (`render/passes.rs`), into the framebuffer the frame has
bound (`Device::begin_owner_pass`), one instance per damage rectangle. Pixels: the server's headless goldens pass
unchanged in the pinned image (4 of 4); the preview scenarios, shot headless on the target machine's GPU with a
release build of `main` before the change and with this one (118 shots), are unchanged except the overview scenarios
(the focus, as above), the dead-key caret at 1.5 and 2 times (18 to 105 pixels; unchanged, 6 of 6, when shot again)
and the menu bar's clock in one launcher shot. The dither and coverage tests hold the generated shadow and rounded
rectangle with their old bounds.

A program the driver refuses is now an error in the log, once, with the program's name and the driver's message, and
a GPU-tier test (`window_popup_backdrop::every_compositor_program_links_on_the_driver_and_the_glass_and_material_passes_draw`)
links every compositor program, generated and hand-written, dithered and not, on the machine's driver, and checks
that a window's sidebar glass and content material were drawn by their passes (`MaterialStats::glass_composites`
and `composites`), not by the fallback fill.

Cost (`scripts/dev/bench`, 3840 x 2160 at 1.5, 3 runs, the same machine, `main` before the change against this one):
compose work p50 1.12 to 1.38 ms and p99 1.98 to 2.24 ms in the hover scenario, p50 1.45 to 1.57 ms and p99 2.48 to
3.38 ms in the drag, compose CPU p99 2.40 to 2.97 ms in the drag (over its 2 ms budget before the change too). Each
pass draw enters and leaves the borrowed device, which reads and puts back the context's state (about 80 queries) for
every shadow, outline and rounded rectangle element; the per-run p50 is 0.2 to 0.3 ms higher in every run. Drawing
all of a frame's rectangle passes inside one enter and leave needs the frame's elements drawn by the device, not one
by one by the renderer: that is step 8's title bar work, which moves the same elements, and is measured again there.

### Step 11: output timing and depth

**The late start.** On the DRM backend, after a vblank whose flip reported the driver's timestamp (the
`flip-timestamp` branch), the display's next frame does not start at once but at

    vblank + min(period - (recent maximum render time + margin), period / 2), and at once when that is not positive

with the recent maximum the longest of the last 60 frames' render times (from the start of drawing to the flip being
queued: one second at 60 Hz) and a margin of 2 ms (the GPU's work after the flip is queued, the commit and the loop's
wake-up). A flip shown after the vblank its frame was drawn for counts as a whole period in the window, which turns
the delay off until it leaves the window. Without render times yet, on the `own-clock` branch, with a test's fixed
time, on the headless and nested backends, and with `CUPERTINO_LATE_LATCH=0` (or `off`), frames start at once as
before. At 60 Hz with frames under 6.3 ms the delay is the half period, 8.33 ms: an input in the first half of a
period is drawn into the frame flipped at the period's end instead of the one after it. Tests: `latch::tests` (the
delay, the half-period cap, the window, a miss, the branch and the switch) and
`schedule::tests::a_held_frame_waits_for_its_late_start_and_the_next_cycle_ends_the_hold`.

**What the DRM trace adds.** `missed` now also counts a flip shown after the vblank its frame was drawn for (before,
only flips so late their clients were called back without them); `start-delay output=… delay_us=…` when the delay
changes by a millisecond or more; `pointer-to-flip output=… latency_us=…` beside `input-to-flip`, from pointer motion
alone; `count flips=… missed_total=…` every 3600 flips, so a summary has the rate of missed refreshes. The server
logs at start whether the late start is on (`late_latch=`) and the depth asked for, and for each display the format
its primary plane got (`output initialised … format=… bits=…`). `scripts/dev/drm-trace-summary` reads a log (its
tests: `python3 -I scripts/dev/test_drm_trace_summary.py`).

**Turning 10 bits on.** OWNER DECISION, after the step 0 probe (`tools/gpu-probes/depth.sh`, above). To turn it on,
add `"output_depth": 10` to `~/.config/cupertino/compositor.json` (a JSON object; keep the other keys) and start a new
session; the server log's `output initialised` line then says `format=AB30 bits=10` (or the 8-bit format, where the
plane has no 10-bit one). To turn it off, remove the key or set it to 8. If the probe shows a visible gain, the
default changes in `crates/cupertino-server/src/output_depth.rs` (`Depth::from_setting`) with the probe's result
recorded here. Screenshots, screen sharing and recordings draw the scene into 8-bit targets of their own and read
those back, so they are the same at either depth
(`output_depth::tests::every_capture_reads_back_8_bits_whatever_the_plane_s_depth` pins every read back in the server
to an 8-bit format). A display on a second card shows the render
GPU's 8-bit copy whatever its plane's depth.

**The step 11 gate.** BLOCKED (owner action): it needs the desktop session, two hours of it. Install a package built
from `main` (`scripts/package-local.sh`, then `pacman -U`), log out, switch to a text console (Ctrl+Alt+F3), log in,
and from the checkout run:

```sh
CUPERTINO_DRM_TRACE=1 CUPERTINO_SESSION_LIMIT=3600 CUPERTINO_LATE_LATCH=0 cupertino-session; CUPERTINO_DRM_TRACE=1 CUPERTINO_SESSION_LIMIT=3600 cupertino-session; scripts/dev/drm-trace-summary --baseline ~/.local/state/cupertino/server.log.1 ~/.local/state/cupertino/server.log
```

The first session is the baseline (frames start at once), the second has the late start; each ends after an hour.
Use the desktop the same way in both (move the pointer, drag and resize windows, scroll in Files, type). The summary
prints each session's branch, planes, flips, missed refreshes per thousand flips, start delays, and input-to-flip and
pointer-to-flip p50, p95 and p99, then `pass` when the second session's input-to-flip p50 and p95 are below the
baseline's and it missed no more refreshes per thousand flips; record its output here. If the branch is `own-clock`,
the late start never engaged and the gate cannot pass on this driver.

### Step 4: retained layers

Taken on the target machine, release builds, CPU renderer, `scripts/dev/bench --size 3840x2160 --scale S --scenario
files-full-scroll --runs 3`, other checkouts building beside it. Before is `6e9ab377`; after is `e4c989bc`. The
step's commits are `96c21721`, `0059c4cb`, `89f790a7`, `e4c989bc`, `a6d1ec27` (a layer new to a frame, which no scroll
frame has) and `1a070506`. Medians of the three runs; the phases are the Files client's own.

| Files two-finger scroll, client frame ms | Scale 1 before | Scale 1 after | Scale 1.5 before | Scale 1.5 after |
|---|---|---|---|---|
| Frames drawn in the scroll | 40 | 94 | 32 | 98 |
| Frame p50 / p99 | 60.8 / 79.3 | 29.5 / 41.2 | 77.0 / 126.4 | 24.6 / 44.1 |
| Update + view + layout p99 | 11.4 | 11.3 | 21.0 | 6.8 |
| Paint p50 / p99 | 4.4 / 5.5 | 4.3 / 6.5 | 1.9 / 4.6 | 1.8 / 2.8 |
| Raster p50 / p99 | 36.5 / 46.4 | 7.2 / 10.1 | 50.6 / 76.3 | 9.2 / 13.5 |
| Copy p50 / p99 | 12.2 / 28.6 | 8.8 / 19.9 | 17.1 / 47.3 | 9.7 / 25.5 |

The budget (p99 under 8 ms) is not met. What a scroll frame costs now:

- Painting into the layer: the rows that came into view and what changed in the layer, nothing else. The renderer's
  frame statistics (`layer_pixels`) count about 80 000 pixels a frame at scale 1 (a dozen rows of the 3617-pixel
  viewport and the strip under the scroll bars), against 7.5 million before the scroll bars moved beside the layer.
- Compositing the layer: the whole viewport is laid over every frame the layer moves, about 33 MB at scale 1, and the
  background under it is filled first. Most of the raster that is left.
- The copy into the shared-memory buffer: the damage of a scroll is the whole moved viewport, so the copy stays at
  about 9 ms p50 however little is painted.
- Paint: the toolkit builds and compares the whole paint tree of about 5000 nodes every frame. An early cut of the
  damage tracker searched every node for each node's layer; with the lookup by node (`e4c989bc`) the paint phase at
  scale 1 is back from 11.2 to 4.3 ms p50.
- View and layout: the view is made again for every frame that shows other rows. More frames are drawn in the same
  scroll (94 against 40), so more of them pay for it.

The layer's pixels are those of the content drawn in place: kept rows are the bytes of a layer drawn whole at the same
offset (`a_scrolled_layer_has_the_bytes_of_one_drawn_whole_at_its_offset`, seeds 0 to 3, scales 1, 1.5 and 2, plain and
rounded clips, effects and a translucent bar over the layer's edge). Pixels at fractional clip edges are painted in
place through the frame, as without layers. Inside, the composite rounds once where painting in place rounds node by
node, within the goldens' 2 of 255 (`a_kept_layer_draws_the_picture_of_its_content_drawn_in_place`). No golden is
re-recorded.

#### Second round: copy, composite, kept groups and rows

The same scenario and machine, release builds, other agents building and benchmarking beside it; the load average is
recorded at the start and end of each scale's three runs. Before is `b4774447b` (load 6.1 then 2.9 and 2.5); after is
`d0016e41c`, measured twice (loads 6.2 to 4.9, then 7.3 to 3.6). The round's commits are `ea4669d77` (the copy
converts each pixel once), `8ac300a25` (the scroll layer laid over its background in one pass, opaque runs copied),
`0db51815d` (the server suspends windows nobody sees), `e06873ab9` (icons made only for built cells), `21f6b794a`
(`Layer::Transform`), `a0605b3e3` (kept group images), `764d806f4` (`Element::kept`) and `d0016e41c` (the grid keeps
its cells). Medians of the three runs; the two after columns are the two measurements.

| Files two-finger scroll, client frame ms | Scale 1 before | Scale 1 after | Scale 1.5 before | Scale 1.5 after |
|---|---|---|---|---|
| Frames drawn in the scroll | 92 | 88 / 117 | 89 | 93 / 96 |
| Frame p50 / p99 | 30.1 / 41.2 | 19.0 / 38.0, 19.5 / 36.7 | 18.5 / 42.8 | 13.9 / 36.9, 13.1 / 37.2 |
| View p50 / p99 | 3.7 / 6.3 | 1.4 / 2.8, 1.4 / 2.4 | 0.0 / 4.5 | 0.0 / 2.5, 0.0 / 2.3 |
| Layout p50 / p99 | 3.3 / 5.9 | 3.1 / 5.7, 3.1 / 6.2 | 0.6 / 2.0 | 0.7 / 2.8, 0.6 / 1.6 |
| Paint p50 / p99 | 4.3 / 6.2 | 4.7 / 12.0, 4.4 / 6.7 | 1.8 / 2.9 | 1.8 / 2.7, 1.7 / 2.7 |
| Raster p50 / p99 | 7.5 / 10.3 | 7.2 / 10.6, 5.9 / 10.0 | 9.1 / 16.6 | 7.5 / 11.2, 7.1 / 11.6 |
| Copy p50 / p99 | 8.9 / 20.2 | 4.6 / 20.6, 3.2 / 20.3 | 7.1 / 21.0 | 3.9 / 20.9, 3.5 / 20.4 |

The spread between runs is wide under this load (frames drawn 88 against 117, the paint p99 12.0 against 6.7); the
view, raster and copy p50 fall in both. What a scroll frame costs now, at scale 1, timed inside the client on a build
with temporary timers (not committed):

- The copy: p50 is down to the moved viewport converted once. Its p99, about 20 ms, is the frames that convert a
  whole buffer: a buffer new to the window or one whose pixels are unknown. It alone is over the budget.
- View: the application's view 1.6 ms p50 and the reconcile 2.5 ms p50 (7.4 p99) before the kept cells; after them
  the whole view is 1.4 ms p50.
- Layout: the cached solve and the placement of every node 2.1 ms p50 (5.7 p99), the window's placements 0.5 and its
  semantics 0.6.
- Paint: the walk over every node 2.0 ms p50 (5.2 p99) and the comparison with the last frame's paint tree 1.0 ms
  (3.1 p99).
- Raster: the layer laid over the viewport and the subtrees around it painted again where the scroll damaged them
  (1.4 ms p50, 7.7 p99).

No pixel changed: the composite in one pass has the bytes of the background written and the layer laid over it, the
copy converts the same pixels, a kept group is laid over only where it was painted and at the same subtree, and a cell
kept unbuilt is the node a build would have left unchanged. No golden is re-recorded.

#### Third round: kept paint and node maps

The same scenario and machine, release builds, three runs per configuration, other agents building and testing beside
it; the load average is recorded before and after each configuration's runs. Before is `d6fd6eaa0`; after is
`0059ca5ec`, measured twice. The round's commits are `706cfb6d5` (maps keyed by node id hash it with a rotate and one
multiply) and `0059ca5ec` (a kept subtree that did not change is laid into the paint unwalked). Medians of the three
runs, the frame p99's range across them in brackets; the two after values are the two measurements.

| Files two-finger scroll, client frame ms | Before | After |
|---|---|---|
| CPU, scale 1: load | 11.5 to 9.9 | 1.5 to 13.0; 8.0 to 8.7 |
| CPU, scale 1: frames, frame p50, p99 | 116, 19.6, 36.4 (34.9–37.5) | 102, 22.4, 45.9 (39.3–67.5); 84, 17.7, 62.6 (49.9–66.4) |
| CPU, scale 1: layout, paint p50 | 3.2, 4.4 | 2.7, 3.1; 1.5, 3.2 |
| CPU, scale 1: raster, copy p99 | 8.8, 21.0 | 17.8, 21.3; 17.4, 26.4 |
| GPU, scale 1: load | 9.9 to 4.0 | 13.0 to 17.5; 8.7 to 8.1 |
| GPU, scale 1: frames, frame p50, p99 | 183, 5.3, 9.4 (8.4–10.6) | 165, 5.6, 23.7 (21.4–57.6); 172, 6.1, 13.4 (11.4–17.8) |
| GPU, scale 1: layout, paint p50 | 1.0, 3.3 | 1.3, 2.9; 1.4, 3.1 |
| CPU, scale 1.5: load | 4.0 to 4.5 | 17.5 to 13.8; 8.1 to 6.9 |
| CPU, scale 1.5: frames, frame p50, p99 | 90, 15.7, 40.3 (36.6–41.0) | 90, 17.8, 44.9 (41.3–63.4); 107, 18.7, 46.5 (42.6–64.8) |
| CPU, scale 1.5: layout, paint p50 | 0.8, 1.9 | 1.2, 1.2; 1.2, 1.3 |
| CPU, scale 1.5: raster, copy p99 | 12.7, 22.3 | 14.3, 23.4; 19.8, 24.8 |
| GPU, scale 1.5: load | 4.5 to 11.0 | 13.8 to 11.3; 6.9 to 7.3 |
| GPU, scale 1.5: frames, frame p50, p99 | 155, 6.6, 23.0 (21.6–35.3) | 176, 4.0, 16.9 (14.4–19.4); 182, 4.1, 11.2 (10.8–14.8) |
| GPU, scale 1.5: layout, paint p50 | 1.2, 3.0 | 0.6, 1.2; 0.6, 1.1 |

Paint p50 falls on both paths at both scales (by a third at scale 1 on the CPU, by more than half at 1.5); the p99s
follow the load, which was heavier for the after runs, and the CPU path's raster and copy p99, which this round did
not touch, rise with it. Timed inside the client at scale 1 on a build with temporary timers (not committed), headless
Files in its icon view over 3000 items, 20-point steps of a two-finger scroll, on the thread's CPU clock at a load of
about 4: the cached solve 0.8 ms p50, the window's placements 0.5, its semantics 0.6, the paint walk 0.95 (1.20 with
the kept runs turned off) and the comparison with the last frame's paint tree 0.7. A frame walks about 60 of the 3700
nodes and lays the rest in as kept runs. At a load of about 30, before the id hash, the same frames took 2.9 ms to
solve, 1.8 to place, 1.5 for semantics and 2.0 to compare, and the walk 3.6 ms without kept runs and 1.8 with them.

No pixel changed: a kept run is laid in only where the subtree, its ground and its order in the paint are those it was
painted with, and each node of it is built by the same function a walk uses; the test compares the list, the own
pixels and the blank region with a new painter's after every scroll. No golden is re-recorded.

## Decisions taken

- Shared maths and one look freeze come first; goldens are not re-recorded per step afterwards.
- Retained layers come before the GPU renderer.
- Surface classes: small, rare, must-draw and static surfaces stay on the CPU.
- Own GPU layer shaped for Vulkan; GLES first; `wgpu` not used for windows (reviewed before step 12).
- WGSL is the single source of our own programs; GLSL ES and SPIR-V are generated; texture programs through Smithay's
  interface stay GLSL ES 1.00.
- The CPU reference computes what the GPU computes.
- No implicit synchronisation; explicit sync or a client-side fence wait.
- Vulkan steps are gated on measured need.

## Open questions

- Does a program binary cache avoid the compiler's start-up memory and time? (step 0) Answered: yes. The first
  program of a fresh process costs 32.5 ms and 46.7 MiB compiled, 4.95 ms and 6.2 MiB loaded from a saved binary,
  and 0.4 ms and 0.7 MiB when the driver's own disk cache holds it ("Measurements").
- Is the panel truly 10-bit, and is a 10-bit plane visibly better than the driver's own dithering? (step 0) The
  plane takes 10-bit formats; the look needs the owner's run of `tools/gpu-probes/depth.sh` ("Measurements").
- Are the driver's flip timestamps real? (step 0) The trace that answers it is in place; it needs the owner's
  traced session ("Measurements").
- Does the dmabuf slice run on llvmpipe inside the goldens container? (step 3) Answered: only with `/dev/udmabuf`
  passed in; without it llvmpipe has no texture export and the slice falls back to shared memory ("Measurements").
