# cupertino-cli

The `cupertino` command. Layer: `tools`. Its subcommands are typed entries of one registry, so the help, the JSON
command tree, the dispatch, and the shell completions and manual pages all come from the same list.

## Usage

```sh
cupertino                                   # the help
cupertino help [<command>]                  # the help of the commands under <command>; a whole command adds its description and examples
cupertino --version                         # this command's version and commit
cupertino version                           # every part: packages, compositor, C ABI, theme, assets, schemas, OS, kernel
cupertino diagnose [--area <area>]... [--strict]   # replaces `cupertino-diagnose`
cupertino desktop start --backend nested|headless|tty [--size WxH] [--scale S] [--spawn <command line>]...
cupertino appearance assets check <dir>     # replaces `cupertino-assets check <dir>`
cupertino completions bash|zsh|fish         # the completion script of a shell
cupertino dev layers [--root <dir>] [--graph]      # preview: the layer and licence checks of a checkout
cupertino dev manual <dir>                  # preview: write the manual pages into <dir>
cupertino --format jsonl diagnose           # JSON lines: a check event per check, the counts, an end record
cupertino diagnose events [--since 24h] [--component <name>]... [--id <prefix>]... [--severity info|warn|error|fatal] [--follow]
cupertino diagnose events clear --yes       # delete the user store's events, keep its salt
cupertino diagnose export --output <file> [--include-logs] [--since <duration|time>]
```

`desktop start` runs `$CUPERTINO_SERVER` when it is set (for development), else the `cupertino-server` beside the
`cupertino` binary, else `/usr/bin/cupertino-server`; `PATH` is never searched. It passes `--ready-fd <n>` and reports
`ready` when the compositor writes `WAYLAND_DISPLAY=<name>` there. The compositor's stdout goes to this command's stderr,
so stdout carries the command's own output alone; it runs in its own process group and gets SIGTERM if this command
dies, even by SIGKILL. It inherits the standard streams and the ready pipe alone: every other descriptor the command
had (a build tool's lock, a terminal multiplexer's pipe) is close-on-exec in the child, for `cupertino-session` too.
`--backend tty` checks it is on a text console outside a graphical session, then `exec`s `cupertino-session`
(beside the binary, else `/usr/bin`).

The packages install the command with `/usr/share/bash-completion/completions/cupertino`,
`/usr/share/zsh/site-functions/_cupertino`, `/usr/share/fish/vendor_completions.d/cupertino.fish` and the pages
`cupertino.1` and `cupertino-<group>.1`, all generated at build time by `cupertino completions` and
`cupertino dev manual`.

## Diagnostic events (`diagnose`)

`src/diagnose_events.rs` reads the diagnostic events with `cupertino_diagnostics::events` (the user store,
`session.log` and the system journal; [docs/src/diagnose.md](../../docs/src/diagnose.md#diagnostic-events) has the
record, the limits and the redaction table). It takes its `Sources` from `Context::provided::<Sources>()` when a test
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
  `cupertino-build-info`, the field names a version report uses), `report` (`cupertino_diagnostics::run(&sources, &[])`,
  its `summary`, fact `value` and `fix` texts through the free-text scrubber), `events` and `sources` (their `path` and
  `detail` scrubbed). `--include-logs` adds `logs`: `server.log`, `server.log.1` and `session.log` of the state
  directory, each line through `events::policy` (the store's salt, so hashes match the events', else a random one). A
  file that exists is `validation`/`file_exists`; one that cannot be written `temporary`/`write_failed`.

`tests/diagnose-events.rs` runs them on a fixture home, a fake journal function and `sh` as a fake
`journalctl --follow`: the three formats, the filters, the warnings, a follow that prints new store and journal lines
and ends on a stop from another thread (promptly, its child reaped) and on a closed output, `clear` with and without
`--yes`, and the export's keys, mode, refusal to overwrite and scrubbed logs.

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
| 3 | `unavailable`: `no_checkout`, `no_cargo`, `no_host_display`, `no_runtime_dir`, `server_not_installed`, `server_failed`, `not_a_vt`, `session_not_installed`, `session_failed` |
| 4 | `denied`: `inside_graphical_session` |
| 5 | `validation`: `pack_unreadable`, `manifest_invalid`, `font_invalid`, `checks_failed`, `layer_violations`, `metadata_failed`, `file_exists` (`diagnose export`) |
| 6 | `temporary`: also `output_failed` (stdout cannot be written), `interrupted` (a finite stream stopped), `write_failed` (also an export file) |
| 7 | `transaction_failed` (reserved) |
| 8 | `unsupported`: also `not_in_this_release` for the reserved groups `system` and `ai` |

