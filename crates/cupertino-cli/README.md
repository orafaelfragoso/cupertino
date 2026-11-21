# cupertino-cli

The `cupertino` command. Layer: `tools`. Its subcommands are typed entries of one registry, so the help, the JSON
command tree, the dispatch, and the shell completions and manual pages all come from the same list.

## Usage

```sh
cupertino                                   # the help
cupertino help [<command>]                  # the help of the commands under <command>; a whole command adds its description and examples
cupertino --version                         # this command's version and commit
cupertino version                           # every part: packages, compositor, C ABI, theme, assets, schemas, OS, kernel
cupertino diagnose [--area <area>]... [--strict]   # the checks of this machine and session
cupertino desktop start --backend nested|headless|tty [--size WxH] [--scale S] [--spawn <command line>]...
cupertino appearance assets check <dir>     # replaces `cupertino-assets check <dir>`
cupertino completions bash|zsh|fish         # the completion script of a shell
cupertino dev layers [--root <dir>] [--graph]      # preview: the layer checks of a checkout
cupertino dev manual <dir>                  # preview: write the manual pages into <dir>
cupertino --format jsonl diagnose           # JSON lines: a check event per check, the counts, an end record
cupertino diagnose events [--since 24h] [--component <name>]... [--id <prefix>]... [--severity info|warn|error|fatal] [--follow]
cupertino diagnose events clear --yes       # delete the user store's events, keep its salt
cupertino diagnose export --output <file> [--include-logs] [--since <duration|time>]
cupertino sdk info [--require] [--root <dir>] # the installed C SDK (the cupertino-kit package), part by part
cupertino sdk check [--root <dir>]          # build and run a probe against it with /usr/bin/cc and pkg-config
cupertino sdk new --lang c|rust <name> [--dir <dir>] [--source <checkout>] # a starter project; `make -C <name>` builds the C one
```

`desktop start` runs `$CUPERTINO_SERVER` when it is set (for development), else the `cupertino-server` beside the
`cupertino` binary, else `/usr/bin/cupertino-server`; `PATH` is never searched. It passes `--ready-fd <n>` and reports
`ready` when the compositor writes `WAYLAND_DISPLAY=<name>` there. The compositor's stdout goes to this command's stderr,
so stdout carries the command's own output alone; it runs in its own process group and gets SIGTERM if this command
dies, even by SIGKILL. It inherits the standard streams and the ready pipe alone: every other descriptor the command
had (a build tool's lock, a terminal multiplexer's pipe) is close-on-exec in the child, for the session too.
`--backend tty` checks it is on a text console outside a graphical session, then `exec`s `cupertino-server --session`
(beside the binary, else `/usr/bin`).

The packages install the command with `/usr/share/bash-completion/completions/cupertino`,
`/usr/share/zsh/site-functions/_cupertino`, `/usr/share/fish/vendor_completions.d/cupertino.fish` and the pages
`cupertino.1` and `cupertino-<group>.1`, all generated at build time by `cupertino completions` and
`cupertino dev manual`.

## Diagnostics (`diagnostics`)

`src/diagnostics/` holds the typed diagnostics of a Cupertino installation and the session it runs in, and the reader
of the desktop's diagnostic events (`diagnostics::events`). `cupertino diagnose` (`src/diagnose.rs`) runs the checks;
`cupertino diagnose events` and `cupertino diagnose export` use the reader.

It exits 0 once the checks ran, whatever they found: a script reads each check's `status` from the JSON form. Nothing
is changed and no device is opened: the checks read sysfs, installed files, `/run/systemd/sessions`, the session's
logs, and ask systemd for unit states (`systemctl show`), and reads the diagnostic events (the user store,
`session.log` and `journalctl CUPERTINO_DIAG_SCHEMA=1`). It runs as the user, from the session or from a text console;
checks that need a session are skipped outside one.

### Checks

