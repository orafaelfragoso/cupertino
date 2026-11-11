---
name: cupertino-live-dev
description: Try a change to a Cupertino shell component (dock, menu bar, launcher, notifications, control panel, OSD, window manager) inside the running Cupertino session without root, through the development folder and `cupertino shell restart`, verify it with desktop control, keep a build that passes and roll back one that fails. Use when developing from inside the installed desktop.
---

# Developing Cupertino from inside its session

The installed server may start some shell components from a development folder instead of `/usr/bin`. Root enables the
folder once; after that you build a component with Cargo, copy it into the folder, restart that one component
and look at the result, all as the session's own user. A build that passes its check stays running until a package
contains the change. The full reference is `docs/src/building.md`, section
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
Files and Settings are regular applications: build them with Cargo and run that build directly; nothing is
swapped. [components.md](guides/components.md) lists what a restart of each component loses.

## 2. Build it

A build that passes stays running (step 6), so build it as the package does: the release profile, scoped to the binary,
production features, and the installed theme directory (without it the binary reads the theme from the checkout,
which may be gone later).

```bash
export PATH=$HOME/.cargo/bin:$PATH
CUPERTINO_THEME_DIR=/usr/share/cupertino/theme \
  cargo build --release --locked -p cupertino-dock --bin cupertino-dock --features app
```

The binary is `target/release/cupertino-dock`. Validate the change with its scoped tests separately. The build must
speak the running server's protocols: if your change touches a protocol or the server, the component waits for a new
server at the next session start.

## 3. Copy it into the folder

Stage the copy and rename it into place; never write into a file a running process may be executing. Record the
source commit (and whether the tree had uncommitted changes) and the file's hash.

```bash
dev=$(cupertino --format json shell list | python3 -c 'import json,sys; print([p for p in json.load(sys.stdin)["data"]["processes"] if p["name"]=="dock"][0]["dev_program"])')
install -m 0755 target/release/cupertino-dock "$(dirname "$dev")/.cupertino-dock.new"
mv -f "$(dirname "$dev")/.cupertino-dock.new" "$dev"
git rev-parse HEAD; git status --short
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
process did not start again; roll back (step 6) at once.

## 5. Verify in the session

Use the `cupertino-desktop` skill: `status`, then `request_control` with only the capabilities you need, then observe,
act and capture to confirm the changed interaction and that the rest of the desktop still works. Desktop control is
permission based: the person decides once per app in Cupertino's native panel, and an app they allowed is not asked
again. Never approve on their behalf. Compare with what the installed build did; `cupertino shell list` confirms the
pid and program you are looking at.

## 6. Keep it or roll back

- **It passes its live check:** leave it running from the folder. Report the component, the source commit (and any
  uncommitted changes), the file's hash and that it runs from the development folder.
- **It fails:** roll back at once with `cupertino shell use-installed dock`. It removes the folder's file and restarts
  the component on its installed program.
- **An installed package contains the change:** roll back the same way; from then on the package's program runs.
  A build left behind keeps replacing newer packages: `cupertino shell list` marks it
  `development folder (older than installed)` (`dev_program_older_than_installed` in JSON) once a package built
  after it is installed, and `cupertino diagnose --area shell` warns with the command to run.

After a package install, a change that lies only in a supervised process (any name `cupertino shell list` shows,
including `portal`, `polkit-agent` and `desktop-control`, which never start from the folder) takes effect with
`cupertino shell restart NAME`; no new login. Defer it while the process is in use (see
[components.md](guides/components.md)). Only a change to the server (or what it links) or to the session manager
needs a new login, which the person decides on.
