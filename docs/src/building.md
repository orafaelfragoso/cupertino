# Building and running

## Requirements

- A current stable Rust toolchain (`rustup` or your distribution's package).
- OpenGL ES drivers (Mesa) and a Wayland session to run nested in. A TTY session needs logind and a DRM/KMS device.
- A Wayland terminal such as `foot` to start inside the session.
- Some crates link system libraries (for example the session manager links libpam); each crate's README lists what it needs.

Build output is large: leave tens of GiB free before starting.

## Development builds without tests

For a local build or binary replacement, build only the package and binary being changed. For example, the portal's
`app` feature includes its production screen-sharing dependency without enabling every workspace test feature:

```bash
cargo build --locked -p cupertino-portal --features app --bin cupertino-portal
cargo build --locked -p cupertino-server --bin cupertino-server
```

These commands compile the selected binary and its dependencies, without test harnesses, examples or benchmarks.
Ordinary `cargo build` does not compile test harnesses either. Do not add `--all-targets`, `--tests`, `cargo test`,
`cargo nextest`, or `ci/build-test-fixtures.py` to a build-only operation. The fixture script explicitly uses
`--all-targets` to prepare a test run; it is not a development build shortcut. `--all-features` alone does not select
test targets, but can enable optional test-support dependencies, so prefer the production features needed by the package.

Run regression tests separately when validating implementation changes. Scope them to the affected behavior and packages;
the workspace test and packaging gates remain separate checks. In particular, when using the fixture script for a
package-specific test run, pass its repeated `--package` options for all required fixture packages as well as selecting
the tests in the child command; a package filter only on nextest does not narrow the script's preparation.

Keep the same profile and build environment during an active development task so Cargo can reuse unchanged
artifacts. Switching between dev and release, or starting from a fresh target, forces more compilation. Incremental
compilation is disabled by this repository's configuration; shared downloads are not a shared compiled target.

## Installing a local build

On Arch Linux, `scripts/package-local.sh` builds the packages of the committed HEAD (release, no test suite) with a
release number from the current time, so each build replaces the installed one:

```bash
scripts/package-local.sh                       # writes ~/.cache/cupertino-pkg/cupertino-*.pkg.tar.zst
sudo pacman -U ~/.cache/cupertino-pkg/cupertino-*.pkg.tar.zst
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
| Files or Settings | Regular applications: build the binary with `--features app` and run that build directly; no installed file is replaced | Quit the installed copy first when the change concerns its single instance; preserve navigation and pending work |
| Dock, menu bar, launcher, notifications, control panel, OSD or window manager | The development folder loop below, as the session's own user | A short interruption of that component; transient state can be lost (overview/switcher, queued notifications, window placement, open panels) |
| Portal | Build only `cupertino-portal` with `--features app`; test it nested or headless and leave it for a planned package update; after the update, `cupertino shell restart portal` | Restarting the portal ends its active screen-sharing sessions and outstanding requests |
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

   Validate the change separately with the scoped tests, as for any implementation change. The build must speak the
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
start --backend nested|headless`, the integration tests) or in a VM, and take them into the installed desktop through
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
The opt-in model acceptance test scopes `default_tools_approval_mode = "approve"` to its isolated fixture server.

The real Counter and Forms examples are tested through the shipped Rust MCP adapter, supervised controller and
native Kit consent panel on private headless sessions. Those tests assert button state changes, Unicode field
updates, stale-action/coordinate rejection, protected values, image content and cancellation.
Run the optional model acceptance test only with an installed, authenticated Codex CLI:
set `CUPERTINO_TERRA_CODEX` to its absolute path and `CUPERTINO_TERRA_EVIDENCE` to a retained JSONL receipt path,
then use the fixture script with `cargo nextest run --run-ignored only -E 'test(terra_controls_kit)'`.
It selects `gpt-6.1-terra`, approves only its isolated MCP server, supplies genuine fixture backend consent,
and checks the actual semantic tool result after a single mutation, image capture and stop.
It uses the host's existing account, so it is excluded from ordinary CI.

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

