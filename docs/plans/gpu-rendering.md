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
- [ ] CPU renderer: rectangle, rounded-rectangle, stroke and clip coverage from the distance function; nested clips
      multiply coverage.
- [ ] CPU renderer: blur and shadows from the shared kernel.
- [ ] CPU renderer: gradients per pixel from the shared rule.
- [ ] CPU renderer: scroll offsets rounded to device pixels when drawn.
- [ ] `cupertino-decorations` shapes (title bar, window buttons) from the shared distance function, so the later move
      to GPU pipelines changes no pixel.
- [x] Compositor dithering from the shared definition (blur, material, glass and shadow outputs), tested against the
      vectors. Off until the look freeze (see "The definitions and where the renderers disagree").
- [ ] Before/after sheet of every golden at scales 1, 1.5 and 2, reviewed by the owner.
- [ ] One re-recording of every golden.

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

Dithering changes pixels, so it is behind the look-freeze switch: `cupertino_raster_math::dither::BY_DEFAULT` is
`false`, and the compositor compiles its blur (into 8-bit levels only), material, glass and shadow programs without it
(byte-identical sources) unless `CUPERTINO_DITHER=1` is set. The freeze turns `BY_DEFAULT` on with the one re-recording
of the goldens. The GL tests (`dither::` in the server's suite, slow tier) draw the shader's matrix and dithered stores
and the dithered shadow against the definition.

Gate:

- [ ] The owner approved the look, the goldens are re-recorded, and no bench scenario's client CPU p99 is more than
      10% above its baseline.

## Step 2: compositor wins

- [ ] Non-glass materials use the overlap-limited backdrop generation glass uses.
- [ ] The minimize effect reuses one captured texture.
- [ ] Per-frame waste removed: HUD signature string, cursor image clone, wallpaper path bookkeeping.
- [ ] Reduced copies per window for windows drawn below half their size (overview, switcher, minimize, zoom), as a
      hand-written texture program with the external-sampler variant.
- [ ] A better minification filter for integer-scale clients shown at a fractional scale.
- [ ] Objective checks: a reduced window against a box-filtered reference (shimmer); run-length check of a captured
      gradient under glass (banding).

Gate:

- [ ] Window drag over glass meets its budget with fewer blur passes than the baseline; the shimmer and banding
      checks pass.

## Step 3: vertical slice of the GPU path

The risk is the handoff, so it is proven first with the smallest renderer possible.

- [ ] The headless backend offers `linux-drm-syncobj-v1` when its device has a render node, with a flag to withhold it.
- [ ] A test client renders solid rectangles with GLES into GBM buffer objects on the feedback device, with the
      offered modifiers, never adding linear on a hardware driver.
- [ ] It attaches them through `zwp_linux_dmabuf_v1` v4.
- [ ] Acquire through native fence → syncobj timeline point; release waited through a syncobj eventfd.
- [ ] The syncobj surface object is destroyed before a shared-memory attach and re-created on return to dmabuf.
- [ ] Without the protocol: wait for the own fence before commit; reuse after release plus one frame.
- [ ] The client counts missed refreshes from presentation-feedback sequence gaps.
- [ ] Overdraw test: many layers of drawing ending in one flat colour; every capture must be uniform.
- [ ] The compositor imports the client's buffers with the driver's default modifier (recorded).
- [ ] Probe: the slice on llvmpipe in the goldens image, allocating by texture export; result recorded.
- [ ] Runs in a headless session and in the live session on the target machine.

Gate:

- [ ] One hour in the live session on the target machine beside a `wl_shm` control client of the same size: no more
      missed refreshes than the control, and every overdraw capture uniform.
- [ ] The slice's offscreen loop of 10 million frames meets the fence budget.

## Step 4: retained layers in the toolkit

Works with the CPU renderer; no GPU needed.

- [ ] The paint tree marks layer boundaries: scroll content, animated subtrees, opacity groups.
- [ ] The CPU renderer keeps layers as images between frames and composites them.
- [ ] Scrolling moves a layer and paints only the exposed strip.
- [ ] Opacity and transform animations change layer properties, not pixels.
- [ ] Minimized and fully covered windows drop their layers.
- [ ] Test: a scissored repaint equals a full repaint, with effects crossing the damage edge, at scales 1, 1.5 and 2.
- [ ] Incremental view and layout for the Files listing: step 0 found update, view and layout above 4 ms in the worst
      frame of a full-screen scroll of 3000 files (5.5–6.1 ms at scales 1 and 1.5; 0.6–1.2 ms at p50).

Gate:

- [ ] Files two-finger scroll meets its budget at scale 1 and 1.5 on the CPU path, and paint and raster of a scroll frame
      cover only the exposed strip.

## Step 5: `cupertino-gpu` and the shader toolchain

- [ ] API: devices, buffers, textures with external memory, immutable pipelines, explicit passes, bind groups,
      fences; borrowed-context mode with a state-restore contract.
- [ ] GLES backend over EGL, with EGL and GBM loaded at run time.
- [ ] Allocators: GBM buffer objects; texture export (`EGL_MESA_image_dma_buf_export`) where there is no GBM.
- [ ] Robustness and purge attributes on every context; reset and purge detection.
- [ ] Shader build step: shared module concatenated, `naga` generating GLSL ES 3.00 and SPIR-V, coordinate adjustment
      off, blocks and samplers bound by name, uniform blocks only, no `mat2x2` in blocks.
- [ ] Generated shaders committed; a test regenerates them and fails on a difference.
- [ ] Shaders tested against the shared definitions' vectors.
- [ ] Atlases (8-bit coverage, RGBA colour glyphs, path masks) with eviction and caps; pooled render targets with caps.
- [ ] Tests per backend: pipelines, orientation, atlas eviction, pool reuse, export and import, fences, context loss.

Gate:

- [ ] CI: the crate's tests pass on llvmpipe.
- [ ] Local tier: the crate's tests pass on the NVIDIA GPU.

## Step 6: the GPU renderer and presenter

`GpuRenderer` implements `UiRenderer` over layers. Each primitive lands with its golden subset passing on both
renderers:

- [ ] Rectangles and rounded rectangles (fill and stroke).
- [ ] Gradients.
- [ ] Glyph runs.
- [ ] Images and icons.
- [ ] Path masks.
- [ ] Clips: analytic, nested by multiplied coverage, path clips through the mask atlas, stencil only for
      pixel-aligned rectangles.
- [ ] Layers with opacity.
- [ ] Blur, drop shadow, inner shadow.

Presenter and process:

- [ ] dmabuf presenter in `cupertino-ui-wayland` with the step 3 synchronisation, size-stepped allocation for live
      resize, two buffers with release points and three without.
- [ ] Fallback rule in full: `CUPERTINO_RENDERER` (`gpu` overriding software and missing feedback), missing libraries,
      three losses, purges not counted, per-surface fallback to shared memory on import or allocation failure.
- [ ] "Buffers lost" after a reset or purge: protocol version bump, contract entry in
      `docs/architecture/contracts.yaml`, compositor event, client repaint, tests.
- [ ] First frame on the CPU while the context and programs warm on a worker thread; after the switch the CPU
      renderer's frame, layers and caches are freed.
- [ ] `cupertino-diagnose` and the frame statistics report the renderer per process.
- [ ] Local GPU tier: dmabuf import, syncobj, injected loss and purge, CPU↔GPU switch, failed-import fallback, resize
      storm, fence soak, goldens on the NVIDIA GPU.
- [ ] Parity at scales 1, 1.5 and 2 with odd sizes.

Gate:

- [ ] Every golden passes on both renderers, the local tier passes on the target machine, client GPU time and the
      first-frame budget are met.

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

- [ ] Title bars and window buttons through the renderer's rectangle and glyph pipelines in borrowed-context mode.
- [ ] Keyboard HUDs the same way.
- [ ] The procedural cursor last, kept a cursor-plane element.

Gate:

- [ ] The hover scenario shows no CPU raster and no upload.

## Step 9: layers and animation in the compositor

- [ ] Protocol XML (new, versioned): fixed-point geometry atomic with the parent commit, glass attached to layers,
      blend roles, timed and spring animations on the compositor clock, retargeting with value and velocity,
      `finished` and `replaced`, input and semantics against target values, clipping, per-client limits, the
      late-content rule.
- [ ] Server global, with its entry in `docs/architecture/contracts.yaml`.
- [ ] Protocol tests (atomicity, retargeting, limits, late content, time scale and fixed time).
- [ ] Compositor animator.
- [ ] Toolkit support; shell and control transitions use it.
- [ ] Vibrant foreground as a blend-role layer.
- [ ] Lifted thumbs and lenses as glass in a raised layer.
- [ ] Pointer-lit highlight and variable blur.

Gate:

- [ ] Opening Control Center and the launcher costs the application no repaint per frame, and each material effect is
      reviewed by eye in both appearances.

## Step 10: retire the duplicates

- [ ] Shadow, rounded-rectangle and wallpaper passes move to borrowed-context passes generated from WGSL.
- [ ] The hand-written GLSL ES 3.00 programs of the glass crate and the compositor replaced by generated ones; the
      tests that pinned the copies pin the generated output.
- [ ] `wgpu` removed from `cupertino-glass`; the headless glass renderer and its tests run on `cupertino-gpu`.

Gate:

- [ ] One WGSL source for every program of ours; the texture programs compiled through Smithay's shader interface
      (clip, reduced copy, minification) stay GLSL ES 1.00 and are listed by name in the design.

## Step 11: output timing and depth on the target display

- [ ] Frame scheduling latches late only where presentation timestamps are real, from the recent maximum render time
      plus a margin, never past half a period.
- [ ] Pointer-to-screen latency reported by the DRM backend's log (the bench reports the headless figure).
- [ ] 10-bit primary plane (`Abgr2101010`) if the step 0 probe showed a visible gain; capture paths convert.

Gate:

- [ ] In the live session over an hour: input-to-flip latency below the baseline and no more missed flips than the
      baseline, from the DRM backend's log; the 10-bit decision recorded.

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
| Compositor frame during a drag over glass: under 4 ms GPU, under 2 ms CPU | GPU p99 2.07 ms (1.5) / 1.43 ms (2); CPU p50 1.32 / 1.60 ms, p99 2.38 / 1.97 ms | confirmed: the CPU p99 at 1.5 is over, which step 2 fixes; the GPU room is for the compositor's own drawing that steps 8 and 9 move there |
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
- Does the dmabuf slice run on llvmpipe inside the goldens container? (step 3)
