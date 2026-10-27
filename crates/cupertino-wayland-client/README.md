# cupertino-wayland-client

Wayland client transport for application windows over standard protocols only, so the same code runs on Cupertino and
on any other compositor. Depends on `cupertino-geometry`, `wayland-client`, `wayland-protocols` and `xkbcommon` (the
workspace's pure-Rust `cupertino-xkbcommon`); `cupertino-layers` fails the build if it ever reaches
`cupertino-protocols` or another compositor's protocol crates.

## Public API

| Item | Contents |
|---|---|
| `Connection::{connect, from_stream}`, `Waker` | the connection (`WAYLAND_DISPLAY` / `WAYLAND_SOCKET`, or a given socket); `Connection::wait` returns the next `Event`s, a `Waker` wakes it from another thread |
| `Capability`, `CapabilitySet` | the optional protocols the compositor offers: seat, clipboard, fractional scale, server-side decorations, activation, cursor shape. `Connection::capabilities` lists them; a call that needs a missing one fails with `Error::Unsupported` |
| `Connection::toplevel`, `ToplevelConfig`, `Toplevel` | an xdg toplevel: app id, title, size limits, the decorations asked for. `set_title`, `set_size_limits`, `set_maximized`, `set_fullscreen`, `minimize`, `start_move`, `start_resize`, `activate`, `destroy` |
| `WindowConfigure`, `WindowStates`, `Decorations` | a configure: serial, size (or none: the client picks), bounds, states, and the decorations granted. A compositor without `zxdg_decoration_manager_v1`, or one that answers client-side, gives `Decorations::Client` |
| `Connection::popup`, `PopupPlacement`, `PopupConfigure`, `Popup`, `Anchor`, `Gravity`, `ConstraintAdjustment` | xdg popups of a toplevel or popup, with an optional grab; `Event::PopupDone` when the compositor dismisses one |
| `Connection::lease`, `BufferLease`, `BufferLayout`, `BufferFormat`, `PixelRect` | an shm buffer for the next frame at the surface's scale (`Argb8888` or `Xrgb8888`), at most `BufferLayout::MAX_EXTENT` (16384) pixels per side, up to three per surface; `present` or `present_damage` acks the latest configure, attaches, damages, asks for a frame callback and commits |
| `Event::{Frame, ScaleChanged, OutputsChanged}`, `FrameCallback`, `Scale`, `OutputInfo`, `OutputId` | frame pacing, the surface's scale, outputs |
| `PointerEvent`, `KeyboardEvent`, `Button`, `Modifiers`, `CursorShape`, `ResizeEdge` | pointer and key input; keys carry keysym and text from the compositor's keymap, key repeat runs in the client. `Connection::set_cursor` sets a named cursor (`wp_cursor_shape_v1`) |
| `Connection::{set_selection, receive}`, `ClipboardOffer`, `Event::Selection` | the clipboard: offer contents in several MIME types, list and read another client's |
| `Error`, `Disconnect` | typed errors: a missing required protocol with its minimum version, the disconnect cause, a configure serial that did not increase, an unsupported format or MIME type, a denied or cancelled transfer, no free buffer |
| `State`, `ExternalRole`, `Connection::{queue_handle, globals, external_surface, wl_surface, wl_output, seat, roundtrip}` | the extension hook: another crate adds protocols of its own on the same queue (below) |

## How it works

`wl_compositor` (version 4 or later), `wl_shm` and `xdg_wm_base` are required; a missing one is
`Error::MissingRequiredProtocol` at connect, never a degraded mode. Everything else is a capability.

A `BufferLease` borrows the connection, so no configure arrives between drawing and presenting: the buffer always
answers the latest configure, which is acked once, with the first buffer presented for it. The scale is the fractional
scale when `wp_fractional_scale_v1` and `wp_viewporter` are offered, else `wl_surface.preferred_buffer_scale`, else the
largest scale of the outputs the surface entered; without the viewporter it is rounded up to a whole number.

Copying serves each paste on a short-lived thread (at most 8 at once, 1 s each); pasting reads at most 16 MiB within
1 s, and returns the client's own contents directly.

## Extensions

A crate above the transport, such as one for the desktop's own protocols, adds them on the connection's queue, so its
events stay in order with the standard ones and `Connection::wait` dispatches both in one pass. The transport itself
names no protocol beyond the standard ones.

- Binding: `Connection::globals` lists the compositor's globals; bind with `Connection::queue_handle` and user data
  of a type the extension crate defines. Rust allows the extension's `impl Dispatch<TheirInterface, TheirData> for
  State` only with that local type; `()` is refused.
- Events: the `Dispatch` impl puts what it receives where the extension reads it, typically a queue shared through the
  user data. `Connection::wait` returns after each read from the compositor, maybe with no `Event`, once everything read
  is dispatched; the extension then reads its queue. A burst of extension events costs one wait, not one each.
- Surface roles: `Connection::external_surface(assign)` creates a `wl_surface` and calls `assign` with it, the queue
  handle and its `SurfaceId`, before the first commit; `assign` creates the role's objects and returns an
  `ExternalRole`. Each configure of the role goes to `State::configure_external(surface, serial, size)` from the
  extension's `Dispatch` impl (`None` leaves the size to the client; a serial that does not increase ends the next
  wait with `Error::InvalidSerial`). The surface then draws through `Connection::lease` like any other: present calls
  `ExternalRole::ack_configure` before the commit, once per configure. `Connection::destroy` destroys its popups, calls
  `ExternalRole::destroy`, then the surface. `Connection::popup` with it as parent creates the popup without a parent
  and hands it to `ExternalRole::adopt_popup` before its first commit; the default refuses.
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
cargo test -p cupertino-wayland-client
cargo nextest run -p cupertino-wayland-client --run-ignored only   # needs weston
```

`tests/protocol-flow.rs` runs a minimal protocol server in-process: required globals, configure and ack, resize, buffer
reuse, scale, decorations, popups, input, clipboard and disconnect. Its `layer` module is an extension written against
the public API (layer shell, a protocol the transport does not know): a layer surface draws through the lease, acks
through its own protocol, takes a popup, and a burst of 100 of its configures arrives in at most three waits. `tests/weston.rs` runs `simple-window` on Weston's
headless backend; `cupertino-server`'s `tests/standard-clients.rs` runs it and `cupertino-testclient` on the real server.
