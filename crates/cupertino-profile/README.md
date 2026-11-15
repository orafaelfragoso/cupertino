# cupertino-profile

Frame timelines of every Cupertino process: where each frame's time goes, as spans on a timeline. Layer: `foundation`.
Licence: MPL-2.0. It depends on `tracing`, `tracing-subscriber` (registry only), `rustix` and `libc`, so the toolkit,
its processes and the server all link it.

The spans themselves are `tracing::trace_span!` in the crates that do the work (the toolkit's run loop, its renderers
and presenter, the compositor's frame); this crate records them when `CUPERTINO_PROFILE` asks, and only then. Unset,
no layer is installed, and a span costs its callsite's check that nothing listens.

## `CUPERTINO_PROFILE`

| Value | What is recorded |
|---|---|
| `chrome:FILE` | the process's spans, written to `FILE` in the Trace Event format (JSON), which Perfetto (`ui.perfetto.dev`) and `chrome://tracing` open |
| `chrome:DIR/` | the same, into `DIR/<program>-<pid>.json`: one file per process, so one value serves a whole session (an existing directory needs no slash) |
| `tracy` | the spans streamed to the Tracy profiler, in a build with its client (below) |
| `chrome:DIR/,tracy` | both |

A span is written as a complete event (`"ph":"X"`) when it ends, its fields as the event's arguments; a field named
`label` goes into the event's name (`ui.message cupertino_launcher::app::Message`). Times are the monotonic clock's
in microseconds, the clock of the frame and presentation times, and every event carries the process's real id, so the
events of the files of one session concatenated into one array open as one trace with a track per process and thread.
`interval(name, start_ns, end_ns)` draws a wait measured already (the compositor's late start after a vblank) on a
track of its thread's own, beside the thread's spans.

The file is written as the process goes, at most `FLUSH_DELAY` (100 ms) after a span ends and a whole event at a time,
by a thread that wakes only when there are events; the array is closed when the process exits (`finish` closes it
earlier). A process killed before that leaves the file without the closing `]`, which the format allows and both viewers
read.

## API

| Item | Contents |
|---|---|
| `layer::<S>()` | the layer of the sinks the variable names, filtered to the workspace's spans (targets starting with `cupertino`), for a process's own subscriber: the server adds it beside its log layers; `None` when unset |
| `init()` | installs a subscriber of that layer for a process with none (every toolkit process, from `WaylandWindowSystem::connect_as`); nothing when unset |
| `parse`, `Sink`, `layer_of`, `program` | the variable's sinks, the layer of given sinks, the process's file name |
| `interval`, `now_ns`, `finish` | a measured wait on the timeline; the clock; closing the traces now |

## Tracy

The client is behind the `tracy` feature, which takes effect only with `--cfg cupertino_tracy` as well: every package
and CI build uses `--all-features`, which turns the feature on, and the cfg keeps the client (and its C++ library, which
listens for the profiler) out of them. A build that streams to Tracy, in a target folder of its own since the flag
rebuilds every crate:

```sh
RUSTFLAGS="--cfg cupertino_tracy" cargo build --release --workspace --all-features --bins --target-dir target/tracy
BIN_DIR=target/tracy/release CUPERTINO_PROFILE=tracy crates/cupertino-server/scripts/session.sh --spawn cupertino-files
```

The client collects only while a profiler is connected, is reachable from this machine alone, and installs no handler
of fatal signals. It speaks the protocol of Tracy 0.14: use that version's profiler.