| Id | Status |
|---|---|
| `gpu.driver` | the primary GPU's driver, and what it needs before a compositor takes the display (`cupertino_devices::preflight`): `fail` for a problem (nvidia-drm without `modeset=1`, no GPU), `warn` for a warning (no nvidia GBM backend, no driver bound) |
| `gpu.renderer` | the last session's line in `session.log` saying it rendered with a software renderer (`cupertino_server::software_render`): `warn` when the last or the running session did, that is when no `starting` line of the session follows it; facts: its time and GPU; `skip` without a `session.log` |
| `session.logind` | logind's record of `XDG_SESSION_ID`: `ok` for an active wayland session; facts: class, PAM service, VT, seat |
| `session.desktop` | `XDG_CURRENT_DESKTOP` names Cupertino; `skip` otherwise |
| `session.wayland` | the compositor answers on `WAYLAND_DISPLAY` (a connect to the socket); `fail` when nothing does |
| `portals.backend` | `cupertino.portal` is installed and lists ScreenCast |
| `portals.conf` | the first `cupertino-portals.conf` xdg-desktop-portal reads (user, `/etc`, `/usr/share`) sends ScreenCast to `cupertino` |
| `portals.service` | `xdg-desktop-portal.service` (user) is active; in a session only |
| `services.login`, `services.network`, `services.power`, `services.bluetooth` | `cupertino-login`, `cupertino-netd`, `cupertino-powerd`, `cupertino-btd` (system): `ok` active, `fail` failed, `warn` inactive or not installed, `skip` when systemd cannot be asked. `cupertino-netd` and `cupertino-btd` are also `ok` in any other state (inactive with any unit file state, failed or not installed) while `NetworkManager.service` or `bluetooth.service` is active, or enabled and inactive (BlueZ stays inactive without an adapter), which does their job |
| `services.session_target`, `services.pipewire` | `cupertino-session.target` and `pipewire.service` (user); in a session only |
| `shell.dev_folder` | the shell host in the development folder the system file declares (`dev_folder`): `warn` when its `cupertino-shell` is older than the installed one (`dev_builds::HOST_PROGRAM`) for a component the folder names and the session runs (all but `agent`); the fix names `cupertino shell use-installed NAME` for each; `skip` without the file or the key |
| `crash.last` | the last end in `session.log`: `warn` when it was not a normal end or a stop by the session; facts: how many ended badly, the last bad end, and the first panic or fatal-signal line of `server.log` and `server.log.1` |
| `crash.gpu_reset` | the GPU resets the server recorded in `session.log` (it makes a new renderer and the clients stay connected): `warn` when there was one; facts: how many, after how many it made a new renderer, and the last one's time, GPU, reason and outcome. Its `fix` says how to investigate one: the JSON form and the `server.log` lines of that time, for an AI assistant or the driver's bug tracker; not run without a `session.log` |
| `logs.paths` | where `session.log`, `server.log` and `server.log.1` are, with their sizes, and the journal unit |
| `events.recent` | the diagnostic events of the last 24 h (`events::read`): `warn` when there were `error` or `fatal` events, with the facts `error` and `fatal` (counts), one `event` per id (`<id>: <n> <severity>, the last at <time>`, worst first) and one `source` per source (`journal: denied`), and a fix naming `cupertino diagnose events --since 24h`; `ok` otherwise; `skip` when no source can be read |

### JSON

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

### Library

`diagnostics::run(&Sources, &[Area]) -> Report`. `Sources` holds the sysfs root, the file system root, the environment, a function
that names a unit's state, a function that reads the system journal (`journal`) and one that makes the command following
it (`journal_follow`), so every area can run against another tree and another journal. `Sources::system()` is this machine: `systemctl`, `events::journalctl` and `events::journalctl_follow`.

#### Development builds (`dev_builds`)

`older_than_installed(dev, installed)` says whether the development folder's file was last modified before the installed
program it starts instead of (`false` when it cannot be told). `components_run` gives the components a `shell` section
runs (its `components` less its `off`), `may_run_from_folder` says whether the folder may give one a host of its own, and
`HOST_PROGRAM` is the installed shell host the folder's `cupertino-shell` replaces; `cupertino dev-folder enable` and the
`shell.dev_folder` check share them.

#### Diagnostic events (`events`)

`events::read(&Sources, &Filter) -> EventsReport` reads the `cupertino.diag.v1` events of three sources and merges them
in time order (events of the same time keep their order):

