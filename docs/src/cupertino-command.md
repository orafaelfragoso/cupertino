# The cupertino command

`cupertino` is the command-line entry point of the desktop. It is built from the `cupertino-cli` crate (`crates/cupertino-cli`). Today it has the subcommands below, and it is not part of the packages yet: `packaging/release.py` lists `cupertino-diagnose` but not `cupertino`, so build it from a checkout (`cargo build -p cupertino-cli`).

This page describes the command as it is on `main`. For the machine check, see [cupertino-diagnose](diagnose.md), which is a separate program for now.

## Usage

```sh
cupertino                                   # the help
cupertino help [COMMAND]                    # the help of the commands under COMMAND
cupertino --version
cupertino appearance assets check DIR       # read the asset pack in DIR and report its problems
cupertino --format json appearance assets check DIR
cupertino --format jsonl help               # JSON lines: one record per line
cupertino sdk info                          # the installed C SDK: header, library, links, pkg-config file, package
cupertino sdk check                         # build and run a probe against it with /usr/bin/cc
cupertino sdk new --lang c demo             # a C project in ./demo; `make -C demo` builds it
```

`--format` takes `human` (the default), `json` or `jsonl` and may stand anywhere on the line before a `--`. Any other value, or none, is a usage error. After `--`, every word belongs to the command. `cupertino help COMMAND` for a whole command also prints its description and examples, and marks a preview or deprecated command.

## Subcommands

