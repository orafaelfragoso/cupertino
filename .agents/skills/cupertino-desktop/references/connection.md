# Host connection and installation

Read this when the Cupertino tools are missing, disconnected, or being installed on another machine. This skill uses the shared SKILL.md format; it adds no runtime or MCP SDK. It does not configure model credentials.

## Discover the selected live session

Run the host as the logged-in user inside Cupertino. Inspect only the relevant environment, not a dump that could contain secrets:

```sh
printf 'WAYLAND_DISPLAY=%s\nXDG_RUNTIME_DIR=%s\n' "$WAYLAND_DISPLAY" "$XDG_RUNTIME_DIR"
test -x /usr/bin/cupertino-control
```

The in-tree Rust bridge runs as `/usr/bin/cupertino-control --mcp`. With no positional display it requires the host's `WAYLAND_DISPLAY`; alternatively its positional argument selects an explicit display. `XDG_RUNTIME_DIR` must belong to the same login user. Do not hardcode another user's runtime directory, persist a display from a previous login, run the host as root, or select the first socket found. A terminal launched inside a nested compositor targets that nested session.

## Codex

When registration is needed, inspect existing configuration before adding a duplicate:

```sh
codex mcp get cupertino
codex mcp add cupertino -- /usr/bin/cupertino-control --mcp
```

The corresponding user configuration is:

```toml
[mcp_servers.cupertino]
command = "/usr/bin/cupertino-control"
args = ["--mcp"]
env_vars = ["XDG_RUNTIME_DIR", "WAYLAND_DISPLAY"]
```

Preserve unrelated settings. Approve the selected server through the host's normal tool-permission UI if required; do not disable approvals globally. Select the user's chosen model, such as Terra 6.1, independently of the bridge.

## Claude Code

Launch Claude Code from the same desktop environment so its stdio child inherits the current session variables. For a user-wide registration, after checking existing settings:

```sh
claude mcp get cupertino
claude mcp add --transport stdio --scope user cupertino -- /usr/bin/cupertino-control --mcp
```

Use `/mcp` inside Claude Code to inspect/reconnect the server. Host tool names commonly have the `mcp__cupertino__` prefix. Do not use a fixed `--env WAYLAND_DISPLAY=...` value that will point at a stale or different display after a later login. The local MCP needs no API key or network service.

These instructions target **Claude Code**, including a local Claude Code session hosted in a desktop app. Installing a local skill does not grant a cloud Claude/Cowork session access to this machine; it also needs an explicitly supported local MCP connection. Do not expose Cupertino's private Unix socket over the network to compensate.

## Install the skill

The canonical source in the Cupertino checkout is `.agents/skills/cupertino-desktop/`. A relative `.claude/skills/cupertino-desktop` symlink exposes the same source to Claude Code when working in that checkout.

For control from other projects, copy that whole skill directory (including `references`) to the user's skill directory. Current Codex documentation uses `~/.agents/skills/`; installations using `CODEX_HOME/skills` or `~/.codex/skills/` should use their existing discovery location. Claude Code uses `~/.claude/skills/`. Prefer a shared installed copy with a symlink for the other host; avoid divergent copies or multiple same-name entries in one host's discovery paths. Never overwrite an existing customized skill without inspecting it.

Invoke it as `$cupertino-desktop` in Codex or `/cupertino-desktop` in Claude Code; automatic discovery is also enabled. Refresh only the host's skill/MCP connection if discovery is cached. This is separate from restarting Cupertino.

## Diagnose connection failures

- Missing tools: check registration, host discovery/approvals, binary path and inherited session environment. A skill supplies instructions; it cannot manufacture tools the host does not expose.
- Native handshake/version failure: confirm matching installed controller and compositor support protocol v3. Stage server updates for the next login; do not restart the current session.
- Failed native response after a controller update: read status to reconnect, then obtain fresh consent. Never replay an uncertain mutation.
- Another host cannot obtain native status while the current host works: the controller serves one connected native peer at a time. Finish the owning task and stop its grant before switching hosts. Do not disconnect someone else’s active control task. A legacy bridge can retain the socket even after Stop and block new peers.
- Revoked epoch that remains stuck across status: an old already-running MCP process can retain the pre-fix lease behavior. Refresh only that host's MCP connection, then call status. Replacing the disk binary does not update an existing process. Do not kill all matching processes or restart the desktop.
- Repeated pauses: inspect phase and whether genuine physical input or protected focus intervened. Read-only coexistence depends on the running compositor version. The absence of a permanent panel is expected. Request a fresh native resume only when the person wants to continue.

Authoritative host references: [Codex skills](https://learn.chatgpt.com/docs/build-skills), [Claude Code skills](https://code.claude.com/docs/en/skills), [Claude Code MCP](https://code.claude.com/docs/en/mcp). Live tool schemas and Cupertino's source define the actual desktop API.