| Source | What is read |
|---|---|
| `store` | the user store, `Sources::store_dir()` (`$XDG_STATE_HOME/cupertino/events`, else `~/.local/state/cupertino/events`), with `store::segments` and `store::read_lines` |
| `session.log` | the `starting` and `the compositor …` lines the session (`cupertino-server --session`) writes, made into records when read (below) |
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
`cupertino-server`, `source` `session.log`, an empty `version`, no `commit`, no `boot_id` or `session`, `pid` 0, and
`count` numbering the records of each id in the log. They are made at read time and never stored a second time.

| Id | Severity | Line | Fields |
|---|---|---|---|
| `session.start` | `info` | `starting`, `starting with a limit of N s` | `limit_seconds` |
| `session.end` | `info` | `the compositor ended normally after N s`, `the compositor was stopped by the session (status S) after N s` | `cause` (`normal`, `stopped`), `status`, `seconds` |
| `session.end_abnormal` | `error` | every other `the compositor …` line | `cause` (`signal` with `signal`, `watchdog`, `reason` with the compositor's `reason`, `status`, `unknown` with the line's `text`), `status`, `seconds` |

`reason` and `text` go through the free-text scrubber of `events::policy(&Sources)` (the sources' `HOME`, `USER` and
host name, keyed by the store's `salt`, read and never made; a random salt without one) and are listed in `redacted`
when it changed them. The module records no events of its own.

### Known limitations

- The portal checks read the installed files and the user service; they do not call the backend on the bus.
- `crash.last` reads the logs of the user running it; another user's sessions are not seen.
- No GPU check of the second GPU of a hybrid machine, only the primary one.
- The journal is read whole for the filter's time span; `journalctl` is not asked to filter by component, id or severity.

## Diagnostic events (`diagnose`)

`src/diagnose_events.rs` reads the diagnostic events with `diagnostics::events` (the user store,
`session.log` and the system journal; [docs/src/diagnose.md](../../docs/src/diagnose.md#diagnostic-events) has the
record, the limits and the redaction table). It takes its `Sources` from `Context::provided::<Sources>()` when the caller
gives some, else `Sources::system()`. It records no events of its own.

- `diagnose events` is a `Run::Stream`: each event is an `event` whose data is the `cupertino.diag.v1` record, and one
  human line (time, severity, component, id, message, then `key=value` fields, a value with spaces in JSON quotes). The
  final data is `{sources, counts}`: each source's `name`, `status` (`ok`, `empty`, `denied`, `unavailable`,
  `corrupt`), `events`, `corrupt`, `partial`, `path` and `detail`, and `counts` `{events, info, warn, error, fatal}`
  of what was printed. A denied journal is a `denied`/`journal_denied` warning and corrupt lines a
  `validation`/`corrupt_lines` one. A wrong `--since` is `invalid_usage`/`invalid_value`.
- `--follow` (`Sink::follow`) then polls the store and `session.log` every 250 ms (`events::Follow`) and follows the
  journal with the command `Sources::journal_follow` makes (`journalctl --follow` from the last cursor read). It never
  blocks in a read or a wait: it `poll`s the child's non-blocking stdout with a timeout of at most 250 ms, reads only
  what is there, and checks `Sink::stop_requested` between steps. A stop ends it with `reason: "stopped"`, a closed
  output with exit 0; either way the child is killed and reaped.
- `diagnose events clear` (`Run::Once`) deletes the store's segments with `Store::clear` (under the store's lock; the
  salt stays). Without `--yes` it is `invalid_usage`/`confirmation_required`; a store that cannot be cleared is
  `validation`/`store_unavailable` (`no_home` without a home).
- `diagnose export` (`Run::Once`) writes one JSON document with `O_EXCL` and mode 0600: `schema`
  (`cupertino.diag.export.v1`), `created`, `versions` (`cli`, `commit`, `os_image`, `kernel`, from
  `cupertino-diagnostic-events`' `build_info`, the field names a version report uses), `report` (`diagnostics::run(&sources, &[])`,
  its `summary`, fact `value` and `fix` texts through the free-text scrubber), `events` and `sources` (their `path` and
  `detail` scrubbed). `--include-logs` adds `logs`: `server.log`, `server.log.1` and `session.log` of the state
  directory, each line through `events::policy` (the store's salt, so hashes match the events', else a random one). A
  file that exists is `validation`/`file_exists`; one that cannot be written `temporary`/`write_failed`.