| Command | What it does |
|---|---|
| `appearance assets check DIR` | Reads the asset pack in `DIR` (it needs a `manifest.json`) the way the desktop processes do. It reports the pack's problems and what provides each logical name, the pack or the built-in fallback. It replaces `cupertino-assets check DIR`. |
| `sdk info [--require] [--root DIR]` | Reports the C SDK of the `cupertino-kit` package: the header `/usr/include/cupertinokit-1/cupertinokit.h` and its ABI version, the library behind `libcupertinokit.so.1` and its soname, the link `libcupertinokit.so`, `cupertinokit.pc` and its `Version`, and the package's build record. Each part is `ok`, `not_installed` or `unreadable`, and files that disagree are listed in `problems`. It exits 0 whatever it finds; with `--require` a missing part exits with 3 (`sdk_not_installed`) and an unreadable or inconsistent SDK with 5 (`sdk_invalid`). `--root` reads an SDK installed under another directory. |
| `sdk check [--root DIR]` | Compiles a probe against the SDK with `/usr/bin/cc` and the flags of `/usr/bin/pkg-config` (never `PATH`), runs it, and reports the steps `compiler`, `pkg_config`, `compile` and `run` as `step` events. Without a compiler or pkg-config it exits with 3 (`no_compiler`, `no_pkg_config`), and with 3 (`sdk_not_installed`) when pkg-config finds no `cupertinokit`. A compile error exits with 5 (`compile_failed`, the compiler's messages in `data.stderr`), as do a header and library of different versions (`abi_mismatch`) and a probe that fails (`probe_failed`). In the `json` form `data` is the report. |
| `sdk new --lang c\|rust NAME [--dir DIR]` | Preview. Writes a project into `NAME` (or `DIR`): for C a `main.c` that runs headless, a `Makefile` that takes its flags from `pkg-config cupertinokit`, `NAME.desktop` and a README; for Rust a `Cargo.toml` on the `cupertino-kit` crate of this release, a `src/main.rs` with a window, `NAME.desktop` and a README. `NAME` is the app id: dot-separated elements of letters, digits, `-` and `_`, none starting with a digit (else 2, `invalid_name`). A directory that is not empty exits with 5 (`directory_not_empty`), one that cannot be written with 4 (`not_writable`). |

The registry that holds this list is `REGISTRY` in `crates/cupertino-cli/src/lib.rs`. The help, the JSON command tree and the dispatch all come from it.

## Output

With `--format human`, the output goes to standard output, an error goes to standard error as `cupertino: MESSAGE`, and a warning as `cupertino: warning: MESSAGE`.

With `--format json`, one line goes to standard output, on success and on failure:

```json
{"schema":"cupertino.cli.v1","command":"version","request_id":"1f3a-…","ok":true,"data":{"cli":"0.1.0"}}
{"schema":"cupertino.cli.v1","command":"appearance","request_id":"1f3b-…","ok":false,"error":{"kind":"invalid_usage","code":"missing_subcommand","message":"…"}}
```

`request_id` is the process id and the time, in hexadecimal. `code` is a stable identifier within its `kind`; `message` is for people and can change.

With `--format jsonl`, standard output carries JSON lines with the schema `cupertino.cli.stream.v1`. Every line is one object that starts with `schema`, `command`, `request_id`, `seq` and `type`. `seq` counts the lines from 0 without a gap. The `type` is one of:

| `type` | Other fields | Meaning |
|---|---|---|
| `event` | `event`, `data` | Something the command reports while it runs. `event` is a snake_case name that stays the same for the command. |
| `error` | `error`: `{kind, code, message}` | A problem the command continued past. |
| `result` | `data` | The command's result: the same object the JSON envelope's `data` holds. |
| `end` | `ok`, `exit_code`, `error` when `ok` is false, `reason` | The last line, always exactly one. `reason` is `stopped` when a stop ended a command that runs until stopped. |

```json
{"schema":"cupertino.cli.stream.v1","command":"version","request_id":"1f3c-…","seq":0,"type":"result","data":{"cli":"0.1.0"}}
{"schema":"cupertino.cli.stream.v1","command":"version","request_id":"1f3c-…","seq":1,"type":"end","ok":true,"exit_code":0}
```

A command that computes its whole result prints `result` and `end`. A usage error found before the command starts prints `end` alone. A failed command prints `end` with its error and no `result`. In the `json` and `jsonl` forms nothing goes to standard error.

A streaming command reports events while it runs. In the human form each event prints one line as it happens. In the `json` form the envelope comes at the end, and its `data` adds `events` (`[{event, data}]`) and `warnings` (`[{kind, code, message}]`) to the command's own fields. A command whose result already holds what its events said, such as a report, keeps its own `data` in the `json` form and adds `warnings` only when there are some.

When the reader of standard output goes away, as in `cupertino --format jsonl help | head -1`, the command stops, exits with 0 and writes nothing to standard error. Any other write error, such as a full disk, exits with 6 (`temporary`, code `output_failed`); in the human form one line on standard error says why.

While a streaming command runs, `Ctrl-C` (SIGINT) and SIGTERM ask it to stop, and a second one of either ends it at once. A command that runs until stopped then ends with `end` `{"ok": true, "exit_code": 0, "reason": "stopped"}`. A command with a finite amount of work ends with `temporary`, code `interrupted`, exit code 6.

## Exit codes

| Code | `error.kind` | Meaning |
|---|---|---|
| 0 | | Success; also a closed standard output, and a command that runs until stopped when a stop ends it. |
| 2 | `invalid_usage` | The command line is not valid: an unknown command or option, or a missing or wrong value. |
| 3 | `unavailable` | What the command needs is not there or not running: the C SDK or the compiler for `sdk info --require` and `sdk check` (`sdk_not_installed`, `no_compiler`, `no_pkg_config`). |
| 4 | `denied` | The caller is not allowed to do this here: `sdk new` into a directory the caller cannot write (`not_writable`). |
| 5 | `validation` | What the command checked or read is not valid: a pack that cannot be read (`pack_unreadable`, `manifest_invalid`, `font_invalid`), an inconsistent C SDK or a probe that does not build or run against it (`sdk_invalid`, `compile_failed`, `abi_mismatch`, `probe_failed`), or a directory `sdk new` cannot use (`directory_not_empty`, `not_a_directory`). |
| 6 | `temporary` | A problem that may pass: output that cannot be written (`output_failed`), files `sdk new` or `sdk check` cannot write (`write_failed`), a compiler or probe that does not finish in time (`timed_out`), or a streaming command stopped before it finished (`interrupted`). |
| 7 | `transaction_failed` | A system or package transaction failed. Reserved. |
| 8 | `unsupported` | This release or this system does not do what was asked: the command groups kept for a later release, `system` and `ai`, answer `not_in_this_release`. |

Today a command line can end with 0, 2, 3, 4, 5, 6 (output that cannot be written) or 8 (`cupertino system`, `cupertino ai`). Code 7 is reserved: it keeps its meaning for the commands to come, and no command of this release ends with it.

## Compatibility

Every command is `stable`, `preview` or `deprecated`; `cupertino help` marks the last two, and the JSON help has a `stability` field.

- Within `cupertino.cli.v1` and `cupertino.cli.stream.v1`, a stable command's options, `kind` and `code` values, exit codes, data fields, record types and `event` names are only added to. Nothing is renamed or removed.
- Consumers ignore fields and `event` names they do not know.
- A change that is not additive raises the schema to `cupertino.cli.v2` or `cupertino.cli.stream.v2`.
- A preview command may change or go away in any release. Every `dev` command is a preview.
- A stable command is removed only after one minor release as `deprecated`. In that release it still works, prints a warning on standard error in the human form (an `error` record with `kind` `unsupported` and `code` `deprecated` in JSON lines), and this page names what replaces it.
- The group names `system` and `ai` are reserved for later releases.

The JSON help, `cupertino --format json help`, lists every command with `path`, `summary`, `args` (each with `name`, `help`, `kind`, `values`, `repeat` and `hint`), `stability`, `streams`, `description` and `examples`. Shell completions and manual pages are to be generated from the same registry.

## Not there yet

- `cupertino diagnose`: the checks run as `cupertino-diagnose`, a program of its own.
- Shell completions and manual pages.
- `cupertino --version` prints only the version of the command.
- `cupertino dev layers` and `cupertino desktop start`. Use `cupertino-layers` and `cupertino-server` for now.
