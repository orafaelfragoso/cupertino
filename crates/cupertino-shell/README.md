# cupertino-shell

The shell host (`docs/architecture/shell-host.md`, decisions D9 and D16 to D18 of
`docs/plans/layered-architecture.md`): one process, `cupertino-shell`, that runs the everyday parts of the shell, each
a component on a thread of its own with its own Wayland connection, roles and session bus connection. A component that
fails is started again alone; the host and the other components go on.

The crate is a `shell` crate: it starts no thread and opens no socket itself. `cupertino-kit-shell`'s `hosting` module
does that, and this crate keeps the policy and the component list.

## Running

The server starts the host; nobody else does. It hands the host a connection with the `host` role alone as
`WAYLAND_SOCKET` and sends it the list of components to run, from the `shell` section of `compositor.json`:

| Command | Started for |
|---|---|
| `cupertino-shell` | the shared host: every component of the list no separate host runs, and the hub of the host bus |
| `cupertino-shell --only NAME` | a separate host (D18): the component the server runs alone (`alone` of the `shell` section, or one lost in the shared host); it runs no other component whatever its list says, and exits when its list empties |

Exit statuses: 0 when the server said to exit (its quiet moment after a request to restart) or a separate host's list
became empty, 1 when the connection to the server failed or the render thread hung, 2 for an unknown argument or a name that is no component of the shell, 76
(`hosting::EXIT_VERSION`) when the server speaks another version of the host protocol. The server starts an exited
host again whatever its status (D16).

## What the host does

| Event | What follows |
|---|---|
| The server's list | a connection asked for every listed component the host was built with ([`COMPONENTS`]), `unavailable` (`absent`) for the others; a component that left the list is stopped and its object released; one listed again while it stops asks for its connection once it ended, so the last list wins |
| A connection | the component starts on it on a thread of its own (8 MiB stack, `hosting::Runtime::start`) |
| A component panics (its thread, a task or a helper thread), its loop fails or ends unasked | the runtime tears it down inside `catch_unwind` (a second panic leaks the state); the host shuts its sockets, sends `failed`, then `connect`, which the server answers once the restart time passed (D16: 0.25 s doubling to 8 s, five failures in 60 s stop it; the server keeps the times and counts) |
| `restart` of a component (`cupertino shell restart NAME`) | the component stops and starts again; no failure |
| The server gave a component up | the shared host publishes `ComponentStopped` on the host bus |
| `restart` of the host (the quiet moment it asked for) | every component stops, within `STOP_GRACE` (5 s), and the host exits |

A component's end wakes the host at once (`Host::wait`); nothing is polled.

The watchdog (D17, `watchdog`) reads each component's loop and the render thread every 0.5 s while one is out of its
wait or left it since the check before; while every loop waits the host sleeps until a loop leaves its wait, which
wakes it (`Host::watch`). It reports, after a start-up grace of 10 s:
`busy` once the loop has been out of its wait for 0.5 s, `unresponsive` at 2 s, `responsive` when it is back, and at
10 s `lost`: the host shuts the component's sockets down first, keeps its thread (a thread cannot be ended), and a
shared host asks the server to start it again (`request_restart`), while a separate host exits at once. A render
thread out of its wait for 10 s hangs every component: the host exits with status 1. The times are `hosting`'s
constants.

A component asked to stop that has not ended after `STOP_GRACE` is cut off (`Running::shut`).

## Trying a failure

A development (debug) build serves `org.cupertino.ShellComponentDebug1` on each component's bus connection:
`cupertino shell crash NAME [--at dispatch|paint|helper]` makes the component panic there, and the host contains it as
any other failure. A release build has no such interface.

## Modules

| Item | Holds |
|---|---|
| `run` | the host program: its connection, its components and what follows each event, each end and each watchdog pass |
| `watchdog` | `liveness` and `render_hung`: what a loop or the render thread out of its wait for a time stands for |
| `COMPONENTS` | the components this build runs: `osd` ([`OSD`], `cupertino-osd`), `notifications` ([`NOTIFICATIONS`], `cupertino-notifications`), `controlcenter` ([`CONTROLCENTER`], `cupertino-controlcenter`), `menubar` ([`MENUBAR`], `cupertino-menubar`), `launcher` ([`LAUNCHER`], `cupertino-launcher`), `agent` ([`AGENT`], `cupertino-shell-agent`), `desktop` ([`DESKTOP`], `cupertino-shell-desktop`), `overview` ([`OVERVIEW`], `cupertino-shell-overview`), `dock` ([`DOCK`], `cupertino-dock`), `windowmanager` ([`WINDOWMANAGER`], `cupertino-windowmanager`) and `dialogs` ([`DIALOGS`], `cupertino-shell-dialogs`) |

## Tests

The program tests run this host (`run`) against the real server, headless, in
`crates/cupertino-session-tests/tests/shell.rs`: a crate of the `testing` layer may depend on the server, this one
may not.