## Output

`--format` takes `human` (the default), `json` or `jsonl`, and may stand anywhere before a `--`.

- `human`: the output on stdout, errors on stderr as `cupertino: <message>`, warnings as `cupertino: warning: <message>`.
- `json`: one `cupertino.cli.v1` envelope line on stdout, on success and on failure:

  ```json
  {"schema":"cupertino.cli.v1","command":"version","request_id":"1f3a-…","ok":true,"data":{"cli":"0.1.0","commit":"4089d615…"}}
  {"schema":"cupertino.cli.v1","command":"appearance","request_id":"1f3b-…","ok":false,"error":{"kind":"invalid_usage","code":"missing_subcommand","message":"…"}}
  ```

- `jsonl`: `cupertino.cli.stream.v1` records, one JSON object per line. Each starts with `schema`, `command`,
  `request_id`, `seq` (from 0, without a gap) and `type`:

  | `type` | Fields |
  |---|---|
  | `event` | `event` (snake_case, stable per command), `data` |
  | `error` | `error: {kind, code, message}`: a problem the command continued past |
  | `result` | `data`: what the envelope's `data` holds |
  | `end` | `ok`, `exit_code`, `error` when `ok` is false, `reason: "stopped"` when a stop ended a follow stream |

  ```json
  {"schema":"cupertino.cli.stream.v1","command":"version","request_id":"1f3c-…","seq":0,"type":"result","data":{"cli":"0.1.0","commit":"4089d615…"}}
  {"schema":"cupertino.cli.stream.v1","command":"version","request_id":"1f3c-…","seq":1,"type":"end","ok":true,"exit_code":0}
  ```

  There is exactly one `end`, always last. A one-shot command prints `result` and `end`; a usage error before the
  command starts prints `end` alone; a failed command prints its events and then `end` with the error, no `result`.

In `json` and `jsonl` nothing goes to stderr. A stream command in `json` form gives one envelope at the end whose `data`
is its final object plus `events` (`[{event, data}]`) and `warnings` (`[{kind, code, message}]`); a report stream
(`Sink::json_result_only`) keeps its own data, with `warnings` only when there are some.

A failure may carry `data` beside `error` in the `json` envelope (`Failure::with_data`): `dev layers` puts its counts
and `problems` there.

`request_id` is the process id and the time, in hex. `code` is a stable snake_case identifier within its `kind`;
`message` is for people and may change.

| Exit code | `kind` |
|---|---|
| 0 | (success; also a closed stdout, and a follow stream stopped by a signal) |
| 2 | `invalid_usage`: also `confirmation_required` |
| 3 | `unavailable`: `no_checkout`, `no_cargo`, `no_host_display`, `no_runtime_dir`, `server_not_installed`, `server_failed`, `not_a_vt`, `session_not_installed`, `session_failed`, `sdk_not_installed`, `no_compiler`, `no_pkg_config`, `tool_failed`, `no_working_directory` |
| 4 | `denied`: `inside_graphical_session`, `not_writable` (`sdk new`) |
| 5 | `validation`: `pack_unreadable`, `manifest_invalid`, `font_invalid`, `checks_failed`, `layer_violations`, `metadata_failed`, `file_exists` (`diagnose export`), `sdk_invalid`, `compile_failed`, `abi_mismatch`, `probe_failed`, `directory_not_empty`, `not_a_directory` |
| 6 | `temporary`: also `output_failed` (stdout cannot be written), `interrupted` (a finite stream stopped), `write_failed` (also an export file and the files of `sdk new` and `sdk check`), `timed_out` (a compiler or probe of `sdk check`) |
| 7 | `transaction_failed` (reserved) |
| 8 | `unsupported`: also `not_in_this_release` for the reserved groups `system` and `ai` |

Code 7 is reserved: no command of this release ends with it yet. `docs/src/cupertino-command.md` marks the reserved codes.

**A closed output.** When the reader of stdout goes away (`EPIPE`, as in `cupertino … | head -1`), the command stops,
exits 0 and writes nothing to stderr. Any other write error (`ENOSPC`, `EIO`) exits 6 with one stderr line,
`cupertino: cannot write the output: …`, in human form only.

