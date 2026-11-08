# cupertino-test-bus

A private session bus for the workspace's integration tests, as a dev-dependency only.

## Public API

| Item | Contents |
|---|---|
| `PrivateBus` | `start()` runs `dbus-daemon --session --nofork` as a child that is killed on drop and, through `PR_SET_PDEATHSIG`, with the thread that started it; `None` when the daemon is not installed. `start_with_config(path)` runs it with a configuration file of the test's own (a policy, a listening address), which the caller keeps until the bus is dropped; `system_limits_config(dir)` writes one that allows everything with the daemon's built-in limits, those of a system bus (64 descriptors queued for one connection), for a test of what a flood does there. `address`, `pid()`, `has_exited()`, `builder()` / `blocking_builder()` (method calls time out after `WAIT`; the handshake has no deadline), `connect()` (blocking) and `connect_async()`, which panic when the handshake takes longer than `WAIT` |
| `WAIT` | the deadline of the daemon's address, of the handshake of `connect()` and `connect_async()`, and of every method call: 5 s |

Start the bus on the test's own thread: the daemon dies with the thread that started it.

## Tests

```
cargo test -p cupertino-test-bus
```
