---
name: cupertino-live-dev
description: Try a change to a Cupertino shell component (dock, menu bar, launcher, notifications, control panel, OSD, window manager) inside the running Cupertino session without root, through the development folder and `cupertino shell restart`, then verify it with desktop control and roll back. Use when developing from inside the installed desktop.
---

# Developing Cupertino from inside its session

The installed server may start some shell components from a development folder instead of `/usr/bin`. Root enables the
folder once; after that you build a component in your worktree, copy it into the folder, restart that one component
and look at the result, all as the session's own user. The full reference is `docs/src/building.md`, section
"Installed-session iteration".

Never restart the server or the session, never run `sudo` yourself, never edit `/etc`, and never select a process by
name with `kill`/`pkill`: nested sessions run the same programs. The `cupertino shell` commands go to the server of
this session only.

## 0. Check that the loop is available

```bash
cupertino shell list
cupertino --format json shell list     # dev_program: where the folder's file for each process goes
```

| What you see | Meaning |
| --- | --- |
| a table with `NAME`, `STATE`, `PID`, `SOURCE`, `PROGRAM` | the server supports the commands |
| `dev_program` set for the component | the folder is enabled for it: continue |
| `dev_program` null for an eligible component | the folder is not enabled for it: ask the person to run `sudo cupertino dev-folder enable` (or `--components NAME,...`) once, then check again; a server that supports the folder reads the change at once |
| exit 8, `not_supported` | the running server is older than these commands: this loop is unavailable until the person installs a newer server and starts a new session; test nested or headless meanwhile |
| exit 3, `no_session_bus` or `no_server` | you are not in the desktop session's environment (no `DBUS_SESSION_BUS_ADDRESS`, or no Cupertino server on it) |
| exit 4, `refused` | the server refuses this process: it only answers an unsandboxed process of the session's own user |

## 1. Pick the component

Only processes whose roles are all among `session`, `menu_bar`, `dock` and `window_manager` may start from the folder.
In the packaged configuration:

| Name (`shell list`) | Program | Crate |
| --- | --- | --- |
| `dock` | `cupertino-dock` | `crates/cupertino-dock` |
| `menubar` | `cupertino-menubar` | `crates/cupertino-menubar` |
| `launcher` | `cupertino-launcher` | `crates/cupertino-launcher` |
| `notifications` | `cupertino-notifications` | `crates/cupertino-notifications` |
| `controlcenter` | `cupertino-controlcenter` | `crates/cupertino-controlcenter` |
| `osd` | `cupertino-osd` | `crates/cupertino-osd` |
| `windowmanager` | `cupertino-windowmanager` | `crates/cupertino-windowmanager` |

Not eligible, whatever the configuration says: `login` (lock screen, session agent), the portal, the authentication
agent, desktop control, data-control clients, the session manager, the system daemons and the server itself. For those,
and for any change that also needs a new server, follow [non-eligible components](guides/components.md#not-eligible).
Files and Settings are regular applications: build them in the worktree and run that build directly; nothing is
swapped. [components.md](guides/components.md) lists what a restart of each component loses.

## 2. Build it in your worktree

Follow the repository's build rules (managed wrapper, scoped to the binary, production features):

```bash
export PATH=$HOME/.cargo/bin:$PATH
python3 ci/worktree-build.py run --profile dev -- cargo build --locked -p cupertino-dock --bin cupertino-dock --features app
```

Validate the change with its scoped tests separately. The build must speak the running server's protocols: if your
change touches a protocol or the server, the component waits for a new server at the next session start.

## 3. Copy it into the folder

Stage the copy and rename it into place; never write into a file a running process may be executing. Record the hash.

```bash
dev=$(cupertino --format json shell list | python3 -c 'import json,sys; print([p for p in json.load(sys.stdin)["data"]["processes"] if p["name"]=="dock"][0]["dev_program"])')
install -m 0755 target/debug/cupertino-dock "$(dirname "$dev")/.cupertino-dock.new"
mv -f "$(dirname "$dev")/.cupertino-dock.new" "$dev"
sha256sum "$dev"
```

The server only starts the file when it is a regular file (no link) of the user or root, not writable by group or
others, executable, in a folder with the same rules. Otherwise it starts the installed program and logs why.

## 4. Restart that one component

Defer this while the component has active work (an open panel the person is using, a screen share).

```bash
cupertino shell restart dock
```

It returns once the new process runs: `pid`, start time, `program` and `SOURCE` `development folder`. If the source
says `installed`, the file failed a check: fix its mode or owner and restart again. Exit 6 `not_started` means the
process did not start again; go back (step 6) at once.

## 5. Verify in the session

Use the `cupertino-desktop` skill: `status`, then `request_control` with only the capabilities you need, then observe,
act and capture to confirm the changed interaction and that the rest of the desktop still works. Desktop control is
permission based: the person decides once per app in Cupertino's native panel, and an app they allowed is not asked
again. Never approve on their behalf. Compare with what the installed build did; `cupertino shell list` confirms the
pid and program you are looking at.

## 6. Go back

```bash
cupertino shell use-installed dock
```

It removes the folder's file and restarts the component on its installed program. Do this after the check, after a
failed candidate, and before you hand off, unless the person asked to keep the build running. Report the source
revision, the hash and the state you left the component in.
