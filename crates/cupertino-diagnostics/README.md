# cupertino-diagnostics

Typed diagnostics of a Cupertino installation and the session it runs in, as text for a person or JSON for a script,
and the reader of the desktop's diagnostic events (`events`). `cupertino diagnose` (crate `cupertino-cli`) runs the
checks; `cupertino-diagnose` is the program it replaces. `cupertino diagnose events` and `cupertino diagnose export`
use the reader. Layer: `tools`.

```
cupertino diagnose                       # every check, as text
cupertino --format json diagnose         # the report below as the envelope's data
cupertino-diagnose --json                # the report alone, as before
cupertino-diagnose --area gpu --area crash
```

`cupertino-diagnose` still works. In the text form it first prints one line on stderr,
``cupertino-diagnose: use `cupertino diagnose`; this command will be removed in a later release``; its `--json`
output is unchanged, with nothing on stderr. It will be removed in a later release.

It exits 0 once the checks ran, whatever they found: a script reads each check's `status` from `--json`. A wrong
argument exits 2, as `invalid_usage` does in the `cupertino` command's exit codes; it never exits 1. Nothing is changed
and no device is opened: the checks read sysfs, installed files, `/run/systemd/sessions`, the session's logs, and ask
systemd for unit states (`systemctl show`), and reads the diagnostic events (the user store, `session.log` and
`journalctl CUPERTINO_DIAG_SCHEMA=1`). It runs as the user, from the session or from a text console; checks that need a session are skipped
outside one.

## Checks

| Id | Status |
|---|---|
| `gpu.driver` | the primary GPU's driver, and what it needs before a compositor takes the display (`cupertino_devices::preflight`): `fail` for a problem (nvidia-drm without `modeset=1`, no GPU), `warn` for a warning (no nvidia GBM backend, no driver bound) |
| `gpu.renderer` | the last session's line in `session.log` saying it rendered with a software renderer (`cupertino_server::software_render`): `warn` when the last or the running session did, that is when no `starting` line of `cupertino-session` follows it; facts: its time and GPU; `skip` without a `session.log` |
| `session.logind` | logind's record of `XDG_SESSION_ID`: `ok` for an active wayland session; facts: class, PAM service, VT, seat |
| `session.desktop` | `XDG_CURRENT_DESKTOP` names Cupertino; `skip` otherwise |
| `session.wayland` | the compositor answers on `WAYLAND_DISPLAY` (a connect to the socket); `fail` when nothing does |
| `portals.backend` | `cupertino.portal` is installed and lists ScreenCast |
| `portals.conf` | the first `cupertino-portals.conf` xdg-desktop-portal reads (user, `/etc`, `/usr/share`) sends ScreenCast to `cupertino` |
| `portals.service` | `xdg-desktop-portal.service` (user) is active; in a session only |
| `services.login`, `services.network`, `services.power`, `services.bluetooth` | `cupertino-login`, `cupertino-netd`, `cupertino-powerd`, `cupertino-btd` (system): `ok` active, `fail` failed, `warn` inactive or not installed, `skip` when systemd cannot be asked. `cupertino-netd` and `cupertino-btd` are also `ok` in any other state (inactive with any unit file state, failed or not installed) while `NetworkManager.service` or `bluetooth.service` is active, or enabled and inactive (BlueZ stays inactive without an adapter), which does their job |
| `services.session_target`, `services.pipewire` | `cupertino-session.target` and `pipewire.service` (user); in a session only |
| `crash.last` | the last end in `session.log`: `warn` when it was not a normal end or a stop by the session; facts: how many ended badly, the last bad end, and the first panic or fatal-signal line of `server.log` and `server.log.1` |
| `crash.gpu_reset` | the GPU resets the server recorded in `session.log` (it makes a new renderer and the clients stay connected): `warn` when there was one; facts: how many, after how many it made a new renderer, and the last one's time, GPU, reason and outcome. Its `fix` says how to investigate one: the JSON form and the `server.log` lines of that time, for an AI assistant or the driver's bug tracker; not run without a `session.log` |
| `logs.paths` | where `session.log`, `server.log` and `server.log.1` are, with their sizes, and the journal unit |
| `events.recent` | the diagnostic events of the last 24 h (`events::read`): `warn` when there were `error` or `fatal` events, with the facts `error` and `fatal` (counts), one `event` per id (`<id>: <n> <severity>, the last at <time>`, worst first) and one `source` per source (`journal: denied`), and a fix naming `cupertino diagnose events --since 24h`; `ok` otherwise; `skip` when no source can be read |

## JSON

```json
{
  "schema": 1,
  "checks": [
    {
      "id": "gpu.driver",
      "area": "gpu",
      "status": "ok",
      "summary": "the primary GPU uses the nvidia driver",
      "facts": [{ "key": "driver", "value": "nvidia" }],
      "fix": "..."
    }
  ]
}
```

