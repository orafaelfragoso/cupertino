# cupertino-ui-wayland

CupertinoKit's window system on the transport (`cupertino-wayland-client`), and drawing a surface of it with tiny-skia.
MPL-2.0. It binds no protocol beyond the
transport's standard set and never reaches the private protocols (`cupertino-layers` denies `cupertino-protocols` and
`cupertino-shell-client`). Depends on `cupertino-wayland-client`, `cupertino-geometry`, `cupertino-modifiers`,
`cupertino-ui-platform`, `cupertino-ui-render`, `cupertino-ui-text`, `cupertino-menu-model`, `cupertino-ui-menus`,
`cupertino-gpu` (the graphics layer, for the GPU presenter), `cupertino-ui-gpu` (the GPU renderer the presenter's
windows draw with) and `tiny-skia`.

## Public API

| Item | Contents |
|---|---|
| `WaylandWindowSystem` | `cupertino-ui-platform`'s `WindowSystem` on one transport `Connection` (`connect(profile)`, `new(connection, profile)`); `connection` and `surface(window)` reach the transport for an extension protocol; on a compositor with `cupertino_app_effects_v1` it learns the session's appearance and display options before the first window (`appearance()`) and reports them as `PlatformEvent::Appearance`; it reports how scroll views show their bars as `PlatformEvent::ScrollBars` after each display options; it takes a steady caret (version 9) into `SystemAppearance::steady_caret` and reports `PlatformEvent::Appearance` only when it changed; it reports the lock state as `PlatformEvent::SessionLock`, and `SessionLock(false)` once at start on a compositor that can't tell (below version 5); `WindowRequest::Backdrop` puts that protocol's material under the whole window at the backdrop's alpha, shaped to the window's size and corner radius on each present, and a new alpha or radius for the same material changes the one shown; `WindowRequest::Materials` puts one more backdrop of that protocol under each region of the list (shape, radius, alpha; a region with `glass`, or a backdrop of a glass material name, is sent as `set_glass` with its style, container and spacing; the tone the compositor tells a glass backdrop, version 7, arrives as `WindowEvent::GlassTone` with the region's index, or none for the window's own backdrop), reuses the backdrops already shown and takes away the ones left over, and an empty list takes them all away; a list or a backdrop past the protocol's limits (64 backdrops on a surface, 128 for the client), which the compositor would answer by ending the connection, is refused before any backdrop is asked for, and a refused list leaves the window without its materials; `WindowRequest::Menus` publishes a toplevel's menus through `cupertino-ui-menus` once it is configured, keeps their items' enabled states, reports chosen items as `WindowEvent::MenuActivated` and withdraws them with `None` or when the window goes; without the effects at version 2 or a session bus the menus are not shown; a window opened with `DecorationMode::{Unified, UnifiedCompact, Sidebar}` asks the effects for that window style at creation, and is titled where the compositor has no effects; `request_activation_token` asks `xdg_activation_v1` for a token naming the application and answers `None` when it can't ask |
| `GpuRenderers` | from `WaylandWindowSystem::gpu_renderers`: `renderer(config, rasterizer)` makes a window's GPU renderer (`cupertino_ui_gpu::WindowRenderer`) on the process's device while it draws on the GPU, `None` otherwise; a renderer factory holds a clone and asks it each time |
| `SurfaceRoles`, `RoleEvent` | `with_roles` gives windows roles from protocols outside the window system (a desktop shell's layer and lock surfaces): `create` makes a window's surface by its `app_id`, `event` turns the transport's extension events into configures and closes, `destroyed`, `keyboard_focus` and `outputs_changed` follow the windows and outputs, and `prepare`, called at the start of every dispatch, sends the requests the application asked for since and reads the transport's state; a role's configure has no activated state, so its `WindowStates::activated` says whether the window or a popup of its chain has the keyboard (between `wl_keyboard.enter` and `leave`), and a configure never turns inactive a window that keys still reach |
| `ShmPresenter`, `Ready` | one per surface, owned by the application, holding no protocol object: `configured(size)` after a configure, `frame_done` on the surface's `Event::Frame`, `request_redraw`; `poll(&Connection)` until `Ready::Idle` reports a new scale (`Ready::ScaleChanged`) first and then `Ready::Draw` once the surface is configured, a frame is asked for, none is in flight and the transport has a free buffer; `draw(&mut Connection, damage, paint)` paints and presents a frame; `draw_solid(&mut Connection, rgba)` shows the surface as one colour, a stretched single pixel (`Connection::present_solid`), or where the compositor has no one-pixel buffers a frame filled with it |
| `popup::placement` | the `PopupPlacement` of a popup covering a parent-local rectangle, rounded outwards to whole points, its top-left corner at the rectangle's; an empty or non-finite rectangle is `Error::InvalidShape` |
| `clipboard::{TEXT_TYPES, set_text, text}`, `clipboard::Clipboard` | the text clipboard: a copy offers `text/plain;charset=utf-8`, `text/plain` and `UTF8_STRING`; a paste reads the first of them an offer has, drops a character cut by the transport's read limit and is `None` when the offer was replaced or its client did not answer in time. `Clipboard` keeps the latest selection (`selection_changed`) and the text this client set, which a paste reads while no selection is known |
| `rgba_to_argb8888`, `Error` | the pixel conversion into shm buffers, and errors |

## How it works

`draw` keeps the last frame's `Pixmap`. Without damage the pixmap is cleared and the frame is whole. With damage
(rectangles in points) only those rectangles, rounded out to device pixels and cut to the buffer, are cleared for
`paint`; only they and what the leased buffer missed of the frames shown since it last held one
(`BufferLease::missed`) are converted into it, and only they are reported with `damage_buffer`. Rectangles that add up
to the buffer convert it once, whole. A frame at another size or scale is drawn whole, and so is the first one after
`configured`: a configure can change what a frame shows (activation, maximized) without changing its size.

`draw_with` hands `paint` the device-pixel rectangles that were cleared (`None` when the frame is whole), for a painter
that draws a whole frame elsewhere and copies those rectangles in (`copy_rects`). `Damage` collects what changed between
frames in points: it is the whole surface when new, merges overlapping rectangles and keeps at most eight apart; `take`
gives the rectangles for the next frame, or `None` for all of it.

The configure serial needs no handling here: the transport sizes the leased buffer from the surface's latest configure
and acknowledges exactly that serial with the commit. The lease holds the connection from `lease` to `present`, so no
configure is dispatched while `paint` runs, and a configure overtaken before the draw is never acknowledged. The frame
callback's time, for an animation clock, is `FrameCallback::time` of the transport's `Event::Frame`.

## The window system

Windows and popups map one to one onto the transport's toplevels and popups; destroying a window destroys its popups.
Configures, close, popup dismissal, scale, pointer, keys, keyboard focus and the input method arrive as `WindowEvent`s
of the window whose surface they name. A popup's configure comes after a `WindowEvent::PopupPlaced` with the rectangle
the compositor placed it at in its parent, after any flip or slide. Key modifiers are logical (`Mods`), through the `Profile` the window system was
made with. Input times are the compositor's milliseconds mapped onto the window system's clock (`CLOCK_MONOTONIC` nanoseconds, the clock the runtime reads, so an event's time compares with its
now): the first one, and any that would land in the future or more than a second in the past, anchors the mapping
at the time it was read, and the ones after keep the compositor's spacing. A button's serial is the transport's latest
input serial.

`present` copies the renderer's frame (premultiplied RGBA) into a leased buffer as ARGB8888: its damage, the damage of
frames that were not presented, and what the buffer missed, or all of it when the buffer's pixels are unknown. The
compositor is told only the frame's damage and the carried damage, or all of the window when the frame changed size.
No free buffer, no configure yet, or a frame at another size than the latest configure asks for is `NotReady`, and the
damage is carried (more than 64 rectangles become the whole frame). The transport acknowledges the latest configure
with every frame. A window's input region (`WindowSpec::input_region`, then `WindowRequest::SetInputRegion`) is sent with
the next present that follows a change, rounded outwards to whole points; a window that never sets one takes input over
its whole surface. A raised layer (`present_layer`) is drawn into a synchronized subsurface at the window's top left
corner, as wide as the window, at its scale and with an empty input region; its raised materials go to the
subsurface's backdrops (`cupertino_app_effects_v1` version 11), so the compositor draws them between the subsurface and
the window's content. Its frame copies only its own damage and what its buffer missed, and it commits before the
window, so it applies with the window's next commit. The capability needs the transport's subsurfaces and version 11
of the effects global. `request_frame` answers at once when no frame callback is outstanding, else with the callback; a
callback nobody asked for is not reported. On a compositor with `wp_presentation`, each presented frame is reported as
`Presented` with the time it turned visible, moved from the compositor's presentation clock onto the window system's
clock and never later than now, or as `Discarded` when the compositor never showed it.

`present_solid` shows a window as one colour without a frame: the transport's single pixel stretched over the window's
size, with its backdrop shaped and its input region sent as for a present. It acknowledges the latest configure and asks
for a frame callback; a frame presented later is whole. Without `Capability::SolidColor` it is
`PlatformError::Unsupported` and the runtime draws the window instead; without a configure it is `NotReady`.

A clipboard read finishes before `read_clipboard` returns (the transport's one-second limit) and its data arrives with
the next `dispatch`. Not offered yet, so their flags are false: touch, the primary selection, and the desktop's
effects protocol, which an extension binds through `connection`. Drags go out (`drag_and_drop` with the clipboard and
a seat): `start_drag` hands the transport the data, the actions and the image as a `DragIcon` in `wl_shm`'s byte order,
one drag at a time (a new one cancels the last), and reports the drag source's events as `PlatformEvent::Drag`.
Drags over a window (from any client, this one included) arrive as `WindowEvent::Drop` with the transport modifiers
held; `answer_drop` accepts the type and sets the actions, `read_drop` reads the drop like a clipboard read, and
`finish_drop` finishes it (setting the action taken first, whatever was chosen during the drag, a copy when the
runtime took it as an ask, and refusing an action the source does not allow) or
refuses it. At most 8 drops are kept while they are read. A popup's `offset` moves
its anchor rectangle; `reactive` is not sent.

## The GPU path

`renderer` holds the fallback rule and the switch: `GPU_BY_DEFAULT` is `false`, so every process draws on the CPU
unless `CUPERTINO_RENDERER=gpu` asks for the GPU (it overrides a software device and missing dmabuf feedback);
`CUPERTINO_RENDERER=cpu` keeps a process on the CPU. Missing libraries or devices (`Unavailable`) select the CPU, and
so do three resets within a minute; purges never count. The current renderer and why (`active`) is logged as
`renderer: <cpu|gpu> (<reason>)` on each change and recorded in `$XDG_RUNTIME_DIR/cupertino/renderers/<pid>`.

`gpu_path::GpuPath`, owned by the window system, makes the device on the feedback's render node and links the
presenter's and the GPU renderer's programs on a worker thread (saved per driver in
`$XDG_CACHE_HOME/cupertino/programs`) while the first frames go out in shared memory, then takes the device over and
reports `PlatformEvent::RendererChanged`: the runtime drops every window's renderer (the CPU renderer's frame, layers
and caches go with it) and draws whole frames with a new one. The new ones come from `GpuRenderers`
(`WaylandWindowSystem::gpu_renderers`), which the kit's renderer factory holds: while the process is on the GPU it
makes a `cupertino_ui_gpu::WindowRenderer` on the presenter's device (shared through an `Rc`) with the programs made
in the warm-up, and the CPU renderer otherwise. On a loss it frees every surface's buffers, reports the
change back to the CPU and warms up again (or stays on the CPU after the third reset). `buffers_lost` from the
compositor checks the device and makes every surface's next upload whole.

`gpu_present::GpuSurface` keeps one surface's dmabufs, allocated in `SIZE_STEP` (128 pixel) steps and cropped by the
viewport, in a modifier the compositor offers and the driver renders to. A frame the GPU renderer drew on the same
device carries its texture (`CpuFrame::device`) and is copied from it into a free buffer texel for texel
(`blit_texels`), nothing uploaded; a CPU frame has its damage uploaded into a staging texture and drawn into a free
buffer. A surface that falls back to shared memory, the raised layer's subsurface and a drag image read a GPU frame
back (`CpuFrame::in_memory`). With `linux-drm-syncobj-v1` the frame's native fence is the buffer's
acquire point and its release point is waited for through an eventfd (two buffers); without it the commit waits for
the own fence and a buffer is reused one commit after its release (three buffers). A buffer that cannot be allocated
or that the compositor refuses moves the surface to shared memory for good; the transport destroys its syncobj
surface object before the first shared-memory attach.

`examples/gpu-window.rs` is the presenter's check window, a client for `cupertino-server`'s `tests/gpu_presenter.rs`:
`--frames`, `--reset-every`, `--purge-every`, `--losses`, `--renderer` (frames drawn by the GPU renderer and copied
from its texture), and a `key value` report of what it did.

## Tests

```
cargo test -p cupertino-ui-wayland
```

Unit tests cover the pixel conversion, the damage cut, popup placement and where the compositor placed a popup, the clipboard text, and the window system's
conversions, input timeline and carried-damage bound. `tests/protocol-flow.rs` drives the presenter, the clipboard and
the window system (frame pacing, carried and missed damage, the input region, a solid window as one pixel at its configured size keeping its input region and refused without the globals, the presenter's solid colour and its filled fallback, a click, the clipboard, a drag with its
icon, the waker, refused requests)
against an in-process server.
