# cupertino-cli

The `cupertino` command. Layer: `tools`. Its subcommands are typed entries of one registry, so the help, the JSON
command tree and the dispatch come from the same list.

## Usage

```sh
cupertino                                   # the help
cupertino help [<command>]                  # the help of the commands under <command>
cupertino --version
cupertino appearance assets check <dir>     # replaces `cupertino-assets check <dir>`
cupertino --format json appearance assets check <dir>
```

`--format human` (the default) prints the output to stdout and errors to stderr as `cupertino: <message>`.
`--format json` prints one line to stdout, on success and on failure:

```json
{"schema":"cupertino.cli.v1","command":"version","request_id":"1f3a-…","ok":true,"data":{"cli":"0.1.0"}}
{"schema":"cupertino.cli.v1","command":"appearance","request_id":"1f3b-…","ok":false,"error":{"kind":"invalid_usage","code":"missing_subcommand","message":"…"}}
```

`--format` may stand anywhere on the line. `request_id` is the process id and the time, in hex. `code` is a stable
snake_case identifier within its `kind`; `message` is for people and may change.

| Exit code | `kind` |
|---|---|
| 0 | (success) |
| 2 | `invalid_usage` |
| 3 | `unavailable`: the backend or service is not running or cannot be reached |
| 4 | `denied` |
| 5 | `validation` |
| 6 | `temporary` |
| 7 | `transaction_failed` |
| 8 | `unsupported` |

## Design

`REGISTRY` in `src/lib.rs` lists the command groups; a group is a module (`src/appearance.rs`) with a
`COMMANDS: &[Command]` constant. A `Command` has its path (`["appearance", "assets", "check"]`), a summary, its
arguments (`Positional`, required; `Option`, `--name value`; `Flag`, `--name`) and a `fn(&Args) -> Result<Reply,
Failure>`. A `Reply` carries the serialized data for JSON and the text for people; a command never prints.

`run(args, request_id)` returns the exit code and both outputs instead of printing them, so the tests in
`tests/cli.rs` compare whole outputs.

The command is an unprivileged client: a group calls libraries of the `foundation` to `tools` layers or, later, the
D-Bus and socket interfaces of the services; it never links compositor state.

## Open issues

- `--format jsonl` for streaming commands, shell completions and manual pages from the registry.
- `cupertino --version` prints only the CLI version; the OS image, compositor and API versions follow.
- `cupertino dev layers` and `cupertino desktop start --backend nested|tty|headless` are not wired yet;
  `cupertino-layers` and `cupertino-server` stay the entry points until then.
- `cupertino-assets check` has no deprecation notice yet.
