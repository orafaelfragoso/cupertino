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

- [ ] `scripts/dev/bench` takes size and scale as arguments.
- [ ] Bench scenarios: idle, window drag over glass, title-bar hover, live resize, Control Center open, launcher
      typing (Files wheel scroll exists).
- [ ] Toolkit processes write per-phase frame timings (update, view, layout, paint, raster, copy, commit) to a file
      named by an environment variable; bench collects them per process.
- [ ] Bench counts client frames longer than one refresh, per process, besides the compositor's missed refreshes.
- [ ] `scripts/dev/measure` reports private dirty memory besides Pss and Rss.
- [ ] Counters in the frame statistics for decoration and HUD rasters and uploads, and for blur passes.
- [ ] The DRM backend logs, on request, which presentation-time branch it takes (flip timestamp or own clock), missed
      flips, plane assignment (cursor plane, direct scanout) and input-to-flip latency.
- [ ] Every timer that wakes a process on an idle desktop recorded with its reason; each is kept (and listed under
      "Measurements") or removed.

Baseline:

- [ ] Every bench scenario at scale 1.5 and 2: frame time p50/p99, missed refreshes, slow client frames, compositor CPU
      and GPU time, per-phase client split, per-process CPU and private memory.
- [ ] Each scenario run five times; the run-to-run spread of every figure recorded as its noise.
- [ ] One measured table for Files replacing the two figures in the docs (full repaint at 3840 x 2160, wheel scroll
      p99 at scales 1, 1.5 and 2).
- [ ] Presentation-time branch and plane assignment on the target driver recorded.

Probes (small programs outside the product; results recorded here):

- [ ] Context creation, first program, later programs and teardown: time and private memory, from a Rust binary.
- [ ] Render-target texture cost in total GPU memory, the first one in a process and later ones.
- [ ] A saved program binary as the first program of a fresh process: time and memory against compiling.
- [ ] Native fence soak: resident-memory slope with and without the fence over 10 million frames.
- [ ] 10-bit (`Abgr2101010`) gradient on the primary plane with the driver's dithering off, against 8-bit.

Gate:

- [ ] All of the above recorded under "Measurements"; if update, view and layout exceed 4 ms in a Files scroll frame,
      step 4 gains the incremental view and layout item.
- [ ] Each budget in the design confirmed or revised once against the baseline, in a reviewed change, with the reason
      recorded.

## Step 1: shared maths and the look freeze

One owner review, one re-recording of the goldens.

- [ ] A graphics layer in `cupertino-layers` that the toolkit, the render layer and the compositor may depend on,
      and that depends on the foundation layer only; `layers.dot`, the architecture page and the layer tests updated.
- [ ] A crate in it with the definitions as plain Rust and test vectors: rectangle, rounded-rectangle and
      continuous-corner distance and coverage; the Gaussian kernel with its pyramid levels, edge rule, premultiplied
      alpha and intermediate precision; drop and inner shadow; gradient interpolation (at most eight stops);
      position-keyed dithering.
- [ ] The blur planner moves from `cupertino-glass` into it; the compositor uses it from there.
- [ ] CPU renderer: rectangle, rounded-rectangle, stroke and clip coverage from the distance function; nested clips
      multiply coverage.
- [ ] CPU renderer: blur and shadows from the shared kernel.
- [ ] CPU renderer: gradients per pixel from the shared rule.
- [ ] CPU renderer: scroll offsets rounded to device pixels when drawn.
- [ ] `cupertino-decorations` shapes (title bar, window buttons) from the shared distance function, so the later move
      to GPU pipelines changes no pixel.
- [ ] Compositor dithering from the shared definition (blur, material, glass and shadow outputs), tested against the
      vectors.
- [ ] Before/after sheet of every golden at scales 1, 1.5 and 2, reviewed by the owner.
- [ ] One re-recording of every golden.

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
- [ ] (Only if step 0 found update, view and layout above 4 ms in a scroll frame) incremental view and layout for the
      Files listing.

Gate:

- [ ] Files wheel scroll meets its budget at scale 1 and 1.5 on the CPU path, and paint and raster of a scroll frame
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

- [ ] Design page in the book, linked from `SUMMARY.md`.
- [ ] Rules in `docs/architecture/rendering.md`.
- [ ] Rendering paragraph of `docs/src/architecture.md`, the roadmap section and `docs/src/displays-and-gpus.md` point
      here.
- [ ] Each step that changes behaviour updates the pages it affects (`apps-on-kit.md`, `kit-getting-started.md`,
      `displays-and-gpus.md`, crate READMEs) in the same change.

## Measurements

Filled in as steps complete.

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

- Does a program binary cache avoid the compiler's start-up memory and time? (step 0)
- Is the panel truly 10-bit, and is a 10-bit plane visibly better than the driver's own dithering? (step 0)
- Are the driver's flip timestamps real? (step 0)
- Does the dmabuf slice run on llvmpipe inside the goldens container? (step 3)
