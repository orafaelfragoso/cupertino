# GPU rendering

How Cupertino draws, and how it moves the rest of its drawing onto the GPU. This page is the design; the
step-by-step plan and its checklist are in `docs/plans/gpu-rendering.md`, and the rules every change keeps are in
`docs/architecture/rendering.md`. Issue [#155](https://github.com/orafaelfragoso/cupertino/issues/155).

## Where we are

| Part | Draws with | Hands over |
|---|---|---|
| Compositor (`cupertino-server`) | OpenGL ES 3 through Smithay's `GlesRenderer`, plus our own GL passes | scanout through `DrmCompositor` |
| Glass and materials | the compositor on the GPU: backdrop captured at half scale, blur pyramid, every glass stage in one shader pass | n/a |
| Window shadows, outlines, the shell's rounded rectangles, the procedural wallpaper | the compositor: passes generated from WGSL, drawn by a `cupertino-gpu` device borrowed from its context into the framebuffer it has bound | n/a |
| Rounded clips of window content, reduced copies | the compositor, texture programs compiled through Smithay's shader interface | n/a |
| Title bars, window buttons, rims, keyboard HUDs | the compositor: each recorded as a list of draws, drawn by the borrowed device with the toolkit's coverage program into a texture of the renderer when its state changes (glyphs and button glyph paths kept in an atlas); painted on the CPU from the same list where the device cannot draw it | n/a |
| The procedural cursor | the compositor, rasterized on the CPU (tiny-skia), cached, uploaded on change: the cursor plane's buffer is filled on the CPU | memory buffers |
| Toolkit windows (shell processes, Files, Settings) | the CPU: tiny-skia, our own glyph rasterizer, resvg icons, box blur for layer effects; with `CUPERTINO_RENDERER=gpu`, the GPU renderer (`cupertino-ui-gpu`) after the first frames, its scroll layers kept as textures | `wl_shm`, three slots, damage rectangles, copied byte for byte as `abgr8888` where the compositor lists it, the next slot's pages made off the frame while a window draws frame after frame; on the GPU, dmabufs copied from the renderer's texture |
| The polkit agent, keyring prompter and portal pick overlay | the CPU, drawing with tiny-skia directly rather than through the toolkit renderer | `wl_shm` |

The material system is already a GPU system: a window asks the compositor for glass by shape and style, and the
compositor shades it against whatever is behind it. What is still on the CPU is the content of every window, the
compositor's own title bars and HUDs, and the material effects that need a window's own pixels (vibrant text and
symbols, lifted control thumbs, displacement).

Three facts decide most of the plan:

- **A toolkit frame has phases**: update, view, layout, paint, raster, copy, commit. A GPU speeds up raster and copy.
  Retained layers remove paint and raster of content that did not change. Update, view and layout are touched by
  neither, so they are measured first and handled on their own where they dominate.
- **A full-window repaint is expensive on the CPU**: Files filling a 3840 x 2160 output takes 57–83 ms (p50) to
  repaint whole and 34–59 ms a frame to scroll, most of it raster (the plan's step 0 measurements).
  Layers do not help a resize, which repaints everything; a GPU does.
- **A GPU context is expensive per process.** On the NVIDIA driver a GLES context plus its first program costs about
  70 MiB of private memory and about 90 ms before the first frame, and tearing it down returns almost none of it. Many
  shell processes draw small or rarely shown surfaces at 5–10 MiB today. (Single-run probe figures; step 0 of the plan
  measures them properly.)

## Design

### 1. Shared maths, frozen once

Every renderer, CPU or GPU, toolkit or compositor, computes coverage, blur, shadows, gradients and dithering from one
set of definitions: a crate of plain Rust functions with test vectors, in a graphics layer the toolkit, the render
layer and the compositor may depend on. The shaders implement the same definitions and are tested against the
vectors. The CPU renderer is changed once to use them (see section 7); that change, and the one re-recording of the
goldens it needs, is the look freeze. After it, a change re-records goldens only when it names the pixels it changes.

### 2. Retained layers before the GPU

The toolkit keeps what it drew as layers: images that survive between frames. A scrolled list moves its layer and
paints only the strip that came into view; an opening panel or a sliding card changes a transform or an opacity, not
pixels. Layers exist in both renderers, so they cut paint and raster cost on every machine without a GPU context.
Scroll offsets are rounded to device pixels when drawn, so a moved layer is pixel-identical to a repaint. Minimized and
fully covered windows drop their layers. A kept subtree that did not change is laid into the next paint as it was and
compared with the last frame's in one go, so a scroll walks and compares only the rows that changed.

### 3. Who draws on the GPU: classes of surface

| Class | Examples | Renderer |
|---|---|---|
| Small or rarely shown | OSD, banners, menu bar, polkit and keyring prompts, portal overlays and file chooser, screenshot toolbar and selection overlay | CPU, with layers |
| Must always draw | lock screen, login | CPU |
| Static and full-screen | the Files desktop instance | CPU |
| Large or continuously repainted | Files windows, Settings | GPU |
| Third-party toolkit applications | anything built on the toolkit or its C ABI | GPU once a window is larger than a quarter of its output or presents more than 30 frames a second for two seconds; CPU otherwise |
| Decided by measurement after layers | dock (and its overview), launcher, Control Center | whichever meets the budgets |

Glass is drawn by the compositor for every class, so a CPU surface keeps the full material look. A GPU process paints
its first frame on the CPU while its context and programs are prepared on a worker thread, then switches and frees the
CPU renderer's frame, layers and caches; a fallback rebuilds them. A hidden GPU process drops its pools and atlases and
keeps its context, because tearing a context down returns almost no memory.

### 4. Our own thin GPU layer, shaped for Vulkan

A new crate, `cupertino-gpu`, exposes what a 2D renderer needs: devices, buffers, textures with external memory,
immutable pipelines, explicit render passes, bind groups and fences. Its shape follows Vulkan so the second backend
is a translation, not a redesign. Backend one is OpenGL ES 3.0 over EGL (`glow` for GL, EGL and GBM through
`libloading`); backend two is Vulkan (`ash`). EGL, GBM and Vulkan are loaded at run time, so a machine without them
takes the fallback instead of failing to start. A borrowed-context mode runs `cupertino-gpu` passes inside the
compositor's GL context with a defined state-restore contract: between entering and leaving, the device saves and
puts back every piece of state it may change (the list is in the crate's `gles::state`), and draws with a vertex
array of its own. A borrowed device draws either into textures of its own or, in a pass begun with
`begin_owner_pass`, into the framebuffer its owner has bound, at the owner's viewport and in the owner's orientation:
its programs place their vertices through the projection the owner hands them, and its scissor regions are the
framebuffer's pixels as the owner counts them. That is how the compositor's own passes draw among Smithay's draws.
Within one frame of its owner the device reads the owner's state once: `Device::pause` puts back only the groups of
state the device's calls changed since entering (it notes them as it goes) without reading anything, and
`Device::resume` enters again on what the first enter read, so a frame of many passes costs one read of the state.
The owner keeps to its side: between passes it changes no framebuffer, viewport, scissor or blending, and sets its
own program, textures and buffers before each draw. A borrowed device also draws into a texture its owner made
(`Device::wrap_texture`), which stays the owner's.

Orientation is the same on every backend: a target's first row (the top of the image handed to the compositor) is at
clip y = -1 and fragment row 0, texture coordinate v = 0 is a texture's first row, and pixels are written and read
first row first. Nothing is flipped anywhere, because both GL and Vulkan already agree on that once the shader
generator's coordinate adjustment is off.

We do not use `wgpu` for windows: it hides buffer allocation, damage and explicit sync, which a toolkit that hands
buffers to its own compositor has to control, and it costs more per process than a fixed-pipeline renderer needs.
That changes if the display list grows toward general vector graphics, or if keeping two backends by hand costs more
than `wgpu`'s overhead; the decision is reviewed before the Vulkan backend is built.

### 5. Buffers and synchronisation without libwayland

The toolkit speaks Wayland in Rust and never hands EGL or Vulkan a `wl_display`. It allocates its own buffers on the
device the compositor names in dmabuf feedback, renders into them and attaches them through `zwp_linux_dmabuf_v1`
version 4. Two allocators exist: GBM buffer objects on a hardware device, and dmabuf export of a texture
(`EGL_MESA_image_dma_buf_export`) where there is no GBM, as with a software rasterizer. A buffer uses only a modifier
the feedback offers and its own driver can render to; linear is never added on a hardware driver. During a live
resize buffers are allocated in size steps and cropped with the viewport. A window keeps two buffers when release
points are available, three otherwise.

A buffer is the composite of the window's layers; its age decides which regions are composited again, as the
`wl_shm` slots do today.

Synchronisation is explicit:

1. After drawing, the client creates a native fence, flushes, imports the fence into a DRM syncobj on the feedback
   device's render node and transfers it to a timeline point; that point is the acquire point of
   `linux-drm-syncobj-v1`.
2. The client waits for the release point through a syncobj eventfd in its event loop before reusing the buffer.
3. The syncobj surface object is destroyed before a surface attaches shared memory (its first CPU frame, a fallback)
   and created again when it returns to dmabuf, because a commit without points on such a surface is a protocol error.
4. Without the syncobj protocol (other compositors, tests), the client waits for its own fence before it commits and
   reuses a buffer only after its release and one further frame. It never relies on implicit synchronisation, which
   the NVIDIA driver does not provide for dmabufs.

A surface whose dmabuf import or allocation fails falls back to `wl_shm` for that surface only.

### 6. One shader source

Our own programs are written once in WGSL. A build step (`cupertino-gpu-shaders`, a development binary) concatenates
the shared module (the definitions of section 1, `crates/cupertino-gpu/shaders/shared.wgsl`) into each shader and
`naga` generates GLSL ES 3.00 for the GLES backend and SPIR-V for Vulkan. It generates three sets: `cupertino-gpu`'s
programs, those of `cupertino-glass` that the compositor draws (`crates/cupertino-glass/shaders/`, where a line
`// include NAME.wgsl` puts a file of `shaders/include/` in place, so the backdrop passes share their block and
vertex stage); for the second set it also writes the assembled WGSL without the functions the program never reaches,
for the glass crate's own headless renderer; and the compositor's rectangle passes
(`crates/cupertino-server/shaders/passes/`, with an include directory of its own). The generated output is committed, so no
product binary builds `naga`; a test regenerates it and fails on a difference. The generator runs with
coordinate-space adjustment off, declares every block `std140` (GLSL ES 3.00 would leave the layout to the driver),
writes the GLSL name of each block and sampler by WGSL group and binding beside the code, since GLSL ES 3.00 has no
binding qualifiers, and refuses a program with anything but uniform blocks and textures or with a `mat2x2` in a
block. Each backend has an orientation test.

Texture programs compiled through Smithay's shader interface sample client buffers, including import-only ones through
`samplerExternalOES`, and that interface takes GLSL ES 1.00 only. They stay hand-written: the clip program, its
vibrant variant, the reduced copy and the minification filter. Every other program of ours is generated: the
backdrop passes (downsample, blur, colour, copy), the material region, the glass composite, the vibrant glyphs and
the mesh of the window effects, which the compositor links itself and the headless glass renderer draws on
`cupertino-gpu`, and the compositor's rectangle passes, which sample no client texture: the window shadow, the
rounded rectangle and the procedural wallpaper (a third set, `crates/cupertino-server/shaders/passes/`, sharing one
placement block and vertex stage), drawn by a device borrowed from the compositor's context into the framebuffer
Smithay has bound, one quad per damage rectangle. The CPU model in `cupertino-glass` stays as the model the glass
shader is tested against.

### 7. Same pixels on both paths

The CPU renderer stays as the standard the GPU renderer is tested against, and as the fallback, so it computes what
the GPU computes:

- coverage of rectangles, rounded rectangles, strokes and clips from the shared distance function, not from
  supersampled paths; nested clips multiply coverage;
- blur, drop shadow and inner shadow from one definition: the Gaussian kernel, its pyramid levels, transparent edges,
  premultiplied alpha and intermediate precision;
- gradients evaluated per pixel with at most eight stops and one interpolation rule;
- dithering keyed to the pixel position, never random, so damage repaints and goldens are stable;
- glyphs from our own rasterizer through an atlas, identical by construction, at quarter-pixel x phase and whole-pixel
  y;
- path coverage rasterized once on the CPU into a mask atlas; path clips through the mask atlas, the stencil only for
  pixel-aligned rectangles.

After the look freeze both renderers agree within the golden tolerance of 2/255 per channel, and no bench scenario's
client CPU p99 is more than 10% above its baseline.

### 8. The fallback rule

A process draws on the CPU when:

- `CUPERTINO_RENDERER=cpu` is set, or its surface class is a CPU class;
- the compositor offers no dmabuf feedback, or EGL, GBM or Vulkan cannot be loaded or initialised on the advertised
  device;
- the device is a software rasterizer;
- its context was lost three times within a minute.

`CUPERTINO_RENDERER=gpu` overrides the software and missing-feedback conditions (it takes the device from EGL), which
tests use. Contexts are created with the same robustness and purge-notification attributes as the compositor's.
Video-memory purges (suspend, VT switch, mode set) are never counted as losses. After a reset or a purge the
compositor tells every client its buffers were lost, so an idle client repaints instead of showing what the driver
left behind. The renderer each process uses is logged once, reported by `cupertino-diagnose` and recorded in the frame
statistics.

Until step 7 the GPU path is built but off: one switch, `GPU_BY_DEFAULT` in `crates/cupertino-ui-wayland/src/renderer.rs`,
is `false`, so every process draws on the CPU (reason "the GPU path is not turned on") and never binds the dmabuf
protocol. `CUPERTINO_RENDERER=gpu` takes the GPU path regardless, which the tests and measurements use; `cpu` keeps a
process on the CPU after the switch is turned on. Step 7 turns the switch on, per surface class.

A process that may use the GPU draws its first frames on the CPU while a worker thread makes the device and links the
programs, then switches (the runtime frees each window's CPU renderer and draws whole frames with a new one). Linked
programs are saved per user in `$XDG_CACHE_HOME/cupertino/programs/<driver>/`, keyed by the driver's vendor, renderer
and version strings, so the next start loads them instead of compiling; a binary the driver refuses is compiled
again and replaced.

### 9. Layers and animation in the compositor

After layers work inside the client, timed and spring transitions move to the compositor, so they run at the
display's rate even while the application is busy. This needs a protocol of our own, not `wl_subsurface`, whose
integer positions cannot land on device pixels at fractional scales. The protocol carries:

- geometry in fixed point, applied atomically with the parent surface's commit;
- glass attached to a layer, so a moving panel and its glass never separate, and the glass of several layers joined
  into one shape from their bounds as shown, so a button buds out of a field's glass as both move;
- a blur of a layer's own content, a Gaussian in points animated like any property, which the compositor draws from an
  offscreen copy of the content through the backdrop blur's pyramid and passes (keeping the content's alpha), so a
  panel's text softens as it closes without a frame of the application's;
- a blend role per layer, so vibrant text and symbols are a layer the compositor blends against the glass behind it;
- an animation as a curve (timed or spring) with start and target, started at the first presentation of its commit
  on the compositor's clock, honouring the time scale and the fixed time tests use;
- retargeting from the presented value and velocity, with `finished` and `replaced (value, velocity)` events;
- input: animated layers take no input, and the client hit-tests against target values;
- semantics: accessible bounds are target values, with a settled signal for desktop control;
- clipping to the window, and per-client limits;
- a presentation of a whole surface (a panel, a popup), so a panel's fade or a popover's growth out of its anchor runs
  on the compositor's clock with no layer of the toolkit's;
- a rule for content that arrives late while scrolling.

Scrolling and dock magnification follow the pointer, so they stay client-driven: a cheap commit per frame that moves
layers, not a repaint.

On the toolkit's side a widget marks a subtree a surface layer (`Widget::surface_layer`): the runtime paints it into a
frame of its own at its layout size, with its own damage and renderer, and the window system makes it a layer of the
window's surface, drawn from buffers of its own (shared memory, or dmabufs from the GPU renderer's texture). The
application then sets or animates the layer's properties with `WindowRequest::Layer`, which costs no frame; layout,
input and semantics keep the layout. A motion that is no single curve of one property (a product of two springs, a
clamped ramp) goes as its values at even steps of its duration, which the compositor follows in straight lines on its
clock.

The launcher's panel is drawn as layers once it rests: the enclosure's glass is a layer that draws nothing but its
glass, its content a layer that does not scale, and its dark edge, drawn just outside the shape, a layer of its own on a
grid around the enclosure with room for the stroke, centred on the point the panel grows about; each mode button is a
layer of glass at its rect and a face (edge, focus ring and symbol) above it. The window itself shows no glass. Its
close is then the compositor's: the glass grows to its end rect and fades, the edge scales with it and blurs, the
content blurs and fades, all on the close's ease-out, with no frame of the application's. While the panel moves, and
under Reduce Motion, it draws itself.

### 10. The compositor

The compositor stays on Smithay's GLES renderer. Its own work, in order:

1. Non-glass materials take the overlap-limited backdrop generation glass already uses.
2. The minimize effect reuses one captured texture; the per-frame HUD signature, cursor image clone and wallpaper
   bookkeeping go.
3. Blur, material, glass and shadow outputs are dithered by position, from the shared definition.
4. Windows shown below half their size (overview, app switcher, minimize, zoom transitions) are sampled from a reduced
   copy kept per window, since textures imported from client buffers cannot hold mipmaps; integer-scale clients shown
   at a fractional scale get a better minification filter.
5. Title bars, window buttons, rims and the keyboard HUDs are recorded as lists of draws and drawn in
   borrowed-context mode into a texture per image, redrawn when its state changes. The shapes take the shared
   distance with per-corner radii and the corner exponent, as `ui_fill` does (its block, `cupertino_gpu::FillBlock`,
   carries both); glyphs and glyph paths are coverage images in an atlas. The chrome has a program of its own,
   which lays each draw over what the ones before it made with the CPU painter's arithmetic (reading the image below
   from a second texture, blending off), so the GPU image is the CPU painting whatever the driver's blending
   precision. The cursor stays a cursor-plane element drawn on the CPU: the DRM compositor fills the cursor plane's
   buffer on the CPU, from a mapped buffer, so a GPU image would come back to the CPU to reach it.
6. Vibrant foreground, lifted thumbs and lenses as glass, the pointer-lit highlight and variable blur, through the
   layer protocol.

Frame scheduling starts a frame later than now only once the output's presentation timestamps are known to be real;
it latches from the recent maximum render time plus a margin, never later than half a refresh period, because a
missed refresh costs more than the latency it saves (the recent maximum is the last 60 frames' render times and the
margin 2 ms; a frame that misses its vblank counts as a whole period, which turns the late start off until it leaves
those 60; `CUPERTINO_LATE_LATCH=0` turns it off for a comparison). A 10-bit primary plane is a setting
(`output_depth`), off until the probe decides; captures draw into 8-bit targets of their own. Direct scanout and
the cursor plane are already permitted by the DRM path; they are verified on the target driver rather than built.
Overlay planes stay off on the proprietary NVIDIA driver. Variable refresh and colour management are out of scope
while the target display supports neither.

**Vulkan in the compositor** means implementing Smithay's renderer traits for Vulkan, so `DrmCompositor`, damage
tracking and the scene's element types survive, and porting the material, glass and mesh passes (about 250 raw GL call
sites today). It is built only when a measured budget needs it.