**Signals.** While a stream command runs, SIGINT and SIGTERM ask it to stop; a second one of either ends the process at
once by the signal's default action. A program the command starts gets the default actions of SIGPIPE, SIGINT and
SIGTERM, unless the parent ignored SIGINT or SIGTERM: the standard library resets the ignored SIGPIPE when it starts a
process, and `exec` resets caught signals A
follow stream (one that runs until stopped) then ends with `end {ok: true, exit_code: 0, reason: "stopped"}`; a finite
stream ends with `temporary`/`interrupted`, exit 6. A one-shot command keeps the default signal actions, and a signal
the parent ignored stays ignored.

## Stability and compatibility

Every command is `stable`, `preview` or `deprecated` (`help` shows it, and the help JSON has `stability`):

- Within `cupertino.cli.v1` and `cupertino.cli.stream.v1`, a stable command's options, `kind` and `code` values, exit
  codes, data fields, record types and `event` names are only added to.
- A preview command may change or go away in any release; every `dev` command is a preview.
- A stable command is removed only after one minor release as `deprecated`: it still works, prints
  `cupertino: warning: … is deprecated …` on stderr in human form (an `error` record `unsupported`/`deprecated` in JSON
  lines), and the docs say what replaces it.
- Consumers ignore fields and `event` names they do not know.
- A change that is not additive raises the schema to `cupertino.cli.v2` or `cupertino.cli.stream.v2`.
- The group names `system` and `ai` are reserved for later releases.

The help JSON (`cupertino --format json help`) lists, per command, `path`, `summary`, `args` (each with `name`, `help`,
`kind`, `values`, `repeat` and `hint`), `stability` (and `replacement` when deprecated), `streams`, `description` and
`examples` (`[{command, description}]`).

## Design

`src/lib.rs` holds the registry types and the runner; `src/stream.rs` the output writer, the stream rules and the stop
flag; each group is a module (`appearance`, `diagnose`, `desktop`, `version`, `completions`, `dev`, `sdk`;
`diagnose_events` adds the event commands to the `diagnose` group). The files `sdk new` writes and the probe `sdk check`
compiles are in `templates/`, built into the binary. `src/manual.rs`
and `src/completions.rs` generate the manual pages and the completion scripts from a registry, `GLOBAL_OPTIONS`,
`BUILTINS` and `FailureKind::ALL`, so a new command or option needs no edit there. The completions take an option's
value after the option or in the same word (`--area=gpu`; bash splits such a word at the `=`, and the script joins it
again), and skip the global options by their table wherever they stand. The crate docs of `stream` state
the rules for stream commands.

### The registry

- `REGISTRY: &[Group]`. A `Group` has a `name` (the first word of its commands), a `summary`, a `description` (its
  manual page body) and `commands: &[&[Command]]`, the command lists of the modules that make up the group, usually one
  (`&[COMMANDS]`). A module adds commands under an existing group by adding its list there (`diagnose events …`
  joins the `diagnose` group as `commands: &[diagnose::COMMANDS, diagnose_events::COMMANDS]`); a new group adds one
  line to `REGISTRY`.
- A `Command` has its `path`, `summary`, `description`, `examples` (complete `cupertino …` lines, each with what it
  does), `stability` (`Stable`, `Preview`, `Deprecated { replacement }`), `args` and `run`.
- An `Arg` is made with `Arg::positional`, `Arg::option` or `Arg::flag`, then `.values(&[…])` (a closed list the parser
  enforces as `invalid_usage`/`invalid_value`), `.repeat()` (an option given several times, or a last positional
  argument taking every remaining word; `Args::values(name)` gives them all) and `.hint(ValueHint::…)` (`None`, `File`,
  `Dir`, `Command`: a program name, `Shell`: a whole command line).
- `GLOBAL_OPTIONS` holds `--format` (with its values), `--help` and `--version`; `BUILTINS` the commands the runner
  answers itself (`help`), whose names no group may take; `RESERVED` the reserved group names.
- `FailureKind::ALL` lists the kinds in exit code order, each with `name()` (the JSON `kind`), `meaning()` and
  `exit_code()`, so the exit status tables are generated too.
