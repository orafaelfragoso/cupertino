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
      typing (Files wheel scroll exists).
- [x] Toolkit processes write per-phase frame timings (update, view, layout, paint, raster, copy, commit) to a file
      named by an environment variable; bench collects them per process.
- [x] Bench counts client frames longer than one refresh, per process, besides the compositor's missed refreshes.
- [x] `scripts/dev/measure` reports private dirty memory besides Pss and Rss.
- [x] Counters in the frame statistics for decoration and HUD rasters and uploads, and for blur passes.
- [x] The DRM backend logs, on request, which presentation-time branch it takes (flip timestamp or own clock), missed
      flips, plane assignment (cursor plane, direct scanout) and input-to-flip latency.
- [x] Every timer that wakes a process on an idle desktop recorded with its reason; each is kept (and listed under
      "Measurements") or removed.

Baseline:

- [ ] Every bench scenario at scale 1.5 and 2: frame time p50/p99, missed refreshes, slow client frames, compositor CPU
      and GPU time, per-phase client split, per-process CPU and private memory.
- [ ] Each scenario run five times; the run-to-run spread of every figure recorded as its noise.
- [ ] One measured table for Files replacing the two figures in the docs (full repaint at 3840 x 2160, wheel scroll
      p99 at scales 1, 1.5 and 2).
- [ ] Presentation-time branch and plane assignment on the target driver recorded.

Probes (small programs outside the product; results recorded here):

- [x] Context creation, first program, later programs and teardown: time and private memory, from a Rust binary.
- [x] Render-target texture cost in total GPU memory, the first one in a process and later ones.
- [x] A saved program binary as the first program of a fresh process: time and memory against compiling.
- [x] Native fence soak: resident-memory slope with and without the fence over 10 million frames.
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
  presentation-time branch when it changes (`flip timestamp` or `own clock`), every missed flip with how many refresh
  periods late, the plane assignment when it changes (direct scanout, cursor plane, overlays) and the input-to-flip
  latency. Without the variable nothing is kept.
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
samples where it sleeps (the call and its timeout) and reads each timerfd's period and count. Headless, 1280 x 800,
Files and Settings open, nobody touching the desktop for 60 s. Before this step's changes (30 s): every process with a
bus connection woke twice a second in its pool thread (120 a minute each, 10 processes), the server's main loop woke
242 times a minute, Settings drew a frame a second.

Removed:

- The pool thread that bus connections run their blocking calls on stayed for good after its first call and woke twice
  a second, in every process with a bus connection: its 1.7.0 release never ends an idle thread. Held at 1.6.2, which
  ends it after half a second without work (`dbus::tests::the_blocking_pool_ends_its_threads_once_idle`).
- Hung-application pings: the server pinged every client with a mapped window once a second, and each woke to answer.
  Pings now go out only after input since the last one (the first input after a rest pings at once), so a desktop
  nobody touches pings no one (`hung::tests`, `hung_apps::a_desktop_nobody_uses_pings_no_one_and_the_first_input_pings_at_once`).
- The main loops' one-second backstop (session and headless): every time-driven piece of work names its deadline (the
  tick's calls are audited), so the backstop is now a minute (`a_session_at_rest_sleeps_and_wakes_its_loop_once_a_minute_at_most`).
- Settings drew a frame every second on a pane of readings (Account, 15 frames in 15 s): its rows all share one
  control, so they shared one key and were mounted again at every refresh. Each row now has a key of its own and a
  refresh that changes no reading draws nothing (`the_rows_of_a_section_have_keys_of_their_own_though_readings_share_their_control`,
  `a_turn_that_changes_no_reading_presents_nothing_on_a_pane_of_readings`).

Kept, with what they cost per minute after the changes:

| Who | Every | Why | Wakeups a minute |
|---|---|---|---|
| Settings, while its window is open | 1 s (100 ms while the input level meter shows) | re-reads the shown pane's live readings: some services send no change signal | Settings about 1200 over its threads, the server about 250 (its bus threads, and its file watches as Settings reads watched files), the bus daemon about 120; headless runs add a system bus connect attempt a pass, as there is none |
| menu bar clock | at each minute | the clock; it reads do-not-disturb and the control panel's state with it | menu bar 22, notifications 15, control panel 2, bus daemon 9 |
| session and headless main loop | 60 s | backstop for a deadline left unnamed | server 1 |

Without Settings open the whole desktop wakes about 50 times a minute, all of it the clock's minute and the backstop.
Three timerfds stay armed without firing in a minute (one each in the server, the menu bar and Settings): long
deadlines, not periods.

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