## Budgets

Measured on the target machine (NVIDIA, 3840 x 2160 at 60 Hz, scale 1.5) unless noted. The gate column names the plan
step whose "done when" checks the row. Every budget is confirmed or revised once against the step 0 baseline, in a
reviewed change that records the reason.

| Measure | Budget | Gate |
|---|---|---|
| Any frame, compositor or client | inside one refresh (16.7 ms at 60 Hz); no missed refreshes in the bench scenarios, and no client frame longer than a refresh | every step: no scenario worse than its baseline beyond the recorded run-to-run noise (step 1: within its 10% budget); absolute from the step that owns the scenario (scroll 4, drag 2, resize and animation 7) |
| Files wheel scroll with the window filling the output, p99 client frame CPU | under 8 ms at scale 1 and 1.5; paint and raster cover only the exposed strip | 4 |
| Files live resize | client CPU excluding raster under 8 ms; client GPU under 4 ms | 7 |
| Toolkit animation frame (Control Center opening, launcher typing), client CPU | under 4 ms | 7 |
| Client GPU time per frame (timer queries) | under 4 ms | 6 |
| Compositor frame while a window is dragged over glass | p99 GPU time per frame (timestamps) under 4 ms; p99 CPU time to compose and submit a frame, not counting the wait for the GPU, under 2 ms | 2 |
| First frame of a GPU process | no later than the CPU path; the switch to the GPU within 150 ms with a warm program cache | 6 |
| Added private memory per GPU process | under 80 MiB; CPU-class processes unchanged | 7 |
| GPU memory per GPU window | at most two buffers and one window-sized layer (width x height x 4 bytes each) plus 32 MiB of atlases and pools, measured as the change in total GPU memory with the window open and closed; a process's first render target adds the one more target's size the driver reserves with it | 7 |
| Client CPU after the look freeze | no bench scenario's client CPU p99 more than 10% above its baseline, or more than its recorded run-to-run spread where that is larger | 1 |
| Fence overhead | resident-memory slope with fences minus the slope without, under 1 byte per frame over 10 million frames | 3 |
| Idle desktop | no frames drawn and no GPU work but the clock's minute (one menu bar frame and its composite); processes wake only by the timers recorded and justified in step 0, listed in the plan's measurements | 7 |
| Pointer motion to screen | input-to-flip latency from the DRM backend's log, below the baseline; the cursor on the cursor plane | 11 |

