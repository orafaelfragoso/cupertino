# Building and running

## Requirements

- Rust 1.100.0, which `rust-toolchain.toml` pins (`rustup` installs it on first use). `sccache` is the compiler wrapper; the default linker is used.
- OpenGL ES drivers (Mesa) and a Wayland session to run nested in. A TTY session needs logind and a DRM/KMS device.
- A Wayland terminal such as `foot` to start inside the session.
- Some crates link system libraries (for example the session manager links libpam); each crate's README lists what it needs.

Build output is large: leave tens of GiB free before starting.

## Development builds without tests

For a local build or binary replacement, build only the package and binary being changed, without enabling every
workspace test feature:

```bash
cargo build --locked -p cupertino-portal --bin cupertino-portal
cargo build --locked -p cupertino-server --bin cupertino-server
```

These commands compile the selected binary and its dependencies, without test harnesses, examples or benchmarks.
Ordinary `cargo build` does not compile test harnesses either. Do not add `--all-targets`, `--tests` or `cargo test` to
a build-only operation. `--all-features` alone does not select test targets, but can enable optional test-support
dependencies, so prefer the production features needed by the package.

`cargo build-programs` builds every program of the workspace in release mode (an alias for `cargo build --locked
--release --workspace --all-features --bins`).

Validate changes separately with `cargo xtask ci --profile agent`, which checks the crates you changed (see
[Tests](#tests)).

Keep the same profile and build environment during an active development task so Cargo can reuse unchanged
artifacts. Switching between dev and release, or starting from a fresh target, forces more compilation. Incremental
compilation is disabled by this repository's configuration; shared downloads are not a shared compiled target.

## Installing a local build

On Arch Linux, `makepkg -si` in `packaging/arch/cupertino` builds the packages of the checkout (release, no test
suite) and installs them:

```bash
cd packaging/arch/cupertino
makepkg -si
```

Install the three packages together, so the server, the shell processes, the applications, the theme and the assets
come from one commit. A change to the server or to what the session manager links takes effect at the next login; a
shell process takes it with `cupertino shell restart NAME`. Leave nothing in the development folder that the package
replaces: the server starts a folder file only when it is no older than the installed program.

## Installed-session iteration

When developing inside Cupertino, choose the smallest change that can demonstrate the fix. Rebuilding the workspace
and restarting the session are separate operations; neither is the default for a change to one application.

| Changed component | Development loop | Effect on the running session |
| --- | --- | --- |
| A setting supported by the live watcher | Change the setting and observe its applied value | No process restart; `shell`, `screenshot` and menu-bar geometry remain startup settings |
| Files or Settings | Regular applications: build the binary (Files with `--features app`) and run that build directly; no installed file is replaced | Quit the installed copy first when the change concerns its single instance; preserve navigation and pending work |
| Dock, menu bar, launcher, notifications, control panel, OSD or window manager | The development folder loop below, as the session's own user | A short interruption of that component; transient state can be lost (overview/switcher, queued notifications, window placement, open panels) |
| Portal | Build only `cupertino-portal`; test it nested or headless and leave it for a planned package update; after the update, `cupertino shell restart portal` | Restarting the portal ends its active screen-sharing sessions and outstanding requests |
| Login, lock screen, authentication prompts, desktop control | Use private backends, a nested session or a VM before a planned package update; after the update, `cupertino shell restart NAME` (`login`, `polkit-agent`, `keyring-prompter`, `desktop-control`) | They never start from the development folder; a restart loses what the process held in memory (an open prompt, every desktop-control run) |
| Session manager, network/Bluetooth/power daemons | Use private backends, a nested session or a VM before a planned package update | These can affect authentication, connectivity or device state; the session manager takes a new package at the next login, the daemons through their system service units |
| Server, its linked libraries or vendored Smithay | Build the server and exercise it nested or headless | A new server binary takes effect in the installed desktop at its next session start |
| A library shared by applications | Rebuild the consuming binaries needed to test the change | Rust libraries are linked into each consumer; copying a library alone does not update them |

### The development folder

Root declares the folder once, and from then on the session's user tries builds of the eligible shell processes
without root:

```bash
sudo cupertino dev-folder enable                      # ~/.local/lib/cupertino/dev, every eligible process
sudo cupertino dev-folder enable --components dock,menubar
```

This sets the `dev_folder` key of `/etc/cupertino/compositor.json` (see [The shell processes](#the-shell-processes))
and creates the folder with mode 0700 for the user who ran `sudo`. Eligible processes are those whose roles are all
among `session`, `menu_bar`, `dock` and `window_manager`: in the packaged configuration `dock`, `menubar`, `launcher`,
`notifications`, `controlcenter`, `osd`, `desktop` (Files showing the desktop's items, program `cupertino-files`) and
`windowmanager`. `login` (the lock screen and the session agent), the portal, the authentication prompts, desktop control, a data-control client and the server itself are never eligible.
A server that supports the key reads it again when the file changes; it applies from each process's next start.
`sudo cupertino dev-folder disable` removes the key and leaves the folder's files in place.

At every start of an eligible process the server looks for `<folder>/<file name of its installed program>`, for
example `~/.local/lib/cupertino/dev/cupertino-dock`. It starts that file, with the same arguments, environment and
roles, only when it is a regular file (not a link) owned by the user or root that neither its group nor others can
write and that the user can execute, in a folder with the same owner and permission rules; otherwise it starts the
installed program and logs why. Its log names the program and the source of every start.

### The loop

1. Find the component's name and program with `cupertino shell list`: the `NAME` column is the name the commands
   take, `installed` is the program, and `dev_program` (in `--format json`) is the file the folder would provide. The
   crate is the program's name: `cupertino-dock` is `crates/cupertino-dock`.
2. Build it with Cargo with its production features, the binary alone. A build that passes stays running (step 6),
   so build it as the package does: the release profile and the installed theme directory (without
   `CUPERTINO_THEME_DIR` the binary reads the theme from the checkout it was built in).

   ```bash
   CUPERTINO_THEME_DIR=/usr/share/cupertino/theme \
     cargo build --release --locked -p cupertino-dock --bin cupertino-dock --features app
   ```

   Validate the change separately with `cargo xtask ci --profile agent`, as for any implementation change. The build must speak the
   running server's protocols: a change that also needs a new server waits for the next session start.
3. Copy it into the folder by a staged write and an atomic rename, never by truncating a file a process may be
   running:

   ```bash
   dev=~/.local/lib/cupertino/dev
   install -m 0755 target/release/cupertino-dock "$dev/.cupertino-dock.new"
   mv -f "$dev/.cupertino-dock.new" "$dev/cupertino-dock"
   ```

   Record the source commit, any uncommitted changes and the file's hash (`sha256sum`).
4. `cupertino shell restart dock`. The server ends the process (SIGTERM, then SIGKILL after 5 s), starts it again at
   once whatever its exit status, and the command prints the new pid, its start time, the program and whether it is
   the development folder's. `cupertino shell list` shows the same at any time. A start that fails is reported, and
   the server keeps retrying after its throttle; going back (step 6) is the rollback.
5. Verify the changed interaction in the session through desktop control (the `cupertino-desktop` skill) or by hand,
   and confirm that the server and the other processes stayed alive. Defer a restart while the component has active
   work or a screen share.
6. A build that passes its live check stays running from the folder; record the component, the source commit and
   the hash with it. `cupertino shell use-installed dock` removes the folder's file and restarts the process on its
   installed program: run it at once when the candidate fails, and once an installed package contains the change.

Only the user's own session runs the folder's files: the server checks them against the user it runs as, so another
account's session, root's files aside, never starts them. A package update wins over the folder: the server starts a
file of the folder only when it was last modified no earlier than the installed program, whose package files keep their
build time, so a build left behind stops starting once a newer package is installed. `cupertino shell list` marks such a file
`development folder (older than installed)` (`dev_program_older_than_installed` in JSON) when it was last modified
before the installed program, whose package files keep their build time, and `cupertino diagnose` warns about it
(`shell.dev_folder`) with the `use-installed` command to run.

After a package install, a change that lies only in a supervised process (a name `cupertino shell list` shows, the
built-in `portal`, `polkit-agent`, `keyring-prompter` and `desktop-control` included) takes effect with `cupertino shell restart NAME`,
deferred while the process is in use. Only a change to the server, to what it links or to the session manager needs
a new login.

### Components that are not eligible

The server, the login process, the session manager, the authentication agent, the portal, desktop control and the
system daemons are not swapped in a running session. Build them, exercise them nested or headless (`cupertino desktop
start --backend nested|headless`) or in a VM, and take them into the installed desktop through
a planned package update: then `cupertino shell restart NAME` for the processes the server supervises (`login`,
`portal`, `polkit-agent`, `keyring-prompter`, `desktop-control`), a new login for the server and the session manager. When a manual replacement is unavoidable for an investigation, follow
the same identity rules as above: find the process through the server's runtime record under
`$XDG_RUNTIME_DIR/cupertino/server-<socket>.json`, its parent and start time, never by name alone (another nested
session may run the same program); keep a rollback copy, stage the new executable beside the installed one and rename
it into place with the required owner and mode, and record it as a local override of a package-owned file to
reconcile at the next package update. Never redirect the live desktop to a worktree or enable `--dev-shell` there.

## Desktop automation development strategy

The first objective is a shorter observe, reproduce, change and verify loop for the
[desktop quality backlog](desktop-quality.md). The provider service and voice features in
[Phase 7](roadmap.md#phase-7-ai-and-voice) are a separate roadmap item. Terra 6.1 should use desktop tools provided
by an in-tree Rust adapter; the compositor owns desktop policy and input routing, without an LLM client or provider
credentials.

### Implementation status

The dependency-free `cupertino-control-model` owns grant state and aggregate input ownership. Private protocol
version 1 stays discovery-only. Version 2 implements explicit output-scoped observation, capture, pointer and
keyboard grants. Version 4 asks the person once per app and remembers the answer. Version 5 lets the controller hold
a run for each agent it serves, on its one connection.

Each request comes from an app. The controller names the process on the other end of its local socket (the agent's
bridge) by process id and start time; the compositor checks the start time and walks up to 32 parents to the first
connected display client. An application client whose windows name one desktop entry makes that entry the app, so a
terminal holds the permission for the agents run inside it; otherwise the app is the executable of the bridge's
parent. A requester whose process is gone or was replaced is refused as unidentified. Observe, semantic actions,
pointer and keyboard need the app's Desktop Control service; capture needs its Screen Capture service. The answers
live in `desktop-control.conf`, which Settings › Privacy edits and the compositor watches. A service turned off
refuses the request at once (`denied`), with no panel. All needed services turned on start the run at once, with no
panel and no consent authority needed. Otherwise Cupertino's own Kit consent panel names the app (its name and icon,
or the program's name and path) and the services, starts with Don't Allow focused and mentions Settings › Privacy.
Allow and Don't Allow need a fresh physical-input serial delivered to the registered consent surface after the
request, and are remembered for the services asked; closing the panel answers this run only. An app whose name a
line cannot hold, or a file that cannot be written, gets an answer for this run only, which is logged. A request that
needs the person while no consent panel is connected is refused with `no_authority`. Turning a service off or
removing the app in Settings revokes a run that uses it; turning one on starts nothing. Injected input cannot
approve consent, reach session/authentication surfaces or invoke reserved session shortcuts or menu-bar power
actions.

The person and the run use the desktop at the same time: physical input never pauses or revokes a run, and each
source's presses are its own, so a key or button both hold is released only when the last of them lets go. The
authentication prompt, or a protected session panel taking the keyboard, suspends the run; it resumes by itself when
that closes, and `resume` only resumes it sooner, never asking again. The consent panel closes after the decision;
no permanent status panel is shown, and the menu bar's capture indicator shows while an active run may capture.
Ctrl+Alt+Escape stops control without changing what the person decided. Lock, session deactivation, loss of the
controller and local peer exit revoke the run and release only its owned input; after the unlock the next request
of an allowed app starts at once. Loss of the consent panel revokes only a run waiting for its answer. Capture
excludes consent surfaces and authentication prompts. Capture sources belong to one approved epoch (the controller
selects its run for each source it creates), and denied or revoked sources cannot acquire a later grant.

Several agents work at once, each through its own bridge and run. Each run has its own epoch, app, phase, refusals,
geometry revision, captures and waits, and its requests act on its own epoch only. Their input interleaves like
input from separate devices: each run's held keys and buttons are its own, released when that run ends, without
touching another run's or the person's. A stop, a revocation or a bridge leaving ends that run alone. Runs whose app
needs the person's answer are asked one at a time on the consent panel, in the order they asked; a later one waits
in the awaiting-consent phase until its turn. The compositor allows eight sessions on the controller's connection
(`MAX_RUNS_PER_CONTROLLER` in `cupertino-control-model`), the controller serves as many local connections, and a further
connection is answered at once with an error saying the controller is full, which the bridge reports. Geometry revisions reject stale input coordinates. Bounded outcome history
suppresses duplicate mutations; evicted outcomes are unknown and never replayed.

The compositor supervises the in-tree Rust `cupertino-control --serve` process on its private connection. Its socket
is `$XDG_RUNTIME_DIR/cupertino/control-WAYLAND_DISPLAY.sock`, mode 0600 in a private directory. Each local connection
gets a fresh inactive run and negotiates `{"op":"hello","version":1}`. The controller checks the same-user peer and
holds its exact process descriptor: a copied socket cannot preserve a dead peer's grant. Commands, queues, capture
allocations and deadlines are bounded per peer: each peer has one command in flight, and a second command before the
response disconnects it. The controller's loop never waits on one peer: replies, phase waits and capture frames are
pending operations with deadlines, multiplexed on the Wayland connection and the peers' wakeups, and each peer's
responses are written (and its capture encoded) on a thread of its own. The controller sleeps on native
transport/accept wakeups when idle.

In a development session, pass `--dev-shell --control-program /absolute/path/to/cupertino-control` to the new server
and run the matching login process with its existing Session and SessionLock roles. Run
`cupertino-control --session WAYLAND_DISPLAY` and send sequential JSON lines on stdin; it negotiates the handshake.
Keep that process connected across commands: closing it ends its run. Each invocation is its own run, and an app
the person allowed starts again at once:

```json
{"op":"request","output":"headless-1","capabilities":15}
{"op":"wait","phase":2,"timeout_ms":30000}
{"op":"move","x":200,"y":200}
{"op":"button","code":272,"pressed":true}
{"op":"button","code":272,"pressed":false}
{"op":"key","code":30,"pressed":true}
{"op":"key","code":30,"pressed":false}
{"op":"scroll","horizontal":0,"vertical":40}
{"op":"capture"}
{"op":"capture","region":{"x":100,"y":80,"width":320,"height":200}}
{"op":"stop"}
```

A request may be repeated: while the run awaits consent, is active or is suspended on the same output with no more
capabilities, it answers with that run as it is. Another output or more capabilities ends that run, releasing its
input, and the controller starts a new one on a fresh session, asking the person only for services the app is not yet
allowed. Once the run ended, for whatever reason, the next request starts a fresh run with a new epoch the same way,
without a `stop` first. The compositor keeps an ended run ended: a start on its session is refused with `stale_epoch`,
after `denied` (a service the person turned off) and before `busy`.

Capability bits are observe 1, capture 2, pointer 4, keyboard 8 and semantic actions 16. Semantic actions also require observe. Keys use evdev codes; coordinates and scrolling
use logical points relative to the approved output. Capture returns an upright PNG as base64 with its geometry: without a
region, the whole output at one image pixel per logical point (downscaled with a sharp cubic filter, never enlarged);
with `region` (logical points relative to the output, entirely inside it, else refused), that rectangle at the output's
device pixels. The result's `width` and `height` are the image's pixels and `region` the logical rectangle it covers,
its edges moved outward to whole device pixels: image pixel `(px, py)` is the logical point
`(region.x + px * region.width / width, region.y + py * region.height / height)`. A frame the compositor did not deliver
upright is refused rather than mis-mapped. The hello reply's `capture_regions: true` says the controller takes regions; a
controller from before it ignores the field and returns the whole output, so the bridge does not send it a region.
`status` reports epoch and phase: inactive 0, consent 1, active 2, suspended 3, revoked 4. A suspended run resumes by
itself; `resume` never asks again. A refusal names its reason; `denied` points the person to Settings › Privacy ›
Desktop Control or Screen Capture. The local protocol stays at version 3 for older bridges; the controller names its
peer to the compositor itself.
Closing the local connection ends its run. The local protocol accepts typed operations, not shell commands or
arbitrary file paths.

Version 3 adds Kit semantics and a Rust MCP stdio bridge in the same executable. Public
`cupertino_semantics_v1` publishers may describe only their own Wayland surfaces; they gain no control authority.
The compositor requests snapshots only while a matching output has an active observe grant. Lock, protected
surfaces, authentication prompts and the consent authority are excluded. Kit exports its existing runtime tree:
stable generational node IDs, roles, names, states, logical bounds, supported actions and list/table positions.
Password values carry only a protected marker and expose no actions. Text is bounded at UTF-8 boundaries and
explicitly marked when truncated. Complete publication replacement removes old nodes; surface/window destruction
invalidates its handles. Third-party windows have compositor metadata with no Kit snapshot.

Native `observe` returns output-scoped windows and complete publication revisions.
`action` takes `window`, `revision`, `node`, `action` and optional typed `data`.
Both the compositor and the owning Kit runtime recheck the live revision and action before dispatch.
Completion follows the runtime update and publication; uncertain/time-limited outcomes require observation before
retry. No action is replayed automatically. Version 3 also invalidates input coordinate revisions when published
state or visible window layout changes. Optional `revision` on local pointer requests enforces the caller's token.

Start the bridge with an explicit selected display:

```bash
cupertino-control --mcp "$WAYLAND_DISPLAY"
```

Omit the display argument to select the host's `WAYLAND_DISPLAY`; a missing environment value is an error,
so it never attaches to an arbitrary runtime socket. This mode lets an installed MCP registration follow the next login.

The bridge implements MCP stdio versions 2025-06-18 and 2025-11-25 (initialization), plus
[2026-07-28 per-request discovery](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning).
It exposes twelve tools: status, request_control, observe, action, capture, move, button, key, scroll, wait, resume
and stop. Status lists output names without exposing their content. Request only the needed capabilities, await
phase 2 (at once for an app the person allowed, after the native panel otherwise), and pass the resulting epoch to
each subsequent call. request_control may be repeated as the local request may; the bridge refuses a call naming an
epoch other than its current run's, or a run that ended (except `wait` and `stop`), before anything reaches it. After a denial the agent tells the person where to turn the app on instead of asking again. Observe accepts window, exact role and name
substring filters, reports truncation, and supplies output-relative bounds. Opaque identifiers and publication
revisions are decimal strings, preserving all 64 bits. Geometry revisions remain numbers. Action returns its
outcome and a fresh observation; capture returns PNG image content with geometry and the logical region the image
covers (the whole output at logical size, or an optional region at device pixels), without duplicating base64 in
text; an image over 16 MiB is refused with a request for a smaller region. Cancellation and stdin EOF close the native lease and release owned input. Queues, messages, images,
publication sizes, waits and stalled stdout writes have explicit bounds. Tool discovery works offline.

The checkout registers the installed controller for agent hosts working in it, and only there:
`.codex/config.toml` for the [Codex MCP host](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) (loaded once
the checkout is a trusted project) and `.mcp.json` for Claude Code (approved once when it asks). Both run
`/usr/bin/cupertino-control --mcp` and pass the host's `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` through, so the
bridge follows the session the host runs in. Do not add a user-wide registration as well. Check the result from the
checkout with `codex mcp get cupertino` or `claude mcp get cupertino`.

For a development candidate, use a stable staged binary path outside the disposable build target.
Choose `gpt-6.1-terra` in the host; this bridge requires no API client or new credential.
A host connection refresh is separate from restarting Cupertino. Host MCP tool approval and native desktop consent
are separate gates. The project Codex configuration approves this one server's tools
(`default_tools_approval_mode = "approve"`), which a host with approval policy `never` needs; the compositor still
asks the person natively the first time the app requests control.
Check the real Counter and Forms examples through the shipped Rust MCP adapter, the supervised controller and the
native Kit consent panel on a private headless session: button state changes, Unicode field updates,
stale-action/coordinate rejection, protected values, image content and cancellation.

Window-scoped grants, CLI registry convenience commands and deployment automation remain future work.
No external control framework, MCP SDK or third-party runtime dependency is added. Server changes take effect at
the next planned installed-session start. The current installed compositor cannot acquire a new protocol without
that activation; builds and isolated tests do not restart it.

### Dependency boundary

The control implementation belongs to Cupertino. Add no external desktop-control library, accessibility framework,
MCP SDK or new third-party runtime dependency. In particular, the initial implementation does not use libei/libeis,
a Python or Node automation service, or a second toolkit. Existing utility dependencies remain as they are; this
work does not remove the dependencies already in the repository.

Use the existing Wayland transport and scanner, Smithay inside the server, Rust's standard library for local process
and socket handling, and the serialization and image support the affected crates already use. MCP and EIS are
protocols, not requirements to link another implementation. MCP's small local stdio interface can be implemented
in Rust. EIS/RemoteDesktop interoperability can be considered later as an in-tree protocol implementation if needed;
it is not a prerequisite for Terra to operate Cupertino.

### Where it lives

| Area | What it does |
| --- | --- |
| `cupertino-control-model` | Grant states, epochs, scope and capability narrowing, the app a request comes from, the remembered answers, and the ownership of held keys and buttons per input source, without I/O |
| `cupertino-protocols` | The private `cupertino_control_v1` and `cupertino_control_authority_v1` protocols |
| `cupertino-semantics`, `cupertino-app-protocols` | Bounded semantic snapshots and actions, and the public `cupertino_semantics_v1` through which a Kit application publishes only its own surfaces |
| `cupertino-server` | The `DesktopControl` role (the role set is 16 bits wide), grants checked on every request, accepted input routed through the existing handlers, capture checked per frame, the controller's held input released when its run ends |
| `cupertino-shell-client` | Binds the control and consent protocols for the controller and the session agent |
| `cupertino-control` | The supervised controller: the local socket, bounded requests, capture, and the MCP stdio bridge |
| `cupertino-login`, `cupertino-menubar`, `cupertino-settings` | The consent panel, the capture indicator, and the Privacy pages that edit `desktop-control.conf` |
| Contract registry and packaging | The globals, the executable and its install path, under the trusted-path rules of the built-in processes |

The public app transport stays free of private shell protocols, and the semantic wire types sit below both the server
and the toolkit: the server does not depend on `cupertino-ui-core`. Publishing semantics grants an application no
observation of other applications and no input injection. Still to do: window-scoped grants, `cupertino` commands over
the same operations, and a preview updater in `cupertino dev` that builds, stages, restarts, verifies and rolls back a
candidate as [Installed-session iteration](#installed-session-iteration) describes.

Native input necessarily changes compositor source. A portal or standalone helper cannot add that capability to an
already running server. Keep development on private nested/headless sessions until the complete loop passes; deploy
the required server change together at the next planned session start. Registering MCP tools may require refreshing
the host connection, independently of the Cupertino session.

### Observation and action contract

Return a bounded snapshot with a session identifier, process/window generation, observation revision, focused
window, output geometry/scale/transform, visible nodes and their supported actions. Use handles scoped to the
application's current process and window generation; reject stale targets after a restart, closed window or incompatible
tree update. Select by stable identifier or role and name, and report ambiguity instead of choosing an arbitrary match.

Subscribe to semantic updates only for consented targets while the run needs them. Publish complete revisions with
explicit removals and window closure; a paged snapshot must never combine pages from different revisions. Ending the
subscription removes the automation overhead from an idle Kit application.

Images must identify the captured output/window, pixel dimensions, crop origin and the transform between image pixels,
window-local logical points and desktop coordinates. This matters on a 4K display at scale 2 and on mixed-scale outputs.
Keep a screenshot's revision with a pointer action; if the target moved, observe again. Prefer semantic actions for
routine navigation and real input for interactions whose behavior is under test.

Wait for state or presentation events with deadlines instead of fixed sleeps. Return whether an action was queued,
applied and verified, with the resulting observation or an explicit timeout. After an uncertain result, inspect before
retrying a non-idempotent action. Keep the model outside timing-sensitive loops: a deterministic Rust runner should
measure key-repeat delay, menu latency and pointer sequences, then return the trace and result.

Bind automation to the selected session and permitted applications, with visible stop control and server-side
lock-state handling. The controller has no grant at startup; a Cupertino-owned session interaction grants scoped
control, not an arbitrary program discovering a socket. Revocation and lock invalidate the grant, including capture,
and require a fresh grant. Existing authentication prompts must remain hidden from capture and unavailable to control.
The controller's crash must leave the desktop usable.

### Protocol and authorization lifecycle

Use a private `cupertino_control_v1` manager and a run-scoped session object, with XML-generated client/server
bindings in `cupertino-protocols`. Protocol discovery is not consent: only the supervised control process sees
the manager, and it begins with no observation, capture or mutation grant. A normal application cannot acquire a
control role from its configuration, executable name, MCP connection or public Wayland socket.

A desktop-control role does not inherit unrestricted Capture or Prompt privileges. Extend capture authorization for
this role with grant checks on creation and every in-flight frame; do not give the controller the portal's existing
unconditional capture role. Keep existing portal behavior intact. The input role cannot operate authentication or
consent surfaces, including through semantic actions.

The controller requests consent through the existing supervised session agent, which presents a Kit dialog for a
specific output or window and capabilities. The session agent receives picker metadata over its own authorized
connection; ungranted content is not forwarded to the controller or MCP. The controller is never its own consent
authority. The compositor creates the grant only from a fresh genuine-user interaction on that consent surface;
injected input and semantic actions cannot authorize, extend or confirm their own grant. Bind the grant to the
controller's live process handle, private Wayland connection, local MCP connection and one run epoch. A run never
outlives process restart, reconnect, lock or logout; what persists is the person's answer for the app, in
`desktop-control.conf`, which lets the app's next run start without the panel. A reserved physical shortcut revokes
control even when the controller is stalled, and leaves the answers as they are. The menu bar shows the capture
indicator while a run may capture; the reserved compositor shortcut must remain available when the menu bar or
session agent is unresponsive. Loss of the grant authority revokes only runs waiting for its answer.

The states are `inactive -> awaiting-consent -> active`, or `inactive -> active` when the app's remembered answers
allow every service it needs, with `active -> suspended -> active` while the authentication prompt or a protected
session panel is up, and `active|suspended -> revoked` for termination. Denial or cancellation returns to inactive.
Lock, session deactivation/VT switch, controller or local-tool disconnect, explicit stop and a service the person
turned off move active or suspended to revoked before any further action is handled. Showing an authentication
prompt suspends control and input delivery and invalidates pending operations; the run resumes by itself after it
closes. A new run after revocation asks the person again only when the app's answer is not remembered. Fixture tests receive a test-owned grant on a private connection, never through a
production flag that makes every client privileged.

| Operation family | Required behavior |
| --- | --- |
| Session/capability discovery | Identify protocol version, actual supported operations and current grant state; disclose no ungranted desktop content |
| Observe | Return a coherent bounded page of permitted output/window geometry and semantic state, tagged with run epoch and revision |
| Capture | Use the existing image-copy protocol with grant-aware authorization; return dimensions, source, mapping and frame metadata |
| Pointer/button/axis/key | Validate scope, state, input ownership and geometry revision, then use normal routing with compositor timestamps |
| Semantic action | Validate the live node, current advertised action and protected state, then route only to its owning application's surface |
| Wait/cancel | Wait on a defined state/presentation condition with a deadline; cancellation stops pending work and reports completed effects |
| Revoke/close | Stop reads and mutation, cancel pending work, release owned input, and invalidate every handle in the run |

Use monotonically increasing request IDs within a run and explicit accepted, completed, refused and cancelled results.
An input acknowledgement establishes dispatch only. Application or presentation observations establish the effect;
the protocol must not claim a button worked merely because it delivered a press. Before applying a batch, validate
every event; if execution later stops, report the completed prefix, never an all-or-nothing result for partial effects.
Duplicate mutation IDs return their bounded cached outcome without replay. An expired outcome returns an explicit
unknown-outcome refusal and requires observation, rather than guessing whether an action happened.

Every queue, message, batch, semantic page, image allocation and outstanding request set has a named size limit.
Bound processing per event-loop turn and apply backpressure outside the compositor. A slow MCP reader cannot hold
a compositor borrow, block its main loop or accumulate unbounded screenshots. Wayland protocol errors close malformed
connections; expected conditions such as a moved window, missing target, locked session or unsupported action return
typed refusals so the controller can observe and recover.

### Local host connection

The supervised Rust control process owns the private Wayland connection. Its separate stdio MCP adapter connects to a
local socket under the selected session's private runtime directory; that socket grants no authority by itself.
Validate peer credentials and bind each proposed run to the actual peer process/connection. Select a live compositor
by socket, PID and start identity, not an arbitrary first runtime record. A connection handshake checks the wire
version and reports the server/controller versions before any grant request.

Expose human/JSON commands through the existing `cupertino desktop` registry and the MCP transport through a dedicated
adapter mode of the in-tree control executable. Both use the same typed operation schema, capability descriptions
and validators. The MCP mode owns stdout directly; it must not run inside a CLI result wrapper that appends human
help, JSON envelopes or end records to the protocol stream.

Keep local IPC and MCP as separate transports over one typed operation model. The local IPC uses bounded framing;
MCP uses newline-delimited JSON-RPC and the negotiated MCP version. Handle initialize, initialized, ping, tools/list,
tools/call, cancellation, malformed input and clean shutdown explicitly. Images are returned as MCP image content;
raw image payloads never enter text logs. No HTTP listener, externally hosted service or API credential is needed
for this local bridge.

Existing Codex model authentication remains with the host. Installing or reconnecting an MCP adapter does not enable
the host's disabled built-in Computer API; the owned desktop tools are the integration surface. Verify the actual
registered tools in this installation before calling the integration complete. If its host needs a tool-connection
refresh, that is separate from restarting the desktop.

### Input ownership and text

Calling the existing input functions alone is insufficient. The current keyboard handler tracks aggregate pressed
keys, so the server needs a source ledger before controller disconnect cleanup can preserve a physical key held by
the person. Track physical and injected presses separately, send aggregate transitions to the existing router, and
remember which target received each press. The person's input never pauses or revokes a run: both sources press
and release at once, and the aggregate release goes out only when the last owner lets go. Cancel or release
controller-owned state before transferring focus to a lock or authentication surface; cleanup must not send a
synthetic release into the newly protected surface.

Define key codes explicitly as Linux evdev codes on the wire, converting once at the existing keyboard boundary.
Use compositor-generated monotonic timestamps and preserve event order. A pointer target is in output/window logical
coordinates tagged with that geometry's revision; the compositor maps it to current desktop space. Reject stale
geometry and targets outside the grant, rather than silently clicking a different window or clamping into one.
Reserved session/power/revocation actions remain unavailable to injected input.

Text has two distinct operations: a supported semantic `SetValue` for an unprotected Kit field, or actual key events
through the existing keyboard layout for an input-path test or third-party client. Report text that cannot be
represented by the supported path; do not silently change the person's keyboard layout or clipboard to type it.
Native text-input support can be negotiated separately. Do not expose clipboard access or application launching
implicitly through the initial control grant.

### Acceptance and evidence

Test protocol and model code without a display, transport code on private connections, and the compositor with
fixture homes, assets and buses. Include malformed/oversized messages, unsupported versions, wrong roles,
stale handles, lock/revoke/disconnect, held-input cleanup, output removal, rotated/mixed-scale coordinates,
and pointer delivery to both native and XWayland applications. Check a slow or stalled client cannot block the
compositor loop. Run `cargo xtask ci` at the level the change needs, and the packaging build, before deployment.

Start regression scenarios with DQ-30 (discover a newly installed fixture desktop entry), DQ-05 (repeat-delay
direction), DQ-01 (sidebar bounds) and DQ-32 (current status-menu IDs). Use real pointer paths for DQ-03, DQ-06,
DQ-19 and DQ-20; semantic invocation alone cannot prove click routing, long-press timing or click-away behavior.

Each scenario should emit a compact evidence bundle: issue ID, source revision and diff, exact binary hashes and
features, running server identity, reproduction steps, state assertions, screenshots where useful, input trace,
diagnostic events and elapsed build/test/update time. A visual pass needs pixels; a functional pass needs an observed
state change. Neither a model's completion message nor a headless fixture closes an installed-session acceptance gate.

## Try it in one minute (nested)

Run the compositor as a window inside your current Wayland session:

```bash
git clone https://github.com/orafaelfragoso/cupertino.git
cd cupertino
cargo run --release -p cupertino-server -- --winit --spawn foot
```

From a checkout, the apps' desktop entries (`crates/*/data/applications`) are not installed, so the menu bar and the dock show
"Cupertino-files" and a generic icon instead of "Files" and its icon. Add the crates' data directories to the search path
(keep the host's directories after them):

```bash
export XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
```

Installed packages put the entries in `/usr/share/applications`; nothing to set there.

The compositor prints `WAYLAND_DISPLAY=<socket>` once it listens; start more clients with that variable set. `Super+Return` starts a terminal (the default terminal through `xdg-terminal-exec` or `$TERMINAL`, else the first existing of `foot`, `alacritty`, `kitty`, `wezterm`, `xterm`; configurable with `terminals`). `Ctrl+Alt+Backspace` quits.

Without a pack of your own, text, icons and cursors come from the built-in fallbacks (see [Asset packs](asset-packs.md)); a missing asset never stops a start.

## The whole desktop on a TTY

From a text console (switch with `Ctrl+Alt+F3`), a checkout can start the compositor on the display with the shell processes of the packaged configuration (dock, menu bar, launcher, notifications, control panel, OSD, the desktop's items, window manager and lock screen), Files and Settings. Build the programs first, then start the server on a fresh session bus:

```bash
cargo build-programs
target/release/cupertino-server --check-hardware
dbus-run-session -- target/release/cupertino-server --udev --dev-shell --run-for 600 --spawn cupertino-files --spawn cupertino-settings 2>&1 | tee ~/cupertino-tty.log
```

`--dev-shell` lets a configuration file name the shell processes from any path, so give the server a shell list whose programs are the build's (a copy of `packaging/files/etc/cupertino/compositor.json` with the paths changed). Do not run it inside a desktop session. `XDG_RUNTIME_DIR` must be set, which a console login does. Always give `--run-for`: a hung server on a console without a limit leaves the keyboard muted.

- `Ctrl+Alt+Backspace` quits.
- `Ctrl+Alt+F<n>` returns to the console session you came from.

The TTY backend is in daily use on real hardware with an NVIDIA GPU and one display, and runs in virtual machines. Other GPUs and setups with several displays are less tested.

## Captures

The server renders the full desktop once headless and writes a PNG:

```bash
cargo run --release -p cupertino-server -- --headless --size 1280x800 --screenshot out.png --exit-after-screenshot --spawn cupertino-files
```

Run it with an empty environment and a temporary home folder when the picture must not depend on the machine.

One surface of Files, the toolkit or the login screens is drawn with the app's own painter on sample data, without a display, a session bus or the server, by the capture example of its crate:

```bash
cargo run -p cupertino-files --example files_capture -- files/sidebar --drop-place Desktop --increase-contrast --sheet
cargo run -p cupertino-kit --example kit_capture -- kit/controls --scale 2
```

It writes one PNG per state and appearance into `captures` under `$CARGO_TARGET_DIR`, else under the workspace's `target` (`--out` changes it) and prints each path. The flags, the file names and the surfaces are in `crates/cupertino-capture/README.md`. It needs an asset pack and exits 2 without one.

## The shell processes

The dock, menu bar, launcher, notifications and the other shell parts are separate processes. Each but the menu bar builds its binary behind an `app` feature:

```bash
cargo build --release -p cupertino-dock --features app
cargo build --release -p cupertino-menubar
```

The server starts and restarts the processes listed under the `"shell"` key of `/etc/cupertino/compositor.json` (the packages install one that starts the whole desktop), for example `[{"name": "dock", "command": ["/usr/bin/cupertino-dock"], "roles": ["session", "dock"]}]`. A process that has roles must be a program under `/usr`; the user file `~/.config/cupertino/compositor.json` can remove or reorder the entries but not add a process with roles, and a checkout or nested run that needs other programs starts the server with `--dev-shell`. A process that exits with failure is restarted at most once per second. The optional `roles` list is what the server lets the process do on its connection: `session`, `menu_bar`, `dock`, `window_manager`, `session_lock`, `clipboard` (data control without an entry in the person's `data-control.conf`); the default is `["session"]`, and a process asks for no role it does not hold. The roles end with the process.

The system file may also declare a development folder, `"dev_folder": {"path": "/home/me/.local/lib/cupertino/dev", "components": ["dock", "menubar"]}`: an absolute directory and the names of the processes that may start from it (`sudo cupertino dev-folder enable` writes it). Only the system file sets it; the user file can neither declare nor widen it. A name whose process holds a role other than `session`, `menu_bar`, `dock` or `window_manager`, or that names no process of the shell list, is ignored with a warning, and the built-in processes never start from it. At every start of a named process the server starts `<path>/<file name of its program>` instead of the installed program when that file passes its checks ([Installed-session iteration](#the-development-folder)). `cupertino shell list` shows what each process runs, `cupertino shell restart NAME` ends one and starts it again at once, and `cupertino shell use-installed NAME` goes back to the installed program. Without the key everything starts from its installed program.

## Backends

| Flag | Backend |
|---|---|
| `--winit` | nested window in a Wayland or X session (development) |
| `--dev-shell` | let any configuration file name the shell processes and the screenshot process, from any path; for checkouts and nested runs only |
| `--udev` | TTY: logind, DRM/KMS and evdev |
| `--headless` | no display; use with `--screenshot out.png --exit-after-screenshot` to render one frame |

Without a flag the server runs nested when `WAYLAND_DISPLAY` or `DISPLAY` is set and on the TTY otherwise. Other useful flags: `--size WxH`, `--scale F`, `--appearance light|dark`, `--spawn "cmd args"` (repeatable), `--run-for S`. `cargo run -p cupertino-server -- --help` lists them all, and the server's README describes each.

The TTY backend is in daily use on real hardware with an NVIDIA GPU and one display, and runs in virtual machines. Other GPUs and setups with several displays are less tested.

## Tests

`cargo xtask ci` is the one validation command. It plans from git, runs formatting, clippy and nextest and the doc tests
on the crates that need them, and writes `target/xtask/ci-latest.json`:

```bash
cargo xtask ci --profile agent                 # the crates you changed
cargo xtask ci --profile integration --base B  # changed crates and their reverse dependencies, the layer check, the gate
cargo xtask ci --profile pr                    # what CI runs on every push
cargo xtask ci --profile full                  # the whole workspace, every feature configuration
cargo xtask ci --profile agent --dry-run       # the plan only
```

The rules and the feature configurations are in `.config/validation.toml`: `--profile agent` for the crates a change touches, `integration` after merging, `pr` in CI on every push and `full` nightly.
Install [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`). It runs every test in its own
process and kills a test that runs longer than 2 minutes, so a hang fails with the test's name. Shell and app crates
with an `app` feature need it to test their process code; the profiles pass the features of each configuration.

Tests live only in `crates/*/tests/` and go through a crate's public interface. The layer rules are checked by
`cargo run -p cupertino-layers`.

### Instruction-count benchmarks

The functions a frame spends its time in have benchmarks counted in instructions, not timed: each runs once under
Valgrind's Callgrind, so its count is the same on a loaded machine as on an idle one, and a rise of a fraction of a
percent shows. They are library benchmarks of the [gungraun](https://crates.io/crates/gungraun) crate, one bench target
named `instructions` per crate (`benches/instructions.rs`), with what each measures in its header:

| Crate | What is counted |
|---|---|
| `cupertino-raster-math` | The blur's plan and kernel for a 4K output, the blur of a 4K backdrop, the coverage of a continuous-corner rounded rectangle and the dithered store over a panel's pixels at 2x |
| `cupertino-glass` | One glass sample of the CPU model: inside, on the edge, in a corner, in the neck between joined shapes |
| `cupertino-ui-layout` | A file manager window's solve (about 480 nodes): cold, cached, resized |
| `cupertino-ui-render` | A 1280 × 800 window with text, a drop shadow and a blur at 2x: its first frame, a whole repaint, one row's damage |
| `cupertino-ui-text` | 64 file names shaped and laid out, as a list and as an icon grid shows them, and laid out again |
| `cupertino-launcher` | The index of 5 000 applications, seven queries, the scorer alone |

```bash
cargo bench -p cupertino-raster-math --bench instructions
cargo bench -p cupertino-glass --bench instructions -- --save-baseline=main   # keep the counts as the baseline `main`
cargo bench -p cupertino-glass --bench instructions -- --baseline=main        # compare with it
```

It needs Valgrind (`pacman -S valgrind`). The framework's runner, `gungraun-runner`, must be the library's version in
`Cargo.lock` exactly (`cargo install gungraun-runner --version VERSION --locked`). Counts and baselines are kept under
`target/gungraun` with the Callgrind profile of each benchmark, which `callgrind_annotate` reads. The setup a benchmark
names (building the tree, the scene, the index) is not counted. The wall-clock benches beside them (`cargo bench -p
cupertino-ui-layout --bench layout`, `cargo bench -p cupertino-launcher --bench matching` and the other crates' `cargo
bench`) stay, for the time a call takes.

Counts move a little with the hash seeds of a run (under 0.1% here), so a change has to rise well past that to count. A
change to a function these benches cover comes with its benchmark; a new hot function gets one in its crate's
`benches/instructions.rs`.

### Frame timelines

`CUPERTINO_PROFILE` records where each frame's time goes, as spans on a timeline, in every process started with it.
A slow frame then shows which part of it grew, without adding timing prints. The spans:

- the toolkit's run loop: each turn (`ui.turn`); its update (`ui.update`: the wake-ups and fades, each platform event
  by its kind, the tasks run, each message by its type); `ui.view`; and per window (`ui.window`) its layout, paint,
  raster per layer (`content`, `raised`, the surface layers) and present; accessibility and semantics; each idle slice;
- the GPU renderer: `gpu.render` per layer with its prepare, plan, record, submit and image uploads, and the programs
  made; the CPU renderer: `cpu.raster` per layer with its plan, scroll layers and paint;
- the Wayland presenter: lease (with allocations), copy, commit, submit and the fence wait or finish, and the GPU
  warm-up's device, program turn, programs and their first draws (`gpu.programs.prime`), on the warm-up thread's
  track;
- the compositor: each output's frame (`server.output_frame`), its scene, its render (lower, supersample, draw) with the
  glass, material, blur, content blur, surface and shrink passes inside, the flip submitted, the GPU wait of a timed
  headless frame, and on the text console the late start after a vblank (`server.latch_wait`) on a track of its own.

| Value | What is recorded |
|---|---|
| `chrome:DIR/` | each process's spans in `DIR/<program>-<pid>.json` (Trace Event format), which [Perfetto](https://ui.perfetto.dev) opens; the files' times are the monotonic clock's, so their events concatenated into one array open as one timeline with a track per process |
| `chrome:FILE` | one process's spans in `FILE` |
| `tracy` | the spans streamed to the Tracy profiler, in a build with its client (`crates/cupertino-diagnostic-events/README.md`) |

```sh
CUPERTINO_PROFILE=chrome:/tmp/trace/ cargo run -p cupertino-server -- --headless --screenshot /tmp/files.png --exit-after-screenshot --spawn cupertino-files
CUPERTINO_PROFILE=chrome:/tmp/trace/ cargo run -p cupertino-server -- --spawn cupertino-files
```

The server passes the variable to every process it starts. A file is written as the process runs, at most 100 ms
behind, and closed when the process exits; a process killed first (the shell processes when the server ends) leaves it
without the closing `]`, which Perfetto reads. Unset, no layer is installed and a span costs its callsite's check that
nothing listens, about a nanosecond. The frame timing file (`CUPERTINO_FRAME_TIMING`, the phases of each frame) is
apart and unchanged.

## Profiling and performance

Before adding timing code to find out where time goes, profile.

| Question | Tool | Command |
|---|---|---|
| Which functions take the CPU time of a program | perf | `perf record --call-graph fp -- target/profiling/PROGRAM`, then `perf report` |
| The same, to browse as a call tree, flame graph or timeline | samply | `samply record target/profiling/PROGRAM`, then the browser opens it |
| What allocates, how often, and what the heap holds at its peak | heaptrack | `heaptrack target/profiling/PROGRAM`, then `heaptrack_gui FILE` |
| Which part of a slow frame grew, on a timeline across every process | frame timelines | `CUPERTINO_PROFILE=chrome:/tmp/trace/ cargo run -p cupertino-server -- ...`, then open the files in Perfetto ([Frame timelines](#frame-timelines)) |
| Whether a change made a hot function slower, in instructions | Callgrind | `cargo bench -p CRATE --bench instructions` ([Instruction-count benchmarks](#instruction-count-benchmarks)) |

Build the profiling build first: `RUSTFLAGS="-C force-frame-pointers=yes" cargo build --profile profiling -p CRATE
--bin PROGRAM --all-features --target-dir target/profiling-fp`. The `profiling` Cargo profile is the release build with
full debug info and no stripping; the separate target folder keeps the release build's cache untouched, and the binary
is `target/profiling-fp/profiling/PROGRAM`.

perf samples user space only (`kernel.perf_event_paranoid` 2 allows no more), with frame-pointer call chains. To follow
a whole session, start the server under the profiler: every process it starts is followed, and `perf report -i FILE
--pid PID` shows one process. heaptrack follows no child process, so run each program on its own under it. A profiler
must start the program: attaching to a running process is not allowed here (`kernel.yama.ptrace_scope` 1).

How to read a CPU profile:

- Threads: each thread's share of the process's samples, so a render or warm-up thread shows apart from the main loop.
- Self time: the functions the samples were in, in percent of the process's samples; a hot loop shows here. A library
  without symbols (the GPU driver) is one summed row, `(code without symbols)`.
- Inclusive time: a function and everything it calls; the phase that is slow shows here (a toolkit's update or
  raster, the compositor's render). A call chain stops at a library built without frame pointers, so a caller's share
  is a lower bound.

How to read a heap profile: the peak heap and peak RSS (RSS includes the tool's own memory), then the allocation sites
by number of calls (temporary allocations in a hot path show here) and by bytes held at the heap's peak (what fills
memory). A site is the innermost frame outside the allocator and the standard containers, with the two frames above it.

## Configuration

Plain files under `~/.config/cupertino/`, each with an environment override: `shortcuts.conf` (chord to action lines), `compositor.json`, `keyboard.conf`, `dock.conf`, `menubar.conf`. The server's README lists every file and key.
