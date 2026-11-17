# cupertino-session-tests

The home of every test that starts programs from more than one crate: a server with the shell processes, an app as a
client of the server, a session as a whole. A test that starts only its own crate's program stays in that crate's
`tests/`. Testing layer: only dev-dependencies may point at it, and it is never shipped.

The tests are in `tests/`, each a whole-process test of public behaviour. The library holds no code; it exists because
a package needs a target.

```
cargo nextest run -p cupertino-session-tests
```

No test starts programs from more than one crate today, so the crate has no `tests/` folder.