Code 7 is reserved: no command of this release ends with it yet. `tests/cli.rs` runs a command line for each code
that is reachable and checks that `docs/src/cupertino-command.md` marks the others reserved.

**A closed output.** When the reader of stdout goes away (`EPIPE`, as in `cupertino … | head -1`), the command stops,
exits 0 and writes nothing to stderr. Any other write error (`ENOSPC`, `EIO`) exits 6 with one stderr line,
`cupertino: cannot write the output: …`, in human form only.

**Signals.** While a stream command runs, SIGINT and SIGTERM ask it to stop; a second one of either ends the process at
once by the signal's default action. A program the command starts gets the default actions of SIGPIPE, SIGINT and
SIGTERM, unless the parent ignored SIGINT or SIGTERM: the standard library resets the ignored SIGPIPE when it starts a
process, and `exec` resets caught signals (`tests/stream-process.rs` checks it). A
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
flag; each group is a module (`appearance`, `diagnose`, `desktop`, `version`, `completions`, `dev`; `diagnose_events`
adds the event commands to the `diagnose` group). `src/manual.rs`
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
  `diagnose events` is never `diagnose` with an argument). `tests/registry.rs` runs it on `REGISTRY`; a test registry
  of another test should pass it too.

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
  alone otherwise), `registry()` (the runner's, for the completions and pages), and `provided::<T>()`, a value a test
  hands in by type with `Context::provide(&value)` (fixture `Sources`, a layer `Policy`, a `StopGrace`). `Context::system()` is the real machine;
  `Context::isolated(root)` has no variables and works in `root`, so a test never reads the real `HOME`, `/usr` or
  session.
- `Runner::new(registry).context(cx).stop(&flag)` runs command lines: `run_to(args, request_id, out, err) -> u8`
  writes as it goes, `run(args, request_id) -> Output` collects both outputs and the exit code for a test.
  `cupertino_cli::run` and `run_to(args, request_id, out, err, stop)` do the same for `REGISTRY` on the real machine.
- `main_with(registry)` is a whole program: the process arguments (one that is not UTF-8 is
  `invalid_usage`/`invalid_encoding`), stdout and stderr, and a `StopFlag::on_signals()` flag. The `cupertino` binary
  is `main_with(REGISTRY)`; `examples/stream_probe.rs` runs probe streams through it for the process tests.

`tests/cli.rs` covers the envelope, usage errors, the parser and the help; `tests/stream.rs` every record type, `seq`,
the place of `end`, a closed output at every boundary, a full disk and stops from another thread; `tests/stream-process.rs`
a real pipe closed early and real SIGINT and SIGTERM; `tests/registry.rs` the registry rules and that the command page
names every command. Per group: `tests/diagnose.rs` (fixture `Sources`), `tests/desktop.rs` (a stand-in compositor that
answers on `--ready-fd`, stops, the kill after the grace, every error code, the installed binary under SIGTERM),
`tests/version.rs` (a fixture root with each part present, missing and unreadable), `tests/dev.rs` (fixture checkouts),
`tests/completions.rs` (bash, zsh and fish run the scripts; a missing shell skips unless `CUPERTINO_SHELLS_REQUIRED=1`)
and `tests/manual.rs` (the pages, and `mandoc -T lint` unless it is missing and `CUPERTINO_MANDOC_REQUIRED` is not 1).
`cupertino-server/tests/desktop-start.rs` runs `desktop start` on the real headless compositor.

The command is an unprivileged client: a group calls libraries of the `foundation` to `tools` layers or the D-Bus and
socket interfaces of the services; it never links compositor state.

## Open issues

None.
