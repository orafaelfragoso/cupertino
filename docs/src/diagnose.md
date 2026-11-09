# Checking a machine with cupertino-diagnose

`cupertino-diagnose` checks this machine and the session it runs in for what Cupertino needs: the GPU and its driver, the session, the desktop portal, the services, how the last sessions ended, where the logs are, and the errors the desktop recorded in the last day. It changes nothing and opens no device. It reads sysfs, installed files, the session's logs, the [diagnostic events](#diagnostic-events) and the states of systemd units. It runs as your user, from the session or from a text console; the checks that need a session are skipped outside one.

This page describes the command as it is on `main` today. The `cupertino` command, with its registry and `--format` option, has [its own page](cupertino-command.md). It reads the diagnostic events with `cupertino diagnose events` and writes a bug report file with `cupertino diagnose export`, but it has no `diagnose` subcommand for the checks yet, so `cupertino-diagnose` stays the way to run them.

## Usage

```sh
cupertino-diagnose                       # every check, as text
cupertino-diagnose --json                # the same as JSON
cupertino-diagnose --area gpu --area crash
```

| Flag | Meaning |
|---|---|
| `--json` | Print the report as JSON. |
| `--area AREA` | Run only this area. Repeat it for more. `AREA` is `gpu`, `session`, `portals`, `services`, `crash`, `logs` or `events`. With no `--area`, every area runs, in that order. |
| `-h`, `--help` | Print the usage and exit 0. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | The checks ran. A failed check does not change the exit code. |
| 2 | The command line was wrong: an unknown flag, or `--area` with a missing or unknown name. The usage goes to standard error. |

The command never exits 1. A script reads each check's `status` from `--json`.

## Text output

One line for each check: the status (`ok`, `warn`, `FAIL` or `skip`), the check id and a one-line summary. Facts and the fix follow, indented. This is the output from the machine this page was checked on, a session-less text run with the NVIDIA driver (it shows what the format looks like, not what your machine will print):

```text
ok    gpu.driver          the primary GPU uses the nvidia driver
                          driver: nvidia
                          note: primary GPU card1 uses the nvidia driver
skip  session.logind      not run from a logind session
warn  portals.conf        no cupertino-portals.conf: xdg-desktop-portal picks the backends on its own
                          fix: reinstall the cupertino package
```

## JSON output

```json
{
  "schema": 1,
  "checks": [
    {
      "id": "gpu.driver",
      "area": "gpu",
      "status": "ok",
      "summary": "the primary GPU uses the nvidia driver",
      "facts": [{ "key": "driver", "value": "nvidia" }]
    }
  ]
}
```

| Field | Content |
|---|---|
| `schema` | The version of the JSON form, 1 today. It changes when a field is removed or changes meaning. |
| `checks[].id` | A stable dotted name such as `gpu.driver`. |
| `checks[].area` | `gpu`, `session`, `portals`, `services`, `crash`, `logs` or `events`. A new area may be added within schema 1. |
| `checks[].status` | `skip`, `ok`, `warn` or `fail`. |
| `checks[].summary` | One line for a person. |
| `checks[].facts` | A list of `key` and `value` pairs. Left out when empty. A key can repeat (`note`, `also`). |
| `checks[].fix` | What to do when the status is `warn` or `fail`. Left out when there is none. |

`skip` means there was nothing to check: not in a session, or the tool the check needs is missing.

## The checks

