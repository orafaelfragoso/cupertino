# Host connection and installation

Read this when the Cupertino tools are missing or disconnected, or when registering the bridge on a machine. This skill uses the shared SKILL.md format; it adds no runtime or MCP SDK. It does not configure model credentials.

## Discover the selected live session

Run the host as the logged-in user inside Cupertino. Inspect only the relevant environment, not a dump that could contain secrets:

```sh
printf 'WAYLAND_DISPLAY=%s\nXDG_RUNTIME_DIR=%s\n' "$WAYLAND_DISPLAY" "$XDG_RUNTIME_DIR"
test -x /usr/bin/cupertino-control
```

The in-tree Rust bridge runs as `/usr/bin/cupertino-control --mcp`. With no positional display it requires the host's `WAYLAND_DISPLAY`; alternatively its positional argument selects an explicit display. `XDG_RUNTIME_DIR` must belong to the same login user. Do not hardcode another user's runtime directory, persist a display from a previous login, run the host as root, or select the first socket found. A terminal launched inside a nested compositor targets that nested session.

## Registration in this checkout

The bridge is registered for this checkout only, in tracked project files; do not add a user-wide registration, which would also expose it in unrelated projects and duplicate the project entry.

- **Codex** reads `.codex/config.toml` once the checkout is a trusted project. It forwards `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` from the host and pre-approves this one server's tools; the compositor still asks the person natively the first time the app requests control. Check it with `codex mcp get cupertino` from the checkout.
- **Claude Code** reads `.mcp.json`. Approve the project server once when Claude Code asks (or through `/mcp`); check it with `claude mcp get cupertino` from the checkout. Launch Claude Code from the same desktop environment so its stdio child inherits the current session variables.

Each git worktree carries the same files; a host may ask to trust or approve each worktree path separately. Preserve unrelated settings when editing either file. Select the user's chosen model independently of the bridge.

## Claude Code

Use `/mcp` inside Claude Code to inspect/reconnect the server. Host tool names commonly have the `mcp__cupertino__` prefix. Do not use a fixed `--env WAYLAND_DISPLAY=...` value that will point at a stale or different display after a later login. The local MCP needs no API key or network service.

These instructions target **Claude Code**, including a local Claude Code session hosted in a desktop app. A local skill or project registration does not grant a cloud Claude/Cowork session access to this machine; it also needs an explicitly supported local MCP connection. Do not expose Cupertino's private Unix socket over the network to compensate.

## Install the skill

The skill is installed in the Cupertino checkout only. Its single source is `.agents/skills/cupertino-desktop/`, which Codex discovers in the repository; a relative `.claude/skills/cupertino-desktop` symlink exposes the same source to Claude Code. Do not copy it to user-wide skill directories (`~/.agents/skills/`, `~/.codex/skills/`, `~/.claude/skills/`): a second copy drifts from the checkout and shows up as a duplicate entry. Start the host in the checkout, or a worktree of it, to use the skill.

Invoke it as `$cupertino-desktop` in Codex or `/cupertino-desktop` in Claude Code; automatic discovery is also enabled. Refresh only the host's skill/MCP connection if discovery is cached. This is separate from restarting Cupertino.

## Diagnose connection failures

- Missing tools: check registration, host discovery/approvals, binary path and inherited session environment. A skill supplies instructions; it cannot manufacture tools the host does not expose.
- Native handshake/version failure: confirm matching installed controller and compositor support protocol v3. Remembered permissions need a compositor with control protocol 4; an older one asks the person on every request. Stage server updates for the next login; do not restart the current session.
- Failed native response after a controller update: read status to reconnect, then request control again. Never replay an uncertain mutation.
- Refused with `denied`: the person turned the app off. Do not retry; tell them they can turn it on in Settings › Privacy › Desktop Control or Screen Capture. Refused with `unidentified`: the bridge's process could not be named (it exited or was replaced); reconnect from a live host process.
- Asked again for an app the person allowed: the app is the window the bridge runs under, or the program that started it. A host started from another terminal or launcher is another app.
- Several hosts can work in one session at once: the controller serves up to eight connected bridges, each on a run of its own. A further bridge is told at once that the controller is full; its calls fail with that error until a place frees, and the next call connects again. Do not disconnect someone else's active control task to make room. A stuck bridge that holds a place ends its place when its host's MCP connection is refreshed. A controller from before this change served one bridge at a time and keeps doing so until the next planned session start; a compositor from before it lets the new controller serve one bridge only.
- Revoked epoch that remains stuck across status: an old already-running MCP process can retain the pre-fix lease behavior. Refresh only that host's MCP connection, then call status. Replacing the disk binary does not update an existing process. Do not kill all matching processes or restart the desktop.
- Repeated suspensions: phase 3 comes only from an authentication prompt or a protected session panel holding the keyboard, and ends by itself when it closes. Physical input never pauses a run on a current compositor; an older running one may still pause until the next planned login. The absence of a permanent panel is expected.

Authoritative host references: [Codex skills](https://learn.chatgpt.com/docs/build-skills), [Claude Code skills](https://code.claude.com/docs/en/skills), [Claude Code MCP](https://code.claude.com/docs/en/mcp). Live tool schemas and Cupertino's source define the actual desktop API.