## Testing

- Every renderer golden runs through both renderers. CI runs the GPU renderer on Mesa llvmpipe with
  `CUPERTINO_RENDERER=gpu`; if the step 3 probe succeeds, the dmabuf presenter runs there too, allocating by texture
  export.
- What CI cannot exercise (GBM, hardware modifiers, syncobj with the DRM backend, the NVIDIA driver) runs in a local
  tier before every push that touches rendering, with `CUPERTINO_GPU_REQUIRED=1` making a missing GPU a failure: dmabuf
  import, syncobj round trips (the headless backend offers the protocol when its device has a render node, and can
  withhold it), injected context loss and purge, the CPU↔GPU switch mid-session, a failed import falling back to shared
  memory, a resize storm, the fence soak, and the goldens re-run on the NVIDIA GPU.
- Parity tests cover scales 1, 1.5 and 2 with odd sizes, and a scissored repaint against a full repaint with effects
  that cross the damage edge.
- Objective image checks replace judging by eye where possible: an overdraw test that must end in one flat colour
  (tearing and synchronisation), a run-length check on captured gradients (banding), and a reduced window compared with
  a box-filtered reference (shimmer).
- `cupertino-gpu` has its own tests per backend: pipelines, orientation, atlas eviction, pool reuse, buffer export and
  import, fences, context loss.
- The headless session and `scripts/dev/bench` report the renderer of every process, so a silent fallback shows in
  the numbers.
