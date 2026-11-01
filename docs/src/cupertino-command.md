# The cupertino command

`cupertino` is the command-line entry point of the desktop. It is built from the `cupertino-cli` crate (`crates/cupertino-cli`). Today it has one subcommand, and it is not part of the packages yet: `packaging/release.py` lists `cupertino-diagnose` but not `cupertino`, so build it from a checkout (`cargo build -p cupertino-cli`).

This page describes the command as it is on `main`. For the machine check, see [cupertino-diagnose](diagnose.md), which is a separate program for now.

## Usage

```sh
cupertino                                   # the help
cupertino help [COMMAND]                    # the help of the commands under COMMAND
cupertino --version
cupertino appearance assets check DIR       # read the asset pack in DIR and report its problems
cupertino --format json appearance assets check DIR
```

`--format` takes `human` (the default) or `json` and may stand anywhere on the line. Any other value, or none, is a usage error.

## Subcommands

| Command | What it does |
|---|---|
| `appearance assets check DIR` | Reads the asset pack in `DIR` (it needs a `manifest.json`) the way the desktop processes do. It reports the pack's problems and what provides each logical name, the pack or the built-in fallback. It replaces `cupertino-assets check DIR`. |

The registry that holds this list is `REGISTRY` in `crates/cupertino-cli/src/lib.rs`. The help, the JSON command tree and the dispatch all come from it.

## Output

With `--format human`, the output goes to standard output and an error goes to standard error as `cupertino: MESSAGE`.

With `--format json`, one line goes to standard output, on success and on failure:

```json
{"schema":"cupertino.cli.v1","command":"version","request_id":"1f3a-…","ok":true,"data":{"cli":"0.1.0"}}
{"schema":"cupertino.cli.v1","command":"appearance","request_id":"1f3b-…","ok":false,"error":{"kind":"invalid_usage","code":"missing_subcommand","message":"…"}}
```

`request_id` is the process id and the time, in hexadecimal. `code` is a stable identifier within its `kind`; `message` is for people and can change.

## Exit codes

| Code | `error.kind` | Meaning |
|---|---|---|
| 0 | | Success. |
| 2 | `invalid_usage` | The command line was wrong. |
| 3 | `unavailable` | The backend or service the command needs is not running or cannot be reached. |
| 4 | `denied` | |
| 5 | `validation` | For example a pack that cannot be read (`pack_unreadable`, `manifest_invalid`, `font_invalid`). |
| 6 | `temporary` | |
| 7 | `transaction_failed` | A system or package transaction failed. |
| 8 | `unsupported` | |

Only `appearance assets check` exists, so today it can end with 0, 2 or 5. The other codes are defined for the commands to come.

## Not there yet

- `cupertino diagnose`: the checks run as `cupertino-diagnose`, a program of its own.
- `--format jsonl`, shell completions and manual pages.
- `cupertino --version` prints only the version of the command.
- `cupertino dev layers` and `cupertino desktop start`. Use `cupertino-layers` and `cupertino-server` for now.
