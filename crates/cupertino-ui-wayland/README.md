# cupertino-ui-wayland

Draws a surface of the transport (`cupertino-wayland-client`) with tiny-skia. MPL-2.0. It binds no protocol beyond the
transport's standard set and never reaches the private protocols (`cupertino-layers` denies `cupertino-protocols` and
`cupertino-shell-client`). Depends on `cupertino-wayland-client`, `cupertino-geometry` and `tiny-skia`.

## Public API

| Item | Contents |
|---|---|
| `ShmPresenter`, `Ready` | one per surface, owned by the application, holding no protocol object: `configured(size)` after a configure, `frame_done` on the surface's `Event::Frame`, `request_redraw`; `poll(&Connection)` until `Ready::Idle` reports a new scale (`Ready::ScaleChanged`) first and then `Ready::Draw` once the surface is configured, a frame is asked for, none is in flight and the transport has a free buffer; `draw(&mut Connection, damage, paint)` paints and presents a frame |
| `popup::placement` | the `PopupPlacement` of a popup covering a parent-local rectangle, rounded outwards to whole points, its top-left corner at the rectangle's; an empty or non-finite rectangle is `Error::InvalidShape` |
| `clipboard::{TEXT_TYPES, set_text, text}`, `clipboard::Clipboard` | the text clipboard: a copy offers `text/plain;charset=utf-8`, `text/plain` and `UTF8_STRING`; a paste reads the first of them an offer has, drops a character cut by the transport's read limit and is `None` when the offer was replaced or its client did not answer in time. `Clipboard` keeps the latest selection (`selection_changed`) and the text this client set, which a paste reads while no selection is known |
| `rgba_to_argb8888`, `Error` | the pixel conversion into shm buffers, and errors |

## How it works

`draw` keeps the last frame's `Pixmap`. Without damage the pixmap is cleared and the frame is whole. With damage
(rectangles in points) only those rectangles, rounded out to device pixels and cut to the buffer, are cleared for
`paint`; only they and what the leased buffer missed of the frames shown since it last held one
(`BufferLease::missed`) are converted into it, and only they are reported with `damage_buffer`. Rectangles that add up
to the buffer convert it once, whole. A frame at another size or scale is drawn whole.

`draw_with` hands `paint` the device-pixel rectangles that were cleared (`None` when the frame is whole), for a painter
that draws a whole frame elsewhere and copies those rectangles in (`copy_rects`). `Damage` collects what changed between
frames in points: it is the whole surface when new, merges overlapping rectangles and keeps at most eight apart; `take`
gives the rectangles for the next frame, or `None` for all of it.

The configure serial needs no handling here: the transport sizes the leased buffer from the surface's latest configure
and acknowledges exactly that serial with the commit. The lease holds the connection from `lease` to `present`, so no
configure is dispatched while `paint` runs, and a configure overtaken before the draw is never acknowledged. The frame
callback's time, for an animation clock, is `FrameCallback::time` of the transport's `Event::Frame`.

## Tests

```
cargo test -p cupertino-ui-wayland
```

Unit tests cover the pixel conversion, the damage cut, popup placement and the clipboard text. `tests/protocol-flow.rs`
drives the presenter and the clipboard against an in-process server.
