# cupertino-diagnostic-events

Diagnostics of every Cupertino process. Layer: `system`. Licence: MPL-2.0. It depends on `serde`, `serde_json`,
`tracing`, `tracing-subscriber` (registry only), `rustix` and `libc`, so every process, the toolkit and the C library
can link it. It holds:

- the versioned diagnostic events: what failed, where, and in which build (this page down to [Event ids](#event-ids));
- `build_info`: what a build is and what it runs on ([Build information](#build-information));
- `profile`: the frame timelines `CUPERTINO_PROFILE` asks for ([Frame timelines](#frame-timelines)).

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
| `Scope::new(area)`, `.enter()`, `Scope::current()`, `.set_context(key, Value)` | One part of a process that records as its own, such as one of several toolkit instances: while a thread is in it (from `enter` until the guard drops), its panic is `<area>.panic`, its `exit_error` `<area>.exit_error`, and its events carry the scope's context fields over the process's. A thread in no scope records as the process. Every scope records through the process's one emitter and its one panic hook. |
| `recent()` | The last events of the process (the memory ring), for crash reports. |
| `Emitter::prepare_fatal(id, message, key, values)` | Lines a fatal signal handler writes with no lock and no allocation, one per value of `key`, to `fatal-<boot>-<pid>.jsonl` in the store (`FatalRecord::path`, `FatalRecord::render`). |
| `store::{Store, read, segments, user_dir}` | The store itself, and reading it back (used by `cupertino_diagnostics::events`). |
| `journal::{datagram, parse, Journal}` | The journal's native protocol. |
| `redact::{Policy, Salt}` | The redaction policy and the salted hash. |
| `time::{format, parse}` | RFC 3339 UTC times with milliseconds. |

### Sinks

| `Sink` | For | Where events go besides the memory ring |
|---|---|---|
| `UserStore` | user processes: the server, shell processes, apps, the portal, the polkit agent | `$XDG_STATE_HOME/cupertino/events/` (else `~/.local/state/cupertino/events/`) |
| `Store(dir)` | a process that keeps its events apart | a store in `dir` |
| `Journal` | system services: `cupertino-sessiond`, `-netd`, `-btd`, `-powerd` | one datagram per event to `/run/systemd/journal/socket`; without the socket, one line on standard error |
| `Memory` | a process that keeps no store | nowhere else |

Two variables are for isolated runs only:
- `CUPERTINO_DIAG_JOURNAL_SOCKET` names another journal socket: a run binds a `UnixDatagram` in a temporary directory
  and reads the datagrams. `cupertino-sessiond` passes it on to the workers it starts with an otherwise empty
  environment, so their events reach the same socket.

An isolated run never writes to the user's store or the system journal: every process it starts that may record events
gets an `XDG_STATE_HOME` and a `CUPERTINO_DIAG_JOURNAL_SOCKET` of its own (as its sink needs).
- `CUPERTINO_DIAG_TEST_PANIC=<area>` makes the process whose `init` names that area panic right after it, so the panic path
  of a real binary can be driven.

## The record (`cupertino.diag.v1`)

One JSON object per line, with its fields in this order:

| Field | Value |
|---|---|
| `schema` | `"cupertino.diag.v1"` |
| `time` | RFC 3339 in UTC with milliseconds, `2026-11-09T14:03:27.512Z` |
| `boot_id` | `/proc/sys/kernel/random/boot_id`, or `null` |
| `session` | `XDG_SESSION_ID`, or `null` |
| `component`, `version`, `commit` | the binary's name, its package version and the source commit (`build_info`), or `null` for an unknown commit |
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

Every value is reduced before it is serialised, so nothing below reaches the ring, the store or the journal.

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

Every id, where it comes from, and how. The table matches `grep -rn 'diag::event(' crates/`, the processes that call `init`, and the ids written here or by the reader.

| Id | Severity | Component | When |
|---|---|---|---|
| `<area>.panic` | fatal | every process below, by its `init` area | the process panicked |
| `<area>.exit_error` | error | every process but the server, which records `server.session_end` | the process is about to exit because of an error |
| `diag.store.unavailable` | warn | any user process | its store works again after it could not be written; `reason`, `kept_in_memory` |
| `server.session_end` | info, error | `cupertino-server` | the server ended: `status`, and `error` for an end with one |
| `server.fatal_signal` | fatal | `cupertino-server` | a fatal signal ended the server (`signal`), written by the signal handler |
| `server.gpu.reset` | warn, error | `cupertino-server` | the GPU was reset (`gpu`, `reason`, `outcome`, `remade`); `error` when it could not be remade |
| `server.renderer.software` | warn | `cupertino-server` | an output renders with a software renderer |
| `server.shell.exited` | warn | `cupertino-server` | a shell process ended and is started again (`role`, `status` or `signal`, `restart_in`) |
| `server.shell.gave_up` | info | `cupertino-server` | a shell process ended successfully and is not started again |
| `server.shell.start_failed` | warn | `cupertino-server` | a shell process could not be started at `stage` |
| `server.xwayland.start_failed` | warn | `cupertino-server` | the X11 satellite could not be started |
| `server.xwayland.exited` | info, warn | `cupertino-server` | the X11 satellite ended |
| `server.authorization.refused` | warn | `cupertino-server` | a connection asked for what a role it does not hold allows (`interface`, `app_id`, `role_required`) |
| `server.lock.client_lost` | warn, error | `cupertino-server` | the lock screen's client went away while the session was locked |
| `server.lock.fallback_shown` | warn | `cupertino-server` | the server covered the locked session itself (`reason`, `outputs`) |
| `power.suspend.failed` | error | `cupertino-server`, `cupertino-sessiond` | a sleep did not happen: the idle policy's (`reason`), or the login screen's that logind failed |
| `power.inhibit.failed` | warn | `cupertino-server` | logind gave no sleep delay inhibitor |
| `toolkit.renderer.fallback` | warn | any toolkit process | a process or surface that wanted the GPU draws on the CPU (`program`, `scope`: `process`, `surface` or `loss`, `reason`); a purge is not one |
| `toolkit.task.overdue` | warn | any toolkit process | an I/O task ran past the deadline its caller named (`instance`, `deadline` in milliseconds, `site`), once a task |
| `login.power.failed` | error | `cupertino-sessiond` | logind failed to restart or shut down (`action`) |
| `login.auth.failed` | warn | `cupertino-sessiond` | PAM refused an authentication (`service`, `stage`, `error`, `outcome`, `failures`); never the account |
| `login.session.start_failed` | error | `cupertino-sessiond` | a session could not be started at `stage` |
| `login.greeter.exited` | warn | `cupertino-sessiond` | the login screen's session ended (`status` or `signal`, `restarting`) |
| `network.backend.start_failed` | error | `cupertino-netd` | the network backend could not start |
| `network.request.failed` | warn | `cupertino-netd` | a D-Bus request failed (`interface`, `method`, `error`, the `ssid` hashed) |
| `bluetooth.backend.start_failed` | error | `cupertino-btd` | the Bluetooth backend could not start |
| `bluetooth.request.failed` | warn | `cupertino-btd` | a D-Bus request failed (`interface`, `method`, `error`) |
| `power.backend.start_failed` | error | `cupertino-powerd` | the power backend could not start |
| `power.request.failed` | warn | `cupertino-powerd` | a D-Bus request failed (`interface`, `method`, `error`) |
| `portal.request.failed` | warn | `cupertino-portal` | a portal request was refused (`interface`, `method`, `app_id`, `error`) |
| `portal.screencast.failed` | error | `cupertino-portal` | a screen cast could not be started |
| `portal.screenshot.failed` | error | `cupertino-portal` | a screenshot could not be taken |
| `portal.permission.store_failed` | warn | `cupertino-portal` | a screen cast permission could not be stored |
| `polkit.register.failed` | error | `cupertino-polkit-agent` | the agent could not register with polkit |
| `polkit.auth.failed` | warn | `cupertino-polkit-agent` | an authentication failed; never its text |
| `session.start`, `session.end` | info | `cupertino-server` | read from `session.log` by `cupertino diagnose events`, not recorded: a session started, or ended normally or by a stop |
| `session.end_abnormal` | error | `cupertino-server` | read from `session.log`, not recorded: the compositor ended abnormally |

The areas: `server`, `dock`, `menubar`, `launcher`, `notifications`, `controlcenter`, `osd`, `screenshot`,
`windowmanager`, `login_ui` (`cupertino-login`), `files`, `settings`, `portal`, `polkit` (`cupertino-polkit-agent`),
`keyring` (`cupertino-keyring-prompter`), `login` (`cupertino-sessiond`), `network`, `bluetooth` and `power`
(`cupertino-netd`, `-btd`, `-powerd`).

## Build information

`build_info` says what a Cupertino build is and what it runs on. It reads nothing at run time beyond the two files
below; the commit is found once, by the crate's build script.

### Public API

| Item | What it gives |
|---|---|
| `COMMIT: Option<&str>` | The full hash of the commit the sources come from, in lower case, or `None` when the build could not know it. |
| `COMMIT_SOURCE: CommitSource` | Where `COMMIT` came from: `Environment`, `Archive`, `Checkout` or `Unknown` (`name()` gives the lower-case word). |
| `short_commit()` | The first 12 digits of `COMMIT`. |
| `Component { name, version }`, `component!()` | A program or library of the desktop. `component!()` takes the caller's `CARGO_PKG_NAME` and `CARGO_PKG_VERSION`; `component!("cupertino")` names a binary whose name differs from its package. Its `Display` is `cupertino 0.1.0 (0123456789ab)`, without the parenthesis when the commit is unknown. |
| `os_image()`, `read_os_image(root)`, `OsImage::parse(text)` | `NAME`, `ID`, `VERSION_ID`, `BUILD_ID`, `IMAGE_ID` and `IMAGE_VERSION` from `etc/os-release`, or `usr/lib/os-release` when the first does not exist, under a root. Every field is optional. `read_os_image` returns `NotFound` when neither file exists and the read error otherwise, so a caller can tell missing from unreadable; `os_image()` reads `/` and gives an empty `OsImage` on any error. |
| `kernel()`, `kernel_in(root)` | The running kernel's release from `proc/sys/kernel/osrelease`, or `None`. |

The parser follows the `os-release` format: blank lines and `#` comments are skipped, a value may be in double quotes
(with `\"`, `\\`, `\$` and `` \` `` escapes) or single quotes (literal), a backslash in an unquoted value stands for the
character after it, a later line for the same key wins, and an empty value counts as not set.

### Where the commit comes from

`build.rs` takes the first source that knows the commit (`build/commit.rs`):

1. `CUPERTINO_BUILD_COMMIT` from the environment, for a build that knows its commit otherwise. It must be a full
   hash, 40 hexadecimal digits (or 64 for a repository with sha256 object names); any other value fails the build,
   because it was set on purpose.
2. The tracked file `data/build-commit`. In the repository it holds `$Format:%H$`, and `.gitattributes` marks it
   `export-subst`, so `git archive` (and so the release source archive) writes the archived commit into it. A file with
   any other text is skipped with a Cargo warning.
3. `git rev-parse HEAD` when the workspace has a `.git` (a directory, or the file of a linked worktree).
4. Otherwise the commit is unknown, and `COMMIT` is `None`.

The script reruns only when `CUPERTINO_BUILD_COMMIT`, `data/build-commit` or the script itself change. A new commit in a
checkout does not rerun it, so it does not rebuild every binary that links this crate, and a shared build directory
stays reusable across commits. In a development build `COMMIT_SOURCE` is then `Checkout` and the value can be older
than the checkout's `HEAD`; `cargo clean -p cupertino-diagnostic-events` or `CUPERTINO_BUILD_COMMIT` gives the exact one.
Package builds start from a fresh source tree, the release archive (stable recipe) or a clone (development
recipe), and get the exact commit.

The commit does not affect reproducibility: it is a fixed input of a given source tree, like the sources themselves.
Two builds of the same archive or the same checkout `HEAD` embed the same value; nothing else here reads the build
machine (no time, host or path).

## Frame timelines

`profile` records where each frame's time goes, as spans on a timeline. The spans themselves are `tracing::trace_span!`
in the crates that do the work (the toolkit's run loop, its renderers and presenter, the compositor's frame); this module
records them when `CUPERTINO_PROFILE` asks, and only then. Unset, no layer is installed, and a span costs its callsite's
check that nothing listens.

### `CUPERTINO_PROFILE`

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

### API

| Item | Contents |
|---|---|
| `layer::<S>()` | the layer of the sinks the variable names, filtered to the workspace's spans (targets starting with `cupertino`), for a process's own subscriber: the server adds it beside its log layers; `None` when unset |
| `init()` | installs a subscriber of that layer for a process with none (every toolkit process, from `WaylandWindowSystem::connect_as`); nothing when unset |
| `parse`, `Sink`, `layer_of`, `program` | the variable's sinks, the layer of given sinks, the process's file name |
| `interval`, `now_ns`, `finish` | a measured wait on the timeline; the clock; closing the traces now |

### Tracy

The client is behind the `tracy` feature, which takes effect only with `--cfg cupertino_tracy` as well: every package
and CI build uses `--all-features`, which turns the feature on, and the cfg keeps the client (and its C++ library, which
listens for the profiler) out of them. A build that streams to Tracy, in a target folder of its own since the flag
rebuilds every crate:

```sh
RUSTFLAGS="--cfg cupertino_tracy" cargo build --release --workspace --all-features --bins --target-dir target/tracy
BIN_DIR=target/tracy/release CUPERTINO_PROFILE=tracy target/tracy/release/cupertino-server --headless --spawn cupertino-files
```

The client collects only while a profiler is connected, is reachable from this machine alone, and installs no handler
of fatal signals. It speaks the protocol of Tracy 0.14: use that version's profiler.
