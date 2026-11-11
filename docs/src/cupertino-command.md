# The cupertino command

`cupertino` is the command-line entry point of the desktop. It is built from the `cupertino-cli` crate (`crates/cupertino-cli`) and installed as `/usr/bin/cupertino` by the `cupertino` package, with its completions for bash, zsh and fish and its manual pages (`man cupertino`, and `man cupertino-GROUP` for each command group).

This page describes the command as it is on `main`. For what the machine checks look for, see [Checking a machine](diagnose.md).

## Usage

```sh
cupertino                                   # the help
cupertino help [COMMAND]                    # the help of the commands under COMMAND
cupertino --version                         # this command's version and commit
cupertino version                           # the version of every part of the desktop
cupertino diagnose [--area AREA]... [--strict]
cupertino desktop start --backend nested|headless|tty [--size WxH] [--scale S] [--spawn CMD]...
cupertino appearance assets check DIR       # read the asset pack in DIR and report its problems
cupertino --format jsonl diagnose           # JSON lines: one record per line
cupertino diagnose events --since 24h       # the diagnostic events of the last day
cupertino diagnose events --follow          # and the new ones, until Ctrl-C
cupertino diagnose events clear --yes       # delete the stored events
cupertino diagnose export --output FILE     # one JSON file for a bug report
cupertino sdk info                          # the installed C SDK: header, library, links, pkg-config file, package
cupertino sdk check                         # build and run a probe against it with /usr/bin/cc
cupertino sdk new --lang c demo             # a C project in ./demo; `make -C demo` builds it
cupertino shell list                        # the shell processes of this session and the program each runs
cupertino shell restart dock                # end the dock and start it again at once
cupertino shell use-installed dock          # take the dock's development build away and restart it
sudo cupertino dev-folder enable            # once: let shell processes start from ~/.local/lib/cupertino/dev
sudo cupertino dev-folder disable           # stop that
```

`--format` takes `human` (the default), `json` or `jsonl` and may stand anywhere on the line before a `--`. Any other value, or none, is a usage error. After `--`, every word belongs to the command. `cupertino help COMMAND` for a whole command also prints its description and examples, and marks a preview or deprecated command.

## Commands

