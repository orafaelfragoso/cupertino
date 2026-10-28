# cupertino-test-bus

A private session bus for the workspace's integration tests, as a dev-dependency only.

## Public API

| Item | Contents |
|---|---|
| `PrivateBus` | `start()` runs `dbus-daemon --session --nofork` as a child that is killed on drop and, through `PR_SET_PDEATHSIG`, with the thread that started it; `None` when the daemon is not installed. `address`, `pid()`, `builder()` / `blocking_builder()` (method calls time out after `WAIT`), `connect()` (blocking) and `connect_async()` |
| `WAIT` | the deadline of the daemon's address and of every method call: 5 s |

Start the bus on the test's own thread: the daemon dies with the thread that started it.

## Tests

```
cargo test -p cupertino-test-bus
```
