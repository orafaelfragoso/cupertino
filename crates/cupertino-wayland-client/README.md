# cupertino-wayland-client

Wayland client transport for application windows over standard protocols only, so the same code runs on Cupertino and
on any other compositor. Depends on `cupertino-geometry`, `wayland-client`, `wayland-protocols` and `xkbcommon` (the
workspace's pure-Rust `cupertino-xkbcommon`); `cupertino-layers` fails the build if it ever reaches
`cupertino-protocols` or another compositor's protocol crates.

## Public API

| Item | Contents |
|---|---|
| `Connection::{connect, from_stream, from_wayland}`, `Waker` | the connection (`WAYLAND_DISPLAY` / `WAYLAND_SOCKET`, a given socket, or an established `wayland_client::Connection` the process also runs event queues of its own on); `Connection::wait` returns the next `Event`s, a `Waker` wakes it from another thread |
| `Capability`, `CapabilitySet` | the optional protocols the compositor offers: seat, clipboard, fractional scale, server-side decorations, activation, cursor shape, output layout, text input, presentation feedback, subsurfaces, one-colour surfaces (`SolidColor`: `wp_single_pixel_buffer_manager_v1` with `wp_viewporter`). `Connection::capabilities` lists them, with the extension protocols bound so far (`Extension`, `CapabilitySet::extension`); a call that needs a missing one fails with `Error::Unsupported` |
| `Connection::toplevel`, `ToplevelConfig`, `Toplevel` | an xdg toplevel: app id, title, size limits, the decorations asked for, and the handle of another client's exported toplevel to open as a child of (`foreign_parent`, xdg-foreign-unstable-v2; without the protocol, or for a handle the compositor does not know, no parent). `set_title`, `set_size_limits`, `set_maximized`, `set_fullscreen`, `minimize`, `start_move`, `start_resize`, `show_window_menu`, `set_decorations`, `activate`, `destroy` |
| `WindowConfigure`, `WindowStates`, `Decorations`, `WmCapabilities` | a configure: serial, size (or none: the client picks), bounds, states, the decorations granted, and what the compositor supports (window menu, maximize, fullscreen, minimize) when it said. A compositor without `zxdg_decoration_manager_v1`, or one that answers client-side, gives `Decorations::Client` |
| `Connection::popup`, `PopupPlacement`, `PopupConfigure`, `Popup`, `Anchor`, `Gravity`, `ConstraintAdjustment` | xdg popups of a toplevel or popup, with an optional grab and an offset; with `xdg_wm_base` version 3 a reactive one is placed again when its parent moves or resizes, and a reactive or `parent_size` placement names the parent's configure not acknowledged yet. `Connection::reposition_popup` moves one (version 3) and returns the token of its `Event::PopupRepositioned`; `Event::PopupDone` when the compositor dismisses one |
| `Connection::lease`, `BufferLease`, `BufferLayout`, `BufferFormat`, `PixelRect` | an shm buffer for the next frame at the surface's scale (`Argb8888` or `Xrgb8888`), at most `BufferLayout::MAX_EXTENT` (16384) pixels per side, up to three per surface; `BufferLease::missed` names the rectangles in which a buffer differs from the frame shown last (what the frames shown in the other buffers since changed; `None` for a new buffer, another size, a whole frame shown elsewhere, more than 32 rectangles, or pixels written but not presented), so a damaged frame redraws only them and its own damage; `present` or `present_damage` acks the latest configure, attaches, damages, asks for a frame callback and commits; `Connection::buffer_free` tells whether a lease would get a buffer now |
| `Connection::present_solid` | a surface of one colour (straight RGBA, sent premultiplied) as a single-pixel buffer the viewport stretches over its configured size in points, at buffer scale 1, whatever the surface's scale: no pixels are drawn or uploaded, and a configure, a new scale or an output change costs a viewport destination. It acks the latest configure, keeps the pixel while the colour stays, keeps the input region and frame callbacks; without `Capability::SolidColor` it is `Error::Unsupported` and the caller presents a filled shm buffer instead. The next `lease` after it reports `missed` as `None` |
| `Event::{Presented, Discarded}`, `Presented`, `PresentedKind`, `Connection::presentation_clock` | with `wp_presentation`, every present asks for feedback: when the frame reached the screen in nanoseconds on the compositor's clock (a `clockid_t`), the refresh interval, the retrace counter and how it was shown; or that it never was |
| `Event::{Frame, ScaleChanged, OutputsChanged}`, `FrameCallback`, `Scale`, `OutputInfo`, `OutputId`, `OutputGeometry`, `OutputTransform` | frame pacing, the surface's scale, outputs: name, description, make, model, physical size and transform (`wl_output`, an empty or `Unknown` make or model and a zero size read as unknown); with `zxdg_output_manager_v1` each output's logical position and size, and `OutputInfo::effective_scale`, the current mode over the logical size (1.5 at a fractional scale) |
| `Connection::set_input_region` | the part of a surface that takes pointer input, rounded outwards; `None` is the whole surface |
| `Connection::request_activation_token`, `TokenRequest`, `Event::ActivationToken` | a token to hand to an application this one launches (`xdg_activation_v1`) |
| `PointerEvent`, `KeyboardEvent`, `Button`, `Modifiers`, `key`, `ScrollSource`, `CursorShape`, `ResizeEdge` | pointer and key input (`key` names the evdev codes of common keys); every event carries the compositor's input time in milliseconds (enter, leave and repeated keys take the latest one advanced by the time since); the axes of one `wl_pointer.frame` arrive as one scroll, with its source, wheel detents in 120ths and whether the finger lifted; keys carry keysym and text from the compositor's keymap, with dead keys and compose sequences resolved through the X compose table of the locale (`XCOMPOSEFILE`, else `$XDG_CONFIG_HOME/XCompose` or `~/.XCompose`, else the locale's file under `XLOCALEDIR` or `/usr/share/X11/locale`; a key inside a sequence has no text, the key that ends it has the result, a key that breaks it has none; results longer than one character are skipped), key repeat runs in the client, for a key that types outside a compose sequence only. `Connection::{modifiers, input_serial}` read the held modifiers and the serial of the latest input event; `Connection::held_press` (`HeldPress`, `PressDevice`) the press the seat still holds: the serial of the pointer button press or touch down that started its implicit grab (the first of the buttons or fingers still down; of the two devices, the one whose grab started last), which `start_move`, `start_resize` and `show_window_menu` answer whatever motion, keys or focus changes came since, and none once every button and finger is up, the pointer left or the touches were cancelled; with none held those three send nothing and are `Error::PermissionDenied`; `Connection::set_cursor` sets a named cursor: the compositor's with `wp_cursor_shape_v1`, else drawn from an XCursor theme at the integer scale of the surface the pointer is on, falling back to the theme's default cursor and following the themes it inherits from, an animated one playing its images while the pointer stays (`Connection::wait` wakes for each), and drawn again from the images of the new scale when the surface under it changes scale; the theme is the user's (`CursorTheme::from_env`: `XCURSOR_THEME`, `XCURSOR_SIZE`, `XCURSOR_PATH` or the XDG icon directories, read when first needed) unless `Connection::set_cursor_theme` names one with its search path; `Connection::hide_cursor` hides it |
| `Connection::{set_selection, receive, receive_async}`, `ClipboardOffer`, `ReadId`, `Event::{Selection, Received}` | the clipboard: offer contents in several MIME types, list and read another client's. `receive` blocks for up to a second; `receive_async` reads in `Connection::wait`'s loop, polling the pipe with the display, and ends as `Event::Received`: at most 16 MiB, `None` when the offer was replaced, the source sent nothing for a second, the read took longer than ten seconds or the pipe failed, eight reads at once |
| `Connection::{start_drag, start_drag_with_icon, accept_drag, set_drag_actions, receive_drop, finish_drag, refuse_drop}`, `DragOffer`, `DragEvent`, `DragSource`, `DragSourceEvent`, `DragActions`, `DragIcon`, `Event::{Drag, DragSource}` | drag and drop (`wl_data_device`): another client's drag over a surface arrives as enter (with the offer's types and the source's actions), motion, the compositor's chosen action, leave and drop; it is accepted in one of its types or refused, a drop is read only once it was accepted in that type and dropped (through `Event::Received`, with the limits of `receive_async`) and then finished, or refused with `refuse_drop` (the offer goes unfinished, which the source hears as a cancel). A dropped offer is kept apart from the drag under the pointer, so another drag can enter while it is read (at most 8 are kept; past them the oldest is refused). A compositor older than drag actions (`wl_data_device_manager` before version 3) sends no actions and takes no finish: its drags allow a copy alone, `set_drag_actions` sends nothing and a finished drop is only destroyed. An enter without an offer (a null source) lets the drag before it go and reports nothing; a drag that leaves, or a surface destroyed under it, lets its offer go. `start_drag` drags this client's contents from the held button press (the one that started the implicit grab) with the actions it allows, serves them like the selection, and reports the target's type, the chosen action, the drop, the finish or the cancel. `start_drag_with_icon` adds a `DragIcon` (premultiplied `argb8888` pixels at a whole-number buffer scale that divides both sides, at most `DragIcon::MAX_EXTENT` (1024) pixels a side, and a hotspot in points) as the `start_drag` icon surface, committed after the request with the hotspot as `wl_surface.offset` (the attach offset before version 5); its surface and buffer are destroyed at the drop, the cancel, or when another drag replaces it. A drag the compositor ignored (the release reached it first: the client hears the release, not a leave) ends as `DragSourceEvent::Cancelled` at that release and frees them too |
| `Connection::set_text_input`, `TextInputState`, `SurroundingText`, `ContentHint`, `ContentPurpose`, `ChangeCause`, `Event::TextInput`, `TextInputEvent`, `TextInputDone`, `Preedit` | input methods (`zwp_text_input_v3`): the focused text field's state goes out, the input method's preedit, commit and delete come back as one batch per `done` |
| `Error`, `Disconnect` | typed errors: a missing required protocol with its minimum version, the disconnect cause, a configure serial that did not increase, an unsupported format or MIME type, a denied or cancelled transfer, no free buffer |
| `Connection::{subsurface, place_subsurface, unmap_subsurface}` | a synchronized subsurface of a window, popup or external surface (`wl_subcompositor`): above its parent, with an empty input region so the pointer and touches reach the parent, drawn through `lease` at the parent's scale and at the size `place_subsurface` gives, with no frame callback or presentation feedback of its own (the parent's commit shows it and stands for both); `unmap_subsurface` attaches no buffer, which hides it with the parent's next commit; destroying the parent destroys it first |
| `State`, `ExternalRole`, `Event::Extension`, `ExtensionEvent`, `Connection::{bind_extension, queue_handle, external_surface, wl_surface, wl_output, seat, pointer, surface_outputs, roundtrip}`, `OutputId::{registry_name, from_registry_name}` | the extension hook: another crate adds protocols of its own on the same queue (below) |

## How it works

`wl_compositor` (version 4 or later), `wl_shm` and `xdg_wm_base` are required; a missing one is
`Error::MissingRequiredProtocol` at connect, never a degraded mode. Everything else is a capability.

A `BufferLease` borrows the connection, so no configure arrives between drawing and presenting: the buffer always
answers the latest configure, which is acked once, with the first buffer presented for it. The scale is the fractional
scale when `wp_fractional_scale_v1` and `wp_viewporter` are offered, else `wl_surface.preferred_buffer_scale`, else the
largest scale of the outputs the surface entered; without the viewporter it is rounded up to a whole number. With the
viewport, `BufferLease::set_zoom` magnifies a frame about its centre on the compositor (the viewport's source is the
middle of the buffer), and the next frame without a zoom shows the whole buffer again.

A surface of one colour (`present_solid`) needs no frame at all: the compositor stretches one pixel over it. The
viewport is created then even without fractional scaling, and an shm frame presented later on the same surface keeps it
(buffer scale 1, destination the logical size), so the surface can go from one to the other in either direction.

An input method gets the focused field's state only while its surface has the text-input focus: it is sent again
after each enter, only when it changed, and a value once sent (surrounding text, cursor rectangle) is withdrawn by
enabling afresh. Surrounding text is cut to the protocol's 4000 bytes around the selection, or around the cursor when
the selection alone is longer, at character boundaries. A password or PIN purpose, or the `hidden_text` or
`sensitive_data` hint, never sends its text and always adds the `sensitive_data` hint, whatever the caller set; a
`Debug` print of the text-input types shows the length of typed text, never the text. A preedit cursor that is off a character boundary is hidden, and a
batch cut short by a focus change is dropped. Applying the batch to the field (clamping deletes to characters, the
order in `TextInputDone`) is the caller's. A state with `reset` set enables the input method afresh once, even when
nothing else changed, and is then kept as sent.

Copying serves each paste on a short-lived thread (at most 8 at once, 1 s each); pasting reads at most 16 MiB within
1 s, and returns the client's own contents directly.

## Extensions

A crate above the transport, such as one for the desktop's own protocols, adds them on the connection's queue, so its
events stay in order with the standard ones and `Connection::wait` dispatches both in one pass. The transport itself
names no protocol beyond the standard ones.

- Binding: `Connection::bind_extension(versions, data)` binds the extension's global with user data of a type the
  extension crate defines, and lists it in `Connection::capabilities` with the version bound; `None` when the
  compositor offers none in the range, which is where the application takes its fallback. Objects the extension creates
  later use `Connection::queue_handle`. Rust allows the extension's `impl Dispatch<TheirInterface, TheirData> for
  State` only with that local type; `()` is refused.
- Events: the `Dispatch` impl hands what it receives to `State::emit`, which `Connection::wait` returns as
  `Event::Extension` in order with the transport's own events (`ExtensionEvent::downcast` gets the value back,
  `downcast_ref` looks at it), or puts it
  where the extension reads it, such as a queue shared through the user data. `State::fail` ends the next wait with an
  error, for a protocol violation the extension found; `State::output_id` names a `wl_output` the extension received. `Connection::wait` returns after each read from the compositor, maybe with no `Event`, once everything read
  is dispatched; the extension then reads its queue. A burst of extension events costs one wait, not one each.
- Surface roles: `Connection::external_surface(assign)` creates a `wl_surface` and calls `assign` with it, the queue
  handle and its `SurfaceId`, before the first commit; `assign` creates the role's objects and returns an
  `ExternalRole`. Each configure of the role goes to `State::configure_external(surface, serial, size)` from the
  extension's `Dispatch` impl (a 0 width or height leaves that dimension to the client, which `lease` fills from its
  `preferred` size; a serial that does not increase ends the next
  wait with `Error::InvalidSerial`). The surface then draws through `Connection::lease` like any other: present calls
  `ExternalRole::ack_configure` before the commit, once per configure. A role whose protocol forbids a commit before the
  first configure, such as `ext_session_lock_surface_v1`, returns false from `ExternalRole::initial_commit`. `Connection::destroy` destroys its popups, calls
  `ExternalRole::destroy`, then the surface. `Connection::popup` with it as parent creates the popup without a parent
  and hands it to `ExternalRole::adopt_popup` before its first commit; the default refuses. `adopt_popup` must attach
  the popup to the role of the surface it was called for, never another one: the transport destroys a surface's popups
  before the surface, and a popup attached elsewhere outlives its parent, a protocol error.
- Objects the extension's requests name: `Connection::{wl_surface, wl_output, seat}`. `Connection::roundtrip`
  dispatches the answers to requests sent so far, for state an extension reads at setup; the transport's own events
  stay for the next wait.

## Example

```
cargo run -p cupertino-wayland-client --example simple-window [-- --frames N]
```

A window that asks for server-side decorations and draws its own title bar when it gets client-side ones. Escape or
the close request exits; `--frames N` animates N frames and prints the size, scale and decorations it ended with.

## Tests

```
python3 ci/worktree-build.py run --profile dev -- python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --all-features -p cupertino-wayland-client
# Mutter check; skips only when mutter is absent.
python3 ci/worktree-build.py run --profile dev -- python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --all-features -p cupertino-wayland-client --test mutter
# Optional Weston check.
python3 ci/worktree-build.py run --profile dev -- python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --all-features -p cupertino-wayland-client --run-ignored only
```

`tests/protocol-flow.rs` runs a minimal protocol server in-process: required globals, configure and ack, resize, buffer
reuse, scale, decorations, popups, input, the held press a move, resize or window menu answers (after motion and keys, from a pointer or a finger, and none once released), text input, clipboard and disconnect. Its `layer` module is an extension written against
the public API (layer shell, a protocol the transport does not know): it is listed in the capabilities once bound and
not when absent, a layer surface draws through the lease, acks through its own protocol, takes a popup, fills a 0
dimension from the preferred size, and a burst of 100 of its configures arrives in at most three waits. A layer surface of one colour commits a 1×1 buffer at buffer scale 1 with its viewport at the configured size, keeps its input region, follows a new size and a fractional scale with the destination alone, replaces the pixel only for another colour, and a compositor without one-pixel buffers or the viewporter refuses it. `tests/mutter.rs` runs
`simple-window` with a private bus and headless Mutter, asserting 30 frames, 320×240 size, scale 1 and client-side
decorations. An absent executable skips; an installed compositor or client failure fails. `tests/weston.rs` remains an
optional check on Weston's
headless backend; `cupertino-server`'s `tests/standard-clients.rs` runs it and `cupertino-testclient` on the real server.