- `check_registry(&[Group])` returns the broken registry rules, one line each: unique group names and command paths,
  paths under their group, a summary, description and example on every command, every `dev` command a preview,
  unique argument names that differ from the global ones, well-formed value lists, a repeatable positional argument
  last, and no positional argument on a command whose path begins another's (the longest path wins at dispatch, so
  `diagnose events` is never `diagnose` with an argument). It runs on `REGISTRY`; any other registry should pass it too.

The parser takes `--name value` and `--name=value`; `--` ends the options (and `--format` and `--help`) and makes every
later word a positional argument. A single option given twice is `repeated_option`; a value on a flag is
`unexpected_value`.

### Running a command

- `Run::Once(fn(&Args, &Context) -> Result<Reply, Failure>)`: computes its whole result. `Reply::new(&data, human)`
  carries the JSON data and the human text; a command never prints.
- `Run::Stream(fn(&Args, &Context, &mut Sink) -> Result<Reply, Failure>)`: reports while it runs, then returns its
  final `Reply`, whose data must be a JSON object. Through the `Sink`:
  - `event(name, &data, human_line)` reports an event (an empty human line prints nothing in human form);
  - `warn(failure)` reports a problem it continues past;
  - `follow()` declares a stream that runs until stopped;
  - `json_result_only()` declares a command whose final data already holds what its events said (a report): in
    `json` form its `data` is then that data alone, with `warnings` added only when there are some;
  - `stop_requested()` says whether SIGINT, SIGTERM or the caller asked it to stop;
  - `format()` says which form is printed.

  `event` and `warn` return `Err(Closed)` once stdout cannot be written; return at once (`?` turns it into a
  `Failure`, which the runner does not print). Wait in steps of at most a few hundred milliseconds (`poll` with a
  timeout) and check `stop_requested()` between them. On a stop, a follow stream returns `Ok`, and a finite one
  returns `Err(Failure::interrupted())` unless it finished its work.
- A `Failure` has a `kind` (`FailureKind`, which gives the exit code), a stable snake_case `code` and a `message`;
  `Failure::usage(code, message)` makes an `invalid_usage` one, and `.with_data(&data)` adds the `data` the `json`
  envelope carries beside the error.
- `Context` is what a command may read of the machine: `root()` and `path("/usr/share/…")` (files of the running
  system, under `/` for the real command), `var(name)` and `vars()`, `current_dir()`, `command(program)` (a process
  with the context's environment and working directory: the process's for the real command, the context's variables
  alone otherwise), `registry()` (the runner's, for the completions and pages), and `provided::<T>()`, a value the caller
  hands in by type with `Context::provide(&value)` (a `Sources` of another root, a layer `Policy`, a `StopGrace`). `Context::system()` is the real machine;
  `Context::isolated(root)` has no variables and works in `root`, so it never reads the real `HOME`, `/usr` or
  session.
- `Runner::new(registry).context(cx).stop(&flag)` runs command lines: `run_to(args, request_id, out, err) -> u8`
  writes as it goes, `run(args, request_id) -> Output` collects both outputs and the exit code.
  `cupertino_cli::run` and `run_to(args, request_id, out, err, stop)` do the same for `REGISTRY` on the real machine.
- `main_with(registry)` is a whole program: the process arguments (one that is not UTF-8 is
  `invalid_usage`/`invalid_encoding`), stdout and stderr, and a `StopFlag::on_signals()` flag. The `cupertino` binary
  is `main_with(REGISTRY)`; `examples/stream_probe.rs` runs probe streams through it.

The command is an unprivileged client: a group calls libraries of the `foundation` to `tools` layers or the D-Bus and
socket interfaces of the services; it never links compositor state.

## Open issues

- `sdk check` compiles with the system's `cc` and checks the C ABI only. The Rust project of `sdk new` depends on the
  release's git tag of `cupertino-kit`, so it builds once that tag is published; until then, or offline, `--source`
  points it at a checkout's crates by path.
- OWNER DECISION: `sdk new` refuses a `--dir` (or `./<name>`) that is a symbolic link, even one to an empty directory,
  as `validation`/`not_a_directory`. It writes only into a real directory it has found empty or makes itself, so a
  link never redirects the project elsewhere; give the link's target as `--dir` instead.