| Command | What it does |
|---|---|
| `appearance assets check DIR` | Reads the asset pack in `DIR` (it needs a `manifest.json`) the way the desktop processes do. It reports the pack's problems and what provides each logical name, the pack or the built-in fallback. A pack that cannot be read exits with 5. It replaces `cupertino-assets check DIR`. |
| `diagnose` | Checks the GPU and its driver, the session, the portals, the services, how the last sessions ended, the logs and the errors the desktop recorded in the last day; `--area` picks areas (repeatable), in the order `gpu`, `session`, `portals`, `services`, `crash`, `logs`, `events`. It exits 0 once the checks ran; with `--strict` a failed check exits with 5 (`checks_failed`). In the `json` form `data` is the report, the object `cupertino-diagnose --json` prints; in `jsonl` each check is a `check` event and the result counts the checks of each status. It replaces `cupertino-diagnose`. |
| `diagnose events [--since DURATION\|TIME] [--component NAME]... [--id PREFIX]... [--severity LEVEL] [--follow]` | Prints the diagnostic events of the user store, `session.log` and the system journal, merged in time order, one line each (an `event` record whose data is the `cupertino.diag.v1` record in the JSON forms); the final data lists the `sources` with their status (`ok`, `empty`, `denied`, `unavailable`, `corrupt`) and the `counts`. `--follow` goes on with the new events until stopped. |
| `diagnose events clear --yes` | Deletes the stored events and keeps the salt; without `--yes` it fails with `confirmation_required` (exit 2). |
| `diagnose export --output FILE [--include-logs] [--since DURATION\|TIME]` | Writes one JSON document, mode 0600, with the versions, the diagnostics report, the events and the sources, and with `--include-logs` the scrubbed `server.log`, `server.log.1` and `session.log`. A file that exists is never overwritten (`file_exists`, exit 5). |
| `desktop start` | Starts the compositor and stays attached to it. `--backend nested` runs it in a window of the current desktop (it needs `WAYLAND_DISPLAY` or `DISPLAY`), `headless` offscreen; both need `XDG_RUNTIME_DIR` and take `--size`, `--scale` and `--spawn`. `--backend tty` becomes `cupertino-session`, the session of a text console, and refuses to run inside a graphical session (4, `inside_graphical_session`) or from anything but a text console (3, `not_a_vt`). Events: `starting`, `ready` (with the compositor's `WAYLAND_DISPLAY`), `exited` (its status or signal, and how long it ran). Ctrl-C or SIGTERM ask the compositor to end with SIGTERM; one still running 10 seconds later is killed, and the command exits 0. A compositor that fails, or ends before it is ready, exits the command with 3 (`server_failed`). |
| `sdk info [--require] [--root DIR]` | Reports the C SDK of the `cupertino-kit` package: the header `/usr/include/cupertinokit-1/cupertinokit.h` and its ABI version, the library behind `libcupertinokit.so.1` and its soname, the link `libcupertinokit.so`, `cupertinokit.pc` and its `Version`, and the package's build record. Each part is `ok`, `not_installed` or `unreadable`, and files that disagree are listed in `problems`. It exits 0 whatever it finds; with `--require` a missing part exits with 3 (`sdk_not_installed`) and an unreadable or inconsistent SDK with 5 (`sdk_invalid`). `--root` reads an SDK installed under another directory. |
| `sdk check [--root DIR]` | Compiles a probe against the SDK with `/usr/bin/cc` and the flags of `/usr/bin/pkg-config` (never `PATH`), runs it, and reports the steps `compiler`, `pkg_config`, `compile` and `run` as `step` events. Without a compiler or pkg-config it exits with 3 (`no_compiler`, `no_pkg_config`), and with 3 (`sdk_not_installed`) when pkg-config finds no `cupertinokit`. A compile error exits with 5 (`compile_failed`, the compiler's messages in `data.stderr`), as do a header and library of different versions (`abi_mismatch`) and a probe that fails (`probe_failed`). In the `json` form `data` is the report. |
| `sdk new --lang c\|rust NAME [--dir DIR] [--source CHECKOUT]` | Preview. Writes a project into `NAME` (or `DIR`): for C a `main.c` that runs headless, a `Makefile` that takes its flags from `pkg-config cupertinokit`, `NAME.desktop` and a README; for Rust a `Cargo.toml` on the `cupertino-kit` crate of this release (the git tag `v<version>`, or with `--source` the crates of a Cupertino checkout by path, for offline work or a release whose tag is not published yet), a `src/main.rs` with a window, `NAME.desktop` and a README. `NAME` is the app id: dot-separated elements of letters, digits, `-` and `_`, none starting with a digit, and not starting with `-` (else 2, `invalid_name`); `--source` that is not a checkout, or given for C, exits with 2 (`invalid_value`). It never replaces a file: a directory that is not empty exits with 5 (`directory_not_empty`), one that cannot be written with 4 (`not_writable`). |
| `shell list` | Preview. Lists the processes the server of the current session supervises, in start order, from `ListShellProcesses` of its `org.cupertino.Session1` interface on the session bus: `name`, `state` (`running`, `stopping`, `waiting`, `stopped`), and while it runs `pid`, `started` (seconds since 1970), `start_ticks` (field 22 of `/proc/PID/stat`, which tells a process from a later one with the same pid), `program` and `from_dev_folder`; `installed` is the configured program and `dev_program` the development folder's file the process starts when it is there (`null` when the folder does not apply). Without a session bus it exits with 3 (`no_session_bus`), without a server on it with 3 (`no_server`), with a server that has no such method with 8 (`not_supported`); a caller the server refuses exits with 4 (`refused`): only an unsandboxed process of the session's own user may call it. |
| `shell restart NAME` | Preview. Asks the server to end the process (SIGTERM, then SIGKILL 5 seconds later) and start it again at once whatever its exit status, the development folder looked at again, and prints the new process once it runs, with the fields of `shell list`. An unknown name exits with 2 (`unknown_process`), a process that ended and could not start again with 6 (`not_started`; the server tries again after its throttle). |
| `shell use-installed NAME` | Preview. Removes the process's `dev_program` file from the development folder when it is there (4, `cannot_remove`, when it cannot), then restarts it as `shell restart` does, so it runs its installed program; `data.removed` names the file removed or is `null`. A process the folder does not apply to exits with 5 (`no_dev_program`) without a restart. |
| `dev-folder enable [--folder PATH] [--components NAME,...]` | Preview, as root. Sets the `dev_folder` key of `/etc/cupertino/compositor.json` (every other key keeps its text; the file is replaced atomically and keeps its mode) to the folder, `--folder` or `~/.local/lib/cupertino/dev` of the user who ran sudo (`SUDO_UID`, `SUDO_GID`), and the processes allowed to start from it, `--components` or every process of the file's shell list whose roles are all among `session`, `menu_bar`, `dock` and `window_manager`. A missing folder is created as that user with mode 0700. Not root exits with 4 (`not_root`), without `SUDO_UID` and `SUDO_GID` with 2 (`no_invoking_user`); a name that is not such a process (`unknown_component`, `ineligible_component`), a folder of another user or that others can write (`unsafe_folder`), and a file that is not a JSON object or has no shell list (`invalid_system_config`, `no_shell_list`) exit with 5, a missing file with 3 (`no_system_config`). |
| `dev-folder disable` | Preview, as root. Removes the `dev_folder` key, keeping everything else; the folder and its files stay. Without the key it changes nothing. |
| `version` | One entry per part: `cli`, `runtime-package`, `data-package`, `compositor-running`, `c-abi`, `theme`, `asset-pack`, the output schemas, `os-image` and `kernel`, each with `name`, `version`, `source`, `status` (`ok`, `not_installed`, `unreadable`, `not_running`), `detail` and `commit`. A part that is not there never fails the command. |
| `completions SHELL` | Prints the completion script of `bash`, `zsh` or `fish`, generated from the registry. |
| `dev layers [--root DIR] [--graph]` | Preview. Checks that the crates of a source checkout depend on each other as the layer rules allow and that the licences the MPL-2.0 crates link are accepted. Outside a checkout it exits with 3 (`no_checkout`), without Cargo with 3 (`no_cargo`), on problems with 5 (`layer_violations`, each problem in `data.problems`). |
| `dev manual DIR` | Preview. Writes the manual pages into `DIR`, dated by `SOURCE_DATE_EPOCH` or a fixed date, so two runs write the same bytes. |

[Checking a machine](diagnose.md#diagnostic-events) describes the events, their limits, what they never contain and journal access.

The registry that holds this list is `REGISTRY` in `crates/cupertino-cli/src/lib.rs`. The help, the JSON command tree, the dispatch, the completions and the manual pages all come from it.

`desktop start` runs `CUPERTINO_SERVER` when it is set (for development), else the `cupertino-server` installed beside `cupertino`, else `/usr/bin/cupertino-server`; it never searches `PATH`. The compositor says it is ready on the descriptor `--ready-fd` names, and while it runs it describes itself in `$XDG_RUNTIME_DIR/cupertino/server-NAME.json`, which `cupertino version` reads for the compositor of `WAYLAND_DISPLAY`.

## Output

With `--format human`, the output goes to standard output, an error goes to standard error as `cupertino: MESSAGE`, and a warning as `cupertino: warning: MESSAGE`.

With `--format json`, one line goes to standard output, on success and on failure:

```json
{"schema":"cupertino.cli.v1","command":"version","request_id":"1f3a-…","ok":true,"data":{"cli":"0.1.0","commit":"4089d615b68e…"}}
{"schema":"cupertino.cli.v1","command":"appearance","request_id":"1f3b-…","ok":false,"error":{"kind":"invalid_usage","code":"missing_subcommand","message":"…"}}
```

`request_id` is the process id and the time, in hexadecimal. `code` is a stable identifier within its `kind`; `message` is for people and can change. A failure may carry `data` beside `error` when the command has more to say, as `dev layers` does with its problems.

With `--format jsonl`, standard output carries JSON lines with the schema `cupertino.cli.stream.v1`. Every line is one object that starts with `schema`, `command`, `request_id`, `seq` and `type`. `seq` counts the lines from 0 without a gap. The `type` is one of:

| `type` | Other fields | Meaning |
|---|---|---|
| `event` | `event`, `data` | Something the command reports while it runs. `event` is a snake_case name that stays the same for the command. |
| `error` | `error`: `{kind, code, message}` | A problem the command continued past. |
| `result` | `data` | The command's result: the same object the JSON envelope's `data` holds. |
| `end` | `ok`, `exit_code`, `error` when `ok` is false, `reason` | The last line, always exactly one. `reason` is `stopped` when a stop ended a command that runs until stopped. |

```json
{"schema":"cupertino.cli.stream.v1","command":"version","request_id":"1f3c-…","seq":0,"type":"result","data":{"components":[…]}}
{"schema":"cupertino.cli.stream.v1","command":"version","request_id":"1f3c-…","seq":1,"type":"end","ok":true,"exit_code":0}
```

A command that computes its whole result prints `result` and `end`. A usage error found before the command starts prints `end` alone. A failed command prints `end` with its error and no `result`. In the `json` and `jsonl` forms nothing goes to standard error. `desktop start --backend tty` prints `starting` and then becomes the session, so no `end` follows it.

A streaming command reports events while it runs. In the human form each event prints one line as it happens. In the `json` form the envelope comes at the end, and its `data` adds `events` (`[{event, data}]`) and `warnings` (`[{kind, code, message}]`) to the command's own fields. A command whose result already holds what its events said, such as a report (`diagnose`, `dev layers`), keeps its own `data` in the `json` form and adds `warnings` only when there are some.

When the reader of standard output goes away, as in `cupertino --format jsonl help | head -1`, the command stops, exits with 0 and writes nothing to standard error. Any other write error, such as a full disk, exits with 6 (`temporary`, code `output_failed`); in the human form one line on standard error says why.

While a streaming command runs, `Ctrl-C` (SIGINT) and SIGTERM ask it to stop, and a second one of either ends it at once. A command that runs until stopped then ends with `end` `{"ok": true, "exit_code": 0, "reason": "stopped"}`. A command with a finite amount of work ends with `temporary`, code `interrupted`, exit code 6.

## Exit codes

| Code | `error.kind` | Meaning |
|---|---|---|
| 0 | | Success; also a closed standard output, and a command that runs until stopped when a stop ends it. |
| 2 | `invalid_usage` | The command line is not valid: an unknown command or option, or a missing or wrong value; also a command that needs a confirmation it did not get (`confirmation_required`). |
| 3 | `unavailable` | What the command needs is not there or not running: no checkout or no Cargo for `dev layers`, no display, runtime directory, compositor or text console for `desktop start`, or a compositor that failed; no session bus or no server on it for `shell` (`no_session_bus`, `no_server`), no system configuration for `dev-folder` (`no_system_config`); the C SDK or the compiler for `sdk info --require` and `sdk check` (`sdk_not_installed`, `no_compiler`, `no_pkg_config`), a tool `sdk check` cannot start (`tool_failed`), or a working directory that cannot be read (`no_working_directory`). |
| 4 | `denied` | The caller is not allowed to do this here: `desktop start --backend tty` inside a graphical session (`inside_graphical_session`), `sdk new` into a directory the caller cannot write (`not_writable`), a `shell` command the server refuses (`refused`) or a development file that cannot be removed (`cannot_remove`), `dev-folder` without root (`not_root`), or a development folder its user cannot create (`cannot_create_folder`). |
| 5 | `validation` | What the command checked or read is not valid: a pack that cannot be read (`pack_unreadable`, `manifest_invalid`, `font_invalid`), layer problems (`layer_violations`, `metadata_failed`), a failed check with `diagnose --strict` (`checks_failed`), an export file that exists (`file_exists`), an event store that cannot be cleared (`store_unavailable`, `no_home`), an inconsistent C SDK or a probe that does not build or run against it (`sdk_invalid`, `compile_failed`, `abi_mismatch`, `probe_failed`), a directory `sdk new` cannot use (`directory_not_empty`, `not_a_directory`), a process the development folder does not apply to (`no_dev_program`), or what `dev-folder enable` refuses (`unknown_component`, `ineligible_component`, `no_eligible_component`, `unsafe_folder`, `invalid_system_config`, `no_shell_list`). |
| 6 | `temporary` | A problem that may pass: output that cannot be written (`output_failed`), a system configuration `dev-folder` cannot replace (`write_failed`), a shell process that did not start again after `shell restart` (`not_started`) or a server that did not answer in time (`no_answer`), manual pages, an export file or files of `sdk new` or `sdk check` that cannot be written (`write_failed`), a compiler or probe that does not finish in time (`timed_out`), or a streaming command stopped before it finished (`interrupted`). |
| 7 | `transaction_failed` | A system or package transaction failed. Reserved. |
| 8 | `unsupported` | This release or this system does not do what was asked: the command groups kept for a later release, `system` and `ai`, answer `not_in_this_release`; a running server older than the `shell` commands answers `not_supported`. |

Code 7 is reserved: it keeps its meaning for the commands to come, and no command of this release ends with it.

## Compatibility

Every command is `stable`, `preview` or `deprecated`; `cupertino help` marks the last two, and the JSON help has a `stability` field.

- Within `cupertino.cli.v1` and `cupertino.cli.stream.v1`, a stable command's options, `kind` and `code` values, exit codes, data fields, record types and `event` names are only added to. Nothing is renamed or removed.
- Consumers ignore fields and `event` names they do not know.
- A change that is not additive raises the schema to `cupertino.cli.v2` or `cupertino.cli.stream.v2`.
- A preview command may change or go away in any release. Every `dev` command is a preview.
- A stable command is removed only after one minor release as `deprecated`. In that release it still works, prints a warning on standard error in the human form (an `error` record with `kind` `unsupported` and `code` `deprecated` in JSON lines), and this page names what replaces it.
- The group names `system` and `ai` are reserved for later releases.

The JSON help, `cupertino --format json help`, lists every command with `path`, `summary`, `args` (each with `name`, `help`, `kind`, `values`, `repeat` and `hint`), `stability`, `streams`, `description` and `examples`.

## The programs it replaces

`cupertino-diagnose` and `cupertino-assets check` still work as before. In the human form each prints one line on standard error naming the command that replaces it (`cupertino diagnose`, `cupertino appearance assets check`); the output of `cupertino-diagnose --json` is unchanged. Both will be removed in a later release.