Run protocol/model tests without a display, transport tests on private connections, and compositor integration tests
with fixture homes, assets and buses. Include malformed/oversized messages, unsupported versions, wrong roles,
stale handles, lock/revoke/disconnect, held-input cleanup, output removal, rotated/mixed-scale coordinates,
and pointer delivery to both native and XWayland applications. Check a slow or stalled client cannot block the
compositor loop. Run the applicable build, scoped regressions, doctests, clippy, format, layer, contract and packaging
checks before deployment.

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

From a text console (switch with `Ctrl+Alt+F3`), a checkout can start the full desktop — compositor, the shell processes of the packaged configuration (dock, menu bar, launcher, notifications, control panel, OSD, the desktop's items, window manager and lock screen), Files and Settings:

```bash
./run-cupertino.sh
```

The script refuses to run inside a desktop session (`WAYLAND_DISPLAY` or `DISPLAY` set). It builds the workspace's release binaries (`cargo build --release --workspace --all-features --bins`), runs `cupertino-server --check-hardware`, copies the packaged shell process list (`packaging/files/etc/cupertino/compositor.json`) to a temporary file with the programs taken from the build, starts a fresh session bus with `dbus-run-session`, and runs `cupertino-server --udev --dev-shell` with Files and Settings. The built-in processes (portal, authentication prompts, desktop control) start from their installed programs under `/usr/bin` when a package is installed, never from the checkout. It installs nothing and enables no service: the session manager and the login screen are not used. `XDG_RUNTIME_DIR` must be set, which a console login does.

- `Ctrl+Alt+Backspace` quits.
- `Ctrl+Alt+F<n>` returns to the console session you came from.
- The log is `~/cupertino-tty.log`; when the server exits with a non-zero status the script prints the last 20 lines.
- Server flags pass through: `./run-cupertino.sh --run-for 600` stops by itself after ten minutes.
- Without `--run-for` the run stops after `CUPERTINO_RUN_FOR` seconds (900 by default), and a watchdog ends the whole process tree 30 seconds later if the server hangs instead of stopping. No limit (`0`) needs `CUPERTINO_ALLOW_NO_TIMEOUT=1`: a hung server on a console without a limit leaves the keyboard muted.

The TTY backend has run in a virtual machine, not yet on real hardware. Treat it as experimental.

## Headless captures

`crates/cupertino-server/scripts/capture.sh` renders the full desktop once and writes a PNG:

```bash
crates/cupertino-server/scripts/capture.sh out.png --spawn cupertino-files
AFTER=6 crates/cupertino-server/scripts/capture.sh switcher.png --spawn cupertino-files --spawn cupertino-settings --hold-modifiers --key 5:Super+Tab
```

The script uses the release binaries in `target/release`, or `target/debug` when the release build is missing (`BIN_DIR` overrides both), with a temporary sample home folder and cleared environment, so a capture never reads your files and looks the same on every machine: a stand-in `wpctl` holds the volume at 50 % (no sound device is read), the shell prompt and terminal title name neither user nor machine, and the pointer starts on the bottom-right corner, over no surface, until a `--pointer` moves it. The Files sidebar still names the computer by the machine's host name. `AFTER` sets the seconds before the shot (default 5), `SIZE` the output size (default `1280x800`), and `dbus-run-session` is required. One asset pack is used: `CUPERTINO_ASSET_DIR`, else the user pack under `$XDG_DATA_HOME` (or `~/.local/share`), else the built-in assets; the system pack is not searched. The script prints the path it wrote.

## Surface captures

`ci/capture.sh` draws one surface of Files or the toolkit with the app's own painter on sample data, without a display, a session bus or the server, for look checks and merge screenshots. `overview/all` runs the server headless (`session.sh`) with test windows and builds the release binaries first unless `BIN_DIR` names them:

```bash
ci/capture.sh files/sidebar --drop-place Desktop --increase-contrast --sheet
ci/capture.sh kit                # lists the surfaces of an app
ci/capture.sh overview/all --windows 3 --scale 2
```

It writes one PNG per state and appearance into `captures` under `$CARGO_TARGET_DIR`, else under the workspace's `target` (`--out` changes it) and prints each path. The flags, the file names and the surfaces are in `crates/cupertino-capture/README.md`. It needs an asset pack and exits 2 without one.

## The shell processes

The dock, menu bar, launcher, notifications and the other shell parts are separate processes. Each builds its binary behind an `app` feature:

```bash
cargo build --release -p cupertino-dock --features app
cargo build --release -p cupertino-menubar --features app
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

The TTY backend has run in a virtual machine, not yet on real hardware. Treat it as experimental.

## Tests

```bash
scripts/test                   # the fast tier of the crates changed since origin/main and of their dependents
scripts/test --all             # the fast tier of the whole workspace
scripts/test --slow            # the slow tier: whole programs, private buses, displays and measurements
scripts/test -p cupertino-dock # the fast tier of the crates named
scripts/test -- -E 'test(/dock/)'   # arguments after -- go to nextest
cargo test --workspace --all-features --doc
cargo run -p cupertino-layers
```

The tests come in two tiers, defined in `.config/nextest.toml`. The fast tier (the `default` profile) is every test
but the slow tier's: `scripts/test` fails a fast-tier test that takes more than 2 seconds and a whole-workspace run that
takes more than 5 minutes, and prints the 20 slowest tests after every run. The slow tier (the `slow` profile) lists by
name the tests that run whole programs, buses or displays or that measure, each with its reason; the `ci` profile runs
both. `scripts/test` prepares the integration-test programs through `ci/build-test-fixtures.py` before nextest starts.
Doctests run separately, as nextest does not run them.

Install [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`; CI uses 0.9.151). It runs every
test in its own process and kills a test that runs longer than 2 minutes, so a hang fails with the test's name. Shell
and app crates need `--features app` to test their process code; `scripts/test` passes `--all-features`. A killed run
can leave a test `dbus-daemon` behind: stop it, never your real session bus.

