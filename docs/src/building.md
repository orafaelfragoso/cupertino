# Building and running

## Requirements

- A current stable Rust toolchain (`rustup` or your distribution's package).
- OpenGL ES drivers (Mesa) and a Wayland session to run nested in. A TTY session needs logind and a DRM/KMS device.
- A Wayland terminal such as `foot` to start inside the session.
- Some crates link system libraries (for example the session manager links libpam); each crate's README lists what it needs.

Build output is large. The managed build wrapper requires at least 30 GiB free before starting.

## Development builds without tests

For a local build or binary replacement, build only the package and binary being changed. For example, the portal's
`app` feature includes its production screen-sharing dependency without enabling every workspace test feature:

```bash
python3 ci/worktree-build.py run --profile dev -- cargo build --locked -p cupertino-portal --features app --bin cupertino-portal
python3 ci/worktree-build.py run --profile dev -- cargo build --locked -p cupertino-server --bin cupertino-server
```

These commands compile the selected binary and its dependencies, without test harnesses, examples or benchmarks.
Ordinary `cargo build` does not compile test harnesses either. Do not add `--all-targets`, `--tests`, `cargo test`,
`cargo nextest`, or `ci/build-test-fixtures.py` to a build-only operation. The fixture wrapper explicitly uses
`--all-targets` to prepare a test run; it is not a development build shortcut. `--all-features` alone does not select
test targets, but can enable optional test-support dependencies, so prefer the production features needed by the package.

Run regression tests separately when validating implementation changes. Scope them to the affected behavior and packages;
the workspace test and packaging gates remain separate checks. In particular, when using the fixture wrapper for a
package-specific test run, pass its repeated `--package` options for all required fixture packages as well as selecting
the tests in the child command; a package filter only on nextest does not narrow the wrapper's preparation.

Keep the same managed target, profile and build environment during an active development task so Cargo can reuse
unchanged artifacts. Switching between dev and release, preparing a fresh worktree target or cleaning it forces more
compilation. Incremental compilation is disabled by this repository's configuration; shared downloads are not a shared
compiled target. Follow the repository's `CONTRIBUTING.md` instructions for managed seeds and cleanup.

## Installed-session iteration

When developing inside Cupertino, choose the smallest change that can demonstrate the fix. Rebuilding the workspace
and restarting the session are separate operations; neither is the default for a change to one application.

| Changed component | Development loop | Effect on the running session |
| --- | --- | --- |
| A setting supported by the live watcher | Change the setting and observe its applied value | No process restart; `shell`, `screenshot` and menu-bar geometry remain startup settings |
| Files or Settings | Regular applications: build the binary with `--features app` in the worktree and run that build directly; no installed file is replaced | Quit the installed copy first when the change concerns its single instance; preserve navigation and pending work |
| Dock, menu bar, launcher, notifications, control panel, OSD or window manager | The development folder loop below, as the session's own user | A short interruption of that component; transient state can be lost (overview/switcher, queued notifications, window placement, open panels) |
| Portal | Build only `cupertino-portal` with `--features app`; test it nested or headless and leave it for a planned package update | Restarting the portal ends its active screen-sharing sessions and outstanding requests |
| Login, lock screen, session manager, authentication agents, desktop control, network/Bluetooth/power daemons | Use private backends, a nested session or a VM before a planned package update | These can affect authentication, connectivity or device state; they never start from the development folder |
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
`notifications`, `controlcenter`, `osd` and `windowmanager`. `login` (the lock screen and the session agent), the
portal, the authentication agent, desktop control, a data-control client and the server itself are never eligible.
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
2. Build it in a linked worktree through the managed wrapper with its production features, the binary alone:

   ```bash
   python3 ci/worktree-build.py run --profile dev -- cargo build --locked -p cupertino-dock --bin cupertino-dock --features app
   ```

   Validate the change separately with the scoped tests, as for any implementation change. The build must speak the
   running server's protocols: a change that also needs a new server waits for the next session start.
3. Copy it into the folder by a staged write and an atomic rename, never by truncating a file a process may be
   running:

   ```bash
   dev=~/.local/lib/cupertino/dev
   install -m 0755 target/debug/cupertino-dock "$dev/.cupertino-dock.new"
   mv -f "$dev/.cupertino-dock.new" "$dev/cupertino-dock"
   ```

   Record the source revision, the dirty diff and the artifact's hash (`sha256sum`).
4. `cupertino shell restart dock`. The server ends the process (SIGTERM, then SIGKILL after 5 s), starts it again at
   once whatever its exit status, and the command prints the new pid, its start time, the program and whether it is
   the development folder's. `cupertino shell list` shows the same at any time. A start that fails is reported, and
   the server keeps retrying after its throttle; going back (step 6) is the rollback.
5. Verify the changed interaction in the session through desktop control (the `cupertino-desktop` skill) or by hand,
   and confirm that the server and the other processes stayed alive. Defer a restart while the component has active
   work or a screen share.
6. `cupertino shell use-installed dock` removes the folder's file and restarts the process on its installed program.

Only the user's own session runs the folder's files: the server checks them against the user it runs as, so another
account's session, root's files aside, never starts them. The folder takes no part in package updates; empty it or
run `dev-folder disable` when the work is done.

### Components that are not eligible

The server, the login process, the session manager, the authentication agent, the portal, desktop control and the
system daemons are not swapped in a running session. Build them, exercise them nested or headless (`cupertino desktop
start --backend nested|headless`, the integration tests) or in a VM, and take them into the installed desktop through
a planned package update and session restart. When a manual replacement is unavoidable for an investigation, follow
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
keyboard grants. Cupertino's own Kit consent panel lists the rights and display and starts with Don't Allow
focused. Approval requires a fresh physical-input serial delivered to the registered consent surface after the
request. Injected input cannot approve consent, reach session/authentication surfaces or invoke reserved session
shortcuts or menu-bar power actions.

New physical gestures pause runs with pointer, keyboard or semantic-action rights; observation/capture-only runs
continue while the person uses the desktop. Releases and trailing touch/gesture frames complete existing gestures. Resume
requires another explicit native response. The consent panel closes after the decision; no permanent active or paused
panel is shown. Ctrl+Alt+Escape stops control. Lock, session deactivation, loss of the
authority/controller and local peer exit revoke the run and release only its owned input. Capture excludes
consent surfaces and authentication prompts. Capture sources belong to one approved epoch, and denied or revoked
sources cannot acquire a later grant. Geometry revisions reject stale input coordinates. Bounded outcome history
suppresses duplicate mutations; evicted outcomes are unknown and never replayed.

The compositor supervises the in-tree Rust `cupertino-control --serve` process on its private connection. Its socket
is `$XDG_RUNTIME_DIR/cupertino/control-WAYLAND_DISPLAY.sock`, mode 0600 in a private directory. Each local connection
gets a fresh inactive run and negotiates `{"op":"hello","version":1}`. The controller checks the same-user peer and
holds its exact process descriptor: a copied socket cannot preserve a dead peer's grant. Commands, queues, capture
allocations and deadlines are bounded. The controller sleeps on native transport/accept wakeups when idle.

In a development session, pass `--dev-shell --control-program /absolute/path/to/cupertino-control` to the new server
and run the matching login process with its existing Session and SessionLock roles. Run
`cupertino-control --session WAYLAND_DISPLAY` and send sequential JSON lines on stdin; it negotiates the handshake.
Keep that process connected across commands, since separate invocations do not share consent:

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
{"op":"stop"}
```

Capability bits are observe 1, capture 2, pointer 4, keyboard 8 and semantic actions 16. Semantic actions also require observe. Keys use evdev codes; coordinates and scrolling
use logical points relative to the approved output. Capture returns an upright PNG as base64 with its geometry.
`status` reports epoch and phase: inactive 0, consent 1, active 2, paused 3, revoked 4. `resume` opens fresh consent.
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
native consent, and pass the resulting epoch to each subsequent call. Observe accepts window, exact role and name
substring filters, reports truncation, and supplies output-relative bounds. Opaque identifiers and publication
revisions are decimal strings, preserving all 64 bits. Geometry revisions remain numbers. Action returns its
outcome and a fresh observation; capture returns PNG image content with geometry, without duplicating base64 in
text. Cancellation and stdin EOF close the native lease and release owned input. Queues, messages, images,
publication sizes, waits and stalled stdout writes have explicit bounds. Tool discovery works offline.

The checkout registers the installed controller for agent hosts working in it, and only there:
`.codex/config.toml` for the [Codex MCP host](https://learn.chatgpt.com/docs/extend/mcp?surface=cli) (loaded once
the checkout is a trusted project) and `.mcp.json` for Claude Code (approved once when it asks). Both run
`/usr/bin/cupertino-control --mcp` and pass the host's `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` through, so the
bridge follows the session the host runs in. Do not add a user-wide registration as well. Check the result from the
checkout with `codex mcp get cupertino` or `claude mcp get cupertino`.

For a development candidate, use a stable staged binary path outside the disposable managed target.
Choose `gpt-6.1-terra` in the host; this bridge requires no API client or new credential.
A host connection refresh is separate from restarting Cupertino. Host MCP tool approval and native desktop consent
are separate gates. The project Codex configuration approves this one server's tools
(`default_tools_approval_mode = "approve"`), which a host with approval policy `never` needs; the compositor still
requires fresh native consent.
The opt-in model acceptance test scopes `default_tools_approval_mode = "approve"` to its isolated fixture server.

The real Counter and Forms examples are tested through the shipped Rust MCP adapter, supervised controller and
native Kit consent panel on private headless sessions. Those tests assert button state changes, Unicode field
updates, stale-action/coordinate rejection, protected values, image content and cancellation.
Run the optional model acceptance test only with an installed, authenticated Codex CLI:
set `CUPERTINO_TERRA_CODEX` to its absolute path and `CUPERTINO_TERRA_EVIDENCE` to a retained JSONL receipt path,
then use the fixture wrapper with `cargo nextest run --run-ignored only -E 'test(terra_controls_kit)'`.
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

### Existing foundations and gaps

- `cupertino-cli` already provides a typed command registry, JSON/JSONL results, request IDs, version reports and
  diagnostic export. Extend this interface before adding a second desktop command vocabulary.
- Kit's own `cupertino-ui-core` already maintains roles, names, states, bounds, protected values and supported
  actions. Export this native model for Cupertino applications. The existing AT-SPI adapter remains an accessibility
  integration; native automation does not depend on AccessKit or AT-SPI.
- The compositor already routes keyboard, pointer, button and axis events through shared handlers. The native
  control endpoint should feed those handlers with explicit device ownership, not bypass menus, grabs or focus.
- `cupertino-ui-test` supplies deterministic input traces, a manual clock, semantic audits and inspectable runtime
  state. The server's integration tests cover real shell processes on private buses; captures supply visual evidence.
- Privileged connections are created by the server and handed to supervised processes as `WAYLAND_SOCKET`.
  Reuse that capability boundary. The current eight-bit capability set is full: adding a control capability also
  requires a wider bitset and tests that existing roles keep exactly their existing privileges.
- The portal currently provides Settings, FileChooser, ScreenCast, AppChooser and Screenshot. Existing guarded
  image-copy capture and window geometry can be reused by a trusted controller without adding a portal backend.
  Screen sharing supplies pixels, not permission or a mechanism to inject input.

### Source changes

The following records the architecture and remaining integration roadmap; the implemented output-scoped interfaces are described above.

| Area | Native change |
| --- | --- |
| `cupertino-protocols` | Define a versioned private control protocol with explicit requests, bounded batches, acknowledgements, refusals, session epochs and output/window handles |
| `cupertino-server` and an in-tree pure control model | Add the control capability, model grants/refusals without I/O, enforce them on every request, route accepted input through existing handlers, expose current geometry, and clean up the controller's held input |
| `cupertino-shell-client` | Bind the private control protocol through the existing transport extension mechanism |
| An in-tree Rust control process | Receive the supervised connection; own the local tool connection and bounded requests, with capture through grant-aware existing protocols |
| `cupertino-login` and `cupertino-menubar` | Let the trusted session agent present consent in Kit and publish active-control status/stop, with the compositor enforcing genuine-user approval and a reserved revoke shortcut |
| `cupertino-app-protocols`, `cupertino-protocol-app` and Kit | Publish only each application's own semantic tree and accept only supported actions routed back to that publication; emit explicit window-close and node-removal updates |
| `cupertino-cli` and the control process | Provide typed local commands and a Rust MCP stdio adapter over the same operations |
| Contract registry and packaging | Register the private globals, executable and local IPC; install the owned process with the existing trusted-path rules |

Keep the public app transport free of private shell protocols. Keep semantic wire types below both the server and
toolkit: the server must not depend on `cupertino-ui-core`. Applications can publish only their own surfaces' data;
publishing semantics grants no observation of other applications and no input-injection capability.

### Delivery order

1. **Establish and test the native control boundary.** Specify request/version limits, input-device ownership,
   grant lifetime and refusals before exposing mutation. Add generated protocol bindings and a pure state model.
   Test that a public Wayland client cannot discover or bind the control global, another privileged role cannot
   use it, and stale sessions or handles cannot act.
2. **Complete one nested control loop.** Run an in-tree Rust controller on a server-created connection. Observe
   outputs and windows, capture a frame, click and type in a fixture, drag its window and check the result. The
   server must enforce lock, authentication-prompt and revocation boundaries independently of the controller.
   On disconnect or revocation, release only injected keys/buttons and cancel its outstanding grabs or drags.
   Verify that physical keys held by the person are preserved and that the controller cannot grant or extend its
   own authorization through synthetic input to its consent UI.
3. **Export Kit semantics.** Publish bounded native snapshots, supported actions and lifecycle events from the
   production runtime. Map window-local bounds through the compositor's authoritative geometry. Support stable
   identifiers for ambiguous role/name queries, and keep secrets out of snapshots, traces and error messages.
   Third-party applications initially use pixels and real input; their semantic interoperability is separate work.
4. **Connect Terra through a Rust MCP adapter.** Use
   [MCP stdio](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#stdio):
   newline-delimited JSON-RPC, negotiated protocol version, tool discovery, bounded tool calls, text/image results,
   cancellation and clean EOF handling. Keep stdout exclusively for protocol messages. Report unsupported operations
   honestly, and never expose arbitrary shell execution as a desktop tool. OpenAI's
   [custom UI tool guidance](https://developers.openai.com/api/docs/guides/tools-computer-use-integration#use-your-own-ui-tools)
   supports this approach. Verify the connected host and Terra's image/tool support before claiming a model-driven pass.
5. **Deploy and shorten the bug-fix loop.** Add a preview updater to `cupertino dev` using the installed-session
   sequence above: explicit package/binary/session, managed scoped build, artifact manifest, stage, restart, verify
   and rollback. Prove it by updating and restoring a launcher candidate while the compositor PID and an unrelated
   application stay alive. Build only the changed consumers. Activate server changes at a planned session restart;
   subsequent compatible controller and app changes can be replaced separately.

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
controller's live process handle, private Wayland connection, local MCP connection and one run epoch. Do not persist
a grant across process restart, reconnect, lock or logout. A reserved physical shortcut revokes control even when
the controller is stalled. Show active-control status and a stop action through the menu bar; the reserved compositor
shortcut must remain available when the menu bar or session agent is unresponsive. Loss of the grant authority
revokes control.

The states are `inactive -> awaiting-consent -> active`, with `active -> suspended -> active` for an explicit resume,
and `active|suspended -> revoked` for termination. Denial or cancellation returns to inactive.
Lock, session deactivation/VT switch, controller or local-tool disconnect, and explicit stop move active or suspended
to revoked before any further action is handled. Showing an authentication prompt suspends control and input delivery; invalidate
pending operations and require an explicit person-driven resume after it closes. A new run after revocation needs
a new consent interaction. Fixture tests receive a test-owned grant on a private connection, never through a
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
remember which target received each press. Cancel or release controller-owned state before transferring focus to a
lock or authentication surface; cleanup must not send a synthetic release into the newly protected surface.

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

From a text console (switch with `Ctrl+Alt+F3`), a checkout can start the full desktop — compositor, dock, menu bar, launcher, notifications, control panel, OSD, window manager, Files and Settings:

```bash
./run-cupertino.sh
```

The script refuses to run inside a desktop session (`WAYLAND_DISPLAY` or `DISPLAY` set). It builds the workspace's release binaries (`cargo build --release --workspace --all-features --bins`), writes a shell process list to `$XDG_RUNTIME_DIR/cupertino-compositor.json`, starts a fresh session bus with `dbus-run-session`, and runs `cupertino-server --udev` with Files and Settings. It installs nothing and enables no service: the session manager and the login screen are not used. `XDG_RUNTIME_DIR` must be set, which a console login does.

- `Ctrl+Alt+Backspace` quits.
- `Ctrl+Alt+F<n>` returns to the console session you came from.
- The log is `~/cupertino-tty.log`; when the server exits with a non-zero status the script prints the last 20 lines.
- Server flags pass through: `./run-cupertino.sh --run-for 600` stops by itself after ten minutes.

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
python3 ci/worktree-build.py run --profile dev -- python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features
python3 ci/worktree-build.py run --profile dev -- cargo test --workspace --all-features --doc
python3 ci/worktree-build.py run --profile dev -- cargo clippy --workspace --all-targets
python3 ci/worktree-build.py run --profile dev -- cargo run -p cupertino-layers
```

Run these commands from a linked worktree. The outer wrapper keeps its target private and lets at most four heavy builds run at once across worktrees (`--slots N` changes that), each with a share of the processors; a new dependency or a changed `Cargo.lock` keeps the target, as Cargo rebuilds only what it affects; the fixture wrapper prepares integration-test programs before nextest starts. Keep the same profile and environment throughout the target's lifecycle; the repository's contribution guide explains seed preparation and cleanup.

Install [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`; CI uses 0.9.151). It runs every test in its own process and kills a test that runs longer than 2 minutes (`.config/nextest.toml`), so a hang fails with the test's name; doctests run separately. Shell and app crates need `--features app` to test their process code; pass the crate selection and features to nextest inside the wrappers above. A killed run can leave a test `dbus-daemon` behind: stop it, never your real session bus.

## Soak

`ci/soak.py` runs the daily-driver workload for hours and samples the server's RSS and open file descriptors every minute: Chrome and an Electron window each play a looping video while the page scrolls itself (from the page's script, not from input events), foot prints a listing five times a second, Files shows the home folder, and `glxgears` draws through `xwayland-satellite`. A `cupertino-testclient --intervals` window draws on every frame callback and logs the time between its presented frames; each sample carries that minute's median and 99th percentile, and the result reports how both changed after the warm-up (reported, not judged). It needs `google-chrome-stable`, `electron43`, `foot`, `glxgears`, `xwayland-satellite` and `ffmpeg` (it encodes the test video). `cupertino-files` and `cupertino-testclient` are taken from `BIN_DIR` (default `target/release`), else from `PATH`; `--headless` also runs the server from there.

```bash
ci/soak.py                                     # in a terminal of the session: 8 h, 1 h warm-up
ci/soak.py --headless --hours 1 --warmup-minutes 10
```

The run fails when the server's RSS at the end is more than 10% above its RSS after the warm-up (the median of five samples each), when any descriptor count after the warm-up is more than 5% from the one after the warm-up, when the server exits, or when a workload program exits. It writes `samples.csv` (a line a minute, as it goes), `frames.txt`, `workloads.log` and `results.json` into `soak-DATE-TIME` (`--out` changes it), prints `PASSED` or `FAILED` with the reasons, and exits 1 on a failure. It ignores the hang-up of the terminal, so a server crash that takes the terminal down still ends with `results.json`. `--without NAME` leaves a workload out and marks the result incomplete. `--headless` runs the same soak as a client of `scripts/session.sh`'s headless desktop and writes the server's log and a screenshot of the desktop three minutes in (`desktop.png`) next to the results.

## Configuration

Plain files under `~/.config/cupertino/`, each with an environment override: `shortcuts.conf` (chord to action lines), `compositor.json`, `keyboard.conf`, `dock.conf`, `menubar.conf`. The server's README lists every file and key.
