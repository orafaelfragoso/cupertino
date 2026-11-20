# cupertino-session-tests

The home of every test that starts programs from more than one crate: a server with the shell processes, an app as a
client of the server, a session as a whole. A test that starts only its own crate's program stays in that crate's
`tests/`. Testing layer: only dev-dependencies may point at it, and it is never shipped.

The tests are in `tests/`, each a whole-process test of public behaviour. The library holds no code; it exists because
a package needs a target.

```
cargo nextest run -p cupertino-session-tests
```

`tests/shell-host.rs` runs the real server headless, drawing one offscreen output with the software rasteriser, and
gives its supervisor the test binary itself as the host program: started with the mark of `tests/shell-host/host.rs`,
the binary is a minimal `cupertino-shell` built from `cupertino_kit_shell::hosting`, hosting the test components of
`tests/shell-host/components.rs`. The hosts write what they see to a report file under `CARGO_TARGET_TMPDIR`, which
the tests read beside the server's own state and the output's pixels. The tests cover a component's first frame,
persistence and ready; the menu bar and dock roles; a held surface across a killed host; a transaction with a hole
claiming a client's window; `capture::once` from a privileged program; the bus bridge between the shared host and a
separate one; and the busy, stuck-helper and restart handling.
