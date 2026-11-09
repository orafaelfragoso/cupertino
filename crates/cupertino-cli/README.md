# cupertino-cli

The `cupertino` command. Layer: `tools`. Its subcommands are typed entries of one registry, so the help, the JSON
command tree, the dispatch, and the shell completions and manual pages all come from the same list.

## Usage

```sh
cupertino                                   # the help
cupertino help [<command>]                  # the help of the commands under <command>; a whole command adds its description and examples
cupertino --version
cupertino appearance assets check <dir>     # replaces `cupertino-assets check <dir>`
cupertino --format json appearance assets check <dir>
cupertino sdk info [--require] [--root <dir>] # the installed C SDK (the cupertino-kit package), part by part
cupertino sdk check [--root <dir>]          # build and run a probe against it with /usr/bin/cc and pkg-config
cupertino sdk new --lang c|rust <name> [--dir <dir>] # a starter project; `make -C <name>` builds the C one
cupertino --format jsonl help               # JSON lines: a result record and an end record
```

## Output

`--format` takes `human` (the default), `json` or `jsonl`, and may stand anywhere before a `--`.

- `human`: the output on stdout, errors on stderr as `cupertino: <message>`, warnings as `cupertino: warning: <message>`.
- `json`: one `cupertino.cli.v1` envelope line on stdout, on success and on failure:

  ```json
  {"schema":"cupertino.cli.v1","command":"version","request_id":"1f3a-…","ok":true,"data":{"cli":"0.1.0"}}
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
  {"schema":"cupertino.cli.stream.v1","command":"version","request_id":"1f3c-…","seq":0,"type":"result","data":{"cli":"0.1.0"}}
  {"schema":"cupertino.cli.stream.v1","command":"version","request_id":"1f3c-…","seq":1,"type":"end","ok":true,"exit_code":0}
  ```

  There is exactly one `end`, always last. A one-shot command prints `result` and `end`; a usage error before the
  command starts prints `end` alone; a failed command prints its events and then `end` with the error, no `result`.

In `json` and `jsonl` nothing goes to stderr. A stream command in `json` form gives one envelope at the end whose `data`
is its final object plus `events` (`[{event, data}]`) and `warnings` (`[{kind, code, message}]`); a report stream
(`Sink::json_result_only`) keeps its own data, with `warnings` only when there are some.

`request_id` is the process id and the time, in hex. `code` is a stable snake_case identifier within its `kind`;
`message` is for people and may change.

| Exit code | `kind` |
|---|---|
| 0 | (success; also a closed stdout, and a follow stream stopped by a signal) |
| 2 | `invalid_usage` |
| 3 | `unavailable`: the backend or service is not running or cannot be reached (reserved) |
| 4 | `denied` (reserved) |
| 5 | `validation` |
| 6 | `temporary`: also `output_failed` (stdout cannot be written) and `interrupted` (a finite stream stopped) |
| 7 | `transaction_failed` (reserved) |
| 8 | `unsupported`: also `not_in_this_release` for the reserved groups `system` and `ai` |

Codes 3, 4 and 7 are reserved: no command of this release ends with them yet. `tests/cli.rs` runs a command line for
each code that is reachable and checks that `docs/src/cupertino-command.md` marks the others reserved.

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
flag; each group is a module (`src/appearance.rs`, `src/sdk.rs`). The files `sdk new` writes and the probe `sdk check`
compiles are in `templates/`, built into the binary. The crate docs of `stream` state the rules for stream commands.

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
  `Failure::usage(code, message)` makes an `invalid_usage` one.
- `Context` is what a command may read of the machine: `root()` and `path("/usr/share/…")` (files of the running
  system, under `/` for the real command), `var(name)`, `current_dir()`, and `provided::<T>()`, a value a test hands in
  by type with `Context::provide(&value)` (fixture `Sources`, a fake clock). `Context::system()` is the real machine;
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
a real pipe closed early and real SIGINT and SIGTERM; `tests/registry.rs` the registry rules. `tests/sdk.rs` runs the `sdk` commands against a temporary
root with a fake SDK, compiler and pkg-config; `ci/c-sdk.py` runs them against a real one, built into a staging root.

The command is an unprivileged client: a group calls libraries of the `foundation` to `tools` layers or the D-Bus and
socket interfaces of the services; it never links compositor state.

## Open issues

- `cupertino diagnose`, `cupertino dev layers` and `cupertino desktop start --backend nested|tty|headless` are not
  wired yet; `cupertino-diagnose`, `cupertino-layers` and `cupertino-server` stay the entry points until then.
- `cupertino --version` prints only the CLI version; the commit (from `cupertino-build-info`), the OS image,
  compositor, data and ABI versions follow with `cupertino version`.
- Shell completions and manual pages are not generated yet; the registry already carries what they need.
- `cupertino-assets check` has no deprecation notice yet.
- `sdk check` compiles with the system's `cc` and checks the C ABI only. The Rust project of `sdk new` depends on the
  release's git tag of `cupertino-kit`, so it builds once that tag is published; `ci/c-sdk.py` checks it against the
  checkout instead.
