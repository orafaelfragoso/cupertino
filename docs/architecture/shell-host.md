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

## Contents

1. [Processes and connections](#processes-and-connections)
2. [The component list](#the-component-list)
3. [The host protocol](#the-host-protocol)
4. [Transactions](#transactions)
5. [`cupertino-kit-shell`](#cupertino-kit-shell)
6. [Decisions for approval](#decisions-for-approval)

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
| `osd` | `cupertino-osd` | on-screen displays, the input-source switcher, the accent picker | `session`, and what the design note of W6.12 adds | `org.cupertino.Osd1` | none |
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
| `components` | names of the table: the components of the session | sets the list | refused, with a logged problem |
| `off` | names of the table: components that do not run | applies | applies |
| `alone` | names of the table: components that each run in a separate host (D18) | applies; the packaged file leaves it empty | applies |
| `processes` *(until W6.close)* | `[{"name", "command", "roles"}]`: supervised processes that are not components, with the rules the list has before the host exists | names processes with roles, under an install root | removes or reorders entries, adds none with roles |

- The components that run are `components` minus `off`. A name that is not in the table is a logged problem and is
  left out; the other names apply.
- `off` and `alone` of the person's file take the place of the system file's, key by key. Neither file names a
  command, a role or a bus name: `shell` holds names of the table and nothing else.
- Which file is the system file comes from the layer, as for every privileged key: `/etc/cupertino/compositor.json`
  and `/etc/cupertino/greeter.json`, named exactly.
- The section is live. When a file changes, the server works out the components of each host again and sends each
  host its list. A component turned off is stopped and its holds end; one turned on starts; one that enters or leaves
  `alone` stops in the host it ran in and starts in the other, its persistent surfaces held in between.
- A system file without a `shell` section runs no component, and the server says so in its log.
- *(until W6.close)* A name is a component (`components`) or a process (`processes`), never both: a process entry
  whose name is in `components` is refused with a logged problem. Each component's own task moves its name.

### Development builds

`dev_folder` keeps its form, `{"path": "/absolute/dir", "components": ["dock", "menubar"]}`, and its rule that only
the system file sets it.

- `components` names components of the table. A named component runs in a separate host that the server starts from
  `<path>/cupertino-shell --only NAME`, when that file passes the folder's checks at that start: a regular file, not a
  link, in a directory, both owned by the user or root and writable by no group or other user, executable, and not
  older than the installed `cupertino-shell`.
- When the file is absent or refused, the component runs as the rest of the configuration says, in the installed
  host; a refusal is logged with its reason.
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
  the server asked for (`restart`, an empty list of a separate host) is not counted.
- The other programs keep the restart policy of the supervised processes: restarted after a failure with the same
  throttle, left stopped after a successful exit.
- `cupertino-audiod` is started by the bus at first use, never by the server.
- *(until W6.14)* `polkit-agent` and `keyring-prompter` are started in the place of `prompts`, each with the `prompt`
  role, and the consent authority is offered to the `session_lock` role.

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
- The login screen draws layer surfaces through `cupertino-kit-shell` and has no lock surface: nothing is locked.
- Every other key of `compositor.json` is valid in the file. A `dev_folder` key names no component that runs, so it
  has no effect.

## The host protocol

### Start

1. The server starts the shared host when the first component is due, on a connection that holds the `host` role.
2. The host binds `cupertino_host_v1` and gets `kind`, then its list: a `component` event for each name, then `done`.
3. For each name the host calls `get_component` and `connect`. The server answers `connection` with a descriptor:
   the client end of a connection that holds the component's roles. The host starts the component's thread on it.
4. The component binds `cupertino_component_v1`, gets its name and its holds, announces its bus name, persists its
   persistent surfaces before their first buffer, and sends `ready` once it shows what it shows at start.
5. A list is sent again in whole whenever it changes. The host stops a component that left the list, then destroys
   its object; it asks for the connection of one that joined.

A host that was built without a listed component answers `unavailable`: the server logs it and leaves the component
stopped in that host.

### Identity

- A component opens its own connection to the session bus, owns its well-known names there, and sends
  `announce_bus_name` with the connection's unique name on its Wayland connection.
- The server asks the bus daemon for the process behind that name and accepts it when it is the host process the
  component's connection was handed to. The answer is `bus_name`, `accepted` or `refused`.
- From then on `org.cupertino.Session1` judges a caller by component: a call from the announced name of `agent` is the
  session agent's; a call from another connection of the same process is not.
- `cupertino-peer` resolves a bus caller for every other service in two steps: the caller's process, which must be
  a host (`ShellHosts` of `org.cupertino.Session1` lists the hosts' processes), and the component, the one whose
  well-known names of the table the caller's connection owns. A component in a separate host resolves the same way.
  A caller that is a host and owns no name of the table resolves to no component.
- No check compares process relations. Component sockets are close-on-exec: a program a component starts inherits
  neither its Wayland connection nor its bus connection.

### Failures and restarts

A component fails when it panics (its own thread, one of its tasks or a helper thread), when its loop ends with an
error, or when its loop returns without being asked. The host tears it down inside `catch_unwind`, shuts its sockets,
and sends `failed` and then `connect`.

| Rule (D16) | Value | Kept by |
|---|---|---|
| First restart after a failure | 0.25 s | `cupertino-server` |
| Each further failure | the time doubles, up to 8 s | `cupertino-server` |
| The time starts over | after the component ran 60 s | `cupertino-server` |
| The component stops | five failures within 60 s | `cupertino-server` |
| A stopped component starts again | `cupertino shell restart NAME`, or the next login | `cupertino-server` |

- The server keeps the counts, so they outlive the host. It answers `connect` with `connection` when the time has
  passed, or with `stopped` when it gave up.
- On `stopped` the host publishes `ComponentStopped` on the host bus, and the notifications component tells the
  person.
- `cupertino shell restart NAME` of a running component reaches its host as `restart` on the component's object: the
  host stops the component and sends `connect`. It is not a failure, and the command is answered when the component
  sends `ready`.

### Hangs

A component's loop records when it leaves its wait and clears the record when it returns. The host's watchdog reads
the records every 0.5 s, after a start-up grace of 10 s for each component.

| The loop has been out of its wait | The host sends | The server |
|---|---|---|
| at one check | `busy` | stops waiting for the component: a transaction that expects it starts without it |
| for 2 s | `unresponsive` | releases its grabs, dismisses its transient surfaces, returns the focus to the window that had it, sends it no input |
| and returns | `responsive` | sends it input again |
| for 10 s | `lost` | holds its persistent surfaces, takes it out of the host's list, starts it alone in a separate host |

- Before `lost` the host shuts the component's sockets down with `shutdown(2)` on descriptors it keeps for that
  purpose. It closes the component's descriptors only when the thread has ended, so none is reused under it.
- After `lost` a shared host sends `request_restart`. The server answers `restart` once the seat has had no input for
  2 s and no component of the host holds a grab or shows a transient surface; the host stops its components and
  exits, and the server starts it again at once. That exit is not counted as a crash.
- The lost component stays in its separate host for the rest of the session, or until `cupertino shell restart NAME`
  puts it back in the shared host.
- The render thread reports to the same watchdog. A render thread out of its wait for 10 s is a hang of the whole
  process: the host exits with failure and the server restarts it.

### Holds

A surface is transient unless the component persists it (`persist`, with a key that is the same for every instance
of the component and names the surface on its output).

| When | Persistent surfaces | Transient surfaces |
|---|---|---|
| The component is reported unresponsive | keep their place and their live picture, without input | dismissed at once; focus returns to the window that had it |
| Its connection ends (failure, restart, lost, host exit) | held: the last picture and the reserved space stay, every animation at its end, without input | dismissed at once; their holes go and each claimed window returns to its frame along the server's return motion |
| The component is declared stopped (`stopped`, turned off) | the holds end: the picture goes and the space is given back | |

What a restarted component takes over:

- A surface it persists with the key of a hold on the same output takes the hold over. With that surface's first
  buffer, in the same frame, the held picture goes and the surface shows in its place; the reserved space passes to
  it with no change of the usable area in between, so no window moves and nothing flashes.
- Nothing else. Role objects are made afresh and the server sends each feed from its start; what the instance before
  knew is gone unless the component stored it (D21). The component hears which keys are held (`hold`), so it knows it
  takes over and shows its surfaces without their opening motion.
- `ready` ends every hold no surface took over. Without it the server ends them 10 s after it handed the connection
  out.

### Separate hosts

- A separate host runs one component. The server starts one for a component in `alone`, for a component of the
  development folder, and for a component that was lost.
- After three crashes of the shared host within 60 s, the server starts every component in a separate host for the
  rest of the session, so an abort, a stack overflow, an exhausted memory or a driver fault costs one component.
- While any component runs there is one shared host, also when its list is empty: it serves the host bus.
- A separate host whose list becomes empty stops its component and exits successfully; the server does not start it
  again.

### The host bus and its bridge

Components talk to each other through the host bus only (D26): typed messages of `cupertino-kit-shell`, published
and heard, never awaited.

- Inside a host the bus is in memory: a publication goes to the queue of every component that follows its topic and
  wakes that component's loop.
- The shared host serves the bus on a Unix stream socket in `$XDG_RUNTIME_DIR` (mode 0600), a path of its choosing
  that it removes when it exits, and tells the server with `serve_bus`.
- For each separate host the server draws a token of 128 random bits and sends it to both sides: `bus_peer` to the
  shared host, `bus` with the path to the separate host. The separate host connects and presents the token; the
  shared host accepts each token once and closes a connection that presents none within 1 s. `bus_peer_gone` voids a
  token and closes its connection.
- When the shared host starts again it serves another socket; the server sends every separate host the path and a
  fresh token. Messages a separate host publishes meanwhile wait in it, the newest 256 kept.

| Bridge rule | Value |
|---|---|
| Frame | a `u32` little-endian length, then JSON with a `type` tag; unknown tags and fields are errors |
| Largest frame | 64 KiB |
| First frame from the separate host | `{"type": "hello", "version": 1, "component": NAME, "token": TOKEN}` |
| Answer | `{"type": "welcome", "version": 1}`, then every retained message, or the connection closes |
| Afterwards, both ways | `{"type": "message", "from": NAME, "message": MESSAGE}` |

The shared host forwards a message from a bridge to its own components and to the other bridges, and never back to
its sender. A retained message stays with the bus until its publisher stops ([The host bus](#the-host-bus)).

## Transactions

### Parts

A transaction has a creator and any number of other participants, each a component connection with a
`cupertino_transaction_v1` object of its own. It is named by its creator's component and a serial the creator chose,
so the creator publishes the name on the host bus in the same turn that creates it, and another component joins by
name.

A participant's part is made of two things:

| Part | How it joins | What it carries |
|---|---|---|
| Staged surfaces | `stage(surface)`: the surface's commits are kept from then on | the buffer and damage; the values, animations and looks of its layers and its presentation (`cupertino_layers_v1`); the state of its holes |
| Window changes | the role requests between `begin_window_changes` and `end_window_changes` | the server's own changes of windows, with the animations it runs for them |

- Motion is described once. An animation in a kept commit is a target with the numbers of a timed curve, a spring or
  samples; the toolkit takes the numbers from the catalogue's `Timing` (D7) and the server evaluates them with
  `cupertino-animation`. Nothing is sent per frame.
- Kept commits of one surface add up in order, so the surface's state never goes backwards. Their frame callbacks are
  answered as if they had been applied, so a component's frame pace does not wait for the transaction.
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
  transaction of its own. A join within 10 s after the transaction ended applies with every animation at its end.

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
- `bind_reverse_gesture(gesture)` keeps a completed transaction for a second gesture, which drives the same
  properties from their values at rest back to the values they had when the first gesture began. Reverted, the
  transaction is over. The binding lasts until the creator destroys its object.
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
  A hole that claims in the commit that first shows it, with its layer at the window's frame, takes the window over
  with no change on screen. Claims need the `overview` or the `window_manager` role. The latest claim of a window is
  in force.
- A claim that the component ends with a commit puts the window at its frame from that commit. A claim that ends any
  other way returns the window from where the hole showed it to its frame along the server's return motion.
- A parent surface holds at most 64 layers and a connection 256 (`cupertino_layers_v1`), so one surface shows at
  most 64 holes.

The compositor draws the stage strip's and the overview's windows and the dock's minimized windows only as holes.
The overview's windows are claimed holes of the overview surface. A window in the stage strip is a claimed hole of
the window manager's surface, and a window of a group the strip does not show is placed `hidden`.

### The plan's requirements

| # | Requirement | Met by |
|---|---|---|
| 1 | One clock | the start time is a presentation time of the transaction's output; every animation counts from it |
| 2 | Several participants, one frame | parts are kept and applied together on the start frame; the server's window changes are a part |
| 3 | Motion described, not streamed | animations travel as `cupertino_layers_v1` curves, springs and samples in kept commits |
| 4 | Deadline; late parts join at the original start time | [Start, deadline and late parts](#start-deadline-and-late-parts) |
| 5 | Gesture binding | [Gestures](#gestures) |
| 6 | Holes | [Holes](#holes) |

### The plan's questions

| Question | Answer |
|---|---|
| A participant that dies mid-flight | What it had kept and not applied is dropped, and the transaction does not wait for it. On its held surfaces every animation is at its end at once; its transient surfaces are dismissed with their holes, and each claimed window returns to its frame along the server's return motion. The other parts go on. When the creator of a gesture-bound transaction dies, the binding ends and a gesture under way settles back to the start. |
| Cancellation | The creator's `cancel` ends the transaction for all. Before the start each part is applied at once, by itself, as ordinary commits and requests; a participant that wants its change undone commits the opposite first. After the start every animation stops at the value shown on the next frame and its client hears `replaced`; window changes carried out stay. A gesture under way settles back to its start. `cancel` or `destroy` from another participant withdraws that participant's part alone. Every participant hears `finished(cancelled)`. |
| Two transactions on one property | The animation of the later start time takes the property from the value and velocity shown at that start, and the earlier animation's client hears `replaced`. A late part never takes a property back from a transaction that started after its own. |
| Outputs with different clocks | A transaction has one output, named at `create`, and one start time. Another output shows the transaction on its first frame presented at or after the start time, each animation as far along as that frame's own presentation time says. When the output is gone or shows no frame, the start time is the server's clock at the moment the transaction was ready or its deadline passed. |
| How a transaction latches with surface commits | `stage` keeps the surface's commits whole (buffer, damage, layer, presentation and hole state, subsurfaces' cached state) until the participant's part is applied, later commits added to earlier ones. The part is applied on the start frame, or on the frame after the participant became ready when it is late. After that the surface commits as usual. |
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
| A join waits for its transaction | 100 ms |
| An ended transaction is remembered | 10 s |
| Transactions of a connection that have not ended | 16 |
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

### Modules

| Module | Holds |
|---|---|
| root | `Component`, `Program`, `ComponentName`, `Entry`, `Surface`, `Role`, `LayerSpec`, `Layer`, `Anchor`, `Margin`, `Keyboard`, `Hole`, `HoleSource`, `ServerWindowId`, `WorkspaceId`, `Event`, `Error`, `run`, `run_lock_screen`, `environment`, `holds` |
| `session` | the lock notice, the idle notice, `activate_app` |
| `outputs` | `OutputId`, `Output`, `Transform`, `all`, `get` |
| `design` | `MenuBar`, `menu_bar` |
| `menu_bar`, `dock`, `window_manager`, `overview` | each role's feed and requests |
| `transaction` | `Id`, `Transaction`, `Handle`, `Gesture`, `Outcome`, `Event`, `create`, `join` |
| `bus` | `Message` and its value types, `Topic`, `publish`, `follow`; `session` (the session bus connection) |
| `host` | `thread`, `Stop`, `spawn` |
| `lock` | `lock`, `unlock`, `Event` |
| `consent` | `bind`, `set_window`, `answer`, `stop`, `Request`, `App`, `Answer`, `Run`, `Event` |
| `capture` | `windows`, `once`, `follow`, `Source`, `Options`, `Window`, `Follow`, `Event` |
| `test_clock` | `display_unix_seconds`, `display_now`, `motion_settled` |
| `hosting` | the host program's side: `Host`, `HostEvent`, `Kind`, `Liveness`, `Failure`, `Connection`, `Runtime`, `Running`, `Ended` |

### Components and programs

```rust
/// A Kit application that shows its windows through the shell's roles.
pub trait Program: cupertino_kit::core::Application {
    /// How a window of `view` reaches the screen. Asked for every window after each `view`.
    fn surface(model: &Self::Model, window: &WindowKey) -> Surface;
    /// What the shell said, as a message of the program; `None` for what it does not follow.
    fn event(event: Event) -> Option<Self::Message>;
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
    /// The window or workspace of the hole on `layer` of `window` is gone.
    HoleClosed { window: WindowKey, layer: u64 },
    MenuBar(WindowKey, Box<menu_bar::Update>),
    Dock(WindowKey, dock::Update),
    WindowManager(OutputId, window_manager::Update),
    Overview(OutputId, overview::Update),
    Transaction(transaction::Event),
    Bus { from: ComponentName, message: bus::Message },
    /// The session bus accepted or refused the component's announced name.
    BusIdentity { accepted: bool },
    Lock(lock::Event),
    Consent(consent::Event),
    Capture(capture::Event),
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
- An `OutputId` is the same in every component of a session, so it travels in a bus message.

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
| `menu_bar` | the active application and its windows, its menus, the window's state and controls, shortcuts, the backdrop bands, the screen capture state, the session agent | `set_menu_open`, `window_command`, `activate_window`, `move_to_workspace`, `session_action`, `edit`, `force_quit_app` |
| `window_manager` | the usable area and the arranged windows with their frames; `ToggleStage` | `place(ServerWindowId, Placement)`, `commit`, `activate_window`; `Placement` is `Stage` or `Hidden` |
| `overview` | the area, the windows of the workspace with their frames, the workspaces; `Toggle { mode, app_id }` | `activate_window`, `switch_workspace`, `add_workspace`, `remove_workspace`, `move_workspace`, `move_window` |

- The types keep the names they have in `cupertino-shell-client` (`DockUpdate`, `DockChange`, `DockWindow`,
  `MenuBarUpdate`, `ActiveApp`, `AppWindow`, `Shortcuts`, `WindowCommand`, `EditAction`, `SessionAction` and the
  rest of that crate's list), inside their role's module. An output in a feed is an `OutputId`, and an image
  (a window's icon) is a `render::ImageResource`.
- A minimized window's picture is a hole (`HoleSource::Thumbnail`), not an image in the feed. The dock's overview
  messages and the window manager's strip placement have no function: the overview is `overview`, and the strip is
  holes.
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
  every persistent window of the first view has presented a frame.

### Transactions

```rust
pub mod transaction {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct Id { pub component: ComponentName, pub serial: u32 }

    /// Creates a transaction on the clock of `output`.
    pub fn create(output: OutputId) -> Transaction;
    /// Joins the transaction another component created.
    pub fn join(id: Id) -> Transaction;

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
- A creator tells the other participants with `bus::Message::Transaction { id, scene }` in the same update. A
  component that hears it calls `join(id)`, stages its window and submits.
- `Started::at` is on the clock of `platform::Timestamp`, so a component that draws something in step evaluates the
  same `Timing` from it.

### The host bus

```rust
pub mod bus {
    /// What components say to each other. Serialised with a `type` tag; unknown tags and fields are errors.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub enum Message {
        /// From the host: the server stopped `component` after repeated failures.
        ComponentStopped { component: ComponentName },
        /// Open the panel, or close it when it is open.
        TogglePanel { panel: Panel, output: Option<OutputId> },
        /// Retained. Whether a panel is shown, and where.
        PanelShown { panel: Panel, shown: bool, output: Option<OutputId> },
        SetDoNotDisturb(DoNotDisturb),
        /// Retained. Whether do not disturb is in effect, and the durations offered (identifier, title).
        DoNotDisturbChanged { on: bool, lifetimes: Vec<(String, String)> },
        /// Show a level without changing it.
        ShowOsd { kind: OsdKind, level: f64 },
        /// Join a transaction.
        Transaction { id: transaction::Id, scene: Scene },
    }
    pub enum Panel { ControlPanel, NotificationCenter, Launcher, Applications }
    pub enum DoNotDisturb { Off, On, For(String) }
    pub enum OsdKind { Volume, Mute, Brightness, KeyboardBrightness }
    pub enum Scene { OverviewOpening, OverviewClosing, StageSwitch }
    pub enum Topic { Host, Panels, DoNotDisturb, Osd, Transactions }

    impl Message {
        pub fn topic(&self) -> Topic;
        /// A retained message is kept by the bus and given to a component when it starts to follow the topic.
        pub fn retained(&self) -> bool;
    }

    /// Sends `message` to every component that follows its topic, the sender excepted.
    pub fn publish(message: Message);
    /// Follows `topics`: their messages arrive as `Event::Bus`, the retained ones at once.
    pub fn follow(topics: &[Topic]);

    /// The component's connection to the session bus, as `kit::bus::connect` gives one, on a socket the host can
    /// shut. Its unique name is announced to the server when it is made; the answer is `Event::BusIdentity`.
    pub fn session<M: Send + 'static>(
        connected: impl FnOnce(zbus::Result<zbus::Connection>) -> M + Send + 'static,
        closed: M,
    ) -> Vec<Command<M>>;
}
```

- A publication never waits and has no reply. A component that needs an answer follows the topic the answer is
  published on.
- The bus keeps the latest retained message of each publisher and variant (and of each `panel`), and drops it when
  its publisher stops.
- A queue holds 256 messages for a component; a component that does not empty its queue loses the oldest, and the
  host logs it once.
- Programs outside the hosts are reached through the session bus or the server's protocols, never through this bus.
- The catalogue is the seven messages above. A message joins it with a change of this file.

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
  not ended by then is kept like a lost component's thread, and a shared host asks for a restart.
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
        pub fn connect() -> Result<Host, Error>;
        /// Waits for events at most `timeout`. The host's main thread is no UI thread: it may wait.
        pub fn wait(&mut self, timeout: Duration) -> Result<Vec<HostEvent>, Error>;
        /// `get_component` and `connect`; `connect` again for a component that has its object.
        pub fn connect_component(&mut self, name: &str);
        pub fn unavailable(&mut self, name: &str);
        /// Destroys the component's object.
        pub fn release(&mut self, name: &str);
        pub fn report(&mut self, component: ComponentName, state: Liveness);
        pub fn failed(&mut self, component: ComponentName, cause: Failure);
        pub fn request_restart(&mut self);
    }
    pub enum Kind { Shared, Separate }
    pub enum Liveness { Busy, Unresponsive, Responsive, Lost }
    pub enum Failure { Panic, Error, Exit }
    pub enum HostEvent {
        Kind(Kind),
        /// A whole list, as sent.
        Components(Vec<String>),
        Connection { component: ComponentName, connection: Connection },
        Stopped { component: ComponentName },
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
        /// A shared host serves the bus and tells the server; a separate one joins it when the server says where.
        pub fn for_host(host: &mut Host, kind: Kind, environment: Environment) -> Result<Runtime, Error>;
        /// Starts `entry` on a thread of its own (8 MiB stack) on `connection`.
        pub fn start(&self, entry: &'static Entry, connection: Connection) -> Running;
        /// Publishes on the host bus as the host.
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
        /// Shuts its Wayland and bus sockets down (`shutdown(2)`); the descriptors close when the thread has ended.
        pub fn shut(&self);
        /// How it ended, once its thread and its helper threads have.
        pub fn ended(&mut self) -> Option<Ended>;
    }
    pub enum Ended { Stopped, Failed(Failure) }
}
```

- `Runtime` handles the bridge by itself: `serve_bus`, `bus`, `bus_peer` and `bus_peer_gone` never reach the host's
  code.
- A panic in a component's thread, task or helper is caught at the thread's top; the state it leaves is dropped
  inside `catch_unwind`, and a second panic there leaks it. `ended` then gives `Failed(Failure::Panic)`.
- The host's constants (check every 0.5 s, grace 10 s, unresponsive 2 s, lost 10 s, helper join 1 s) are named in
  `cupertino-shell`; the restart and hold constants in `cupertino-server`.

## Decisions for approval

Each is the default this specification takes where the plan leaves the choice open. Approving the specification
approves them.

1. **A third protocol file.** The overview component needs a role object of its own, since the dock holds the dock's.
   Default: `cupertino-overview-v1.xml`, with holes and transactions in place of the dock's overview frames, drags
   and workspace pictures. W4.10 generates bindings for three files.
2. **The `shell` schema.** Default: `components` (system file only), `off` and `alone` (either file), and `processes`
   until W6.close for the processes that are not components yet. Rejected: one list of objects with a mode for each
   name, which lets a person's file restate the system's list.
3. **`alone` in a person's file.** Default: allowed, since it names no command and no role and costs memory only.
4. **Roles.** Default: `host`, `overview`, `windows`, `session_agent` and `consent` join the server's roles; the
   development folder's allow-list gains `overview` and `windows`, and `agent` never starts from it.
5. **Bus names.** Default: `org.cupertino.SessionAgent1` for `agent` and `org.cupertino.Dialogs1` for `dialogs`; the
   components without a service (`dock`, `desktop`, `windowmanager`, `overview`) own none.
6. **`osd`'s role for the switcher and the picker.** Left to the design note of W6.12, which adds it to the tables
   here.
7. **The development folder.** Default: each named component runs alone from `<path>/cupertino-shell`, so one
   restart command runs one rebuilt component. Rejected: a development build of the shared host, which would put
   `agent` in a build from the folder or need a second shared host.
8. **`cupertino-login` is started by the server**, not listed: the lock screen in a user session, the login screen in
   the login session. A person's file cannot turn it off.
9. **What counts as a failure.** Default: a panic, a loop that ends with an error and a loop that returns unasked all
   count towards D16's five.
10. **`busy`.** Default: sent at the first watchdog check that finds the loop out of its wait; the server's only
    reaction is not to wait for the component.
11. **One shared host always.** Default: while any component runs there is a shared host, with an empty list when
    every component runs alone, because D26 has the shared host serve the bus.
12. **The bridge's trust.** Default: D26's socket in `$XDG_RUNTIME_DIR`, with a one-time token the server gives both
    sides. Alternative: the server hands both hosts the two ends of a socket pair, which needs no path and no token
    and changes D26's wording.
13. **How `cupertino-peer` finds the hosts.** Default: `ShellHosts` of `org.cupertino.Session1`, the hosts' process
    ids and start times.
14. **Holds.** Default: a surface is persistent by `persist` with a key; a held surface's animations are at their end
    at once; holds that nothing took over end on `ready`, or 10 s after the connection was handed out.
15. **The latch.** Default: `stage` keeps whole commits and adds later ones; frame callbacks of kept commits are
    answered at once; window changes join by a bracket around the role objects' own requests. Rejected: a second set of
    animation requests on the transaction, which repeats `cupertino_layers_v1`.
16. **Naming a transaction.** Default: the creator's component and a serial it chose, so no round trip precedes the
    invitation; a join waits 100 ms for a `create` that has not arrived.
17. **Gesture binding.** Default: the progress is linear in every property, one spring settles all of them, window
    changes are carried out at rest at 1, a reverse gesture is a second binding of the same transaction, and binding
    needs the `overview` role.
18. **Cancellation.** Default: before the start the parts are applied separately, never dropped, so no surface's
    state differs from what its client believes; after the start animations stop where they are shown.
19. **Holes.** Default: a hole is a whole layer, fitted and centred; claims are a hole's, the latest in force; a
    third source, `workspace`, serves the overview's workspace pictures; one surface shows at most 64 holes, the
    layer protocol's limit for a parent.
20. **Tiling.** Default: the server's window model decides tiled frames; the window manager component decides the
    stage and reaches windows through placements, claims and `activate_window`.
21. **The bus catalogue.** Default: the seven messages listed; panels, do not disturb and the level display go over
    the bus between components, and their session bus interfaces stay for other programs.
22. **Where the host's mechanisms live.** Default: thread creation, sockets, the bridge and the host protocol's
    client are `cupertino_kit_shell::hosting`, because a `shell` crate may start no thread; `cupertino-shell` holds
    the policy and its constants.
23. **Limits of the two protocols.** Default: one module of named constants in `cupertino-protocols`, as
    `cupertino_app_protocols::layers` is for the layer protocol.
