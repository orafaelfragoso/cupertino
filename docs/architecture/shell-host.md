# The shell host: specification

How the server runs the shell's components in a host process, how changes of several components start on one frame,
and what `cupertino-kit-shell` gives a component. The decisions it rests on (D9 to D21, D26) and the tasks that build
it are in `docs/plans/layered-architecture.md`. The wire is three protocol files in
`crates/cupertino-protocols/protocols/`:

| File | Interfaces | For |
|---|---|---|
| `cupertino-host-v1.xml` | `cupertino_host_v1`, `cupertino_host_component_v1`, `cupertino_component_v1` | hosts, component connections, identity, liveness, holds |
| `cupertino-transaction-v1.xml` | `cupertino_transaction_manager_v1`, `cupertino_transaction_v1`, `cupertino_hole_v1` | transactions, gesture binding, holes |
| `cupertino-overview-v1.xml` | `cupertino_overview_manager_v1`, `cupertino_overview_v1` | the overview component's windows, workspaces and commands |

The XML states each message exactly. This file states what the parts do together, the configuration, and the Rust
API. A change to the behaviour changes this file first, in its own reviewed change.

Every change of a message raises the version of its interface, and a message added after version 1 carries the
version that added it (`since`). A host speaks the versions it was built with and no other
([Versions](#versions)).

## Contents

1. [Processes and connections](#processes-and-connections)
2. [The component list](#the-component-list)
3. [The host protocol](#the-host-protocol)
4. [Transactions](#transactions)
5. [`cupertino-kit-shell`](#cupertino-kit-shell)
6. [Edits of the existing protocol files](#edits-of-the-existing-protocol-files)
7. [Decisions for approval](#decisions-for-approval)

## Processes and connections

| Process | Started as | Connection | Holds |
|---|---|---|---|
| Shared host | `cupertino-shell` | one, with the `host` role alone | the components no separate host runs; the host bus |
| Separate host | `cupertino-shell --only NAME` | one, with the `host` role alone | one component (D18) |
| Component | a thread of a host | one of its own, handed over by the server | the component's roles; a session bus connection of its own |
| `cupertino-login`, `cupertino-prompts`, `cupertino-screenshot` | programs of their own (D10) | one each, as `WAYLAND_SOCKET` | their roles |

- A role is bound to a connection and ends with it. The host's connection sees none of the components' roles; a
  component's connection sees none of another component's.
- A component holds its roles in one host at a time: the server hands out a component's connection only after it
  closed the one before.
- A component connection is valid while the host process it was handed to lives.

### Roles

| Role | Lets a connection | Holders |
|---|---|---|
| `host` | bind `cupertino_host_v1` | the host connections |
| `session` | follow appearance, display options and lock state; `activate_app` (`cupertino_session_v1`) | every component, `cupertino-login`, `cupertino-prompts`, `cupertino-screenshot` |
| `menu_bar` | be the menu bar of an output (`cupertino_menu_bar_v1`) | `menubar` |
| `dock` | be the dock of an output, end applications, draw the application switcher (`cupertino_dock_v1`) | `dock` |
| `window_manager` | arrange the windows of an output (`cupertino_window_manager_v1`); claim windows in holes | `windowmanager` |
| `overview` | follow an output's windows and workspaces (`cupertino_overview_v1`); claim windows in holes; show workspaces in holes; bind gestures to transactions | `overview` |
| `keyboard_panels` | draw the input-source switcher and the accent picker (`cupertino_keyboard_panels_v1`); pick a candidate of the open picker | `osd` |
| `windows` | show windows and minimized windows' pictures in holes (`cupertino_hole_v1`) | `dock`, `overview`, `windowmanager` |
| `session_agent` | receive the session's requests for its panels (`log_out_requested`); be the session agent for `org.cupertino.Session1` | `agent` |
| `session_lock` | take the session lock (`ext_session_lock_v1`); receive `lock_requested` | `cupertino-login` |
| `prompt` | draw prompt surfaces: above every other surface, left out of captures, no application holding the pointer or the shortcuts meanwhile | `cupertino-prompts` |
| `consent` | answer desktop-control consent (`cupertino_control_authority_v1`) | `cupertino-prompts` |
| `capture` | capture outputs and windows; follow the windows on screen (`cupertino_screenshot_v1`) | `cupertino-screenshot`, `cupertino-portal` |
| `global_shortcuts` | bind the portal's shortcuts | `cupertino-portal` |
| `desktop_control` | discover desktop-control sessions | `cupertino-control` |
| `clipboard` | data control without an entry in `data-control.conf` | none shipped |

Every component connection also sees `cupertino_component_v1` and `cupertino_transaction_manager_v1`. Layer surfaces
(`zwlr_layer_shell_v1`) and the layers protocol (`cupertino_layers_v1`) need no role.

## The component list

### Components

The table is the server's, a constant of `cupertino-server`: no configuration file names a component's command, its
roles or its bus names. The host holds the same names, each bound to its component crate.

| Component | Crate | Shows | Roles | Well-known bus names | Persistent surfaces |
|---|---|---|---|---|---|
| `menubar` | `cupertino-menubar` | the menu bar and its menus | `session`, `menu_bar` | `com.canonical.AppMenu.Registrar`, `org.kde.StatusNotifierWatcher` | the bar of each output |
| `dock` | `cupertino-dock` | the dock, the application switcher, the force-quit dialog | `session`, `dock`, `windows` | none | the dock of each output |
| `desktop` | `cupertino-shell-desktop` | the desktop items | `session` | none | the desktop of each output |
| `windowmanager` | `cupertino-windowmanager` | the stage strip; stage and tiling policy | `session`, `window_manager`, `windows` | none | the strip of each output |
| `overview` | `cupertino-shell-overview` | the overview | `session`, `overview`, `windows` | none | none |
| `launcher` | `cupertino-launcher` | the launcher | `session` | `org.cupertino.Launcher1` | none |
| `notifications` | `cupertino-notifications` | banners, the notification list | `session` | `org.freedesktop.Notifications`, `org.cupertino.NotificationCenter1` | none |
| `controlcenter` | `cupertino-controlcenter` | the control panel | `session` | `org.cupertino.ControlPanel1` | none |
| `osd` | `cupertino-osd` | on-screen displays, the input-source switcher, the accent picker | `session`, `keyboard_panels` | `org.cupertino.Osd1` | none |
| `agent` | `cupertino-shell-agent` | application restore, login items, the log out, restart and shut down panels and the panel that interrupts them, the power button and low battery panels | `session`, `session_agent` | `org.cupertino.SessionAgent1` | none |
| `dialogs` | `cupertino-shell-dialogs` | the portal's file and application choosers | `session` | `org.cupertino.Dialogs1` | none |

- The server sends a host's list in the table's order, and the host starts its components in that order.
- A component owns its well-known names on its own bus connection and announces that connection's unique name to the
  server ([Identity](#identity)).
- `cupertino shell list` names components by the first column; `cupertino shell restart NAME` takes the same names
  and the names of the programs under [Programs the server starts itself](#programs-the-server-starts-itself).

### The `shell` section of `compositor.json`

```json
{
  "shell": {
    "components": ["menubar", "dock", "desktop", "windowmanager", "overview", "launcher",
                   "notifications", "controlcenter", "osd", "agent", "dialogs"],
    "off": [],
    "alone": []
  }
}
```

| Key | Value | System file | A file of the person |
|---|---|---|---|
| `components` | names of the table: the components of the session; every component of the table when the key is absent | sets the list | refused, with a logged problem |
| `off` | names of the table: components that do not run | applies | applies |
| `alone` | names of the table: components that each run in a separate host (D18) | applies; the packaged file leaves it empty | applies |
| `processes` | ignored: a list of processes for the server to start, which a file edited before every component ran in a host may still hold | ignored, with a logged warning | ignored, with a logged warning |

- The components that run are `components` minus `off`. A name that is not in the table is a logged problem and is
  left out; the other names apply.
- `off` and `alone` of the person's file take the place of the system file's, key by key. `components`, `off` and
  `alone` hold names of the table and nothing else: no command, no role, no bus name.
- Which file is the system file comes from the layer, as for every privileged key: `/etc/cupertino/compositor.json`
  and `/etc/cupertino/greeter.json`, named exactly.
- The section is live. When a file changes, the server works out the components of each host again and sends each
  host its list. A component turned off is stopped and its holds end; one turned on starts; one that enters or leaves
  `alone` stops in the host it ran in and starts in the other, its persistent surfaces held in between.
- A system file without a `shell` section runs every component of the table.
- The login session runs no component, whatever its file says ([`greeter.json`](#greeterjson)).

Every component runs in a host, and nothing else runs for one:

- A `shell` that is an array, or a `processes` list in the object, names processes for the server to start, as an
  edited `/etc/cupertino/compositor.json` kept through a package update may. Nothing of it is read or runs: the server
  logs one warning each time it reads the file, and the rest of the section applies. An array is a section with no
  key, so every component of the table runs.
- A component that no host runs does not run: while the host program is not installed (one log line at start), after
  it was refused or could not be spawned (logged, and not tried again until `cupertino shell restart host` or a
  change of the section), and when its host answers `unavailable` for it (logged; the component stays stopped in
  that host).
- The programs of [Programs the server starts itself](#programs-the-server-starts-itself) are no components: no file
  names them, and the server supervises them whatever the section says.

### Development builds

`dev_folder` keeps its form, `{"path": "/absolute/dir", "components": ["dock", "menubar"]}`, and its rule that only
the system file sets it.

- `components` names components of the table. A named component runs in a separate host that the server starts from
  `<path>/cupertino-shell --only NAME`, when that file passes the folder's checks at that start: a regular file, not a
  link, in a directory, both owned by the user or root and writable by no group or other user, executable, and not
  older than the installed `cupertino-shell`.
- When the file is absent or refused, the component runs as the rest of the configuration says, in the installed
  host; a refusal is logged with its reason.
- A host from the folder that was built with another version of an interface it uses does not run its component
  ([Versions](#versions)): the server logs the versions and runs the component in the installed host, until
  `cupertino shell restart NAME` tries the folder again.
- Only a component whose roles are all in the allow-list may be named: `session`, `menu_bar`, `dock`,
  `window_manager`, `overview`, `windows`. A name with another role (`agent`) or outside the table is ignored with a
  warning.
- The shared host and the programs of the next section start from their installed paths, whatever the folder holds.
- `cupertino shell restart NAME` ends a separate host and starts it from the file that is there then, so a build
  placed in the folder runs after one command.
- With `--dev-shell` (checkouts and nested runs) the host and the programs are found by name on `PATH`, as the
  screenshot program is, and any configuration file may set `dev_folder`.

### Programs the server starts itself

No configuration file names, changes or removes these. Each program is checked before every start as the built-in
programs are (installed under an install root, owned by root, not writable by others).

| Name | Program | Session | Runs | Roles |
|---|---|---|---|---|
| `host` | `/usr/bin/cupertino-shell` | user | while a component runs | `host` |
| `host-NAME` | `/usr/bin/cupertino-shell --only NAME`, or the development folder's | user | while component NAME runs alone | `host` |
| `login` | `/usr/bin/cupertino-login` | user | always: the lock screen | `session`, `session_lock` |
| `login` | `/usr/bin/cupertino-login --greeter` | login | always: the login screen | `session` |
| `prompts` | `/usr/bin/cupertino-prompts` | user, on the hardware | always | `session`, `prompt`, `consent` |
| `portal` | `/usr/bin/cupertino-portal` | user, on the hardware | always | `capture`, `global_shortcuts` |
| `desktop-control` | `/usr/bin/cupertino-control --serve` | user, on the hardware | always | `desktop_control` |
| `screenshot` | the `screenshot` key's command, `/usr/bin/cupertino-screenshot` by default | user | on demand | `session`, `capture` |

- A host is restarted when it exits, whatever its status: never sooner than 1 s after its last start, a time that
  doubles with each exit up to 30 s and starts over after a run of 60 s. The server never gives a host up. An exit
  the server expects (after `restart`, after `lost` from a separate host, an empty list of a separate host) is not
  counted.
- A host that the server expects to exit and that has not exited after 2 s is killed through its pidfd.
- The other programs keep the restart policy of the supervised processes: restarted after a failure with the same
  throttle, left stopped after a successful exit.
- The server holds a pidfd of every program of the table from its start. A bus caller is one of them when the bus
  daemon gives the caller's process as that one and it has not exited ([Identity](#identity)).
- `cupertino-audiod` is started by the bus at first use, never by the server.
- W5.5 makes `login` a row of this table: the server starts `cupertino-login` itself in both sessions, and no file
  lists it.

### `greeter.json`

The login session's server reads `/etc/cupertino/greeter.json`, which the login manager names in
`$CUPERTINO_SERVER_CONFIG`. Its `shell` section runs no component:

```json
{
  "shell": { "components": [] }
}
```

- No host starts. The server starts `cupertino-login --greeter` with the `session` role, as the table above says, and
  none of the user session's other programs.
- The login session's server runs no component whatever the file holds: it reads `components` as empty, also when the
  key is absent or the section is an array, and logs a `components` that names one.
- The login screen draws layer surfaces through `cupertino-kit-shell` and has no lock surface: nothing is locked.
- Every other key of `compositor.json` is valid in the file. A `dev_folder` key names no component that runs, so it
  has no effect.
- W5.5 writes the packaged file in this form.

## The host protocol

### Start

1. The server starts the shared host when the first component is due, on a connection that holds the `host` role.
2. The host binds `cupertino_host_v1` and gets `kind`, then its list: a `component` event for each name, then `done`.
3. For each name the host calls `get_component` and `connect`. The server answers `connection` with a descriptor:
   the client end of a connection that holds the component's roles. The host starts the component's thread on it.
4. The component binds `cupertino_component_v1`, gets its name, its instance number and its holds, announces its bus
   connection, persists its persistent surfaces before their first buffer, and sends `ready` once it shows what it
   shows at start.
5. A list is sent again in whole whenever it changes. The host stops a component that left the list, then destroys
   its object; it asks for the connection of one that joined.

- A host that was built without a listed component answers `unavailable` with the reason `absent`: the server logs
  it and leaves the component stopped in that host.
- `get_component` with a name outside the table is a protocol error, so a host holds at most one object for each row
  of the table. An object for a name of the table that is not in the host's list is inert: its `connect` waits. It
  becomes live when a later list holds the name, and the waiting `connect` is answered then.
- The instance number counts the connections the server handed out for the component in this session, from 1. It
  tells one instance's transactions from another's ([Parts](#parts)).

### Versions

A host is built with one version of each interface of `cupertino-protocols` it uses, and the server offers one
version of each. `cupertino-kit-shell` compares the two before it binds anything: on the host's connection for
`cupertino_host_v1`, on a component's connection for every global the component binds.

| Where the versions differ | The host | The server |
|---|---|---|
| On the host's connection | exits with status 76, bound to nothing | logs both versions |
| On a component's connection | starts no component on it and sends `unavailable` with the reason `version` | logs it and leaves the component stopped in that host |

- A host of the development folder that ends either way gives its component back to the installed host: the server
  runs the component as the configuration says without the folder, until `cupertino shell restart NAME`.
- The installed host and the server come from one package, so their versions differ only between a package update
  and the next login. The server restarts such a host under the host policy and logs the versions each time.

### Identity

A component is known on the session bus by the unique name of its own bus connection, which it proves to the server.

1. The component opens its connection to the session bus and sends `announce_bus_name` with the connection's unique
   name on its Wayland connection.
2. The server answers `bus_nonce`: 128 random bits drawn for that announcement, which travel on that component's
   Wayland connection and nowhere else.
3. The component calls `org.cupertino.Session1.AnnounceComponent(nonce)` from the announced bus connection.
4. The server accepts when the call's sender is the announced name, the nonce is that announcement's, unused and at
   most 5 s old, the bus daemon gives the sender's process as the host process the component's connection was handed
   to, and the name is accepted for no other component. A nonce is good for one call, whatever its answer.
5. The answer is `bus_name`, `accepted` or `refused`, and the method returns or fails with it. No call within 5 s is
   a refusal.

- A unique name is one component's. An announcement of a name that is accepted for another component is refused,
  and the other component keeps it. Sharing a process proves nothing: a component of the shared host that announces
  another's name cannot make the call from that name.
- Announcing again takes the component's accepted name away until the fresh announcement is accepted.
- Before the acceptance, and after a refusal, the server takes a call from the component's bus connection as an
  unknown caller's.
- `cupertino-kit-shell` makes the announcement and the call when the component opens its session bus connection
  (`bus::session`) and hands the connection to the component after the answer, so no call of the component precedes
  its identity. A component never sees a nonce.

Who a bus caller is:

| Caller | Known by | To |
|---|---|---|
| A component | its accepted unique name | the server: a call from the accepted name of `agent` is the session agent's, and a call from another connection of the same process is not |
| A component | `ComponentOf(name)` of `org.cupertino.Session1`, which answers from the accepted names: the component's name, or nothing | another component or a service, through `cupertino-peer` |
| A shell host, whichever component | `ShellHosts` of `org.cupertino.Session1`: the hosts' process ids and start times | a service that asks only whether the shell calls |
| The server | the owner of `org.cupertino.Session1`, a name the server owns before it starts a program and gives up to nobody | a component: the check that takes the place of "my parent process" |
| `cupertino-login`, `cupertino-prompts`, `cupertino-screenshot`, `cupertino-portal`, `cupertino-control` | the process the server started, by the pidfd it holds | the server, as `caller::authorize` of `cupertino-server` judges its agents and `StartScreenshot` the portal |
| A program the server starts itself | `ProgramOf(name)` of `org.cupertino.Session1`, which the server answers from the pidfds it holds: the program's name in [Programs the server starts itself](#programs-the-server-starts-itself) (`portal`, `login`, `desktop-control`, `screenshot`, ...), or nothing | a service, through `cupertino-peer`: the screenshot program takes the portal's calls alone |

- `cupertino-peer` resolves a caller in this order: the owner of `org.cupertino.Session1` is the server; a caller
  whose process is a host (`ShellHosts`) is the component `ComponentOf` names for its unique name, or no component;
  any other caller is the program `ProgramOf` names for its unique name, or no part of the shell. A component in a
  separate host resolves the same way.
- `ComponentOf` and `ProgramOf` answer any caller: they tell which unique name is which component or program, and
  nothing else. `ProgramOf` names a program only while its process runs, and never a host or a component.
- The programs the server starts itself bind no `cupertino_component_v1` and announce nothing: each is a process of
  its own, so its process is its identity.
- No check compares process relations. Component sockets are close-on-exec: a program a component starts inherits
  neither its Wayland connection nor its bus connection.

### Failures and restarts

A component fails when it panics (its own thread, one of its tasks or a helper thread), when its loop ends with an
error, when its loop returns without being asked, or when it is lost to a hang ([Hangs](#hangs)). For the first
three the host tears it down inside `catch_unwind`, shuts its sockets, and sends `failed` and then `connect`.

| Rule (D16) | Value | Kept by |
|---|---|---|
| First restart after a failure | 0.25 s | `cupertino-server` |
| Each further failure | the time doubles, up to 8 s | `cupertino-server` |
| The time starts over | after the component ran 60 s | `cupertino-server` |
| The component stops | five failures within 60 s | `cupertino-server` |
| A loss in that count | the run of an instance that is lost within 60 s of its start is left out of the 60 s | `cupertino-server` |
| A stopped component starts again | `cupertino shell restart NAME`, or the next login | `cupertino-server` |

- The server keeps the counts, so they outlive the host. It answers `connect` with `connection` when the time has
  passed, or with `stopped` when it gave up.
- A loss takes 10 s or more to be declared, so five of them never fall within 60 s of the clock. Leaving the lost
  run out of the 60 s stops a component that hangs at every start after its fifth loss.
- When the server gives a component up, in whichever host it ran, it tells the shared host (`component_stopped`).
  The shared host publishes `ComponentStopped` on the host bus, and the notifications component tells the person.
- `cupertino shell restart NAME` of a running component reaches its host as `restart` on the component's object: the
  host stops the component and sends `connect`. It is not a failure. The command is answered when the component
  sends `ready`, and after 10 s without it with an error that names the component as started and not ready.

### Hangs

A component's loop records when it leaves its wait and clears the record when it returns. The host's watchdog reads
the records every 0.5 s, after a start-up grace of 10 s for each component.

| The loop has been out of its wait | The host sends | The server |
|---|---|---|
| for 0.5 s: at two checks in a row, with no return between them | `busy` | stops waiting for the component: a transaction that expects it starts without it |
| for 2 s | `unresponsive` | releases its grabs, dismisses its transient surfaces, returns the focus to the window that had it, sends it no input |
| and returns | `responsive` | sends it input again |
| for 10 s | `lost` | counts a failure, holds its persistent surfaces, takes it out of the host's list, starts it alone in a separate host |

- A loop caught in the middle of a turn at one check is not busy: a transaction's deadline bounds the wait for it
  anyway, and a working component is never reported.
- Before `lost` the host shuts the component's sockets down with `shutdown(2)` on descriptors it keeps for that
  purpose. It closes the component's descriptors only when the thread has ended, so none is reused under it.
- With the sockets, the host cuts the component off inside the process: it leaves the host bus (nothing it
  publishes is delivered, its retained messages go, its queue is dropped), and the results of its tasks and helper
  threads are dropped unread. A lost thread that wakes reaches nothing.
- After `lost` a shared host sends `request_restart`. The server answers `restart` once the seat has had no input for
  2 s and no component of the host holds a grab or shows a transient surface, and at the latest 60 s after the
  request; the host stops its components and exits, and the server starts it again at once. That exit is not counted
  as a crash.
- After `lost` a separate host exits at once, without waiting for the thread. The server kills one that has not
  exited after 2 s, and starts a separate host for the component again when the failure's restart time has passed.
- The lost component stays in a separate host for the rest of the session, or until `cupertino shell restart NAME`
  puts it back in the shared host.
- The render thread reports to the same watchdog. A render thread out of its wait for 10 s is a hang of the whole
  process: the host exits with failure and the server restarts it.

### Holds

A surface is transient unless the component persists it (`persist`, with a key that is the same for every instance
of the component and names the surface on its output).

| When | Persistent surfaces | Transient surfaces |
|---|---|---|
| The component is reported unresponsive | keep their place and their live picture, without input | dismissed at once; focus returns to the window that had it |
| Its connection ends (failure, restart, lost, host exit) | held: the last picture and the reserved space stay, every animation at its end, without input; their holes and claims stay | dismissed at once; their holes go and each claimed window returns along the server's return motion to where it shows without the claim |
| The component is declared stopped (`stopped`, turned off) | the holds end: the picture goes and the space is given back; held claims and placements end, and each window returns to its frame along the server's return motion | |

What is held with a persistent surface:

- **Its holes and claims.** A hole of a held surface draws its source as before: a window live, a minimized
  window's picture, a workspace live. A claim of a held hole stays in force, so the window stays in the hole and
  takes no input there. The stage strip and the dock's minimized windows look the same through a restart.
- **The component's placements.** The placements a window manager committed
  (`cupertino_window_manager_v1.set_placement`) are held while a hold of that component lasts on the output: a
  window placed `hidden` stays hidden. No window comes back on screen because its window manager restarts.
- A hole whose window closes while it is held fades the window out as a live hole does, then draws nothing.

What a restarted component takes over:

- A surface it persists with the key of a hold on the same output takes the hold over. With that surface's first
  buffer, in the same frame, the held picture goes and the surface shows in its place; the reserved space passes to
  it with no change of the usable area in between, so no window moves and nothing flashes.
- In that frame the held surface's holes and claims go and the holes and claims of that commit apply, and the held
  placements of the output give way to the placements the instance committed before it: until that frame the
  server keeps the instance's placement commits for the output and does not apply them. An instance that has
  committed no placement by then leaves the held ones in force until its first `commit`.
- When the surface that takes over has another exclusive zone than the held one, the usable area changes once, in
  that frame, as for any change of an exclusive zone.
- Nothing else. Role objects are made afresh and the server sends each feed from its start; what the instance before
  knew is gone unless the component stored it (D21). The component hears which keys are held (`hold`), so it knows it
  takes over and shows its surfaces without their opening motion.
- `ready` ends every hold no surface took over, with its holes, claims and placements. Without it the server ends
  them 10 s after it handed the connection out.

### Separate hosts

- A separate host runs one component. The server starts one for a component in `alone`, for a component of the
  development folder, and for a component that was lost.
- After three crashes of the shared host within 60 s, the server starts every component in a separate host for the
  rest of the session, so an abort, a stack overflow, an exhausted memory or a driver fault costs one component.
- While any component runs there is one shared host, also when its list is empty: it is the hub of the host bus.
- A separate host whose list becomes empty stops its component and exits successfully; the server does not start it
  again.
- A separate host never keeps a thread it cannot end: after `lost` it exits at once, and the server kills it when it
  does not ([Hangs](#hangs)).

### The host bus and its bridge

Components talk to each other through the host bus only (D26): typed messages of `cupertino-kit-shell`, published
and heard, never awaited.

- Inside a host the bus is in memory: a publication goes to the queue of every component that follows its topic and
  wakes that component's loop.
- A separate host reaches the bus through a bridge to the shared host: a Unix stream socket pair the server makes.
  The server sends one end to the shared host (`bus_peer`, with the component's name) and the other to the separate
  host (`bus`). No path, no token: the descriptor is the peer, and no other process can connect.
- The server sends a fresh pair, one event to each side, when either host starts again and when either side says
  its end closed or failed (`bus_dropped`), no sooner than 1 s after the pair before. Each side closes the end it
  held. `bus_peer_gone` tells the shared host that a separate host ended.
- Messages a separate host publishes while it has no bridge wait in it, the newest 256 kept.

| Bridge rule | Value |
|---|---|
| Frame | a `u32` little-endian length, then JSON with a `type` tag; unknown tags and fields are errors |
| Largest frame | 64 KiB |
| First frame from the separate host | `{"type": "hello", "version": 1, "component": NAME}` |
| Answer | `{"type": "welcome", "version": 1}`, then every retained message; or the bridge closes |
| After `welcome`, from the separate host | every retained message of its component, again |
| Afterwards, both ways | `{"type": "message", "from": SENDER, "message": MESSAGE}`; `SENDER` is a component's name or `host` |

What the shared host checks on a bridge:

- `hello.component` is the component the server named for that descriptor, and `version` is its own.
- `from` of every message is that component.
- No message of the `Host` topic comes over a bridge: the shared host alone publishes on it, as `host`.

A bridge that fails a check is closed and logged, and the shared host reports it with `bus_dropped`. The shared host
forwards a message from a bridge to its own components and to the other bridges, and never back to its sender. A
retained message stays with the bus until its publisher stops ([The host bus](#the-host-bus)).

## Transactions

### Parts

A transaction has a creator and any number of other participants, each a component connection with a
`cupertino_transaction_v1` object of its own. It is named by its creator's component, that component's instance
number and a serial the instance chose, so the creator publishes the name on the host bus in the same turn that
creates it, and another component joins by name. The instance number keeps the names of a restarted component apart
from the names of the instance before, which the server remembers for 10 s.

A participant's part is made of two things:

| Part | How it joins | What it carries |
|---|---|---|
| Staged surfaces | `stage(surface)`: the commits of the surface and of its layer surfaces are kept from then on | the buffer and damage; the values, animations and looks of its layers and its presentation (`cupertino_layers_v1`); the content of its layers; the state of its holes |
| Window changes | the role requests between `begin_window_changes` and `end_window_changes` | the server's own changes of windows, with the animations it runs for them |

- Motion is described once. An animation in a kept commit is a target with the numbers of a timed curve, a spring or
  samples; the toolkit takes the numbers from the catalogue's `Timing` (D7) and the server evaluates them with
  `cupertino-animation`. Nothing is sent per frame.
- Kept commits of one surface add up in order, so the surface's state never goes backwards.
- A layer's content applies on the layer surface's own commit, without waiting for its parent (`cupertino_layers_v1`).
  While the parent is staged, the commits of its layer surfaces are kept with the parent's and applied with the
  part, also for a layer made while the parent is staged. So a title or a label drawn in a layer shows on the start
  frame and not before.
- The frame callbacks of a kept commit are answered on the frame clock of the surface's output: with the next frame
  of that output, once for each frame, as for a commit that was shown. A component that draws keeps the output's
  pace and never runs ahead of it.
- A kept buffer that a later kept commit of the same surface replaces is released at once, so a surface with two or
  three buffers goes on drawing while its commits are kept.
- A connection has one open bracket at most, whichever of its transactions opened it: the role requests belong to
  the connection, so a second bracket could not tell its requests from the first's.
- The requests that count as window changes:

| Role object | Window changes |
|---|---|
| `cupertino_dock_v1` | `activate_app`, `activate_window`, `restore_window`, `app_command`, `switch_workspace`, `add_workspace`, `remove_workspace`, `move_workspace` |
| `cupertino_menu_bar_v1` | `window_command`, `activate_window`, `move_to_workspace` |
| `cupertino_window_manager_v1` | `commit` with the placements set before it, `activate_window` |
| `cupertino_overview_v1` | `activate_window`, `switch_workspace`, `add_workspace`, `remove_workspace`, `move_workspace`, `move_window` |

### Start, deadline and late parts

- A participant is ready when it sent `submit` and each surface it staged has committed since. One that has not
  submitted 100 ms after it created or joined is taken as submitted.
- A part never waits without end. 100 ms after its `submit`, a participant with a staged surface that has not
  committed is ready as it stands: that surface leaves the part, and the rest (the kept commits of its other
  surfaces, its window changes) applies. `cupertino-kit-shell` commits a staged window in the update that submits,
  also when the view draws nothing that differs, so a part of a Kit window is ready with its `submit`. In a
  transaction bound to a gesture the surface stays in the part: it waits for the gesture anyway, and a new window's
  first frame, drawn after its `submit` and often later than 100 ms, is kept for the gesture like the others rather
  than shown at once.
- The transaction starts on the first frame of its output that the server prepares after the creator and every
  component it expects (`expect`) are ready, and at the latest on the first frame prepared after the deadline. The
  deadline counts from the creator's `submit`: one frame interval of the output plus 4 ms unless `set_deadline` gives
  another, never more than 100 ms. A component that is stopped, lost, unresponsive or busy is not waited for.
- On that frame the server applies every ready participant's kept commits and carries out its window changes. The
  frame's presentation time is the start time. Every participant hears `started` with it.
- A participant that becomes ready later joins on the next frame at the original start time: each of its animations
  runs as if it had started then, from the value and velocity its property had then. An animation whose end has
  passed is at its end.
- A join by name that reaches the server before the creator's `create` waits for it, at most 100 ms, then starts as a
  transaction of its own, on the clock of the output the join names. When the `create` is there, the creator's output
  is the transaction's and the join's is not read.
- A transaction bound to no gesture ends once it started and every participant that did not withdraw had its part
  applied and destroyed its object: no event says when its animations end, so a participant keeps its object while
  it waits for them and destroys it when it is done.
- A join within 10 s after the transaction ended applies with every animation at its end.
- A connection takes part in at most 16 transactions that have not ended, the ones it created and the ones it
  joined counted together.

### Gestures

`bind_gesture(gesture, response, damping)` arms a transaction for a touchpad gesture the server's recogniser owns:
swipe up, swipe down, pinch, spread. It needs the `overview` role.

| Phase | What the server does | Event |
|---|---|---|
| Armed | keeps the staged commits; the gesture on that output goes to the transaction and not to the dock | |
| The recogniser claims the gesture | applies the kept commits; values set in them take effect; every animation is tracked | `gesture_began` |
| Fingers move | each frame, every tracked property shows start + progress × (target − start); an animation through samples shows the sample at the progress | none |
| Fingers lift | the recogniser decides completed or reverted; the progress moves to 1 or 0 along the binding's spring, from the gesture's velocity | `gesture_ended` |
| At rest at 1 | carries out the window changes | `finished(completed)` |
| At rest at 0 | drops the window changes | `finished(reverted)` |

- The progress, its velocity and the rule that decides completed or reverted are the recogniser's
  (`cupertino-input`), the same for every binding.
- A touchpad gesture belongs to the output under the pointer at the moment the recogniser claims it, and stays with
  that output until the fingers lift.
- `bind_reverse_gesture(gesture)` keeps a completed transaction for a second gesture, which drives the same
  properties from their values at rest back to the values they had when the first gesture began. Reverted, the
  transaction is over. The binding lasts until the creator destroys its object.
- One binding takes a gesture. When several bindings wait for the same gesture on the same output, the server picks
  at the moment the recogniser claims the gesture:

  | Order | Binding | Among several |
  |---|---|---|
  | 1 | a reverse binding of a transaction at rest completed: what is on screen is taken back first | the transaction that completed last |
  | 2 | a binding of an armed transaction | the transaction armed last |

  The bindings passed over stay as they are and take a later gesture. A gesture under way is never handed from one
  binding to another.
- An armed transaction keeps its staged surfaces from showing later commits, so it suits a surface that shows nothing
  before the gesture. A participant whose surface draws all the time joins on `gesture_began` and shows at the
  progress reached.
- Window changes are carried out once and are not undone by a reverse gesture.

### Holes

A hole is a layer (`cupertino_layer_v1`) of a component's surface in which the server draws a source:

| Source | Draws | Role |
|---|---|---|
| `window` | a window as it shows, with decorations and shadow, live | `windows` |
| `thumbnail` | a minimized window as it last showed | `windows` |
| `workspace` | a workspace as it is, wallpaper and windows, live | `overview` |

- The hole fills the layer's bounds, with the source fitted and centred, and moves, scales, fades and is clipped
  with the layer. The layer's animations are the hole's, so a hole takes part in a transaction through its parent
  surface's kept commit.
- A component never receives a hole's pixels. A capture leaves out of a hole what it leaves out at the source's own
  place.
- `set_claim` takes a window off its own place: it is drawn in the hole alone and gets no pointer or touch input.
  A hole that claims in the commit that first shows it, with its layer where the window shows, takes the window over
  with no change on screen. Claims need the `overview` or the `window_manager` role.
- The claims of a window stack in the order they were applied. The latest is in force: the window is drawn in its
  hole. Every claim below it is suspended, and its hole draws nothing meanwhile.
- When the claim in force ends, the one below it is in force again and the window is drawn in that hole. The window
  is back at its own frame when no claim is left. So where a window shows without a claim is the hole of the claim
  below, or its own frame.
- A claim that the component ends with a commit puts the window where it shows without that claim, from that
  commit. A claim that ends any other way (the surface is dismissed, the connection ends without a hold) moves the
  window there from where the hole showed it, along the server's return motion.
- The overview over the stage strip is this stack: the strip's claim of a window is suspended while the overview
  claims it, and the window is in the strip again from the commit that ends the overview's claim. The overview's
  feed gives each window's frame as where it shows without the overview's claim, so the closing motion ends there.
- A parent surface holds at most 64 layers and a connection 256 (`cupertino_layers_v1`), so one surface shows at
  most 64 holes.

The compositor draws the stage strip's and the overview's windows and the dock's minimized windows only as holes.
The overview's windows are claimed holes of the overview surface. A window in the stage strip is a claimed hole of
the window manager's surface, and a window of a group the strip does not show is placed `hidden`.

### The plan's requirements

| # | Requirement | Met by |
|---|---|---|
| 1 | One clock | the start time is a presentation time of the transaction's output; every animation counts from it |
| 2 | Several participants, one frame | parts are kept and applied together on the start frame: a staged surface's commits, the commits of its layer surfaces, and the server's window changes |
| 3 | Motion described, not streamed | animations travel as `cupertino_layers_v1` curves, springs and samples in kept commits |
| 4 | Deadline; late parts join at the original start time | [Start, deadline and late parts](#start-deadline-and-late-parts) |
| 5 | Gesture binding | [Gestures](#gestures) |
| 6 | Holes | [Holes](#holes) |

### The plan's questions

| Question | Answer |
|---|---|
| A participant that dies mid-flight | What it had kept and not applied is dropped, and the transaction does not wait for it. On its held surfaces every animation is at its end at once, and their holes and claims stay ([Holds](#holds)); its transient surfaces are dismissed with their holes, and each claimed window returns along the server's return motion to where it shows without the claim. The other parts go on. When the creator of a gesture-bound transaction dies, the binding ends and a gesture under way settles back to the start. |
| Cancellation | The creator's `cancel` ends the transaction for all. Before the start each part is applied at once, by itself, as ordinary commits and requests; a participant that wants its change undone commits the opposite first. After the start every animation stops at the value shown on the next frame and its client hears `replaced`; window changes carried out stay. A gesture under way settles back to its start. `cancel` or `destroy` from another participant withdraws that participant's part alone. Every participant hears `finished(cancelled)`. |
| Two transactions on one property | The animation of the later start time takes the property from the value and velocity shown at that start, and the earlier animation's client hears `replaced`. A late part never takes a property back from a transaction that started after its own. |
| Outputs with different clocks | A transaction has one output, named at `create`, and one start time. Another output shows the transaction on its first frame presented at or after the start time, each animation as far along as that frame's own presentation time says. When the output is gone or shows no frame, the start time is the server's clock at the moment the transaction was ready or its deadline passed. |
| How a transaction latches with surface commits | `stage` keeps the surface's commits whole (buffer, damage, layer, presentation and hole state, subsurfaces' cached state) and the commits of its layer surfaces (their content) until the participant's part is applied, later commits added to earlier ones and a replaced buffer released. Frame callbacks of kept commits are answered with the output's frames. The part is applied on the start frame, or on the frame after the participant became ready when it is late; a staged surface that has not committed 100 ms after `submit` leaves the part. After that the surface and its layers commit as usual. |
| Reduced motion | The layer protocol's rule holds inside a transaction: spatial animations are at their target in their first frame and opacity animates as asked, and the server's window changes take their reduced form. A component describes the reduced form of its own change, a fade. Under a gesture the progress drives opacity alone; spatial properties take their target when the progress rests at 1. |
| A hole whose window closes | The hole keeps the window's last picture and fades it out where it lies, along the server's closing animation, then draws nothing. The component hears `source_closed` and lays out without the hole. |

### Limits

Named constants of `cupertino-protocols`, stated in the XML and checked by the server and by `cupertino-kit-shell`
before it sends.

| Limit | Value |
|---|---|
| Deadline without `set_deadline` | one frame interval + 4 ms |
| Longest deadline | 100 ms |
| A participant is taken as submitted | 100 ms after it created or joined |
| A part is ready as it stands | 100 ms after its `submit` |
| A join waits for its transaction | 100 ms |
| An ended transaction is remembered | 10 s |
| Transactions of a connection that have not ended, created and joined | 16 |
| Open brackets of a connection | 1 |
| Staged surfaces of a participant | 32 |
| Expected components | 16 |
| Holes of a connection | 256 |
| Settle spring | response 0.01 to 10 s, damping ratio 0.05 to 4 |

## `cupertino-kit-shell`

The one crate through which a `shell` crate reaches the shell's protocols (D23). It is built on `cupertino-kit`:
windows are Kit windows, motion is `kit::motion`, slow work is `kit::task`.

### Rules

- A `shell` crate names `cupertino_kit_shell::…` and `cupertino_kit::…`, and no `cupertino-shell-client`,
  `cupertino-protocols` or `cupertino-wayland-client` item.
- The crate names no Wayland object, no transport type, no raw surface and no descriptor. A surface is named by its
  Kit `WindowKey`, a layer by its key, an output by `OutputId`.
- Every function that sends a request acts on the component of the calling thread and returns at once; the request
  goes out before the component's loop next waits. Called on a thread that runs no component, it does nothing and
  logs once.
- Nothing blocks. An answer arrives as an `Event`, which the component turns into a message.
- `hosting` is `cupertino-shell`'s alone: `cupertino-layers` fails any other crate that names
  `cupertino_kit_shell::hosting`.

### Modules

| Module | Holds |
|---|---|
| root | `Component`, `Program`, `ComponentName`, `Entry`, `Surface`, `Role`, `LayerSpec`, `Layer`, `Anchor`, `Margin`, `Keyboard`, `Hole`, `HoleSource`, `ServerWindowId`, `WorkspaceId`, `Event`, `Error`, `run`, `run_lock_screen`, `environment`, `holds` |
| `session` | the lock notice, the idle notice, `activate_app` |
| `outputs` | `OutputId`, `Output`, `Transform`, `all`, `get` |
| `design` | `MenuBar`, `menu_bar` |
| `menu_bar`, `dock`, `window_manager`, `overview` | each role's feed and requests |
| `transaction` | `Id`, `Transaction`, `Handle`, `Gesture`, `Outcome`, `Event`, `create`, `join` |
| `bus` | `Message` and its value types, `Topic`, `Sender`, `publish`, `follow`; `session` (the session bus connection), `Caller`, `caller` |
| `host` | `thread`, `Stop`, `spawn` |
| `lock` | `lock`, `unlock`, `Event` |
| `consent` | `bind`, `set_window`, `answer`, `stop`, `Request`, `App`, `Answer`, `Run`, `Event` |
| `capture` | `windows`, `once`, `follow`, `Source`, `Options`, `Window`, `Follow`, `Event` |
| `test_clock` | `display_unix_seconds`, `display_now`, `motion_settled` |
| `hosting` | the host program's side: `Host`, `HostEvent`, `Kind`, `Liveness`, `Failure`, `Unavailable`, `Connection`, `Runtime`, `Running`, `Ended` |

### Components and programs

```rust
/// A Kit application that shows its windows through the shell's roles.
pub trait Program: cupertino_kit::core::Application {
    /// How a window of `view` reaches the screen. Asked for every window after each `view`.
    fn surface(model: &Self::Model, window: &WindowKey) -> Surface;
    /// What the shell said, as a message of the program; `None` for what it does not follow.
    fn event(event: Event) -> Option<Self::Message>;
    /// Whether the view shows what the program has to show; until it does, no view counts as the first (`ready`).
    /// Every view counts by default.
    fn showing(model: &Self::Model) -> bool { true }
}

/// A part of the shell that runs in a host.
pub trait Component: Program {
    const NAME: ComponentName;
}

/// A name of the component table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ComponentName(/* private */);
impl ComponentName {
    pub const MENUBAR: ComponentName;
    // one constant for each row of the table
    pub fn as_str(self) -> &'static str;
    pub fn from_name(name: &str) -> Option<ComponentName>;
}

/// A component as a host lists it.
pub struct Entry(/* private */);
impl Entry {
    pub const fn of<C: Component>() -> Entry;
    pub fn name(&self) -> ComponentName;
}

/// Runs a program of its own (`cupertino-login`, `cupertino-prompts`, `cupertino-screenshot`) on the connection the
/// server gave the process, laying out with `environment` (`kit::app::environment`), until it exits.
pub fn run<P: Program>(environment: Environment) -> Result<(), Error>;
/// `run` for the lock screen, whose accessibility tree the session's lock never mutes.
pub fn run_lock_screen<P: Program>(environment: Environment) -> Result<(), Error>;

/// The theme, text system and icons the calling thread's component lays out with before it has a window. The
/// components of a host share the text system and the icons.
pub fn environment() -> Environment;
```

- A component crate is a library that exposes one type implementing `Component`. `cupertino-shell` lists them as
  `Entry::of::<T>()`; a host starts an entry with `hosting::Runtime::start`.
- `Application::init` runs on the component's thread at every start of the component. The model lives in the
  runtime, outside the frames a panic unwinds.
- `Application::RENDERER` is the component's renderer class, as for an application.

### Surfaces and roles

```rust
pub struct Surface {
    pub role: Role,
    /// The key the surface is persisted with: it is held when the component's connection ends, and the surface of
    /// the same key takes the hold over. `None` is a transient surface.
    pub persistent: Option<&'static str>,
    /// The layers of the window that are holes.
    pub holes: Vec<Hole>,
}

pub enum Role {
    /// Kit's own: a toplevel, or a popup when the window has `popup`.
    Window,
    /// A layer surface.
    Layer(LayerSpec),
    /// The menu bar of the spec's output (`menu_bar` role).
    MenuBar(LayerSpec),
    /// The dock of the spec's output (`dock` role).
    Dock(LayerSpec),
    /// A lock surface of an output, shown while the program holds the session lock (`session_lock` role).
    Lock(OutputId),
}

pub struct LayerSpec {
    pub namespace: &'static str,
    /// `None` leaves the output to the server: the active one.
    pub output: Option<OutputId>,
    pub layer: Layer,
    pub anchor: Anchor,
    /// Points; an extent of 0 stretches between the anchored edges.
    pub size: Size,
    pub margin: Margin,
    pub exclusive_zone: i32,
    pub keyboard: Keyboard,
}
pub enum Layer { Background, Bottom, Top, Overlay }
pub struct Anchor { pub top: bool, pub bottom: bool, pub left: bool, pub right: bool }
pub struct Margin { pub top: i32, pub right: i32, pub bottom: i32, pub left: i32 }
pub enum Keyboard { None, OnDemand, Exclusive }
```

- A window's role is fixed when it opens. Anchor, size, margin, exclusive zone and keyboard follow `surface` and
  apply with the window's next frame; another output or layer needs another window key.
- The server sizes a layer surface; the window is laid out at the size it configures. `Event::Closed(WindowKey)`
  says the server closed or dismissed a surface: the component drops the window from its view.
- **Prompt surfaces.** On a connection with the `prompt` role every layer surface is a prompt surface: drawn above
  every other surface, left out of captures, and while one is mapped no application holds the pointer or the
  shortcuts.
- **Lock surfaces.** `lock::lock()` asks for the session lock; `lock::Event::Locked` says the server hides the
  session, and the windows with `Role::Lock(output)` are what it shows. `lock::unlock()` ends the lock;
  `lock::Event::Finished` says the server ended it. `session::Change::LockRequested { explicit }` is the server asking
  the lock screen to lock.

### Events

```rust
pub enum Event {
    Session(session::Change),
    /// An output came, went or changed: read `outputs::all()`.
    Outputs,
    Closed(WindowKey),
    /// The server configured the layer or lock surface of `window` to `size`, in points: the area its anchors and
    /// margins leave it within the space other surfaces reserve, which the window is laid out at. With every configure,
    /// before the frame drawn at that size.
    Configured { window: WindowKey, size: Size },
    /// The window or workspace of the hole on `layer` of `window` is gone.
    HoleClosed { window: WindowKey, layer: u64 },
    MenuBar(WindowKey, Box<menu_bar::Update>),
    Dock(WindowKey, dock::Update),
    WindowManager(OutputId, window_manager::Update),
    Overview(OutputId, overview::Update),
    Transaction(transaction::Event),
    Bus { from: bus::Sender, message: bus::Message },
    /// The server accepted or refused the component's announced bus connection. It arrives before `bus::session`
    /// hands the connection over.
    BusIdentity { accepted: bool },
    Lock(lock::Event),
    Consent(consent::Event),
    Capture(capture::Event),
    /// The appearance and display options a Kit program's theme follows (dark, Increase Contrast, Reduce
    /// Transparency, Reduce Motion, the steady caret): once at start and on every change.
    Appearance(SystemAppearance),
    /// Caps Lock turned on or off on the component's keyboard (the password field's badge).
    CapsLock(bool),
}
```

### The session, outputs and design values

```rust
pub mod session {
    pub enum Change {
        /// The lock notice: the session is locked or unlocked. Also sent once at start.
        Locked(bool),
        /// The idle notice: the seat had no input for the watched time, or input came back.
        Idle(bool),
        LockRequested { explicit: bool },
        LogOutRequested,
        AppActivation { serial: u32, result: Activation },
    }
    pub enum Activation { Activated, Resident, NotRunning }
    pub fn locked() -> bool;
    /// Follows the seat's idleness after `after` without input; dropping the watch ends it.
    pub fn watch_idle(after: Duration) -> IdleWatch;
    /// Brings a running application forward; the answer is `Change::AppActivation` with `serial`.
    pub fn activate_app(serial: u32, desktop_id: &str);
}

pub mod outputs {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct OutputId(/* private */);
    pub struct Output {
        pub id: OutputId,
        /// The connector, such as `DP-1`.
        pub name: Option<String>,
        pub description: Option<String>,
        /// Position and size in logical points.
        pub geometry: Option<Rect>,
        /// Device pixels per logical point.
        pub scale: f64,
        pub transform: Transform,
    }
    pub enum Transform { Normal, Rotated90, Rotated180, Rotated270, Flipped, Flipped90, Flipped180, Flipped270 }
    pub fn all() -> Vec<Output>;
    pub fn get(id: OutputId) -> Option<Output>;
}

pub mod design {
    /// The menu bar's values of the theme data.
    pub struct MenuBar {
        pub height: f64,
        pub selection_radius: f64,
        /// The style of a menu title and of the application's name.
        pub item: TextStyle,
        pub app_name: TextStyle,
        /// Cap height over the em: the bar centres its text on it.
        pub cap_height: f64,
    }
    pub fn menu_bar() -> MenuBar;
}
```

- Appearance and display options reach a component as they reach an application: `platform::SystemAppearance` and
  the theme its widgets are painted with.
- An `OutputId` is the registry name of the output's `wl_output` global, which the server gives every connection
  alike. It is the same in every component of a session, so it travels in a bus message.
- It names the output while the output is plugged in. An output that is unplugged and plugged in again is another
  output: `Event::Outputs` says that the one went and the other came, and the server may give the number of an
  output that is gone to a later one. A component keeps nothing under an `OutputId` that `outputs::get` does not
  know, drops a bus message that names one, and knows a monitor again by `Output::name`.
- `Output::geometry` and every frame of a feed are in one space: global logical points, the space the outputs are
  arranged in. A component subtracts its output's origin to lay a frame out on a surface of that output.

### Windows and applications

```rust
/// A window of the session by the server's id: the same in every feed and every component, stable for the
/// window's lifetime. Not Kit's `WindowKey`, which names a window of the component itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServerWindowId(pub u32);
/// A workspace of an output by the server's id, stable for the workspace's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub u32);
```

Each role module holds the feed as an `Update`, applied whole, and one function for each request of the role's
interface, with the request's name.

| Module | Feed | Requests |
|---|---|---|
| `dock` | the window and application list: windows (application id, title, minimized, active), icons, launching and resident applications, attention, applications that do not respond; the application switcher; hot corners; `Engage`, `ToggleAppGrid`, `ToggleAutoHide`, `ForceQuitDialog` | `activate_app`, `activate_window`, `restore_window`, `set_minimize_target`, `force_quit_app`, `app_command`, `run_action`, `switcher_select`, `switcher_activate`, `switcher_cancel`, `switch_workspace`, `add_workspace`, `remove_workspace`, `move_workspace` |
| `menu_bar` | the active application and its windows, its menus, the window's state and controls, shortcuts, the backdrop bands, the screen capture state | `set_menu_open`, `window_command`, `activate_window`, `move_to_workspace`, `session_action`, `edit`, `force_quit_app` |
| `window_manager` | the usable area and the arranged windows with their frames; `ToggleStage` | `place(ServerWindowId, Placement)`, `commit`, `activate_window`; `Placement` is `Stage` or `Hidden` |
| `overview` | the area, the windows of the workspace with their frames, the workspaces, each with whether it is the current one and whether it is a full-screen one; `Toggle { mode, app_id }` | `activate_window`, `switch_workspace`, `add_workspace`, `remove_workspace`, `move_workspace`, `move_window` |

- The types keep the names they have in `cupertino-shell-client` (`DockUpdate`, `DockChange`, `DockWindow`,
  `MenuBarUpdate`, `ActiveApp`, `AppWindow`, `Shortcuts`, `WindowCommand`, `EditAction`, `SessionAction` and the
  rest of that crate's list), inside their role's module. An output in a feed is an `OutputId`, and an image
  (a window's icon) is a `render::ImageResource`.
- The session menu's state (the person's name, what the session can carry out) is no part of the menu bar's feed:
  the `agent` component publishes it as `bus::Message::SessionMenu`, and the menu bar takes it from that sender
  alone.
- A minimized window's picture is a hole (`HoleSource::Thumbnail`), not an image in the feed. The dock's overview
  messages and the window manager's strip placement have no function: the overview is `overview`, and the strip is
  holes.
- A request that names a full-screen workspace where the server takes none (`remove_workspace`, `move_workspace`,
  `move_window`) does nothing: the feed's flag lets the component offer no such choice.
- The object of a role exists while a window with that role does (`Role::MenuBar`, `Role::Dock`); the window
  manager's and the overview's exist for every output from the component's start.

### Holes and holds

```rust
pub struct Hole {
    /// The key of the window's surface layer (`core::SurfaceLayerMark`) that is the hole.
    pub layer: u64,
    pub source: HoleSource,
    /// Takes the window off its own place while the hole shows it.
    pub claim: bool,
}
pub enum HoleSource { Window(ServerWindowId), Thumbnail(ServerWindowId), Workspace(WorkspaceId) }

/// The keys of the persistent surfaces the server holds from the instance before; empty at a first start.
pub fn holds() -> Vec<String>;
```

- The holes of a window are the ones `surface` returns. A hole that appears, changes or goes does so with the frame
  that draws the view it was returned for, so a claim and the layer's place arrive together.
- A hole's layer changes place with `WindowRequest::Layer`, as any surface layer does; the layer's widget paints
  what lies over the window (a title, a highlight).
- The crate persists a surface whose `Surface::persistent` is set before its first frame, and sends `ready` once
  every persistent window of the first view has presented a frame. The first view is the first for which
  `Program::showing` holds: a component that shows nothing until it has what to show (the desktop, until its folder is
  listed) says so, and the held pictures stay in place until then; the server's 10 s end them if it never shows.
- A window that takes a hold over shows its holes, its claims and, for the window manager, its placements in its
  first frame: the server swaps them for the held ones in that frame ([Holds](#holds)). A window manager commits
  its placements in the update that first shows its window, so both arrive together.

### Transactions

```rust
pub mod transaction {
    /// A transaction's name: the creator, the instance of the creator the server numbered, and a serial that
    /// instance chose.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Id { pub component: ComponentName, pub instance: u32, pub serial: u32 }

    /// Creates a transaction on the clock of `output`.
    pub fn create(output: OutputId) -> Transaction;
    /// Joins the transaction another component created. `output` is the creator's: the part starts on its clock
    /// when the creator's transaction never comes.
    pub fn join(id: Id, output: OutputId) -> Transaction;

    /// A part being described. Dropped without `submit`, it is withdrawn.
    pub struct Transaction(/* private */);
    impl Transaction {
        pub fn id(&self) -> Id;
        /// Waits for `component`'s part until the deadline. The creator's only.
        pub fn expect(&mut self, component: ComponentName);
        /// The creator's only; at most `MAX_DEADLINE`.
        pub fn deadline(&mut self, deadline: Duration);
        /// Puts `window` in the part: the `WindowRequest::Layer` and `WindowRequest::Present` commands this update
        /// returns for it, and the frame that draws the view after this update.
        pub fn stage(&mut self, window: &WindowKey);
        /// Puts in the part the window changes that `changes` asks for through the role modules.
        pub fn window_changes(&mut self, changes: impl FnOnce());
        /// Arms the transaction for `gesture`; it settles along `settle`. The creator's only.
        pub fn bind(&mut self, gesture: Gesture, settle: SpringPreset);
        pub fn bind_reverse(&mut self, gesture: Gesture);
        pub fn submit(self) -> Handle;
    }

    /// A submitted part. Dropping it lets go of the events; the creator's handle of a gesture-bound transaction
    /// also holds its bindings.
    pub struct Handle(/* private */);
    impl Handle {
        pub fn id(&self) -> Id;
        pub fn cancel(self);
    }

    pub enum Gesture { SwipeUp, SwipeDown, Pinch, Spread }
    pub enum Outcome { Completed, Reverted, Cancelled }
    pub enum Event {
        /// The part was applied; every animation of the transaction counts from `at`.
        Started { id: Id, at: Timestamp },
        GestureBegan { id: Id, gesture: Gesture },
        GestureEnded { id: Id, outcome: Outcome },
        Finished { id: Id, outcome: Outcome },
    }
    pub const MAX_DEADLINE: Duration;
}
```

- A component describes its change as it does without a transaction: `LayerChange` and `PresentChange` with a
  `motion::Timing`, in the commands its update returns. `stage` only says which transaction the change belongs to.
- The crate sends `stage` for a window while none of the window's frames is handed over and not yet committed, so
  the next commit is the frame that carries the change.
- The crate commits each staged window in the update that submits, also when the view draws nothing that differs:
  a part is ready with its `submit` and never waits for a frame that does not come.
- `window_changes` opens the connection's one bracket and closes it before it returns.
- A creator tells the other participants with `bus::Message::Transaction { id, output, scene }` in the same update.
  A component that hears it calls `join(id, output)`, stages its window and submits.
- `Started::at` is on the clock of `platform::Timestamp`, so a component that draws something in step evaluates the
  same `Timing` from it.

### The host bus

```rust
pub mod bus {
    /// What components say to each other. Serialised with a `type` tag; unknown tags and fields are errors. Every
    /// variant is a struct variant, which the tag needs.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Message {
        /// From the host alone: the server stopped `component` after repeated failures.
        ComponentStopped { component: ComponentName },
        /// Open the panel, or close it when it is open.
        TogglePanel { panel: Panel, output: Option<OutputId> },
        /// Retained. Whether a panel is shown, and where.
        PanelShown { panel: Panel, shown: bool, output: Option<OutputId> },
        /// Turn do not disturb on or off; `lifetime` is the identifier of one of the durations offered.
        SetDoNotDisturb { on: bool, lifetime: Option<String> },
        /// Retained. Whether do not disturb is in effect, and the durations offered (identifier, title).
        DoNotDisturbChanged { on: bool, lifetimes: Vec<(String, String)> },
        /// Retained, from `agent`. The session menu's state: the person's name and what the session can carry out.
        SessionMenu {
            user_name: String,
            lock: bool,
            log_out: bool,
            switch_user: bool,
            sleep: bool,
            restart: bool,
            shut_down: bool,
        },
        /// Show a level without changing it.
        ShowOsd { kind: OsdKind, level: f64 },
        /// Join a transaction of `output`.
        Transaction { id: transaction::Id, output: OutputId, scene: Scene },
    }
    pub enum Panel { ControlPanel, NotificationCenter, Launcher, Applications }
    pub enum OsdKind { Volume, Mute, Brightness, KeyboardBrightness }
    pub enum Scene { OverviewOpening, OverviewClosing, StageSwitch }
    pub enum Topic { Host, Panels, DoNotDisturb, Session, Osd, Transactions }

    /// Who published a message. Its serialised form is a string: the component's name, or `host`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Sender { Host, Component(ComponentName) }

    impl Message {
        pub fn topic(&self) -> Topic;
        /// A retained message is kept by the bus and given to a component when it starts to follow the topic.
        pub fn retained(&self) -> bool;
    }

    /// Sends `message` to every component that follows its topic, the sender excepted. A message of `Topic::Host`
    /// is the host's to publish: from a component it is dropped and logged.
    pub fn publish(message: Message);
    /// Follows `topics`: their messages arrive as `Event::Bus`, the retained ones at once.
    pub fn follow(topics: &[Topic]);

    /// The component's connection to the session bus, as `kit::bus::connect` gives one, on a socket the host can
    /// shut. The crate announces it to the server and proves it (`AnnounceComponent`); `Event::BusIdentity` says
    /// how that went, and `connected` gets the connection after it, accepted or not.
    pub fn session<M: Send + 'static>(
        connected: impl FnOnce(zbus::Result<zbus::Connection>) -> M + Send + 'static,
        closed: M,
    ) -> Vec<Command<M>>;

    /// Who sent a call to the component's bus connection.
    pub enum Caller {
        /// The server: the owner of `org.cupertino.Session1`.
        Server,
        /// A component, by the name the server accepted for it.
        Component(ComponentName),
        /// Anyone else, a host's connection without an accepted name included.
        Other,
    }
    /// Resolves `sender` through `cupertino-peer`. Awaited in a bus handler, on the bus's executor, never on the
    /// component's thread.
    pub async fn caller(connection: &zbus::Connection, sender: &zbus::names::UniqueName<'_>) -> Caller;
}
```

- A publication never waits and has no reply. A component that needs an answer follows the topic the answer is
  published on.
- A message's sender is set by the bus, never by the publisher: the component whose thread published it, the
  component of the bridge it came over, or the host. A component trusts a message by its `from`: the menu bar takes
  `SessionMenu` from `Sender::Component(ComponentName::AGENT)` alone.
- The bus keeps the latest retained message of each publisher and variant (and of each `panel`), and drops it when
  its publisher stops.
- A queue holds 256 messages for a component; a component that does not empty its queue loses the oldest, and the
  host logs it once.
- A component makes no call of its own before its identity is settled: `session` hands the connection over after
  the server's answer, and a call the server would take as an unknown caller's is never sent early. With
  `BusIdentity { accepted: false }` the component has a connection the server gives no component's rights.
- A component accepts the server as a caller by `Caller::Server` and another component by `Caller::Component`. No
  component compares process ids or asks for its parent.
- Programs outside the hosts are reached through the session bus or the server's protocols, never through this bus.
- The catalogue is the eight messages above. A message joins it with a change of this file.

What the components read of each other goes over this bus and not over their session bus interfaces:

| Reader | What | Message | Publisher |
|---|---|---|---|
| `menubar` | the session menu's state | `SessionMenu` | `agent` |
| `menubar` | do not disturb and its durations | `DoNotDisturbChanged` | `notifications` |
| `menubar` | which panels are open, and where | `PanelShown` | `controlcenter`, `notifications`, `launcher` |
| `notifications` | a component the server stopped | `ComponentStopped` | the host |
| any | open or close a panel, set do not disturb, show a level | `TogglePanel`, `SetDoNotDisturb`, `ShowOsd` | the component that asks |

### Threads and child processes

```rust
pub mod host {
    /// Starts a helper thread of the calling thread's component and sends `done` with its result as a message. The
    /// thread runs for the component: its diagnostic events are the component's, and its panic is the component's
    /// failure.
    pub fn thread<T: Send + 'static, M: Send + 'static>(
        name: &'static str,
        work: impl FnOnce(&Stop) -> T + Send + 'static,
        done: impl FnOnce(T) -> M + Send + 'static,
    ) -> Command<M>;

    /// Set when the component stops: the helper ends its work.
    pub struct Stop(/* private */);
    impl Stop {
        pub fn is_set(&self) -> bool;
        /// Waits up to `timeout`; true when the stop was set meanwhile.
        pub fn wait(&self, timeout: Duration) -> bool;
    }

    /// Starts a program detached from the component. The host reaps it, also after the component has stopped. False
    /// when it did not start.
    pub fn spawn(command: std::process::Command) -> bool;
}
```

- A helper thread is for work that waits without end (a watch, a device). Work that ends is a `kit::task`.
- When a component stops, the host sets the stop of each of its helpers and joins them for 1 s. A helper that has
  not ended by then is kept like a lost component's thread, cut off from the bus and its result dropped, and the
  host asks for a restart (`request_restart`): a shared host gets it at the next quiet moment, a separate host at
  once.
- A child inherits no component socket. In a program of its own (`run`), `spawn` hands the child to the program's
  Kit instance, which reaps it.

### Consent and capture

```rust
pub mod consent {
    /// Becomes the session's consent authority (`consent` role): requests arrive as `Event::Requested`.
    pub fn bind();
    /// The window the person answers on.
    pub fn set_window(window: &WindowKey);
    /// Sends the answer with the serial of the last press or key that window received.
    pub fn answer(run: Run, answer: Answer);
    pub fn stop(run: Run);

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Run(/* private */);
    pub struct Request {
        pub run: Run,
        pub capabilities: u32,
        pub output: String,
        pub resuming: bool,
        pub app: Option<App>,
    }
    pub struct App {
        pub desktop_entry: bool,
        pub identity: String,
        pub name: String,
        pub services: u32,
        pub remembered: bool,
    }
    pub enum Answer { Allow, Deny, Dismiss }
    pub enum Event { Requested(Request), Changed { run: Run, phase: u32 } }
}

pub mod capture {
    pub enum Source { Output(OutputId), Window(String) }
    pub struct Options { pub cursor: bool, pub region: Option<Rect> }
    pub struct Window { pub identifier: String, pub frame: Rect }
    /// Follows the windows on screen, front to back: `Event::Windows`.
    pub fn windows();
    /// Captures one frame and sends `done` with it as a message.
    pub fn once<M: Send + 'static>(
        source: Source,
        options: Options,
        done: impl FnOnce(Result<ImageResource, Error>) -> M + Send + 'static,
    ) -> Command<M>;
    /// Captures every frame that changes, as `Event::Frame`; dropping the value ends it.
    pub fn follow(source: Source, options: Options) -> Follow;
    pub enum Event { Windows(Vec<Window>), Frame { follow: u32, image: ImageResource } }
}
```

- `consent` is `cupertino-prompts`'s. The screenshot and global-shortcut consent dialogs reach that program over its
  private bus interfaces, not through this module.
- `capture` needs the `capture` role and is `cupertino-screenshot`'s: screen, window and selection capture, the
  colour picker (`follow` with a region) and the screen-sharing picker (`windows`, `once`). A frame is copied into an
  `ImageResource` on the I/O pool, never on the UI thread.

### The test clock

```rust
pub mod test_clock {
    /// The wall clock as drawn, in Unix seconds.
    pub fn display_unix_seconds() -> Result<i64, SystemTimeError>;
    /// The monotonic clock for what is drawn.
    pub fn display_now() -> Instant;
    /// Whether an opening animation starts at its end, because a test fixed the clock.
    pub fn motion_settled() -> bool;
}
```

A debug build reads `CUPERTINO_TEST_CLOCK`; a release build has neither the override nor the variable's name. Only
for what is drawn, never for authentication, back-off, locks or deadlines.

### The host program's side

`cupertino-shell` is a `shell` crate: it starts no thread and opens no socket itself. What a host does with threads,
sockets and the host protocol is in `hosting`; the host keeps the policy (the watchdog's times, what follows each
event) and the component list.

```rust
pub mod hosting {
    /// The host's connection (`WAYLAND_SOCKET`, `host` role).
    pub struct Host(/* private */);
    impl Host {
        /// `Error::Version` when the server offers another version of `cupertino_host_v1` than the crate was built
        /// with: the host exits with `EXIT_VERSION`.
        pub fn connect() -> Result<Host, Error>;
        /// Waits for events at most `timeout`. The host's main thread is no UI thread: it may wait.
        pub fn wait(&mut self, timeout: Duration) -> Result<Vec<HostEvent>, Error>;
        /// `get_component` and `connect`; `connect` again for a component that has its object. `name` is one the
        /// server listed, so it is a name of the server's table.
        pub fn connect_component(&mut self, name: &str);
        pub fn unavailable(&mut self, name: &str, reason: Unavailable);
        /// Destroys the component's object.
        pub fn release(&mut self, name: &str);
        pub fn report(&mut self, component: ComponentName, state: Liveness);
        pub fn failed(&mut self, component: ComponentName, cause: Failure);
        pub fn request_restart(&mut self);
    }
    /// The status a host exits with when the server's version of the host protocol is not its own.
    pub const EXIT_VERSION: i32 = 76;
    pub enum Kind { Shared, Separate }
    pub enum Liveness { Busy, Unresponsive, Responsive, Lost }
    pub enum Failure { Panic, Error, Exit }
    pub enum Unavailable { Absent, Version }
    pub enum HostEvent {
        Kind(Kind),
        /// A whole list, as sent.
        Components(Vec<String>),
        Connection { component: ComponentName, connection: Connection },
        /// The answer to a connect: the server starts the component no more by itself.
        Stopped { component: ComponentName },
        /// To the shared host: the server gave `component` up, in whichever host. The host publishes
        /// `bus::Message::ComponentStopped`.
        ComponentStopped { component: ComponentName },
        RestartComponent { component: ComponentName },
        /// The quiet moment came: stop and exit.
        Restart,
    }
    /// A component's connection, as the server handed it over.
    pub struct Connection(/* private */);

    /// What the components of a process share: the environment, the pools, the render thread, the host bus and
    /// the reaper of child processes.
    pub struct Runtime(/* private */);
    impl Runtime {
        /// A shared host is the hub of the bus; a separate one joins it over the socket the server passes.
        pub fn for_host(host: &mut Host, kind: Kind, environment: Environment) -> Result<Runtime, Error>;
        /// Starts `entry` on a thread of its own (8 MiB stack) on `connection`. `Error::Version` when the
        /// connection offers another version of an interface the component binds than the crate was built with:
        /// no thread starts, and the host answers `unavailable` with `Unavailable::Version`.
        pub fn start(&self, entry: &'static Entry, connection: Connection) -> Result<Running, Error>;
        /// Publishes on the host bus as the host. In a separate host a message of `Topic::Host` is dropped and
        /// logged: the shared host alone publishes on that topic.
        pub fn publish(&self, message: bus::Message);
        /// How long the render thread has been out of its wait.
        pub fn render_busy_for(&self) -> Option<Duration>;
    }

    /// A component that was started.
    pub struct Running(/* private */);
    impl Running {
        /// How long its loop has been out of its wait; `None` while it waits.
        pub fn busy_for(&self) -> Option<Duration>;
        /// Asks its loop to end; teardown runs inside `catch_unwind`.
        pub fn stop(&self);
        /// Cuts the component off. Its Wayland and bus sockets are shut down (`shutdown(2)`; the descriptors close
        /// when the thread has ended). It leaves the host bus: nothing it publishes is delivered, its retained
        /// messages go, its queue is dropped. The results of its tasks and helper threads are dropped unread.
        pub fn shut(&self);
        /// How it ended, once its thread and its helper threads have.
        pub fn ended(&mut self) -> Option<Ended>;
    }
    pub enum Ended { Stopped, Failed(Failure) }
}
```

- `Runtime` handles the bridge by itself: `bus_peer`, `bus`, `bus_peer_gone` and `bus_dropped` never reach the
  host's code.
- A component the host started again publishes on the bus as itself from its first turn; the instance that was shut
  is no publisher, so the two never speak side by side.
- A panic in a component's thread, task or helper is caught at the thread's top; the state it leaves is dropped
  inside `catch_unwind`, and a second panic there leaks it. `ended` then gives `Failed(Failure::Panic)`.
- The host's constants (check every 0.5 s, busy after 0.5 s, grace 10 s, unresponsive 2 s, lost 10 s, helper join
  1 s, the exit status 76) are named in `cupertino-shell` and `cupertino-kit-shell`; the restart, hold, quiet-moment
  and kill constants in `cupertino-server`.

## The input-source switcher and the accent picker

The design note of W6.12. `osd` draws both panels. The server keeps their keys, their choice and the text they
change, as it does for the application switcher the dock draws (`cupertino_dock_v1`). No key reaches a component,
so nothing a component does or fails to do delays a switch or an edit.

| | The server | `osd` |
|---|---|---|
| The switch chord, its steps, Escape, Return, Up, Down, the release of the modifiers | takes them from the seat; no client gets them | never sees them |
| The sources, their order, the selection, the keymap | `InputSources` of `cupertino-server-input` | shows what it is sent |
| The held letter, its first repeat, the keys of the open picker | `AccentUi` and the key filters | never sees them |
| The candidates and the focused one | `AccentPicker` of `cupertino-keyboard` | shows what it is sent; reports a click |
| The text field: the pre-edit, the commit, the restore | its own input method | never sees the text |
| The panels: surfaces, layout, text, material, shadow, fade | draws nothing | all of it |

### The role, the object and the surfaces

1. The role `keyboard_panels` lets a connection create `cupertino_keyboard_panels_v1` with
   `cupertino_shell_v1.get_keyboard_panels`. `osd` holds it; no other component and no program does.
2. The object needs no surface and exists from the component's start, as the window manager's does. A second
   object on the connection is the `role` error; a connection without the role gets `permission`.
3. A panel is an ordinary transient layer surface (`Role::Layer`). The server knows it by its namespace on a
   connection that holds the role; the same namespace on another connection is any layer surface.
4. A panel's window exists only while the panel shows or fades.

| | The switcher's panel | The picker's panel |
|---|---|---|
| Namespace | `cupertino-osd-input-sources` | `cupertino-osd-accents` |
| Layer | overlay | overlay |
| Output | the one `sources_changed` names | the one `accents_changed` names |
| Anchor | none: centred on the output | top and left |
| Margin | none | the origin of the layout's rectangle |
| Size | `switcher::hud_content_size` of its rows | the layout's rectangle |
| Exclusive zone | -1 | -1 |
| Keyboard | none | none |
| Input region | empty | the whole surface |

- The picker's layout is `cupertino_keyboard::accent::layout_popup` of the candidates, the anchor rectangle and
  the output less `top` points at its top, with `PopupMetrics::default()`.
- The look is today's: the switcher on the regular glass of an alert with its rows, source codes and 0.3 s fade
  (`switcher::fade_alpha`); the picker on the menu material with its numbered cells. `osd_capture` gains both
  surfaces, and the review sheet shows them before and after, light and dark.
- The rows and the candidates are Kit labels and buttons named by their text. W6.12 adds no announcement.
- Neither panel ever takes the keyboard: a focus change would end the text input the picker edits.

### Messages

`cupertino_shell_v1` gains one request and its version rises by one (28 when no other edit lands first).
`cupertino_keyboard_panels_v1` is new in the same file, at version 1.

| Interface | Message | Arguments | Says |
|---|---|---|---|
| `cupertino_shell_v1` | request `get_keyboard_panels`, `since` the new version | `id: new_id` | creates the object |
| `cupertino_keyboard_panels_v1` | request `destroy` | | destructor |
| | event `source_entry` | `id: string`, `name: string` | one source of the switch, most recently used first |
| | event `sources_changed` | `output: wl_output` or null, `selected: uint` | closes the list before it and selects one; sent when the switch opens and on every step, with the whole list |
| | event `sources_closed` | | the switch ended, committed or cancelled |
| | event `accent_entry` | `text: string` | one candidate, in order; at most nine |
| | event `accents_changed` | `serial: uint`, `output: wl_output` or null, `x`, `y`, `width`, `height: int`, `top: int`, `focused: int` | closes the list before it; sent when the picker opens and on every move of the focus |
| | event `accents_closed` | `serial: uint` | that picker is gone |
| | request `accent_pick` | `serial: uint`, `index: uint` | the pointer was released on a candidate |

- An `output` is null when the client has not bound that output; the panel then goes on its first one.
- `x`, `y`, `width`, `height` are the text cursor's rectangle in the output's logical points, from its top left
  corner: the rectangle the server anchors at today. `top` is the menu bar's height while the configuration
  shows one, else 0. `focused` is -1 when no candidate is.
- `serial` counts the pickers of the session. An `accent_pick` that names another picker than the open one, or an
  index outside its list, is ignored.
- Strings are cut as the dock's are.

```rust
pub mod keyboard_panels {
    pub const SOURCES_NAMESPACE: &str = "cupertino-osd-input-sources";
    pub const ACCENTS_NAMESPACE: &str = "cupertino-osd-accents";
    pub struct Source { pub id: String, pub name: String }
    pub enum Update {
        Sources { output: Option<OutputId>, sources: Vec<Source>, selected: usize },
        SourcesClosed,
        Accents { serial: u32, output: Option<OutputId>, anchor: Rect, top: f64, candidates: Vec<String>,
                  focused: Option<usize> },
        AccentsClosed { serial: u32 },
    }
    pub fn accent_pick(serial: u32, index: u32);
}
// Event gains: KeyboardPanels(keyboard_panels::Update)
```

### The switch

1. The chord opens the switch in the server's model and the server sends the list (`source_entry` for each
   source, then `sources_changed`) naming its first output, where the panel is today.
2. Each further chord, Up and Down step the selection; the server sends the whole list again.
3. The server commits the selected source the moment the chord's modifiers are up, or on Return, and cancels on
   Escape. It changes the keymap itself and then sends `sources_closed`. It waits for nothing.
4. `osd` maps the panel on the first `sources_changed` and fades it from `sources_closed`. When the close
   arrives before the panel's first frame, the fade starts with that frame, so a quick tap still shows the choice.
5. A chord without held modifiers (the globe key, a caps lock tap) commits at once and sends nothing.
6. When the session locks, the server cancels an open switch and dismisses an open picker.

### The picker

1. A text field enables `zwp_text_input_v3` (`enable`, `set_surrounding_text`, `set_cursor_rectangle`,
   `commit`). The server's input method hears `activate`, `surrounding_text` and `done`. It is the in-process
   `zwp_input_method_v2` client of `cupertino-server-input`, the only client that sees
   `zwp_input_method_manager_v2`. The cursor rectangle reaches the server through that client's popup surface,
   which is never mapped.
2. The server arms a held letter only while the text input is active, the letter has candidates, the application
   allows the picker, and `osd` has its object and is not reported busy, unresponsive or lost. Otherwise the
   letter repeats in the application.
3. On the letter's first repeat the server opens the picker and sends `accent_entry` for each candidate, then
   `accents_changed`. When no output holds the anchor's centre the picker does not open.
4. Tab, Shift+Tab, Left and Right move the focus. The server sends the list again and shows the focused
   candidate in the field: `delete_surrounding_text` for the letter, once, then `set_preedit_string` and
   `commit`, which the field gets as `delete_surrounding_text`, `preedit_string` and `done`.
5. A digit, Return, or `accent_pick` inserts a candidate: `set_preedit_string` with no text, `commit_string`,
   `commit`. Escape puts the letter back with `commit_string` when a preview had replaced it.
6. The picker also closes, as cancelled, when the text input ends, the focus changes, the pointer is pressed
   anywhere but on the picker's panel (the press goes on to what it hit), or `osd` is gone. Any other key closes
   it and goes to the application.
7. The server sends `accents_closed` after every close. The pre-edit is the server's from start to end: `osd`
   holds no part of an edit, so none is left in a field when `osd` ends.

### When `osd` is not there

| `osd` | The switch | The picker |
|---|---|---|
| Not started, turned off, stopped, or between two instances | works without a panel: the release commits | not armed: letters repeat |
| Its connection ends while a panel shows (D19) | goes on; the panel is dismissed at once | dismissed: the letter is put back |
| The modifiers come up while it restarts | committed at once by the server; the new instance is sent nothing | |
| A new instance binds during a switch | sent the list at once | |
| Busy | goes on | an open one stays; no new one is armed |
| Unresponsive (D17) | goes on; the panel is dismissed | dismissed: the letter is put back |
| Responsive again during a switch | sent the list again | armed again |

- A hung `osd` can leave a stale panel on screen for at most the 2 s before it is reported unresponsive. A click
  on a stale picker changes nothing: its `serial` is not the open picker's.
- The server's scripted input (`--key`, `--accent`, `--hold-modifiers`) drives the same models.

### What the server stops drawing

| Goes from the server | Stays in the server |
|---|---|
| `input_hud`, `accent_popup`, `render_hud`, `render_accent`, `hud_backdrop`, the palette and metrics of both | `InputSources`, `Switcher`, keymaps, `activate_input_source`, the caps lock tap |
| The scene layers `InputHud` and `AccentPopup`; the scene kinds `InputHud`, `InputHudTint`, `InputHudMaterial`, `InputHudShadow`, `AccentPopup`, `AccentMaterial`, `AccentShadow` | `AccentUi`, `AccentPicker`, `TextOp`, the input method, the key filters and the compositor's key repeat |
| The fade of the switcher (`HudView::alpha`, `fading_since`, `InputSources::tick`) | whether a switch is open |
| `accent_layout`, `hud_size`, `HUD_LABEL_EXTRA`, the pointer hit test of the picker | the press that dismisses the picker |

The notice that an application holds the shortcuts or the pointer stays the server's: it must show when no
component runs. The plan's table does not list it; W6.12 reports it there.

### Who may do what

1. Only the connection that holds `keyboard_panels` creates the object; each request checks the role again
   (D20: the role is bound to `osd`'s connection and ends with it).
2. No key of a switch or a picker is sent to any client, `osd` included.
3. `osd` learns the sources' names, the candidates and the cursor's rectangle: not the text around the cursor,
   not the application.
4. `accent_pick` can insert only a candidate of the open picker, into the field the server already edits.
5. `zwp_input_method_manager_v2` stays hidden from every connection but the server's own.
6. A layer surface with a panel's namespace on a connection without the role is no panel: a press on it
   dismisses the picker like any other.

### Tests

Program tests in `crates/cupertino-session-tests/tests/shell-keyboard-panels.rs`, headless, with two input
sources and an accent table:

1. `the_chord_shows_the_sources_panel_and_the_release_switches_the_source`
2. `a_quick_tap_switches_and_still_shows_the_choice_fading`
3. `the_release_while_the_panel_component_restarts_switches_the_source` (crash `osd` with the modifier held)
4. `a_panel_component_that_starts_during_a_switch_shows_its_list`
5. `a_hung_panel_component_delays_no_switch_and_its_panel_goes` (a stand-in `osd` of the test host that hangs)
6. `without_the_panel_component_the_chord_switches_and_a_held_letter_repeats`
7. `a_held_letter_opens_the_picker_at_the_caret_and_a_digit_inserts_its_candidate` (a Kit text field)
8. `escape_puts_the_letter_back_and_a_click_on_a_candidate_inserts_it`
9. `a_press_off_the_picker_closes_it_and_reaches_the_window_under_it`
10. `the_picker_closes_and_the_letter_returns_when_the_panel_component_ends`
11. `locking_ends_the_switch_and_the_picker`

Server tests in `crates/cupertino-server/tests/keyboard-panels.rs`, with clients in the server's process:

12. `a_connection_without_the_role_gets_the_permission_error`
13. `a_second_object_is_the_role_error`
14. `a_pick_of_another_picker_or_past_the_list_inserts_nothing`
15. `the_panels_connection_gets_no_key_of_a_switch`
16. `the_input_method_global_is_offered_to_no_component`
17. `a_surface_of_the_panels_namespace_without_the_role_is_no_panel`
18. `the_server_draws_no_panel` (a switch and a picker with no `osd`: the output's picture does not change)

`crates/cupertino-osd/tests/` covers the panels' views through `Osd`'s messages, and `osd_capture` their look.

## Edits of the existing protocol files

This specification changes messages of `cupertino-shell-v1.xml`, which holds the session, menu bar, dock and window
manager interfaces. Each edit belongs to the task that builds what it describes, raises the version of
`cupertino_shell_v1` (26 before the first), and lands with the server code that implements it.

| Task | Interface | Edit |
|---|---|---|
| W5.5 | the protocol's description | the sentence that lets interfaces change without a version bump goes: every change raises the version |
| W5.5 | `cupertino_session_v1` | `log_out_requested` is sent to the connection that holds the `session_agent` role, not the `session_lock` role |
| W5.5 | `cupertino_menu_bar_v1` | `session_agent` names a process only while the agent is a program of its own; its trust is `none` when a component is the agent |
| W5.5 | `cupertino_window_manager_v1` | when the connection of a window manager that is a component ends, its placements are held with its holds and replaced by the instance that takes the hold over ([Holds](#holds)); "every window is drawn and hit at its own frame again at once" holds for `destroy` and for the end of the holds; `commit` and `activate_window` are window changes inside a bracket |
| W5.5 | `cupertino_dock_v1`, `cupertino_menu_bar_v1` | the requests of [Parts](#parts) are window changes inside a bracket |
| W6.8 | `cupertino_dock_v1` | the overview leaves the dock: the events `overview_window`, `overview_workspace`, `overview_changed`, `overview_closed`, the enum `overview_mode` and the requests `overview_frame`, `overview_select`, `overview_close`, `overview_drag`, `overview_move_window`, `overview_workspace_frame` go, with the overview paragraph of the interface's description |
| W6.8 | `cupertino_dock_v1` | `window_thumbnail` goes: a minimized window's picture is a `thumbnail` hole |
| W6.8 | `cupertino_dock_v1` | `gesture` is sent only for a gesture no transaction is bound to on the output |
| W6.8 | `cupertino_shell_v1` | `get_dock`: the clause on the origin of windows entering the overview goes |
| W6.9 | `cupertino_window_manager_v1` | the strip leaves the server: the `strip` entry of `placement`, the rectangle of `set_placement`, `strip_pressed` and the strip half of `invalid_placement` go; the description says that a strip window is a claimed hole of the window manager's surface and that the server draws no strip |
| W6.9 | `cupertino_shell_v1` | `get_window_manager`: the window manager has a surface of its own for the strip, an ordinary layer surface it persists |
| W6.12 | `cupertino_shell_v1` | `get_keyboard_panels` (version 30) and the new interface `cupertino_keyboard_panels_v1`: the switcher's list, the picker's candidates and `accent_pick`, for the connection that holds `keyboard_panels` |
| W6.close | `cupertino_menu_bar_v1` | `session_agent` and `agent_trust` go with the process path (version 31): from W6.13 no client reads them |

- W5.5's edits add to what the server does; the messages W6.8 and W6.9 remove keep working until those tasks, as
  the server keeps drawing its own overview and strip until then.
- W5.2 builds the holds, the roles and the identity that these descriptions name; the descriptions follow in W5.5,
  the first task after it that owns the file.
- Window frames of `cupertino_window_manager_v1` are in global logical coordinates, as those of
  `cupertino_overview_v1` are: no edit.

## Decisions for approval

Each is the default this specification takes where the plan leaves the choice open. Approving the specification
approves them. A mark says what changed after review: *(changed)* for another default, *(extended)* for a default
that keeps its choice and gains a rule, no mark for a default that stands as it was reviewed.

1. **A third protocol file.** The overview component needs a role object of its own, since the dock holds the dock's.
   Default: `cupertino-overview-v1.xml`, with holes and transactions in place of the dock's overview frames, drags
   and workspace pictures. W4.10 generates bindings for three files.
2. **The `shell` schema.** *(extended)* Default: `components` (system file only), `off` and `alone` (either file). An
   array or a `processes` list, from a file written before every component ran in a host, is ignored with a logged
   warning, and a system file without the section or without `components` runs every component of the table. Rejected: one list of objects with a
   mode for each name, which lets a person's file restate the system's list.
3. **`alone` in a person's file.** Default: allowed, since it names no command and no role and costs memory only.
4. **Roles.** *(extended)* Default: `host`, `overview`, `windows`, `session_agent` and `consent` join the server's
   roles; the development folder's allow-list gains `overview` and `windows`, and `agent` never starts from it.
5. **Bus names.** Default: `org.cupertino.SessionAgent1` for `agent` and `org.cupertino.Dialogs1` for `dialogs`; the
   components without a service (`dock`, `desktop`, `windowmanager`, `overview`) own none.
6. **`osd`'s role for the switcher and the picker.** Left to the design note of W6.12, which adds it to the tables
   here.
7. **The development folder.** Default: each named component runs alone from `<path>/cupertino-shell`, so one
   restart command runs one rebuilt component. Rejected: a development build of the shared host, which would put
   `agent` in a build from the folder or need a second shared host.
8. **`cupertino-login` is started by the server**, not listed: the lock screen in a user session, the login screen in
   the login session. A person's file cannot turn it off. *(extended)* W5.5 makes the change and writes
   `greeter.json`.
9. **What counts as a failure.** *(extended)* Default: a panic, a loop that ends with an error, a loop that returns
   unasked and a loss to a hang all count towards D16's five. The run of an instance lost within 60 s of its start
   is left out of the 60 s, so a component that hangs at every start is stopped.
10. **`busy`.** *(changed)* Default: sent when the loop has been out of its wait for a whole check interval, 0.5 s,
    not at one check that catches it mid-turn; the server's only reaction is not to wait for the component.
11. **One shared host always.** Default: while any component runs there is a shared host, with an empty list when
    every component runs alone, because the shared host is the hub of the host bus.
12. **The bridge's trust.** *(changed)* Default: the server makes a socket pair for each separate host and passes one
    end to each side; a fresh pair follows a restart or a dropped bridge. No path, no token, no time limit on the
    first frame. The shared host checks the component and the sender of every frame and alone publishes on the
    `Host` topic. Rejected: a socket in `$XDG_RUNTIME_DIR` with a one-time token.
13. **Who a bus caller is.** *(changed)* Default: a component proves its bus connection with a nonce the server
    sends on its Wayland connection and it returns through `Session1.AnnounceComponent`; a name is one component's.
    The server answers which component a unique name is (`ComponentOf`), from the accepted names. `ShellHosts`
    (process ids and start times) says only whether a caller is a shell host. The server is the owner of
    `org.cupertino.Session1`. The programs outside the host are known by their process. Rejected: resolving a
    component by the well-known names its connection owns.
14. **Holds.** *(extended)* Default: a surface is persistent by `persist` with a key; a held surface's animations are
    at their end at once; a held surface keeps its holes and claims, and a window manager's placements are held with
    its holds; the surface that takes a hold over replaces all of them in one frame; holds that nothing took over
    end on `ready`, or 10 s after the connection was handed out.
15. **The latch.** *(changed)* Default: `stage` keeps whole commits and adds later ones, with the commits of the
    surface's layer surfaces; frame callbacks of kept commits are answered on the output's frame clock; a kept
    buffer that a later kept commit replaces is released; a staged surface that has not committed 100 ms after
    `submit` leaves its part; window changes join by a bracket around the role objects' own requests, one open
    bracket for a connection. Rejected: a second set of animation requests on the transaction, which repeats
    `cupertino_layers_v1`.
16. **Naming a transaction.** *(changed)* Default: the creator's component, the instance number the server gave
    that instance, and a serial it chose, so no round trip precedes the invitation and a restarted component's names
    never meet the ones before; a join waits 100 ms for a `create` that has not arrived, then starts on the clock of
    the output it names; joins count towards the 16 transactions of a connection.
17. **Gesture binding.** *(extended)* Default: the progress is linear in every property, one spring settles all of
    them, window changes are carried out at rest at 1, a reverse gesture is a second binding of the same transaction,
    and binding needs the `overview` role. A gesture belongs to the output under the pointer when it is claimed.
    Among several bindings for it, a reverse binding of a transaction at rest comes first, the last completed; then
    the transaction armed last.
18. **Cancellation.** Default: before the start the parts are applied separately, never dropped, so no surface's
    state differs from what its client believes; after the start animations stop where they are shown.
19. **Holes.** *(extended)* Default: a hole is a whole layer, fitted and centred; a third source, `workspace`, serves
    the overview's workspace pictures; one surface shows at most 64 holes, the layer protocol's limit for a parent.
    The claims of a window stack: the latest is in force, the ones below are suspended and come back in order when
    it ends.
20. **Tiling.** *(extended)* Default: the server's window model decides tiled frames; the window manager component
    decides the stage and reaches windows through placements, claims and `activate_window`. The edits of
    `cupertino-shell-v1.xml` belong to W5.5, W6.8 and W6.9
    ([Edits of the existing protocol files](#edits-of-the-existing-protocol-files)).
21. **The bus catalogue.** *(changed)* Default: the eight messages listed; panels, do not disturb, the level display
    and the session menu's state go over the bus between components, and their session bus interfaces stay for other
    programs. `SessionMenu` from `agent` takes the place of the menu bar's reads of `org.cupertino.Login1` and of
    the process the server named for it. The host alone publishes on the `Host` topic.
22. **Where the host's mechanisms live.** *(extended)* Default: thread creation, sockets, the bridge and the host
    protocol's client are `cupertino_kit_shell::hosting`, because a `shell` crate may start no thread;
    `cupertino-shell` holds the policy and its constants. `cupertino-layers` lets `cupertino-shell` alone name
    `hosting`.
23. **Limits of the protocols.** Default: one module of named constants in `cupertino-protocols`, as
    `cupertino_app_protocols::layers` is for the layer protocol.
24. **Versions.** *(changed)* Default: every change of a message raises its interface's version; no interface
    changes without it. A host binds only the versions it was built with: a host of the development folder that
    finds another gives its component back to the installed host.
