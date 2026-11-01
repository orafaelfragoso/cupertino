# cupertino-screencast

PipeWire video source streams for screen sharing: one stream for each share, which a consumer such as a browser
connects to by its node id. This is the only crate that links libpipewire, and only `cupertino-portal` (feature `app`)
uses it. Layer: `platform`.

## Public API

| Item | Contents |
|---|---|
| `Screencast` | the connection to the user's PipeWire, served by a thread of its own: `connect(notices)`, `open(stream, name, offer)`, `resize(stream, size)`, `dequeue(stream)`, `queue(stream, buffer, frame)`, `close(stream)`, `is_connected()`; dropping it closes every stream |
| `Notice` | what the thread reports: `Node` (the node id), `State`, `Added` (a buffer and its memfd), `Removed`, `Dequeued` (a buffer to fill, or none) |
| `State` | `Connecting`, `Negotiated(Format)`, `Streaming(Format)`, `Closed(reason)` |
| `Offer` | the source size and the pixel layouts it can be captured in, in order of preference |
| `Format` | the fixed `width`, `height`, `pixel` (`Bgrx` or `Bgra`) and `framerate`; `stride()`, `frame_len()` |
| `Frame` | the damage of a filled buffer and its timestamp on `CLOCK_MONOTONIC` |
| `format::negotiate` | the format a consumer's fixed `Format` param means for an offer, or the `Mismatch` |
| `fake::Daemon`, `fake::Consumer` | feature `fake`: a private `pipewire` daemon in a temporary runtime directory, and a video consumer that links itself to a node and reports the format and frames it sees, for the tests of the crates that share screens |

## How it works

- `connect` starts the thread `pipewire`, which runs the PipeWire main loop; commands reach it over a
  `pipewire::channel`, and `notices` runs on it, so the owner hands them on without blocking. The connection goes to
  the PipeWire that `PIPEWIRE_REMOTE` or the runtime directory names.
- Each stream is an output that drives the graph (`DRIVER`) and allocates its own buffers (`ALLOC_BUFFERS`). It offers
  the source size in each layout of the offer, a variable framerate up to 60 with 30 as the default, and takes the
  maximum framerate the consumer fixes.
- A fixed format sets the buffer params: 4 buffers (2 to 8) of one memfd each, with a header meta (the timestamp and
  a sequence number) and a damage meta of up to 16 rectangles. Each memfd is announced as `Added` with a duplicate fd,
  so the owner writes frames into it directly.
- The owner asks for a buffer with `dequeue`, writes the frame at offset 0 and the format's stride, and hands it back
  with `queue`. The damage is cut to the frame; more rectangles than fit become the whole frame. Each queue triggers one
  cycle of the graph.
- `resize` drops the buffers, offers the new size and waits for the consumer to fix a format again.
- PipeWire going away reports every stream `Closed` and ends the thread.

## Known limitations

- Shared memory only: no dmabuf buffers, so every frame is copied once into the memfd.
- No cursor metadata: the cursor is either painted into the frame or left out.
- Video only, no audio.
- A frame is at most `i32::MAX` bytes, the largest `wl_shm` pool (`Size::shareable`): the portal ends the share of a
  larger source.

## Tests

```
cargo test -p cupertino-screencast --features fake
```

The format tests run anywhere. `tests/stream.rs` (feature `fake`) starts a private `pipewire` daemon in a temporary
runtime directory, never the user's, links a consumer stream to a shared stream, and checks the negotiated format, one
frame with its pixels, damage and timestamp, and a resize. It is skipped when `pipewire` is not installed.
`cupertino-server`'s `tests/screen_share.rs` runs the whole path: the portal binary against the headless server, a
private bus, and a consumer that reads frames of the output.