`scripts/done` runs every check a change must pass, stopping at the first that fails: the project gate
(`ci/gate.py`) and the test suite layout, formatting, clippy on the changed crates and their dependents, a test change
for every crate whose sources changed (`NO_TEST_REASON="..."` accepts one without, for a reason), `scripts/test`,
`scripts/dev/build` and `scripts/preview --changed`.

## Development scripts

| Script | What it does |
|---|---|
| `scripts/dev/build` | The release build of every program (`cargo build --locked --release --workspace --all-features --bins`) into `target/release`, the one build the package recipe, the headless checks and `scripts/preview` share. |
| `scripts/dev/shot OUT.png [server flags]` | One headless screenshot of the whole desktop with the current checkout, built first. `SIZE` (default `1512x982`), `SCALE` (default 2), `APPEARANCE` (`dark` or `light`), `AFTER` (seconds before the shot, default 5) and `WALLPAPER` change it. |
| `scripts/preview` | Shoots every scenario in `scenarios/*.toml` headless with this checkout and with the installed release (or the programs in `PREVIEW_BEFORE=DIR`), at scales 1, 1.5 and 2 in light and dark, and shows them side by side with the changed pixels in `target/preview/index.html`. `--changed` takes only the scenarios of the programs changed since `origin/main`, `--no-before` skips the comparison, and `--live` runs the checkout's desktop in a window of the current session. |
| `scripts/dev/bench` | The desktop's rest and its busiest moments in headless sessions on the real render path, every process measured. `--size WxH` and `--scale S` set the output (default 2560 × 1600, scale 2), `--rate` the refresh rates (default 60), `--runs N` the sessions per scenario, `--scenario` which ones. The baseline scenarios: idle (which process wakes, how often, where it sleeps and its timers), a window drag over glass, a hover along the window buttons, a live resize, Control Center open and closed, typing in the launcher and a scrolled Files list; `--scenario all` adds a dock sweep, the overview, the launcher, a context menu with its submenu, and Files full screen (`files-full-repaint`: the appearance turned, each a repaint of the whole window; `files-full-scroll`: its list scrolled). Per scenario: the compositor's frame intervals, missed refreshes, work p50/p99, the CPU time to compose and submit a frame without waiting for the GPU (`cpu99`) and the GPU time p50/p99, title bar and HUD rasters and uploads, blur passes, input latency and CPU time; per program the frames, slow frames (longer than a refresh) and the p50/p99 of each phase (update, view, layout, paint, raster, copy, commit) from `CUPERTINO_FRAME_TIMING`; per program the CPU and private memory. `--out F.json` writes every run and the median of the runs with their spread; `NO_BUILD=1` or `BIN_DIR=DIR` measures other programs, `BENCH_KEEP=DIR` keeps the raw files and `BENCH_SHOTS=DIR` a screenshot of each scenario. |
| `scripts/dev/measure` | Memory (PSS, RSS and private dirty) and CPU of every desktop process in a headless session at rest, while used and at rest after use, the median of `RUNS` sessions (default 3). `--out F.json` writes the figures and `--compare A.json B.json` shows the change process by process; `NO_BUILD=1` or `MEASURE_BIN=DIR` measures other programs. |
| `scripts/package-local.sh [OUT_DIR]` | The Arch packages of the committed HEAD ([Installing a local build](#installing-a-local-build)). |
| `scripts/build-iso.sh [OUT_DIR]` | The live and install ISO ([Install from a USB stick](install-usb.md)). |

## Soak

`ci/soak.py` runs the daily-driver workload for hours and samples the server's RSS and open file descriptors every minute: Chrome and an Electron window each play a looping video while the page scrolls itself (from the page's script, not from input events), foot prints a listing five times a second, Files shows the home folder, and `glxgears` draws through `xwayland-satellite`. A `cupertino-testclient --intervals` window draws on every frame callback and logs the time between its presented frames; each sample carries that minute's median and 99th percentile, and the result reports how both changed after the warm-up (reported, not judged). It needs `google-chrome-stable`, `electron43`, `foot`, `glxgears`, `xwayland-satellite` and `ffmpeg` (it encodes the test video). `cupertino-files` and `cupertino-testclient` are taken from `BIN_DIR` (default `target/release`), else from `PATH`; `--headless` also runs the server from there.

```bash
ci/soak.py                                     # in a terminal of the session: 8 h, 1 h warm-up
ci/soak.py --headless --hours 1 --warmup-minutes 10
```

The run fails when the server's RSS at the end is more than 10% above its RSS after the warm-up (the median of five samples each), when any descriptor count after the warm-up is more than 5% from the one after the warm-up, when the server exits, or when a workload program exits. It writes `samples.csv` (a line a minute, as it goes), `frames.txt`, `workloads.log` and `results.json` into `soak-DATE-TIME` (`--out` changes it), prints `PASSED` or `FAILED` with the reasons, and exits 1 on a failure. It ignores the hang-up of the terminal, so a server crash that takes the terminal down still ends with `results.json`. `--without NAME` leaves a workload out and marks the result incomplete. `--headless` runs the same soak as a client of `crates/cupertino-server/scripts/session.sh`'s headless desktop and writes the server's log and a screenshot of the desktop three minutes in (`desktop.png`) next to the results.

## Configuration

Plain files under `~/.config/cupertino/`, each with an environment override: `shortcuts.conf` (chord to action lines), `compositor.json`, `keyboard.conf`, `dock.conf`, `menubar.conf`. The server's README lists every file and key.