`schema` changes when a field is removed or changes meaning. Ids and fact keys are stable; `facts` and `fix` are left out
when empty. `status` is `skip`, `ok`, `warn` or `fail`; `area` is `gpu`, `session`, `portals`, `services`, `crash`,
`logs` or `events` (added within schema 1).

## Library

`run(&Sources, &[Area]) -> Report`. `Sources` holds the sysfs root, the file system root, the environment, a function
that names a unit's state, a function that reads the system journal (`journal`) and one that makes the command following
it (`journal_follow`), so `tests/diagnose.rs` and `tests/events.rs` run every area against a fixture tree and a fake
journal. `Sources::system()` is this machine: `systemctl`, `events::journalctl` and `events::journalctl_follow`.

### Diagnostic events (`events`)

`events::read(&Sources, &Filter) -> EventsReport` reads the `cupertino.diag.v1` events of three sources and merges them
in time order (events of the same time keep their order):

| Source | What is read |
|---|---|
| `store` | the user store, `Sources::store_dir()` (`$XDG_STATE_HOME/cupertino/events`, else `~/.local/state/cupertino/events`), with `store::segments` and `store::read_lines` |
| `session.log` | the `starting` and `the compositor …` lines `cupertino-session` writes, made into records when read (below) |
| `journal` | `journalctl --output=json --no-pager --all CUPERTINO_DIAG_SCHEMA=1 [--since=@SECONDS]`, with no input and `LC_ALL=C`; `CUPERTINO_DIAG_JSON` is a string, or an array of byte values for a value that is not UTF-8 or is large (`events::journal_entry`) |

Each source has a `SourceReport`: `name`, `status`, `events` (how many the filter let through), `corrupt`, `partial`, and
`path` and `detail` when there are some. `status` is:
- `ok`: it gave events; `empty`: none the filter lets through;
- `denied`: the journal of a caller outside `systemd-journal`, `wheel` and `adm`. It is told by journalctl's own notice on
  standard error (`insufficient permissions`, `not seeing messages from other users`) and, without one, by the caller's
  groups (`getgroups` against those groups, root always reads) when journalctl printed no entry;
- `unavailable`: no store yet, no `session.log`, no home, no journalctl or journalctl failing without output;
- `corrupt` (`status_text()` gives `corrupt: N`): `corrupt` of its lines are not records of this schema.

The `Filter` has `since` (`events::parse_since`: `30s`, `10m`, `24h`, `2d` back from now, or an RFC 3339 time), repeated
`components` (exact names), repeated `ids` (prefixes) and a minimum `severity`.

`events::start` also returns a `Follow`, whose `poll()` gives the store's and `session.log`'s lines written since,
reading each segment from where the last read ended (a segment deleted and made again is read from its start; a line
still being written waits), and the report's `follow_from` says where a journal follow starts without a gap or a repeat:
after the cursor of the last entry read, else at the time the read began. `cupertino diagnose events --follow` runs the
command `Sources::journal_follow` makes from it.

**`session.log`.** Its lines carry local time without a zone (`date '+%Y-%m-%d %H:%M:%S'`), read with `mktime`
(`events::local_to_utc`; `events::local_time` is the reverse). They become records with `component`
`cupertino-session`, `source` `session.log`, an empty `version`, no `commit`, no `boot_id` or `session`, `pid` 0, and
`count` numbering the records of each id in the log. They are made at read time and never stored a second time.

| Id | Severity | Line | Fields |
|---|---|---|---|
| `session.start` | `info` | `starting`, `starting with a limit of N s` | `limit_seconds` |
| `session.end` | `info` | `the compositor ended normally after N s`, `the compositor was stopped by the session (status S) after N s` | `cause` (`normal`, `stopped`), `status`, `seconds` |
| `session.end_abnormal` | `error` | every other `the compositor …` line | `cause` (`signal` with `signal`, `watchdog`, `reason` with the compositor's `reason`, `status`, `unknown` with the line's `text`), `status`, `seconds` |

`reason` and `text` go through the free-text scrubber of `events::policy(&Sources)` (the sources' `HOME`, `USER` and
host name, keyed by the store's `salt`, read and never made; a random salt without one) and are listed in `redacted`
when it changed them. This crate records no events of its own.

## Known limitations

- The portal checks read the installed files and the user service; they do not call the backend on the bus.
- `crash.last` reads the logs of the user running it; another user's sessions are not seen.
- No GPU check of the second GPU of a hybrid machine, only the primary one.
- The journal is read whole for the filter's time span; `journalctl` is not asked to filter by component, id or severity.

## Tests

```
cargo test -p cupertino-diagnostics
```

`tests/exit-codes.rs` runs the program: its exit codes, the notice on stderr in the text form, and a `--json` stdout
that is the report alone with nothing on stderr.
