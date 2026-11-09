# cupertino-diagnostic-events

Versioned diagnostic events of every Cupertino process: what failed, where, and in which build. Layer: `foundation`.
Licence: MPL-2.0. It depends on `cupertino-build-info`, `serde` and `serde_json` only, so every process, the toolkit and
the C library can link it.

An event is recorded beside a failure's existing log line and error return, never instead of them: `emit` returns
nothing, never panics and waits at most for one bounded write.

## Use

```rust
use cupertino_diagnostic_events::{self as diag, Severity, Value};

fn main() {
    // the first line of `main`: once per process
    diag::init(diag::component!(), "dock", diag::Sink::UserStore);
    // …
}

// at a failure site, beside its log line
diag::event(Severity::Warn, "dock.icon.load_failed", "an icon could not be loaded")
    .field("app_id", Value::Public(app_id))
    .field("error", Value::Public(&e.to_string()))
    .emit();
```

| Item | What it does |
|---|---|
| `init(component, area, sink)` | Installs the process's `Emitter` (a second call keeps the first), chains the panic hook and writes the open rate-limit summaries at exit. `area` starts the ids of the process's own events: `<area>.panic` and `<area>.exit_error`. |
| `Emitter::install(component, area, sink)` | The emitter alone; `Err(first)` when one is installed. `Emitter::get()` gives it. |
| `event(severity, id, message)` | Starts an event. `id` is stable and dotted (`area.subject.what`); `message` is a `&'static str` fixed by the code and never formatted from inputs, which go into fields. |
| `.field(key, Value)`, `.emit()` | Adds a typed field; records the event. Before `init` an event is dropped and counted (`dropped_before_install()`). |
| `install_panic_hook()` | Records `fatal` `<area>.panic` (the message through the scrubber, the location's file basename and line, the thread name), writes the open summaries, then calls the hook that was there before (so the server's own hook still writes `server.log`). |
| `exit_error(&error)` | Records `error` `<area>.exit_error` with the error's text through the scrubber, for a process about to exit because of it; the caller still prints and returns its error. |
| `set_context(key, Value)` | A field every later event of the process carries (the server's `backend` and `renderer`); an event's own field of that key wins. |
| `recent()` | The last events of the process (the memory ring), for crash reports and tests. |
| `Emitter::prepare_fatal(id, message, key, values)` | Lines a fatal signal handler writes with no lock and no allocation, one per value of `key`, to `fatal-<boot>-<pid>.jsonl` in the store (`FatalRecord::path`, `FatalRecord::render`). |
| `store::{Store, read, segments, user_dir}` | The store itself, and reading it back (used by `cupertino_diagnostics::events`). |
| `journal::{datagram, parse, Journal}` | The journal's native protocol. |
| `redact::{Policy, Salt}` | The redaction policy and the salted hash. |
| `time::{format, parse}` | RFC 3339 UTC times with milliseconds. |

### Sinks

| `Sink` | For | Where events go besides the memory ring |
|---|---|---|
| `UserStore` | user processes: the server, shell processes, apps, the portal, the polkit agent | `$XDG_STATE_HOME/cupertino/events/` (else `~/.local/state/cupertino/events/`) |
| `Store(dir)` | tests | a store in `dir` |
| `Journal` | system services: `cupertino-sessiond`, `-netd`, `-btd`, `-powerd` | one datagram per event to `/run/systemd/journal/socket`; without the socket, one line on standard error |
| `Memory` | tests | nowhere else |

Two variables are for tests only:
- `CUPERTINO_DIAG_JOURNAL_SOCKET` names another journal socket: a test binds a `UnixDatagram` in a temporary directory
  and reads the datagrams.
- `CUPERTINO_DIAG_TEST_PANIC=<area>` makes the process whose `init` names that area panic right after it, so a test
  drives the panic path of a real binary.

## The record (`cupertino.diag.v1`)

One JSON object per line, with its fields in this order (`tests/goldens/event-v1.jsonl`):

| Field | Value |
|---|---|
| `schema` | `"cupertino.diag.v1"` |
| `time` | RFC 3339 in UTC with milliseconds, `2026-11-09T14:03:27.512Z` |
| `boot_id` | `/proc/sys/kernel/random/boot_id`, or `null` |
| `session` | `XDG_SESSION_ID`, or `null` |
| `component`, `version`, `commit` | the binary's name, its package version and the source commit (`cupertino-build-info`), or `null` for an unknown commit |
| `pid` | the process id |
| `id` | stable and dotted, `area.subject.what`, for example `server.gpu.reset` |
| `severity` | `info`, `warn`, `error` or `fatal` |
| `message` | a fixed text chosen by the code |
| `fields` | the event's typed values, in the order given, then the process's context fields |
| `source` | the crate and the file's basename with the line, `cupertino-server:gpu_reset.rs:120`; no directories |
| `redacted` | the keys of the fields the policy removed or rewrote |
| `count` | how many events of this id the process has recorded, this one and the suppressed ones included |
| `suppressed` | on a rate-limit summary, how many events of this id its window did not write; 0 otherwise |
| `truncated` | present and `true` when the event was longer than `EVENT_BYTES` and lost field text or fields |

A duration is written as whole milliseconds. Consumers ignore fields they do not know; a change that is not additive
raises the schema to `cupertino.diag.v2`.

## Limits

| Constant | Value | What it bounds |
|---|---|---|
| `SEGMENT_BYTES` | 1 MiB | a segment of the store; the next line that would pass it opens the next one |
| `STORE_BYTES` | 8 MiB | the whole store; the oldest segments are deleted while it holds more |
| `RETENTION_DAYS` | 14 | a segment last written longer ago is deleted |
| `EVENT_BYTES` | 4 KiB | one event's line: a longer one has its string fields cut to `FIELD_TEXT_LIMIT` (256) characters, then fields dropped from the last, and `truncated: true` |
| `RING_EVENTS`, `RING_BYTES` | 256 events, 256 KiB | the memory ring of each process |
| `RATE_PER_ID`, `RATE_WINDOW` | 10 events, 10 s | events of one id written per window, from its first event; the rest are counted and written as one summary (the last suppressed event with `suppressed: N`) when the window closes, by a thread started on first need, or at exit |

The store is local only and on by default; nothing is uploaded. `cupertino diagnose events clear` deletes it.

### The store

- `events/` is mode 0700, its files 0600. A directory that is a symbolic link, belongs to another user or that the
  group or others may write is refused, checked at every write.
- Segments are `events-<boot>-<seq>.jsonl` (`<boot>` the first 8 digits of the boot id, `<seq>` a number that grows
  across boots), `fatal-<boot>-<pid>.jsonl` holds what a fatal signal handler wrote, `lock` is the lock, `salt` the
  salt of the hashes.
- A line is one `write(2)` to a file opened with `O_APPEND`, under `flock(LOCK_EX)` on `lock`, so the lines of many
  processes never mix. A short write (a full disk) is cut back off, so the next line starts on a line of its own.
  Rotation and deletion happen under the same lock.
- **Unavailable** (a read-only file system, a full disk, permission denied, a refused directory, no home): events go to
  the ring only, one `warning` line per reason per process goes to standard error, and once a write works again a
  `diag.store.unavailable` event says why (`reason`) and how many events were kept in memory only (`kept_in_memory`).
- Readers tolerate a last line cut short (`partial`), lines that are not records of this schema (`corrupt`: garbage,
  invalid UTF-8, another schema) and segments deleted while they read (`vanished`).

### The journal

A system service sends one datagram of the journal's native protocol per event, written here without libsystemd:
`MESSAGE`, `PRIORITY` (`fatal` 2, `error` 3, `warn` 4, `info` 6), `SYSLOG_IDENTIFIER` (the binary),
`CUPERTINO_DIAG_SCHEMA=1`, `CUPERTINO_DIAG_ID`, `CUPERTINO_DIAG_SEVERITY` and `CUPERTINO_DIAG_JSON` (the whole redacted
line). A value with a newline uses the binary-safe form (the key, a newline, its length as 64-bit little-endian, the
value, a newline). The socket is non-blocking, so a full journal never holds the service up; the journal's own limits
bound what it keeps. A service's salt is `diagnostic-salt` in the state directory systemd gives it
(`STATE_DIRECTORY`), else one for the process alone.

## What diagnostic events never contain

Every value is reduced before it is serialised, so nothing below reaches the ring, the store or the journal
(`tests/redaction.rs` has one row per class).

| Class | `Value` | Kept as |
|---|---|---|
| Passwords, tokens, keys, cookies, `Authorization` values, the text of an authentication conversation | `Secret` | `"[redacted]"` |
| Typed text, the clipboard, window titles, notification bodies, searches, file names a person chose, URLs | `UserText` | `"[redacted]"` |
| A path under the home directory | `Path` | `~` and the desktop's own `.local/state/cupertino/…` and `.config/cupertino/…` paths as they are; anything else `~/…#<hash>`, the salted hash of its basename |
| A path under `/usr`, `/etc`, `/run`, `/dev`, `/sys` or `/proc` | `Path` | the path, with user and host names replaced |
| Any other path (another home, `/tmp`, removable media, a relative path) | `Path` | `/home/…#<hash>`, `/root/…#<hash>` or `…#<hash>` |
| IP, MAC and Bluetooth addresses, SSIDs, device names | `Address(kind, …)` | `<kind>#<hash>`: `ip#`, `mac#`, `bluetooth#`, `ssid#`, `device#` |
| User and host names | `Identifier(User \| Host, …)` | `"[user]"`, `"[host]"` |
| App ids, component and unit names, error kinds, errno names, interfaces and methods, GPU vendor, driver and PCI ids, connector and output names | `Public` | as they are |
| Counts, numbers, durations, flags | `Count`, `Int`, `Duration`, `Bool` | as they are |

A hash is 8 hexadecimal digits of SipHash-2-4 keyed by the store's `salt` (16 random bytes, 0600): the same value
correlates within one machine and never across machines.

**The free-text scrubber** runs on every `Public` string (an error's text may hold anything) and on panic messages. It
rewrites, in this order: paths under a home directory (`/home/…`, `/root/…`, `~/…` and the user's own home, spaces
included, up to a quote, a bracket, a comma or a colon or semicolon followed by a space), URLs (`[url]`), other absolute
paths as above, e-mail addresses (`[email]`), `user@host` (`[user]@[host]`), `key=value` and `key: value` pairs whose
key contains `pass`, `secret`, `token`, `key`, `cookie` or `auth` (the value becomes `[redacted]`, after a `Bearer` or
`Basic` too), IPv4, IPv6 and MAC addresses (`ip#…`, `mac#…`), runs of 32 or more hexadecimal or base64 characters that
are all hexadecimal or mix digits and letters (`[token]`), and the user's and host's names as whole words. Connector
names (`DP-1`), PCI ids (`0000:01:00.0`), times (`12:30:45`) and Rust paths (`a_b::c`) stay.

## Phase 6 hook: output-scoped events

Events about one output carry its identity as plain `Public` fields, which the policy keeps: `output` (the output's
name) and `connector` (the connector, `DP-1`), plus the GPU's PCI id when it matters. An output that fails every frame
is bounded by the per-id limit: ten events per id per ten seconds, then one summary per window with `suppressed`, so a
flood costs at most two lines a window per id. Atomic-commit failures, scanout reasons and per-output metrics use this
form; this crate adds no backend or DRM events itself.

## Event ids

Every id a test can drive, where it comes from, and how. `tests/registry.rs` checks this table against
`grep -rn 'diag::event(' crates/`, the processes that call `init`, and the ids written here or by the reader.

<!-- event ids: begin -->
<!-- event ids: end -->