| Id | What it checks |
|---|---|
| `gpu.driver` | The primary GPU's driver, and what it needs before a compositor takes the display. `fail` for a problem (for example `nvidia-drm` without `modeset=1`, or no GPU), `warn` for a warning (for example no NVIDIA GBM backend, or no driver bound). |
| `gpu.renderer` | Whether the last session, or the one running, drew with a software renderer, from its line in `session.log`. `warn` when it did, `ok` otherwise, `skip` without a `session.log`. Facts: `time` and `gpu`. The fix says to install the GPU's Mesa driver or to give a virtual machine a GPU with 3D. |
| `session.logind` | logind's record of the session in `XDG_SESSION_ID`; `skip` when there is none. |
| `session.desktop` | `XDG_CURRENT_DESKTOP` names Cupertino; `skip` otherwise. |
| `session.wayland` | The compositor answers on `WAYLAND_DISPLAY`. `fail` when nothing answers or `XDG_RUNTIME_DIR` is not set. |
| `portals.backend` | `cupertino.portal` is installed and lists ScreenCast; `warn` otherwise. |
| `portals.conf` | The first `cupertino-portals.conf` that xdg-desktop-portal reads sends ScreenCast to `cupertino`; `warn` otherwise. |
| `portals.service` | `xdg-desktop-portal.service` (user) is active. Runs only in a session. |
| `services.login`, `services.network`, `services.power`, `services.bluetooth` | The system units `cupertino-login`, `cupertino-netd`, `cupertino-powerd` and `cupertino-btd`. |
| `services.session_target`, `services.pipewire` | The user units `cupertino-session.target` and `pipewire.service`. Run only in a session. |
| `crash.gpu_reset` | The GPU resets the server recorded in `session.log`. `warn` when there was one, `ok` when there was none. Facts: `resets`, `remade` (how many ended with a new renderer, as `N of M`), `last_time`, `last_gpu`, `last_reason` and `last_outcome`. The fix says how to investigate: give the JSON form and the `server.log` lines of that time to an AI assistant or the driver's bug tracker (see [A GPU reset](daily-use.md#a-gpu-reset)). |
| `crash.last` | How the last session in `session.log` ended. `warn` when it was not a normal end or a stop by the session. Facts name how many ended badly, the last bad end, and the first panic or fatal-signal line of `server.log` and `server.log.1`. |
| `logs.paths` | Where `session.log`, `server.log` and `server.log.1` are, with their sizes, and the journal command for the login unit. |
| `events.recent` | The [diagnostic events](#diagnostic-events) of the last 24 hours. `warn` when there were `error` or `fatal` events, with the facts `error` and `fatal` (the counts), one `event` per id (`server.gpu.reset: 2 error, the last at …`, worst first) and one `source` per source (`journal: denied`); the fix names `cupertino diagnose events --since 24h`. `ok` otherwise, `skip` when no source can be read. |

For a unit check, the status follows the unit's state: `ok` when active, `fail` when failed, `warn` when inactive, not installed or in another state, and `skip` when systemd cannot be asked. The `fix` names `systemctl status` and `journalctl` for the unit, or says to install the cupertino package.

`services.network` and `services.bluetooth` also give `ok` when `cupertino-netd` or `cupertino-btd` is not running, whatever its unit file state (`static`, `indirect`, `disabled`, `masked` or `enabled`), while `NetworkManager.service` or `bluetooth.service` is active or enabled, because that service does the same job. The summary then reads `... is inactive (static): managed by NetworkManager` (or `BlueZ`). They warn only when neither the Cupertino daemon nor that service runs. A machine installed from the [USB image](install-usb.md) is set up this way. BlueZ may be enabled but stopped on a machine without a Bluetooth adapter, and that still counts.

## Known limits

- The portal checks read installed files and the user service. They do not call the backend on the bus.
- `crash.last` reads the logs of the user who runs the command; another user's sessions are not seen.
- Only the primary GPU is checked, not the second GPU of a hybrid machine.

## Diagnostic events

Every Cupertino process records a diagnostic event beside a failure's log line: what failed, where, and in which build. Events stay on this machine; nothing is uploaded.

### The record

Each event is one JSON object on one line, with the schema `cupertino.diag.v1` and its fields in this order:

| Field | Value |
|---|---|
| `schema` | `"cupertino.diag.v1"` |
| `time` | RFC 3339 in UTC with milliseconds, `2026-11-09T14:03:27.512Z` |
| `boot_id` | the kernel's boot id, or `null` |
| `session` | `XDG_SESSION_ID`, or `null` |
| `component`, `version`, `commit` | the program's name, its package version and the source commit, or `null` for an unknown commit |
| `pid` | the process id |
| `id` | stable and dotted, `area.subject.what`, for example `server.gpu.reset` |
| `severity` | `info`, `warn`, `error` or `fatal` |
| `message` | a fixed text chosen by the code, never made from inputs |
| `fields` | the event's typed values, already reduced by the redaction policy |
| `source` | the crate and the file's name with the line, `cupertino-server:gpu_reset.rs:120` |
| `redacted` | the keys of the fields the policy removed or rewrote |
| `count` | how many events of this id the process has recorded |
| `suppressed` | on a rate-limit summary, how many events of this id were not written in its window; 0 otherwise |
| `truncated` | present and `true` when the event was longer than 4 KiB and lost field text or fields |

Readers ignore fields they do not know. A change that is not additive raises the schema to `cupertino.diag.v2`.

### Where events are kept

- **Your own processes** (the compositor, the shell, apps, the portal, the polkit agent) write to a store in `$XDG_STATE_HOME/cupertino/events/`, else `~/.local/state/cupertino/events/`. The directory is mode 0700 and its files 0600; a directory that is a symbolic link, belongs to someone else or that others may write is refused. Segments are `events-<boot>-<seq>.jsonl`; `lock` is the lock every writer holds, and `salt` keys the hashes.
- **System services** (`cupertino-sessiond`, `cupertino-netd`, `cupertino-btd`, `cupertino-powerd`) write to the system journal. Each entry has `CUPERTINO_DIAG_SCHEMA=1`, `CUPERTINO_DIAG_ID`, `CUPERTINO_DIAG_SEVERITY` and the whole line in `CUPERTINO_DIAG_JSON`, so `journalctl CUPERTINO_DIAG_SCHEMA=1` finds them.
- **Session starts and ends** come from `session.log`, which `cupertino-session` writes; see [below](#sessionlog).

### Limits

| Limit | Value |
|---|---|
| One store segment | 1 MiB; the next line that would pass it opens the next segment |
| The whole store | 8 MiB; the oldest segments are deleted first |
| Age | 14 days after a segment's last write |
| One event | 4 KiB; a longer one has its text fields cut to 256 characters, then fields dropped, and `truncated: true` |
| The memory ring of each process | the last 256 events or 256 KiB |
| Rate | 10 events per id per 10 s; the rest are counted and written as one summary with `suppressed` when the window closes |

When the store cannot be written (a read-only file system, a full disk, a refused directory), events stay in the memory ring, one warning per reason goes to standard error, and an event says why once the store works again.

### What diagnostic events never contain

Every value is reduced before it is written, so nothing below reaches the ring, the store or the journal.

| Class | Kept as |
|---|---|
| Passwords, tokens, keys, cookies, `Authorization` values, the text of an authentication conversation | `"[redacted]"` |
| Typed text, the clipboard, window titles, notification bodies, searches, file names a person chose, URLs | `"[redacted]"` |
| A path under the home directory | `~` and the desktop's own `.local/state/cupertino/…` and `.config/cupertino/…` paths as they are; anything else `~/…#<hash>`, the salted hash of its basename |
| A path under `/usr`, `/etc`, `/run`, `/dev`, `/sys` or `/proc` | the path, with user and host names replaced |
| Any other path (another home, `/tmp`, removable media, a relative path) | `/home/…#<hash>`, `/root/…#<hash>` or `…#<hash>` |
| IP, MAC and Bluetooth addresses, SSIDs, device names | `<kind>#<hash>`: `ip#`, `mac#`, `bluetooth#`, `ssid#`, `device#` |
| User and host names | `"[user]"`, `"[host]"` |
| App ids, component and unit names, error kinds, errno names, interfaces and methods, GPU vendor, driver and PCI ids, connector and output names | as they are |
| Counts, numbers, durations, flags | as they are |

A hash is 8 hexadecimal digits keyed by the store's 16-byte random `salt`: the same value gives the same hash on one machine and never across machines. Free text (an error's message, a panic message) goes through a scrubber that removes home paths, URLs, e-mail addresses, `user@host`, the values of `key=value` pairs whose key names a secret, network and hardware addresses, long runs that look like tokens, and the user's and host's names.

### Commands

```sh
cupertino diagnose events                                  # every event, oldest first
cupertino diagnose events --since 24h --severity error     # the errors and fatal events of the last day
cupertino diagnose events --component cupertino-server --id server.gpu
cupertino --format jsonl diagnose events --follow          # new events as JSON lines, until Ctrl-C
cupertino diagnose events clear --yes                      # delete the store's events
cupertino diagnose export --output report.json             # one file for a bug report
cupertino diagnose export --output report.json --include-logs --since 2d
```

| Command | What it does |
|---|---|
| `diagnose events` | Prints the events of the store, `session.log` and the journal, merged in time order. `--since` takes a duration back from now (`30s`, `10m`, `24h`, `2d`) or an RFC 3339 time; `--component NAME` and `--id PREFIX` may be repeated; `--severity info\|warn\|error\|fatal` keeps that severity and worse. In the human form each event is one line: its time, severity, component, id and message, then its fields as `key=value`. In the JSON forms each event is an `event` whose data is the record. The final data has `sources` (each with `name`, `status`, `events`, `corrupt`, `partial`, and `path` and `detail` when there are some) and `counts` (`events`, `info`, `warn`, `error`, `fatal`). A denied journal and lines that are not events are warnings. |
| `diagnose events --follow` | The same, then the new events until it is stopped: it reads the store and `session.log` every 250 ms (segments made, rotated or deleted meanwhile are fine) and follows the journal with `journalctl --follow` from the last entry it read. A stop (Ctrl-C) ends it with exit 0, as does a closed output. |
| `diagnose events clear --yes` | Deletes every segment of the store under its lock and keeps the salt. Without `--yes` it deletes nothing and fails with `invalid_usage`/`confirmation_required` (exit 2). The journal and `session.log` are not changed. |
| `diagnose export --output FILE` | Writes one JSON document, mode 0600, to a file that must not exist (`validation`/`file_exists`, exit 5, otherwise): `schema` (`cupertino.diag.export.v1`), `created`, `versions` (`cli`, `commit`, `os_image`, `kernel`), `report` (every check above, its texts through the scrubber), `events` and `sources`. `--since` limits the events. `--include-logs` adds `logs` with `server.log`, `server.log.1` and `session.log`, every line through the scrubber with the store's salt, so their hashes match the events'. Read the file before you share it. |

### Journal access

The system services' events are in the system journal, which only root and members of the `systemd-journal`, `wheel` or `adm` groups can read. For anyone else the `journal` source is `denied` and a warning says so; the store and `session.log` are still read. To see them, add yourself to `systemd-journal` (`sudo usermod -aG systemd-journal $USER`, then log in again). A source is `ok` when it gave events, `empty` when it gave none the filter lets through, `unavailable` when it cannot be read (no store yet, no `session.log`, no `journalctl`) and `corrupt` with a count when some of its lines are not events of this schema.

### session.log

`cupertino-session` writes a line to `~/.local/state/cupertino/session.log` when a session starts and when the compositor ends. Its lines are turned into `cupertino.diag.v1` records when they are read, with `component` `cupertino-session`, `source` `session.log`, an empty `version`, no `commit` and `pid` 0. They are never stored a second time. The lines carry local time without a zone, which is read with this machine's time zone.

| Line | Record |
|---|---|
| `starting`, `starting with a limit of N s` | `session.start`, `info`; `limit_seconds` |
| `the compositor ended normally after N s` | `session.end`, `info`; `cause` `normal`, `status`, `seconds` |
| `the compositor was stopped by the session (status S) after N s` | `session.end`, `info`; `cause` `stopped` |
| every other `the compositor …` line | `session.end_abnormal`, `error`; `cause` `signal` (with `signal`), `watchdog`, `reason` (with the compositor's `reason`, scrubbed), `status` or `unknown` (with the scrubbed `text`) |
