# Plan: the layered architecture and the shell host

The workspace moves to one set of layer rules, CupertinoKit becomes the only way any program other than the compositor
draws or animates, and the shell's everyday components run as Kit components inside one shell host process. The work
starts at the bottom layer and moves up one layer at a time, carried out by several implementing agents and merged by a
lead.

This file is the single source of truth for the work. Everything an implementer needs is here or in a file this plan
names. Anything not written here is not decided: an implementer stops and reports instead of deciding it (see
[Rules for implementers](#rules-for-implementers)).

## Status

Updated by the lead at every merge.

| | |
|---|---|
| Current wave | 4 (not started) |
| Running tasks | none |
| Enforced layers | foundation, graphics, models, protocol, transport, testing; system after W4.5 (134 remaining-work lines) |
| Waiting on the owner | nothing |
| Last closed wave | 3 |

## Contents

1. [Decisions](#decisions)
2. [Target architecture](#target-architecture)
3. [Crate map](#crate-map)
4. [Rules for implementers](#rules-for-implementers)
5. [Waves and tasks](#waves-and-tasks)
6. [Measurements](#measurements)
7. [Glossary](#glossary)
8. [Decision log](#decision-log)

## Decisions

Every decision the work depends on. "Owner" means the project owner made it; "plan" means this plan proposes it and the
owner may change it before the wave that first depends on it starts. A changed decision is edited here and recorded in
the [decision log](#decision-log), never changed silently in a task.

### Toolkit and drawing

| # | Decision | Why | Consequence | By |
|---|---|---|---|---|
| D1 | CupertinoKit is the only way a program other than the compositor creates surfaces, draws, lays out text, decodes or encodes images for display, or animates. | One look, one motion system, one place to fix a control. | The shell, the login and lock screens, the prompts and the apps lose their own drawing code. | Owner |
| D2 | The compositor draws only what [What the compositor draws](#what-the-compositor-draws) lists. | Same as D1: the compositor is not a second toolkit. | Overlays it draws today move to components (W6.12). | Owner (D1), plan (list) |
| D3 | Kit's runtime is re-entrant: state that can differ between Kit instances lives in a runtime object, never in a process-wide static, so several instances run in one process. Process-wide and allowed: immutable data built once, counters that only hand out unique ids, the caches of D5, and the panic hook and diagnostic sink, installed once by the program's `main`, which route each event by the calling thread's instance (a thread-local scope Kit sets on its UI thread and carries into its tasks). The C interface keeps one runtime per process; its ABI and baselines do not change. | The shell host runs several components; apps and tests gain nothing from globals either. | `diag::event()` keeps its signature and finds its instance through the thread's scope, so its 20 calling crates do not change; every other static is listed with its fate in W4.5. | Plan |
| D4 | Every Kit process has one render thread that owns the GPU device and its only context, and draws the display lists that each instance's UI thread records. Surfaces drawn on the CPU (small ones, and all of them where there is no GPU: headless runs, virtual machines) are rasterised on the worker pool, one task per surface, and the render thread presents them. The render thread commits: the UI thread hands it the configure serial with the display list, and it acknowledges, attaches and commits in that order. It reports a heartbeat to the host's watchdog; a stuck render thread is a hang of the whole process (D17). | One GPU context per process instead of one per window or component: less memory, no locking between contexts, one place that handles a device reset. A UI thread busy recording does not stall a frame already recorded. | Kit's presenter moves its drawing off the UI thread (W4.6). The one-device rule lives in `cupertino-ui-render`; `cupertino-gpu` stays usable with a device the server lends it. | Plan |
| D5 | Shared caches in a Kit process (font database, glyph cache, icon cache, design catalogue) take one of three forms: immutable after start; append-only, with immutable entries behind `Arc` and a lock held only for the map insert; or owned by one service thread reached by messages. No other form. | A panic in one instance can then never leave a cache half-changed for the others. | W4.5 and W6.1 build them in these forms only. | Plan |
| D6 | One concurrency model for every Kit program: the UI thread runs Kit's event loop; slow work (files, bus calls, decoding) goes through Kit's task API, whose results come back to the UI thread as messages. Each process has two pools: one for CPU work, sized to the cores, and one for I/O, where every task carries a deadline its caller names, a task past it is reported with the instance that started it, and one instance holds at most a quarter of the pool, so a blocked mount cannot starve the other components. Each instance's bus connection is ticked by its own UI loop (no executor thread of its own). In `shell` and `apps` crates, Clippy's `disallowed-methods` bans `std::thread::spawn`, `block_on`, `std::fs` and blocking bus calls: files and bus calls go through `kit::task` and Kit's async bus API, so a UI thread has no blocking call within reach. `system` crates offer blocking functions and start no threads; callers run them as tasks. The compositor keeps its own event loop. | A blocking call on a UI thread is how a desktop freezes; every program inventing its own threads is how panics escape and resources leak. | `kit::task` and the bus integration in W4.5; the `clippy.toml` of each `shell` and `apps` crate is generated from one list by `cupertino-layers`, which checks they match (W0.2); each component and app task removes its own threads and `block_on` calls. | Plan |
| D7 | Motion comes from the design catalogue only. Kit's API names motions from the catalogue; a program that needs a new motion adds it to the catalogue. On the wire, motions travel as numbers (curve or spring parameters). | One source of motion; a host with a different catalogue still animates correctly. | Kit exposes no constructor for curves or springs from numbers (W4.7). | Owner (single source), plan (form) |
| D8 | Files on screen are a Kit family: the views (icon grid, list, columns, the open and save panel) and the operations behind them (open, rename, trash, copy and move with progress, places, thumbnails, notices). Files, the desktop component and the shell's file chooser use it. | One implementation of the most-used views on the desktop, and one of the file operations users depend on. | Files' views and operations, the portal's chooser (`file_chooser.rs`, `dialog/`) and the file picker's model meet in `cupertino-ui-controls`; where two copies meet one stays (W4.8). | Plan |

### Processes

| # | Decision | Why | Consequence | By |
|---|---|---|---|---|
| D9 | One shell host process, `cupertino-shell`, runs the everyday components: dock (with the application switcher and the force-quit dialog), menu bar, launcher, notifications, control panel, on-screen displays (with the input-source switcher and the accent picker), overview, window manager (stage and tiling policy), desktop items, session agent (application restore, login items, the log out, restart and shut down panels and the panel that interrupts them, the power button and low battery panels), portal dialogs (file and application chooser). Each has its own thread, Wayland connection, roles and bus connection; a failed one is restarted alone. | Memory: one toolkit, one GPU context, one set of caches instead of eleven processes. Speed: warm caches. Safety: a failure stays inside its component. | See [The shell host](#the-shell-host). | Owner |
| D10 | Privileged interface stays out of the shell host, in small processes: `cupertino-login` (login screen, lock screen), `cupertino-prompts` (authentication, keyring, and the consent prompts for desktop control, screenshots and global shortcuts) and `cupertino-screenshot` (everything that shows the screen's contents: capture, the colour picker, the screen-sharing picker). | Roles are bound to connections because a process must not gain another's privilege; inside one process every component could use every role. Screen capture, consent and password entry are kept away from the components that handle untrusted content (notification bodies and images, tray menus, application names), and password entry away from window titles and screen contents. | Three processes, two of them always running; `cupertino-prompts` replaces two processes (`cupertino-polkit-agent`, `cupertino-keyring-prompter`), so the count does not grow. | Plan (narrows the owner's one-process decision for these three) |
| D11 | Processes with no interface of their own are services: the portal backend (`cupertino-portal`), the desktop-control bridge (`cupertino-control`), and the audio service (D12). They do not use Kit. | A service has no reason to load a toolkit. | The portal asks the shell host's dialogs component (file and application chooser), `cupertino-prompts` (consent) or `cupertino-screenshot` (colour picker, sharing picker) over private bus interfaces (W6.11). | Plan |
| D12 | `cupertino-audiod` owns the mixer and the default output and serves `org.cupertino.Audio1`, started by bus activation. Nothing else uses the mixer. | Three processes drive the mixer today with nothing coordinating them. | Control panel, on-screen displays and Settings call it. | Owner (service), plan (form) |
| D13 | Log out, restart, shut down and sleep have one entry point: the server's `org.cupertino.Session1`, which quits applications first and then asks `cupertino-sessiond` (log out) or logind (power). The login screen, which has no user session, asks `cupertino-sessiond`. | One place for open applications, inhibitors and confirmation. | No other program calls logind's `Manager` for these; `cupertino-prompts` keeps the logind call that finds its own session. Quitting applications moves from `cupertino-login` into `Session1` (W5.4); `org.cupertino.Login1` keeps only `Lock`, and its log-out, power-key and request methods move to `Session1`, which asks the session agent for the panels (W6.13). | Plan |
| D14 | Display configuration has one entry point for programs: the server's `org.cupertino.Displays1`. | Settings opens raw output-management connections today; programs never speak the window system's protocols directly (D1). | Server side in W5.4, Settings in W7.2. | Plan |
| D15 | Always running in a user session: server, shell host, `cupertino-login` (the lock screen must appear at once), `cupertino-prompts` (the authentication agent must stay registered), `cupertino-portal`, `cupertino-control`. Started on demand: `cupertino-screenshot`; `cupertino-audiod` by bus activation at first use, then kept. | Today the server starts nine shell processes and four built-in ones in every session. | Six processes always running instead of fourteen, seven once sound is used. | Plan |

### The shell host

| # | Decision | Why | Consequence | By |
|---|---|---|---|---|
| D16 | Restart policy. A component that panics restarts on a fresh thread after 0.25 s, doubling to 8 s, reset after 60 s of running. Five panics within 60 s stop it until `cupertino shell restart <component>` or the next login, and the notifications component says so. The server keeps the counts, so they survive a host restart. The server restarts an exited host with its existing policy (1 s throttle, 30 s cap, reset after 60 s stable) and never gives up, whatever its exit status. A host that restarts itself (D17) asks for it over the host protocol first; that restart is not counted as a crash. | Bounded damage, no restart storms, the shell always comes back. | Named constants in `cupertino-shell` and `cupertino-server`. | Plan |
| D17 | Hangs. A component's UI loop records when it leaves its wait and clears it when it returns; the host checks every 0.5 s, only while the component is busy, after a 10 s start-up grace. Busy for 2 s: the host reports it unresponsive and the server releases its grabs, dismisses its transient surfaces and stops sending it input. Back in its wait before that, it is reported responsive and gets input again. Busy for 10 s: the component is lost; the host shuts its sockets down (`shutdown(2)`; descriptors are closed only after the thread has ended, so none is reused under it), the server starts that component alone in a separate host process (D18), and the shell host restarts itself at the next quiet moment (no input for 2 s) to reclaim the lost thread. The component stays in its own host for the rest of the session or until `cupertino shell restart <component>`; the server gives a component's roles to one host at a time, so two instances never run. | A thread cannot be killed; a separate process can. One hung component never stops the others or the compositor. | Host protocol events in W4.9; server in W5.2; host in W6.1. | Plan |
| D18 | Any component can run in a host process of its own (`cupertino-shell --only <component>`). The server uses this for a component lost to a hang (D17); after three host crashes within 60 s it starts every component this way for the rest of the session, so a crash Rust cannot contain (abort, stack overflow, out of memory, driver fault) then costs one component, not the shell; developers use it to try one changed component. Shipped configuration never sets it. | One mechanism for isolation, recovery and development. | The server accepts several host connections; each component's roles go to one of them. | Plan |
| D19 | Holds. When a component's connection drops, its persistent surfaces (menu bar, dock, desktop items) keep their last picture and their reserved screen space, without input, until the component is back or declared stopped. Its transient surfaces (launcher panel, menus, overview, on-screen bubbles, dialogs) are dismissed at once and focus returns to the window that had it; an overview transition settles at the windows' resting places. | No flash and no window reflow when a component restarts; nothing frozen on screen that looks usable. | Server side in W5.2. | Plan |
| D20 | Identity. Each component announces its bus unique name over its own Wayland connection; the server checks it belongs to the host and then checks bus callers per component, not per process. A component authorises a bus caller the same way: `cupertino-peer` resolves the caller's connection to a host process (the shared one or one of D18) and to the component whose well-known names that connection owns, so a component in a host of its own keeps its rights. No check compares process relations. Component sockets are close-on-exec. | Checks like "only the session agent may" and "only the launcher may" must keep meaning one component. | `cupertino-peer` resolves a caller to a component (W5.2). | Plan |
| D21 | The notification list survives a restart of the notifications component, in a file in `$XDG_RUNTIME_DIR` (mode 0600, cleared at log out). Transient notifications are not written. Every component's README lists what else a restart loses. | Losing notifications on a crash is visible data loss; keeping them on disk across sessions is a privacy decision not taken here. | One file written on change, off the UI thread. | Plan |

### Structure, tests and outside code

| # | Decision | Why | Consequence | By |
|---|---|---|---|---|
| D22 | The [Layers](#layers) table is the rule; there are no exceptions. Until a layer's wave closes, its remaining violations are listed in a generated file the check compares against: a violation must be listed and a fixed one must leave the list; a change may add lines until the last wave, after which the list only shrinks, and it is empty for every closed layer. | Rules that protect from the first day, without relaxing them. | W0.2. | Owner (no exceptions), plan (enforcement) |
| D23 | Above `ui`, the toolkit is reached only through `cupertino-kit`, and the shell's extension of it only through `cupertino-kit-shell` (see [Facades](#facades)). | D1 made checkable. | Kit gains what programs use directly today, and nothing on the must-not-expose list (W4.7). | Owner |
| D24 | A merge keeps the name of the crate that absorbs the others; the only new names are the ones in the [crate map](#crate-map). No crate is renamed; where a new crate takes over an old one's role (`cupertino-polkit-agent` into `cupertino-prompts`), the map lists the old one as merged into it. | Names are how agents and people find things. | | Plan |
| D25 | The server becomes four crates: `cupertino-server-backend`, `cupertino-server-render`, `cupertino-server-input` and `cupertino-server`. Direction: `-backend` (display devices, session, frame scheduling and timing, GPU reset) and `-input` depend on no other server crate, `-render` may depend on `-backend`, and `cupertino-server` on all three. A call against that direction goes through a trait the lower crate defines and `cupertino-server` implements; code that needs Smithay's handler pattern is generic over the server's state type bounded by those traits, as Smithay's own modules are. Smithay handler implementations for the server's state stay in `cupertino-server`. The groups come from the server's measured module graph (W5.6 step 0); a module that refers to a group above it stays in `cupertino-server`. | 81k lines in 82 modules around one state struct. | A serial chain beside the shell work (W5.6 to W5.8). | Plan |
| D26 | Components never depend on each other's crates; inside the host they talk through the host's message bus (typed, serialisable messages in `cupertino-kit-shell`, no blocking replies); a component in a host of its own (D18) is on the same bus through a socket the shared host serves in `$XDG_RUNTIME_DIR`; other processes are reached through the compositor's protocols or the session bus. | A component can be restarted, stopped or run alone. | | Plan |
| D27 | `cupertino-defaults` stays one crate in the `system` layer. | Its coupling is to the settings schema, which a split would not change. | | Plan |
| D28 | Licences are not a constraint on this work. Every licence check that could refuse a merge of this plan is removed in W0.2 (the toolkit's MPL closure, the GPL-dependency check); the check that each crate declares a licence stays. Licences are redefined later. | Owner. | Merges may cross today's licence boundaries; declared licences are reconciled when they are redefined. | Owner |
| D29 | Tests exercise public boundaries only. At the end no source file holds test code: every test is in a crate's `tests/` and uses only its public API, or is a program test (whole processes: headless sessions). Tests of internals are removed in the crate's `T` task; one that guarded a behaviour no other test covers is replaced by a public or program test before it goes. No item is made more visible and no feature is added to keep a test; Kit's `test` feature, for program tests, is the only test feature. Frame timings, instruction counts and per-process PSS (profiled by hand) are part of the acceptance bar. Behaviour this plan adds (restarts, hangs, holds, transactions, deadlines) is proven by program tests. | Behaviour and performance are what count; tests of internals are what make refactoring expensive, and behaviour tests are the refactor's safety net. | A `T` task per crate in every wave; `cupertino-session-tests` holds tests that start programs from more than one crate. | Owner |
| D30 | `third-party/smithay` stays vendored, kept as the upstream release plus a documented patch series: `third-party/smithay/patches/` holds one patch file per change, `PATCHES.md` lists each (what, why, files), `UPSTREAM.sha256` records every file of the upstream release, and a check reverses the patches on a copy of the tree and compares the result with those hashes. Offering the patches upstream, so they can leave the tree, is later work outside this plan. | Smithay is the right foundation; an undocumented fork is not. | W0.3. | Owner (upstream later), plan (form) |
| D31 | Keyboard layouts come from libxkbcommon through the `xkbcommon` crate. `cupertino-xkbcommon` and `third-party/xkbcommon-rs` are deleted. The server keeps the last working layout when one does not compile. | The vendored pure-Rust port follows an old release of the C library, has had no release since February 2025, and a panic in it was already fixed here; a panic compiling a layout ends the session. Layout data and the library are maintained together upstream. | Packaging depends on libxkbcommon (W1.7). | Owner |
| D32 | A crate exists for one of four reasons only: (1) it builds a program or a loadable library; (2) it is a facade (D23); (3) two or more crates need it that do not, and must not, depend on each other; (4) it would otherwise pass about 25k lines and splits along an interface (D25, the toolkit's crates). Code with one user and none of these reasons is a module of that user. A component is always a crate of its own (D26). A crate added later names its reason in the [decision log](#decision-log). | Every crate costs a manifest, a layer line, a README, a public API and the tests of that API; a crate with one user adds a boundary nobody else crosses. | Thirteen merges beyond today's obvious ones, listed in the [crate map](#crate-map). | Plan |

## Target architecture

### Layers

From the bottom. A crate depends only on the layers its row allows, in normal, build and dev dependencies, except where
a row says otherwise. Every crate may also have `testing` crates as dev-dependencies.

| Layer | Holds | May depend on |
|---|---|---|
| `foundation` | Values, maths and data models with no I/O: geometry, motion maths, design catalogue, keyboard model, menu model. | `foundation` |
| `graphics` | Drawing maths shared by every renderer, and the GPU device. | `foundation`, `graphics` |
| `models` | Desktop policy as pure logic: window model, input routing, displays, effects, scene. | `foundation`, `models` |
| `system` | User-space operating-system access: settings store, application database, asset packs and icons, thumbnails, trash, diagnostics, bus-caller identity, clients of the network and Bluetooth services. | `foundation`, `system` |
| `platform` | Kernel interfaces that more than one program needs: devices and their input events, the seat. A kernel interface one service needs is part of that service (D32). | `foundation`, `platform` |
| `protocol` | Wire schemas: generated Wayland types, versioned IPC messages. | `foundation`, `protocol` |
| `transport` | Client connections without widgets: the Wayland client, the shell protocol client. | `foundation`, `system`, `platform`, `protocol`, `transport` |
| `render` | The compositor's own drawing: materials, decorations, cursors. | `foundation`, `graphics`, `system`, `platform`, `render` |
| `ui` | CupertinoKit. | `foundation`, `graphics`, `system`, `protocol`, `transport`, `ui` |
| `shellkit` | `cupertino-kit-shell`. | `foundation`, `system`, `protocol`, `transport`, `ui` |
| `core` | The compositor. | `foundation`, `graphics`, `models`, `system`, `platform`, `protocol`, `render`, `core` |
| `services` | Processes with no interface of their own. | `foundation`, `models`, `system`, `platform`, `protocol`, `transport`, `services` |
| `shell` | The shell host, its components, and the separate Kit programs of D10. | `foundation`, `models`, `system`, `protocol`, `ui`, `shellkit`, `shell`, through [Facades](#facades) |
| `apps` | Applications. | `foundation`, `models`, `system`, `protocol`, `ui`, through [Facades](#facades) |
| `tools` | Developer and theming tools. | every layer except `core`, `shell`, `services`, `apps`; `testing` as dev-dependencies |
| `testing` | Test harnesses and tests that start programs from more than one crate. | every layer. Only dev-dependencies point at a `testing` crate. |

Further rules:

- **Purity.** `foundation` and `models` sources use no `std::fs`, `std::net`, `std::process`, `std::env`, `std::thread`,
  no `unsafe` and no FFI. Build scripts and `tests/` are exempt.
- **No layer reaches up**, also not through `tools` or `testing`.
- **Components stay apart (D26).** A `shell` crate never depends on another `shell` crate, except `cupertino-shell` on
  the component crates.
- **Window-system boundaries.** `cupertino-wayland-client` never reaches `cupertino-protocols` or the wlr, misc or
  plasma protocol crates; `cupertino-app-protocols` never reaches `cupertino-protocols`; `cupertino-kit` and the
  `cupertino-ui-*` crates never reach `cupertino-protocols` or `cupertino-shell-client`; `cupertino-ui-render` never
  reaches `cupertino-ui-wayland` or `cupertino-wayland-client`.
- **Test placement (D29).** No `#[cfg(test)]` in `src/` of a closed layer.

### Facades

| Never a dependency of a `shell` or `apps` crate | Reached through |
|---|---|
| `cupertino-ui-core`, `cupertino-ui-controls`, `cupertino-ui-text`, `cupertino-ui-render`, `cupertino-ui-wayland`, `cupertino-ui-test`, `cupertino-ui-abi`, `cupertino-theme`, `cupertino-animation`, `cupertino-raster-math`, `cupertino-gpu`, `cupertino-wayland-client`, `cupertino-app-protocols` | `cupertino-kit` |
| `cupertino-shell-client`, `cupertino-protocols` | `cupertino-kit-shell` (for `shell` crates only) |

`cupertino-kit` must not expose: Wayland objects or any transport type, raw surfaces, GPU objects, curve or spring
constructors from numbers (D7), or a whole lower crate re-exported as a module. It exposes the headless backend for
program tests behind a `test` feature, enabled only in dev-dependencies.

`services` crates may use `cupertino-wayland-client`, `cupertino-shell-client`, `cupertino-protocols` and
`cupertino-app-protocols` directly; they never use `cupertino-kit`.

### External crates

| External crate | Allowed in |
|---|---|
| `smithay` and its sub-crates | `core` only |
| `tiny-skia`, `resvg`, `usvg`, `image`, `png` | `graphics`, `system`, `render`, `ui`, `core`, `services`, `tools`, `testing`; `models` as dev-dependencies |
| `wayland-client`, `wayland-backend`, `wayland-protocols*`, `wayland-scanner` | `protocol`, `transport`, `ui`, `shellkit`, `core`, `services`, `tools`, `testing` |
| `xkbcommon` | `transport`, `ui`, `core`, `services`, `tools`, `testing` |
| `wayland-server` | `protocol`, `core`, `testing`; every other layer as dev-dependencies only |
| `zbus` | `system`, `platform`, `ui`, `shellkit`, `core`, `services`, `shell`, `apps`, `tools`, `testing` |
| `libc`, `rustix`, `nix`, `inotify`, `notify` | every layer except `foundation` and `models` |

### Processes

| Process | Crates | Runs | Role |
|---|---|---|---|
| `cupertino-server` | `cupertino-server`, `-server-backend`, `-server-render`, `-server-input` | always | Compositing, window mechanics, input routing, cursor, materials, supervision, transactions, `Session1`, `Displays1`. |
| `cupertino-shell` | `cupertino-shell` and the component crates | always | The components of D9. |
| `cupertino-login` | `cupertino-login` | always | Login screen (greeter session), lock screen (user session). |
| `cupertino-prompts` | `cupertino-prompts` | always | Authentication, keyring, consent for desktop control, screenshots and global shortcuts. |
| `cupertino-screenshot` | `cupertino-screenshot` | on demand | Screen, window and selection capture, the colour picker, the screen-sharing picker. |
| `cupertino-portal` | `cupertino-portal` | always | Desktop portal backend, screen-sharing streams. |
| `cupertino-control` | `cupertino-control` | always | Desktop-control bridge for local agents. |
| `cupertino-audiod` | `cupertino-audiod` | bus-activated | Mixer and default output. |
| `cupertino-sessiond`, `cupertino-netd`, `cupertino-btd`, `cupertino-powerd` | as named | as today | Unchanged. |
| `cupertino-files`, `cupertino-settings` | as named | when opened | Applications. |

### Kit's runtime

- One **UI thread** per Kit instance: its event loop dispatches its Wayland connection, input, timers, its bus
  connection (ticked from the loop, D6) and task results; it lays out and records display lists. It never blocks.
- One **render thread** per process (D4): owns the GPU device and its context, draws recorded display lists for every
  instance's surfaces, presents them, and handles device loss once for the process.
- One **worker pool** per process: `kit::task` runs slow work and posts the result to the instance that asked.
- **Shared caches** in D5's three forms only.
- No process-wide mutable statics in Kit or anything below it that Kit uses (D3).

### The shell host

- **Start.** The server starts `cupertino-shell` with a connection holding only the host role, and sends it the
  component list over the host protocol. The server alone decides that list, from the `shell` section of
  `compositor.json` (user file over the system one, live), in the schema W4.9 defines.
  The host asks for one connection per component; the server gives each the component's roles.
- **Components.** Each component is a Kit instance on its own thread (8 MiB stack), with its own Wayland and bus
  connections. Helper threads are started only through the host's API, which reports their panics as the component's.
  Child processes are started through the host, which reaps them.
- **Panics.** The shell binary unwinds. A component's state lives outside the frames that unwind; teardown runs inside
  `catch_unwind`, and a second panic during teardown leaks the state instead of aborting. The host then shuts the
  component's sockets, so the server and the bus clean up, and restarts it on a fresh thread (D16).
- **Hangs, holds, identity, isolation.** D17, D19, D20, D18.
- **Messages between components.** D26.
- **Unsafe code.** Component crates are `#![forbid(unsafe_code)]` and hold no mutable statics. Local time comes from
  Kit (`kit::time`), the one place that asks the system for it.

### Transactions

Specified in W4.9 and approved before any code is written for it. The specification meets these requirements at least,
and answers every question below.

1. One clock: the compositor's presentation time for the output concerned.
2. A transaction carries changes from one or more participants (component connections, the compositor's own window
   changes) and starts them on the same presented frame.
3. Motion is described, not streamed: target values plus curve or spring parameters (D7). The compositor evaluates them
   with `cupertino-animation`, the same functions Kit evaluates.
4. A deadline (default one frame interval plus 4 ms, at most 100 ms). A participant that misses it joins at the
   transaction's original start time, its motion already that far along; one that misses the end lands at the end.
5. A transaction can be bound to a gesture the compositor owns: the compositor maps the gesture's progress to the
   transaction each frame and settles it on release with the gesture's velocity. No per-frame traffic between processes.
6. Holes: a component reserves an area of a layer where the compositor draws a window's content or thumbnail, moving
   with that layer.

The specification answers: a participant that dies mid-flight; cancellation; two transactions on one property (the
later one starts from the current value and velocity); outputs with different clocks; how a transaction latches with
surface commits; reduced motion; a hole whose window closes.

### What the compositor draws

| Drawn by the compositor today | After |
|---|---|
| Client buffers, popups that linger while their clients fade them, the icon of a drag returning | Stays |
| Materials (blur, glass) under shapes clients give it | Stays |
| Server-side window decorations, for clients that ask for them | Stays |
| The cursor | Stays |
| The dim and sleep overlay, the lock's black fill until the lock screen draws | Stays |
| Shadows and outlines of windows that draw their own frames | Stays |
| The wallpaper (also the source every backdrop samples) | Stays |
| Window thumbnails for the dock and the overview | Stays, as holes (transaction requirement 6) |
| The input-source switcher, the accent picker | `osd` component (W6.12) |
| The stage strip | `windowmanager` component, which gains a surface (W6.9) |
| Placeholders for the dock and the overview | Removed; the components draw their own surfaces with holes (W6.8) |

W6.12 checks this table against the code; anything else the compositor draws is reported, not moved.

## Crate map

Every crate that exists today, and where it goes. "Keep" means it stays a crate under its name.

| Crate today | Layer today | Target | Target layer | Tasks |
|---|---|---|---|---|
| `cupertino-alsa` | platform | merged into new `cupertino-audiod` once only it uses it (D32) | — | W5.9, W6.16 |
| `cupertino-animation` | foundation | keep (motion maths) | foundation | W1.6 |
| `cupertino-app-protocols` | protocol | keep; absorbs `cupertino-semantics` | protocol | W3.1 |
| `cupertino-apps` | foundation | keep | system | W1.3 |
| `cupertino-assets` | foundation | keep; gains the design specification lookup | system | W1.3, W1.2 |
| `cupertino-bluetooth` | platform | merged into `cupertino-btd` (D32) | — | W5.11 |
| `cupertino-btd` | services | keep; absorbs `cupertino-bluetooth` | services | W5.11 |
| `cupertino-build-info` | foundation | merged into `cupertino-diagnostic-events` | — | W1.3 |
| `cupertino-capture` | foundation | merged into `cupertino-kit`, behind its `test` feature (D32) | — | W4.11 |
| `cupertino-cli` | tools | keep; absorbs `cupertino-diagnostics` | tools | W6.15, W7.3 |
| `cupertino-control` | shell | keep | services | W5.12 |
| `cupertino-control-model` | models | keep | models | W2.3 |
| `cupertino-controlcenter` | shell | keep; a component | shell | W6.5 |
| `cupertino-cursor` | render | keep | render | W4.1 |
| `cupertino-decorations` | render | merged into `cupertino-server-render` once only the server uses it (D32) | — | W4.1, W6.16 |
| `cupertino-defaults` | foundation | keep (D27) | system | W1.3 |
| `cupertino-devices` | platform | keep; absorbs `cupertino-evdev` | platform | W2.3, W2.4 |
| `cupertino-diagnostic-events` | foundation | keep; absorbs `cupertino-build-info` and `cupertino-profile`; per-instance state (D3) | system | W1.3, W4.5 |
| `cupertino-diagnostics` | tools | merged into `cupertino-cli`; the deprecated `cupertino-diagnose` program goes (D32) | — | W6.15, W7.3 |
| `cupertino-displays` | models | keep | models | W2.3 |
| `cupertino-dock` | shell | keep; a component; its overview chrome moves to `cupertino-shell-overview` | shell | W6.8 |
| `cupertino-effects` | models | keep; its raster helper becomes dev-only | models | W2.3 |
| `cupertino-evdev` | platform | merged into `cupertino-devices` (D32) | — | W2.4 |
| `cupertino-filepicker` | foundation | merged into `cupertino-ui-controls` | — | W1.5 |
| `cupertino-files` | apps | keep; its file views move to Kit (D8); its desktop mode moves to `cupertino-shell-desktop` | apps | W4.8, W6.10, W7.1 |
| `cupertino-geometry` | foundation | keep | foundation | W1.6 |
| `cupertino-glass` | render | keep | render | W4.1 |
| `cupertino-glyph` | foundation | merged into `cupertino-raster-math` (D32) | — | W2.4 |
| `cupertino-gpu` | graphics | keep; one context per process (D4) | graphics | W2.3, W4.6 |
| `cupertino-gpu-shaders` | tools | keep | tools | W7.3 |
| `cupertino-gpu-slice` | tools | keep | tools | W7.3 |
| `cupertino-icons` | foundation | keep; absorbs `cupertino-thumbnails`; cache in D5's form | system | W1.3, W4.5 |
| `cupertino-input` | models | keep | models | W2.3 |
| `cupertino-key-bindings` | foundation | merged into `cupertino-keyboard` | — | W1.1 |
| `cupertino-keyboard` | foundation | keep; absorbs `cupertino-modifiers` and `cupertino-key-bindings`; loses its I/O | foundation | W1.1 |
| `cupertino-kit` | ui | keep; the facade; absorbs `cupertino-capture` behind its `test` feature | ui | W4.5–W4.7, W4.11 |
| `cupertino-launcher` | shell | keep; a component | shell | W6.6 |
| `cupertino-layers` | tools | keep; the target rules | tools | W0.2 |
| `cupertino-login` | shell | keep; login and lock screens only | shell | W6.13 |
| `cupertino-login-ipc` | protocol | keep | protocol | W3.3 |
| `cupertino-menu-model` | foundation | keep | foundation | W1.6 |
| `cupertino-menubar` | shell | keep; a component | shell | W6.7 |
| `cupertino-modifiers` | foundation | merged into `cupertino-keyboard` | — | W1.1 |
| `cupertino-netd` | services | keep; absorbs `cupertino-netlink` | services | W5.11 |
| `cupertino-netlink` | platform | merged into `cupertino-netd` (D32) | — | W5.11 |
| `cupertino-notifications` | shell | keep; a component | shell | W6.3 |
| `cupertino-osd` | shell | keep; a component; gains the input-source switcher and accent picker | shell | W6.2, W6.12 |
| `cupertino-overview` | models | merged into `cupertino-wm` | — | W2.1 |
| `cupertino-peer` | platform | keep; resolves callers to components (D20) | system | W1.3, W5.2 |
| `cupertino-pipewire` | platform | merged into new `cupertino-audiod` once only it uses it (D32) | — | W5.9, W6.16 |
| `cupertino-polkit-agent` | shell | merged into new `cupertino-prompts` | — | W6.14 |
| `cupertino-portal` | shell | keep; no interface of its own (D11); absorbs `cupertino-screencast` | services | W6.11 |
| `cupertino-powerd` | services | keep | services | W5.11 |
| `cupertino-profile` | foundation | merged into `cupertino-diagnostic-events` (D32) | — | W1.3 |
| `cupertino-protocol-app` | transport | merged into `cupertino-wayland-client` | — | W3.2 |
| `cupertino-protocols` | protocol | keep; gains the host and transaction protocols | protocol | W3.3, W4.10 |
| `cupertino-radios` | foundation | keep | system | W1.3 |
| `cupertino-raster-math` | graphics | keep; absorbs `cupertino-glyph` | graphics | W2.3, W2.4 |
| `cupertino-render-api` | models | merged into `cupertino-scene` | — | W2.2 |
| `cupertino-scene` | models | keep; absorbs `cupertino-render-api` | models | W2.2 |
| `cupertino-screencast` | platform | merged into `cupertino-portal` (D32) | — | W6.11 |
| `cupertino-screenshot` | shell | keep; a separate Kit program (D10) | shell | W6.4 |
| `cupertino-seat` | platform | keep; its Smithay session implementation moves to `cupertino-server-backend` | platform | W5.6 |
| `cupertino-semantics` | protocol | merged into `cupertino-app-protocols` | — | W3.1 |
| `cupertino-server` | core | split into four crates (D25) | core | W5.1–W5.8 |
| `cupertino-sessiond` | services | keep | services | W5.11 |
| `cupertino-settings` | apps | keep | apps | W5.9, W7.2 |
| `cupertino-shell-client` | transport | keep; reached only through `cupertino-kit-shell` or by services | transport | W3.3 |
| `cupertino-stage` | models | merged into `cupertino-wm` | — | W2.1 |
| `cupertino-theme` | foundation | keep; absorbs `cupertino-ui-theme`; parses the specification it is given | foundation | W1.2 |
| `cupertino-thumbnails` | foundation | merged into `cupertino-icons` (D32) | — | W1.3 |
| `cupertino-trash` | foundation | keep | system | W1.3 |
| `cupertino-typeahead` | foundation | merged into `cupertino-ui-controls` | — | W1.5 |
| `cupertino-ui-abi` | ui | keep | ui | W4.6 |
| `cupertino-ui-atspi` | ui | merged into `cupertino-ui-wayland` | — | W4.4 |
| `cupertino-ui-controls` | ui | keep; absorbs `cupertino-typeahead` and `cupertino-filepicker`; gains the file views (D8) | ui | W1.5, W4.8 |
| `cupertino-ui-core` | ui | keep; absorbs `cupertino-ui-layout` and `cupertino-ui-platform` | ui | W4.2 |
| `cupertino-ui-gpu` | ui | merged into `cupertino-ui-render` | — | W4.3 |
| `cupertino-ui-layout` | ui | merged into `cupertino-ui-core` | — | W4.2 |
| `cupertino-ui-menus` | ui | merged into `cupertino-ui-wayland` | — | W4.4 |
| `cupertino-ui-platform` | ui | merged into `cupertino-ui-core` | — | W4.2 |
| `cupertino-ui-render` | ui | keep; absorbs `cupertino-ui-gpu`; the render thread (D4) | ui | W4.3, W4.6 |
| `cupertino-ui-test` | ui | keep; Kit's headless backend, a normal dependency of `cupertino-ui-abi` | ui | W4.6 |
| `cupertino-ui-text` | ui | keep; caches in D5's form | ui | W4.5 |
| `cupertino-ui-theme` | ui | merged into `cupertino-theme` | — | W1.2 |
| `cupertino-ui-wayland` | ui | keep; absorbs `cupertino-ui-menus` and `cupertino-ui-atspi` | ui | W4.4, W4.5 |
| `cupertino-wayland-client` | transport | keep; absorbs `cupertino-protocol-app` | transport | W3.2 |
| `cupertino-windowmanager` | shell | keep; a component; draws the stage strip | shell | W6.9 |
| `cupertino-wm` | models | keep; absorbs `cupertino-stage` and `cupertino-overview` | models | W2.1 |
| `cupertino-xkbcommon` (package `xkbcommon`) | foundation | deleted, with `third-party/xkbcommon-rs` (D31) | — | W1.7 |

New crates:

| Crate | Layer | Task |
|---|---|---|
| `cupertino-session-tests` | testing | W1.4 |
| `cupertino-kit-shell` | shellkit | W5.10 |
| `cupertino-audiod` | services; absorbs `cupertino-alsa` and `cupertino-pipewire` | W5.9, W6.16 |
| `cupertino-server-backend` | core | W5.6 |
| `cupertino-server-render` | core; absorbs `cupertino-decorations` | W5.7, W6.16 |
| `cupertino-server-input` | core | W5.8 |
| `cupertino-shell` | shell | W6.1 |
| `cupertino-shell-overview` | shell | W6.8 |
| `cupertino-shell-desktop` | shell | W6.10 |
| `cupertino-shell-agent` | shell | W6.13 |
| `cupertino-shell-dialogs` | shell | W6.11 |
| `cupertino-prompts` | shell | W6.14 |

Count: 90 crates today (the four test crates are already gone), 30 merged away or deleted, 12 new: 72 at the end. Every crate left meets one of D32's reasons.

## Rules for implementers

These apply to every task. A task's own text can narrow them, never widen them.

### Scope

- Your task is the box you were given. **Owns** lists the crates and modules you may change. **Touches** lists where you
  may make only mechanical edits: `use` paths, `Cargo.toml` dependency lines, the layer line, test module names.
- A move task moves code: item bodies stay as they are; only paths, visibility and imports change. No renames, no
  reformatting, no fixes on the way, no behaviour change. A task that changes behaviour says so in its title.
- Every task may also update, as mechanical edits: `source` paths in `docs/architecture/contracts.yaml`.
- A merge, move or deleted binary is searched for in the whole repository (`rg -n '<old name>'`, `xtask/`, `.config/`,
  `packaging/` and `docs/` included), and every mention is updated in the same task.
- Each task has a size (S: hours, M: a day, L: several days). L tasks are split into numbered steps; do them in order,
  one commit per step.

### No slop

- No compatibility layers: no re-export crate left behind, no `pub use old_crate::*`, no deprecated wrappers, no aliases
  for old names. A merged crate's directory is deleted in the same task and every consumer updated.
- No placeholders: no `todo!()`, `unimplemented!()`, empty modules, stub functions, commented-out code, "TODO" notes.
- No abstractions the task does not name: no new traits, generic parameters, builders, managers or registries.
- No item made more visible and no feature added to keep a test (D29).
- Comments say what the code does, never its history ("moved from", "previously", "new").
- Every public item keeps its documentation when it moves. The README of every crate you own says what it holds now.
- Where two copies of the same logic meet, keep one, route the other's callers to it, and say so in your report.
- In `shell` and `apps` code: no `std::thread::spawn`, no `block_on`, no file or bus call on the UI thread (D6).

### Tests (D29)

- Tests move with their code. A test that only uses the public API goes to `tests/`.
- In a `T` task: every `#[cfg(test)]` module in `src/` goes. A test that uses only the public API moves to `tests/`.
  A test of internals is removed; before removing it, check whether a public or program test covers its behaviour, and
  if none does, write that test first. List removed tests by the behaviour they covered.
- One test per behaviour a caller relies on, not per input: cases of one behaviour are rows of one table-driven test.
  No test restates a constant, a name, a version or a derived trait; no test of a type's plumbing (`Display` of each
  variant, `From`, `Clone`). A crate's tests stay proportional to its public behaviour; the report gives the count
  and says why it is that size.
- Tests are written from the current public API, never restored from git history.

### Stop and report instead of deciding

Stop, change nothing more, and report when:

- a dependency, cycle or consumer appears that the plan does not foresee;
- the work needs a behaviour change, a public type or a decision the plan does not name;
- the work needs something outside your Owns list, or a crate another running task owns;
- tests fail for a reason outside your change.

### Checks before you report

Only for what you changed; the lead runs the layers check, the gate and the reverse dependencies at each merge
(`cargo xtask ci --profile integration --base <batch base>`) and the full gates when a wave closes.

```bash
cargo xtask ci --profile agent --base <B> --dry-run  # first: see what the plan selects
cargo xtask ci --profile agent --base <B>            # fmt, clippy and nextest of the crates you changed; no layers, no gate
cargo run -q -p cupertino-layers -- --graph > crates/cupertino-layers/graph/layers.dot
cargo run -q -p cupertino-layers -- --write-remaining   # once W0.2 is merged
cargo run -q -p cupertino-layers
```

`<B>` is the commit your brief names as your base. Always pass it: without it the plan diffs against `origin/main`,
which is behind the local waves, so every crate merged away since then shows as paths "in no package" and selects the
whole workspace. That fallback is expected, not a problem to investigate.

`.config/validation.toml` sends a change to `Cargo.toml`, `Cargo.lock`, `.config/`, any `build.rs` or `third-party/` to
the whole workspace, whatever the scope; a crate merge always changes `Cargo.lock`. When the dry run says "checks the
whole workspace", do not run it: check the crates you own and touch by name instead,

```bash
cargo fmt -p <crate>...
CARGO_BUILD_WARNINGS=deny cargo clippy --locked --all-targets --all-features -p <crate>...
cargo nextest run --locked --all-features -p <crate>...
```

and say so in the report. At most two `cargo xtask ci` runs build at once across checkouts (`[limits] slots`); a run
waiting for a slot is not a failure. Commit the regenerated graph and remaining-work list with your change. No
benchmarks, no workspace-wide validation, no `sleep` or `pgrep` loops.

### Report

One message: what moved where; consumer files changed; the commands above with their last lines; anything you stopped
on and why. Commit on your branch; do not merge or push.

### Concurrency and merging (the lead)

- At most three implementers at a time, never two tasks owning the same crate or module. A task starts when everything
  in its **After** list is merged.
- The lead resolves conflicting mechanical edits at merge, and regenerates `Cargo.lock`, `layers.dot` and the
  remaining-work list instead of merging them by hand; a conflict in them is never resolved by editing.
- The lead ticks a task's boxes at merge with the merge commit's short hash and updates [Status](#status).
- A wave closes when its close task passes on merged `main`.

## Waves and tasks

Each task: **Owns**, **Touches** (mechanical edits only), **After**, size, and a **Done when** checklist. Tasks marked
"(lead)" are the lead's own.

### Wave 0: measure, check, document the fork

#### W0.1 Baselines (lead)

- [ ] Per-process PSS (from `/proc/<pid>/smaps_rollup`) of the server, every shell and built-in process, Files and
      Settings, and
      frame timings (`CUPERTINO_PROFILE` timelines), for each interaction that starts the shell, installed
      packages. Recorded in [Measurements](#measurements).
- [ ] The memory baseline: per-process PSS is compared against this machine's baseline by hand (heaptrack and `smaps_rollup`).

#### W0.2 `cupertino-layers` checks the target rules (M)

**Owns** `cupertino-layers`. **After** nothing.

- [x] (6b9e146c4) `Layer` gains `System`, `Shellkit`, `Testing`. `RULES` is the [Layers](#layers) table in full; the further rules,
      [Facades](#facades) and [External crates](#external-crates) are checks.
- [x] (6b9e146c4) Problems are listed in `crates/cupertino-layers/remaining.txt`, generated by `--write-remaining`; the check fails
      on a problem not in the file and on a line in the file that is no longer a problem (D22). `ENFORCED` lists closed
      layers, whose lines must be absent from the file.
- [x] (6b9e146c4) Today's table and exception list are deleted; the licence checks of D28 go (`REQUIRED_MPL_ROOTS`, the
      GPL-dependency check); the declared-licence check stays. Today's guard that the toolkit never reaches the shell
      protocols becomes a window-system boundary of the target rules.
- [x] (6b9e146c4) `disallowed-methods` for D6: one list in `cupertino-layers`, written to the `clippy.toml` of a `shell`
      or `apps` crate by `--write-clippy <crate>` once its banned calls are gone; until then a missing or differing file is
      a remaining-work line (D22), never allowed in an `ENFORCED` layer.
- [x] (6b9e146c4) For each new check, a recorded run where a seeded violation is caught (in the report, not committed).
- [x] (6b9e146c4) `crates/cupertino-layers/README.md` describes the rules, the remaining-work list and `ENFORCED`.

#### W0.3 Smithay as release plus documented patches (M)

**Owns** `third-party/smithay`, a new check in the `cargo xtask ci` gate. **After** nothing.

- [x] (f17dd132a) `third-party/smithay/PATCHES.md`: the upstream release, then one entry per change (what, why, files), matching the
      Unreleased section of its changelog; `patches/` holds each change as a patch file.
- [x] (f17dd132a) `UPSTREAM.sha256` records each file of the upstream release (from its published package).
- [x] (f17dd132a) The check fetches nothing: on a copy of the tree it reverses the patches in order and compares every file with
      `UPSTREAM.sha256`, failing on any other difference. Run by `cargo xtask ci`.

#### W0.T Tests of `cupertino-layers`

As in W1.T. Done in 6b9e146c4: 31 tests in `tests/` over fixture workspaces written at test time.

#### W0.close (lead)

- [x] (9cd523297) `cargo xtask ci --profile integration` passes; `remaining.txt` committed.

### Wave 1: foundation, system, testing

#### W1.1 `cupertino-keyboard` absorbs `cupertino-modifiers` and `cupertino-key-bindings` (M)

**Owns** `keyboard`, `modifiers`, `key-bindings`; the keyboard module of `defaults`; the binding-file reading in
`ui-text`. **Touches** `controlcenter dock files input kit launcher login menu-model menubar notifications osd
screenshot server settings ui-menus ui-platform ui-text ui-wayland`. **After** W1.3.

- [x] (d0d651793) Both crates are modules of `cupertino-keyboard`; their directories are gone; consumers import from it.
- [x] (d0d651793) `cupertino-keyboard` has no I/O and no dependency on `cupertino-defaults`. `KeyboardSettings` and the reading of
      `keyboard.conf` live in `cupertino-defaults`' keyboard module; the binding file is read by `cupertino-ui-text`
      and parsed by `cupertino-keyboard`. Anything else that does I/O is reported.

#### W1.2 `cupertino-theme` absorbs `cupertino-ui-theme` (M)

**Owns** `theme`, `ui-theme`, the specification lookup in `assets`. **Touches** `kit ui-controls ui-core ui-test
controlcenter decorations files glass menubar notifications polkit-agent server settings ui-gpu ui-render dock launcher
login osd portal screenshot`. **After** W1.3.

- [x] (50a18e5bd) The tokens (`ThemeTokens`, `ColorRole`, `StateVariant`, appearance) are a module of `cupertino-theme`. The
      thread-local activation value of `cupertino-ui-theme` moves to `cupertino-ui-core`, which owns runtime state.
- [x] (50a18e5bd) `cupertino-theme` parses a specification it is given; `cupertino-assets` finds the files
      (`CUPERTINO_THEME_DIR`, `CUPERTINO_SPEC_DIR`, the installed path) and offers theme loading that composes both;
      callers of `Theme::load` and `Theme::from_dir` use it.
- [x] (7ecd5785a) Durations, curves and springs of the design are named in `cupertino-theme`; `cupertino-animation` holds the maths. The timing presets moved in 50a18e5bd; the five cubic-bezier constants and the default spring move in W1.6.

#### W1.3 The `system` layer (S)

**Owns** layer lines and dependencies of `apps assets defaults diagnostic-events icons peer radios trash`; `build-info`,
`profile`, `thumbnails`. **Touches** `cli server settings files launcher ui-wayland`. **After** W0.close.

- [x] (a02b1972e) Each declares `layer = "system"` and depends only on `foundation` and `system` crates.
- [x] (a02b1972e) `cupertino-build-info` is a module of `cupertino-diagnostic-events`, with its build script; so is
      `cupertino-profile`, its frame timelines on the same per-instance state.
- [x] (a02b1972e) `cupertino-thumbnails` is a module of `cupertino-icons`.

#### W1.4 The `testing` layer and `cupertino-session-tests` (M)

**Owns** the layer line of `capture`; new `cupertino-session-tests`, the home of tests that start programs from more
than one crate. **Touches** `menubar server` (dev-dependencies), `contracts.yaml`. **After** W0.close.

- [x] (44782bc91) `capture` and `cupertino-session-tests` declare `layer = "testing"`; nothing has a normal or build dependency on
      them.
- [x] (44782bc91) Any test that starts programs from more than one crate lives in `cupertino-session-tests`;
      `cupertino-menubar` no longer depends on `cupertino-files` or `cupertino-settings`; `cupertino-server`'s tests start
      no other crate's program.

#### W1.5 `cupertino-ui-controls` absorbs `cupertino-typeahead` and `cupertino-filepicker` (S)

**Owns** `ui-controls typeahead filepicker`, the exports of `kit`; the `app` features of `menubar settings portal`.
**After** W0.close.

- [x] (9b0f48e58) Both are modules of `cupertino-ui-controls`; their directories are gone; `menubar`, `settings` and `portal` reach
      them through `cupertino-kit`, now a normal dependency of all three: their `app` features and the builds without
      Kit go (the portal leaves Kit in W6.11).

#### W1.6 Foundation purity (S)

**Owns** `animation geometry glyph menu-model keyboard theme`. **After** W1.1, W1.2. (`glyph` joins `graphics` in W2.4.)

- [x] (7ecd5785a) Each passes the purity check; whatever does not moves to its caller or to a `system` crate, listed in the report.

#### W1.7 Keyboard layouts from libxkbcommon (M, behaviour change)

**Owns** `cupertino-xkbcommon`, `third-party/xkbcommon-rs`, the root `Cargo.toml` patch section, the server's keymap
compile, packaging dependencies and the package recipes' licence files. **Touches** `wayland-client portal kit`
(its package's dependencies). **After** W0.close.

- [x] (8efe5f162) Before the swap, each difference listed at the top of `cupertino-xkbcommon/src/xkb/mod.rs` is checked against the
      C library, and every caller that relied on one is made safe: names asked for out of range (the C library returns
      null), activity of an unknown layout or LED, empty options, keysym names. Listed in the report.
- [x] (8efe5f162) The `xkbcommon` patch entry and both directories are gone; Smithay, the client transport and the portal use the
      `xkbcommon` crate over libxkbcommon; packaging depends on libxkbcommon; `docs/src/architecture.md` says so.
- [x] (8efe5f162) A layout that does not compile leaves the last working layout in place and logs the failure. Program test.

#### W1.T Tests of wave 1's crates

One task per crate, after its structural task: `keyboard theme apps assets defaults diagnostic-events icons peer
radios trash animation geometry menu-model`. (`glyph` is tested with `raster-math` in W2.T, `capture`
with `kit` in W4.T.)

- [x] (eb41c041e) The crate follows [Tests](#tests-d29): no `#[cfg(test)]` left in `src/`, removed tests listed by behaviour,
      behaviour nothing else covered now covered by a public or program test.

#### W1.close (lead)

- [x] (eb41c041e) `ENFORCED` gains `Foundation`, `System`, `Testing`.
- [x] (eb41c041e) `docs/src/architecture.md`'s layer table, the layer rule in the repository's agent instructions and the READMEs of
      absorbing crates match.
- [x] (eb41c041e) `cargo xtask ci --profile integration` passes on merged `main`.

### Wave 2: graphics, models, platform

#### W2.1 `cupertino-wm` absorbs `cupertino-stage` and `cupertino-overview` (S)

**Owns** `wm stage overview`. **Touches** `server dock windowmanager`. **After** W1.close.

- [x] (ebd29b793) Both are modules of `cupertino-wm`; their directories are gone.

#### W2.2 `cupertino-scene` absorbs `cupertino-render-api` (S)

**Owns** `scene render-api`. **Touches** `server`. **After** W1.close.

- [x] (04eafdc52) The `Renderer` trait and its types are a module of `cupertino-scene`.
- [x] (04eafdc52) The report says whether the server's frames go through `Renderer` or past it, with the call sites:
      every scene frame goes through `impl Renderer for Gles` (`render/gles.rs`); only the cross-GPU path's
      presentation of the copied buffer on the display card (`backend/udev.rs`, `render_frame`) goes past it.

#### W2.3 Conformance of `graphics`, `models`, `platform` (S)

**Owns** layer lines and dependencies of `gpu raster-math control-model displays effects input alsa bluetooth devices
evdev netlink pipewire screencast seat`. **After** W1.close.

- [x] (af87c33b8) `models` crates pass the purity check; `cupertino-effects`' raster helper is a dev-only module.
- [x] (af87c33b8) `cupertino-seat` keeps its Smithay dependency until W5.6 moves the session implementation; it is the one line
      left in `remaining.txt` for `platform`.

#### W2.4 `cupertino-devices` absorbs `cupertino-evdev`; `cupertino-raster-math` absorbs `cupertino-glyph` (S)

**Owns** `devices evdev raster-math glyph`. **Touches** `powerd server diagnostics seat ui-text decorations`. **After**
W2.3.

- [x] (e19357510) `cupertino-evdev` is a module of `cupertino-devices`; `cupertino-glyph` one of `cupertino-raster-math`, which
      stays free of I/O.

#### W2.T Tests of wave 2's crates

`wm scene gpu raster-math control-model displays effects input alsa bluetooth devices evdev netlink pipewire screencast
seat`, as in W1.T.

- [x] (0ddf758b5) The crates follow [Tests](#tests-d29); about 1280 public tests. Found and fixed: the atlas's and the
      screencast size check's overflow on huge sizes, and `glyph::rasterize` returning a mask for a non-finite outline.

#### W2.close (lead)

- [x] `ENFORCED` gains `Graphics`, `Models`; `Platform` joins when W5.6 lands. Docs and gates as in W1.close.

### Wave 3: protocol, transport

#### W3.1 `cupertino-app-protocols` absorbs `cupertino-semantics` (S)

**Owns** `app-protocols semantics`. **Touches** `control protocol-app server shell-client ui-core ui-platform`.
**After** W2.close.

- [x] (ea73318f8) `cupertino-semantics` is `cupertino_app_protocols::semantics`; its directory is gone.

#### W3.2 `cupertino-wayland-client` absorbs `cupertino-protocol-app` (S)

**Owns** `wayland-client protocol-app`. **Touches** `controlcenter launcher menubar notifications polkit-agent portal
shell-client ui-menus ui-wayland`. **After** W3.1.

- [x] (75e3d9677) `cupertino-protocol-app` is `cupertino_wayland_client::app`; its directory is gone.

#### W3.3 Conformance of `protocol`, `transport` (S)

**Owns** layer lines and dependencies of `protocols login-ipc shell-client`. **After** W3.2.

- [x] (75e3d9677) No `protocol` or `transport` line was left in `remaining.txt` after W3.2; no change was needed.

#### W3.T Tests of wave 3's crates

`app-protocols wayland-client protocols login-ipc shell-client`, as in W1.T.

- [x] (548058785) The crates follow [Tests](#tests-d29); about 380 public tests, the wire tests against an in-process
      server on a socket pair. Found and fixed: `Event::Reset` and the semantic `Value::Protected` took unknown
      fields; `Layer::animate` sent curves the server refuses, which ends the connection.

#### W3.close (lead)

- [x] `ENFORCED` gains `Protocol`, `Transport`. Docs and gates as in W1.close.

### Wave 4: render and the toolkit

The toolkit is the foundation everything above stands on; its runtime and API are settled here, once.

#### W4.1 Conformance of `render` (S)

**Owns** layer lines and dependencies of `cursor decorations glass`. **After** W3.close.

#### W4.2 `cupertino-ui-core` absorbs `cupertino-ui-layout` and `cupertino-ui-platform` (M)

**Owns** `ui-core ui-layout ui-platform`. **Touches** `kit ui-controls ui-test ui-atspi ui-wayland polkit-agent`.
**After** W3.close.

#### W4.3 `cupertino-ui-render` absorbs `cupertino-ui-gpu` (S)

**Owns** `ui-render ui-gpu`, the `cupertino-ui-gpu` entry in the root `Cargo.toml` profiles. **Touches** `kit
ui-wayland`. **After** W4.2.

- [ ] The two renderers are modules of one crate; their agreement tests still run.

#### W4.4 `cupertino-ui-wayland` absorbs `cupertino-ui-menus` and `cupertino-ui-atspi` (S)

**Owns** `ui-wayland ui-menus ui-atspi`. **Touches** `kit files login polkit-agent menubar`. **After** W4.3.

#### W4.5 Kit runtime: re-entrant, tasks, shared caches (L)

**Owns** `kit ui-core ui-wayland ui-text ui-render icons diagnostic-events apps` (`chromium.rs`, `launch.rs`).
**Touches** every crate that calls `diag::init`. **After** W4.4.

1. [ ] Every process-wide static in Kit and the crates it uses (`rg -n 'static .*(Mutex|RwLock|Cell|OnceLock|Atomic)'`)
       is listed in the report with its fate under D3: a field of the runtime object (profiling state, renderer choice,
       device-loss counter), the thread's scope (the diagnostic emitter; the panic hook stays one per process), a cache
       in D5's form (`cupertino-apps`' Chromium table), or allowed as it is (id counters, immutable tables, the C
       interface's registry). `cupertino-apps`' launch starts no thread; the child is reaped by its caller.
2. [ ] Font, glyph and icon caches in D5's forms.
3. [ ] `kit::task` and the bus integration of D6; `kit::time` for local time.
4. [ ] Program test: two Kit instances in one process draw and take input independently; a panic in one
       leaves the other drawing.

#### W4.6 One render thread per process (L, behaviour change)

**Owns** `gpu ui-render ui-wayland ui-test ui-abi`. **After** W4.5.

1. [ ] `cupertino-ui-render`: one device and one context per process, owned by the render thread; `cupertino-gpu`
       keeps working with a lent device.
2. [ ] Presenting and committing move to the render thread in D4's order; UI threads hand over recorded display lists
       with their configure serials; CPU surfaces are rasterised on the CPU pool; the heartbeat reaches the watchdog.
3. [ ] Device loss handled once per process.
4. [ ] Frame timings and per-process PSS of Files and Settings no worse than W0.1 (lead compares with a profile).

#### W4.7 Kit's public API (L, design then code)

**Owns** `kit`. **After** W4.6.

1. [ ] A written proposal in `crates/cupertino-kit/README.md`: every item `shell` and `apps` crates use today from
       Kit-private crates, where each goes in Kit, the motion API by catalogue names (D7), the `test` feature, and what
       is refused per the must-not-expose list. Reviewed by a reviewer who did not write it and approved by the owner;
       approval in the [decision log](#decision-log).
2. [ ] The approved API, and nothing else.

#### W4.8 File views in Kit (L)

**Owns** `ui-controls`; the view and operation code of `files` (`kit/` views, `operations`, `open`, `rename`,
`name_edit`, `trash`, `thumbnail`, `places`, `notice`); `portal` (`file_chooser.rs`, `dialog/`). **After** W4.7.

1. [ ] The views (icon grid, list, columns) and the operations of D8 move from `cupertino-files` to
       `cupertino-ui-controls`, reached through Kit.
2. [ ] The open and save panel is built from the portal's chooser and the file picker's model on those views; where
       two copies of the same logic meet, one stays (listed in the report).
3. [ ] Files and the portal use them from Kit; behaviour and look unchanged (review sheet, Files and chooser
       captures).

#### W4.9 Host and transaction protocols: specification (M, design, no code)

**Owns** new `docs/architecture/shell-host.md`, the protocol XML in `crates/cupertino-protocols/protocols/`. **After**
W4.7 step 1 (its approved types).

- [ ] The host protocol: the host role; the component list sent by the server; asking for a component connection; bus
      name announcement (D20); busy, responsive, unresponsive and lost events, and the host's restart request (D16,
      D17); separate hosts and the bridge of the host bus (D18, D26).
- [ ] The component list: its schema in the `shell` section of `compositor.json` (which components run, which run
      alone; a user file may turn a component off but names no commands or roles), a table of every component with its
      roles and bus names, how `dev_folder` points the server at a development build of the host, the built-in programs
      the server still starts, and `greeter.json` for the login session.
- [ ] The transaction protocol meeting [Transactions](#transactions), every question answered.
- [ ] `cupertino-kit-shell`'s API: `Component`, layer surfaces and roles, lock surfaces, the surfaces of the
      privileged programs (prompts, screenshot) and capture, window and application list, the window manager's
      protocol, holes, holds (what a restarted component takes over), transactions, gesture binding, the host bus
      (D26), the host's thread and child-process API.
- [ ] Reviewed and approved as in W4.7.

#### W4.10 Protocol bindings (S)

**Owns** the scanner invocations in `cupertino-protocols`. **After** W4.9.

- [ ] Client and server bindings for both protocols are generated and compile; nothing uses them yet.

#### W4.11 `cupertino-kit` absorbs `cupertino-capture` (S)

**Owns** `capture`, `kit` (`Cargo.toml`, the new module). **Touches** the capture examples of `login files kit`.
**After** W4.7.

- [ ] The capture command's shared part is a module of `cupertino-kit` behind the `test` feature, beside the headless
      backend; the examples enable the feature as a dev-dependency.

#### W4.T Tests of wave 4's crates

`cursor decorations glass kit ui-core ui-controls ui-text ui-render ui-wayland ui-test ui-abi`, as in W1.T.

#### W4.close (lead)

- [ ] `ENFORCED` gains `Render`, `Ui`. Docs and gates as in W1.close.

### Wave 5: compositor, services, the shell toolkit

Tasks owning `cupertino-server` run one at a time, in the order W5.1, W5.2, W5.3, W5.4, W5.5, W5.6, W5.7, W5.8. Wave 6
starts when W5.2, W5.5, W5.9 and W5.10 are merged; the server split continues beside it, since no shell crate depends
on a server crate.

#### W5.1 The compositor never waits without a deadline (M, behaviour change)

**Owns** `server`. **After** W4.close.

- [ ] A stall recorder: with `CUPERTINO_PROFILE`, any turn of the compositor's loop longer than 4 ms is logged with what
      it ran. Recorded in the report over the common interactions, before and after.
- [ ] Every wait found by the recorder, and by `rg -n 'recv\(|join\(|wait\(|block_on|blocking::|std::fs::'
      crates/cupertino-server/src` on the loop's thread, has a deadline and a fallback or moves off the thread.
- [ ] Program test: a client and a shell component that stop answering do not delay the compositor's frames.

#### W5.2 Host protocol, server side (L, behaviour change)

**Owns** `server`, `peer`. **After** W5.1, W4.10.

1. [ ] Host role, component list over the protocol, per-component connections and roles, several hosts (D18).
2. [ ] Bus identity per component (D20); `cupertino-peer` resolves callers to components.
3. [ ] Busy, unresponsive and lost handling (D17); restart counts kept by the server (D16).
4. [ ] Holds (D19).
5. [ ] Program test with a stand-in host: a dropped component keeps the menu bar's space and picture; a
       transient surface is dismissed and focus returns; an unresponsive component gets no input.

#### W5.3 Transactions, server side (L, behaviour change)

**Owns** `server`. **After** W5.2.

1. [ ] Transactions and deadlines as specified; late joiners start at the original time.
2. [ ] Gesture-bound transactions.
3. [ ] Holes from window content and thumbnails.
4. [ ] Program tests: two participants start on one frame; a late one joins in step; a gesture settles with its
       velocity.

#### W5.4 `Session1` and `Displays1` (M, behaviour change)

**Owns** `server` (`quit`, the session bus service, output management), `login` (`logout.rs`, `control.rs`). **After**
W5.3.

- [ ] `Session1`: log out, restart, shut down, sleep, quitting applications first (D13). The quitting moves from
      `cupertino-login`'s `logout.rs`; `Login1`'s log-out, power-key and request methods call `Session1` until W6.13
      removes them.
- [ ] `Displays1`: what Settings configures today through output management (D14).
- [ ] `docs/architecture/contracts.yaml` lists both.

#### W5.5 What the components need from the server (M)

**Owns** `server` (`config`, `capture/launch`, `shell`, `window_manager`), `defaults` (the `shell` section of
`compositor.rs`). **After** W5.4.

- [ ] The component list of W4.9: the server starts a component through the host when the list says so, and as its own
      process otherwise. Each component task of wave 6 switches its own entry; W6.close removes the process path. This
      is the plan's one period with two paths.
- [ ] `cupertino-screenshot` is started on demand.
- [ ] The server side of the overview transition and of the stage strip (transactions, holes, the window manager's
      surface); the server keeps drawing its own until W6.8 and W6.9 switch to the components.

#### W5.6 to W5.8 Server split (D25)

Step 0, in W5.6, before any code moves: a script prints the server's module graph (which module names which), and the
task proposes the three groups from it under D25's direction, including where `overlays` goes (it leaves the server in
W6.12). The lead records the approved groups in the [decision log](#decision-log); the lists below are the starting
proposal, not the rule. Then each step moves one group: first, inside `cupertino-server`, the modules stop naming the
server's state (`crate::state`, `Cupertino`) and call against D25's direction only through the lower crate's trait;
then the modules move to the crate. A module that does not fit its group is reported.

| Task | Crate | Steps (one commit each) | Size |
|---|---|---|---|
| W5.6 | `cupertino-server-backend` | (a) the Smithay session implementation from `cupertino-seat`; (b) `schedule`, `latch`, `sync_file`, `output_depth`, `drm_trace`, `frame_stats`, `acquire_waits`, `software_render`, `gpu_reset`; (c) `displays/`; (d) `backend/` | L |
| W5.7 | `cupertino-server-render` | (a) `frame_fill`, `popup_linger`, `drag_return`, `suspended`, `toolkit_programs`, `cursor`, `fx`; (b) `decor`, `decor_access`; (c) `render/`, `frame` | L |
| W5.8 | `cupertino-server-input` | (a) `idle`, `hung`, `pointer_constraints`, `shortcut_inhibit`, `popup_grab`, `global_shortcuts`; (b) `kbd`, `kbd_filters`, `input_method`, `kbd_ui`; (c) `input`, `events` | L |

**Owns** `server`, `seat` (W5.6 only) and the new crate. **After** W5.5, then each the one before.

- [ ] `rg -n 'crate::state|Cupertino\b'` finds nothing in the moved modules.
- [ ] Frame timings unchanged within noise (lead compares with a profile after merge).

#### W5.9 `cupertino-audiod` (M, behaviour change)

**Owns** new `audiod`; the sound code of `settings`. **After** W4.close. The control panel and the on-screen displays
keep their sound code until W6.5 and W6.2 move them to the service; W6.16 then folds the two crates in.

- [ ] Serves `org.cupertino.Audio1`, bus-activated; the mixer and default-output logic moves from Settings.
- [ ] Settings' sound pane calls the service; Settings no longer depends on `cupertino-alsa` or `cupertino-pipewire`.
- [ ] Packaging and `contracts.yaml` list the binary, the activation file and the interface.

#### W5.10 `cupertino-kit-shell` (L)

**Owns** new `kit-shell`. **After** W4.10, W4.7.

- [ ] The API of W4.9 over the generated bindings; nothing beyond it.

#### W5.11 Conformance of `services` (S)

**Owns** layer lines and dependencies of `btd netd powerd sessiond`; `bluetooth`, `netlink`. **After** W4.close.

- [ ] `cupertino-bluetooth` is a module of `cupertino-btd`, `cupertino-netlink` one of `cupertino-netd`.

#### W5.12 `cupertino-control` is a service (S)

**Owns** `control`. **After** W4.close.

- [ ] `layer = "services"`; dependencies allowed by its row.

#### W5.T Tests of wave 5's crates

A crate with more than 150 tests in `src/` gets one `T` task per group of top-level modules, at most 150 tests each;
the server's four crates each get theirs after their split step.

`server server-backend server-render server-input audiod kit-shell btd netd powerd sessiond control`, as in W1.T.

#### W5.close (lead)

- [ ] After W5.8: `ENFORCED` gains `Core`, `Services`, `Shellkit`, `Platform`. Docs and gates as in W4.close.

### Wave 6: the shell host, its components, the separate programs

Every component task has this shape:

- **Owns** the component's crate (and the new crate where named). **Touches** the component list in `cupertino-shell`,
  `packaging/files/etc/cupertino/compositor.json`, the package recipes' binary list, `contracts.yaml`. **After**
  W6.2 (the memory checkpoint), unless stated.
- [ ] A library exposing one `Component`; its `main` and binary target are gone; its entry in the component list
      switches to the host (W5.5).
- [ ] `#![forbid(unsafe_code)]`; the `unsafe` it has today (menu bar, launcher, notifications) goes behind a safe API in
      a
      lower crate.
- [ ] Dependencies within the `shell` row and [Facades](#facades); no other component crate.
- [ ] Everything drawn goes through Kit; every motion uses catalogue names, through Kit or a transaction; no duration,
      curve or spring literal in the crate.
- [ ] No thread started, no `block_on`, no file or bus call on its UI thread; Clippy's `disallowed-methods` passes.
- [ ] Its bus names are owned on its own connection; `contracts.yaml` names `cupertino-shell` as the binary.
- [ ] Its README says what a restart loses (D21).
- [ ] Program test: a panic injected mid-paint, mid-dispatch and on a helper thread restarts it alone while
      the others keep drawing; host memory is stable after 50 such restarts.
- [ ] Its drawing matches before and after, light and dark, checked in captures.

#### W6.1 `cupertino-shell`, the host (L)

**Owns** new `shell`, `osd`. **After** W5.2, W5.5, W5.9, W5.10.

1. [ ] Component threads, shared runtime, panic containment, the busy watchdog, helper-thread and child-process API,
       the host bus.
2. [ ] `--only` (D18) and `cupertino shell crash <component>` in development builds.
3. [ ] The pilot: the on-screen displays run in the host, sound through `cupertino-audiod`, with the component
       checklist above met and every program test passing.

#### W6.2 Memory checkpoint (lead)

**After** W6.3.

- [ ] The host with `osd` and `notifications` against the two processes of W0.1. Go when its PSS is at most 60% of
      theirs and frame p99 is no worse; the lead records the numbers and the result in the
      [decision log](#decision-log). No-go stops wave 6 and goes to the owner.

#### W6.3 to W6.11 Components

At most three at a time, in this order:

| Task | Component | Notes | After |
|---|---|---|---|
| W6.3 | `notifications` | D21; before the checkpoint | W6.1 |
| W6.4 | `screenshot` | not a component: a separate Kit program started on demand (D10); same checklist minus the host items; gains the colour picker (from `cupertino-portal`'s `pick_overlay.rs`) and the screen-sharing picker | W6.2, W5.5 |
| W6.5 | `controlcenter` | audio through `cupertino-audiod`, power through `Session1` | W6.2, W5.4, W5.9 |
| W6.6 | `launcher` | | W6.2 |
| W6.7 | `menubar` | | W6.2 |
| W6.8 | `dock` and new `shell-overview` | the overview's chrome and gestures move out of the dock; windows are holes; transitions are gesture-bound transactions | W6.2, W5.5 |
| W6.9 | `windowmanager` | gains a surface for the stage strip; placements as transactions | W6.2, W5.5 |
| W6.10 | new `shell-desktop` | the desktop items, from `cupertino-files --desktop`, on Kit's file views | W6.2, W4.8 |
| W6.11 | new `shell-dialogs`, `portal` as a service | `cupertino-screencast` becomes a module of the portal; file and application chooser in the component; the screenshot and global-shortcut consent dialogs in `cupertino-prompts`; the colour and sharing pickers in `cupertino-screenshot`; private bus interfaces between them, in `contracts.yaml`; the portal has no `#[cfg(test)]` left, since `services` is closed | W6.2, W6.4, W6.14 |

#### W6.12 Overlays leave the server (M, behaviour change)

**Owns** `osd`, `server-input` (`kbd_ui`, `input_method`). **After** W6.2, W5.8.

- [ ] Before code: a design note in `docs/architecture/shell-host.md` on how the switcher holds the keyboard while its
      modifier is down and how the picker reaches the focused text field, approved as in W4.7.
- [ ] The input-source switcher and the accent picker are drawn by `osd`; the server routes keys and the text-input
      protocol and draws neither.
- [ ] [What the compositor draws](#what-the-compositor-draws) checked against the code.

#### W6.13 `cupertino-login` on Kit, session agent out (L)

**Owns** `login`, new `shell-agent`. **After** W6.2, W5.4.

1. [ ] Where `cupertino-login`'s modules go:

       | Module | Goes to |
       |---|---|
       | `restore`, `autostart` | `shell-agent` |
       | `panel`, `interrupt`, `power_button`, `low_battery` | `shell-agent`, shown when `Session1` or the power key asks |
       | `logout` | `Session1` (W5.4) |
       | `control` (`Login1`) | `Lock` stays in `cupertino-login`; the other methods are gone (D13) |
       | the consent prompt | `cupertino-prompts` (W6.14) |
       | everything else | stays: login and lock screens |
2. [ ] Login and lock screens draw through Kit and `cupertino-kit-shell` (lock surfaces); no `tiny-skia`,
       `cupertino-raster-math` or `cupertino-seat`; power through `cupertino-sessiond` (login screen) or `Session1`
       (lock screen).

#### W6.14 `cupertino-prompts` (L)

**Owns** `polkit-agent`, new `prompts`; the consent prompt code in `login`. **After** W6.13.

1. [ ] One process with the authentication and keyring prompts, drawn through Kit.
2. [ ] The desktop-control consent prompt moves here from `cupertino-login`; the screenshot and global-shortcut consent
       dialogs from `cupertino-portal` (W6.11 calls them).

#### W6.15 Everything that names the old shell binaries (M)

**Owns** `cli` (`shell.rs`, `dev_folder.rs`), `diagnostics` (`dev_builds.rs`), `docs/src/building.md`, the tests in `cupertino-session-tests`. **After** each component as it lands.

- [ ] Each names the host and its components; `cupertino shell list` and `restart` act on components.

#### W6.16 Crates left with one user (S)

**Owns** `decorations alsa pipewire`, the new modules in `server-render` and `audiod`. **After** W6.14 (the prompts stop
using the decorations), W6.5 and W6.2 (the last users of the sound crates besides the service).

- [ ] `cupertino-decorations` is a module of `cupertino-server-render`; `cupertino-alsa` and `cupertino-pipewire` are
      modules of `cupertino-audiod`. Before each move, `cargo tree -i` shows the absorbing crate as the only user.

#### W6.T Tests of wave 6's crates

Every crate of wave 6, as in W1.T.

#### W6.close (lead)

- [ ] `ENFORCED` gains `Shell`. Old binaries are gone from packaging and `contracts.yaml`.
- [ ] Memory measured against W0.1; numbers in [Measurements](#measurements).
- [ ] `docs/src/architecture.md`'s process table matches [Processes](#processes). Gates as in W4.close.

### Wave 7: applications, tools, documents

#### W7.1 `cupertino-files` through Kit (M)

**Owns** `files`. **After** W6.close. Desktop mode is gone (W6.10).

#### W7.2 `cupertino-settings` through Kit (M)

**Owns** `settings`. **After** W6.close. Displays through `Displays1`, power through `Session1`.

#### W7.3 Conformance of `tools` (S)

**Owns** layer lines and dependencies of `cli diagnostics gpu-shaders gpu-slice`. **Touches** `packaging/arch/*/PKGBUILD`,
`contracts.yaml`, `docs/src/diagnose.md`. **After** W6.close.

- [ ] `cupertino-diagnostics` is a module of `cupertino-cli`; the deprecated `cupertino-diagnose` program is gone from
      the tree, packaging and the documents, which name `cupertino diagnose`.

#### W7.T Tests of wave 7's crates

`files settings cli gpu-shaders gpu-slice`, as in W1.T.

#### W7.close (lead)

- [ ] `ENFORCED` lists every layer; `remaining.txt` is empty and deleted with its check.
- [ ] `docs/src/architecture.md`, the repository's agent instructions, `CONTRIBUTING.md`, `cupertino-layers/README.md`
      and every touched crate's README describe the result.
- [ ] `cargo xtask ci --profile full` passes on merged `main`.

## Measurements

Filled in by W0.1, W6.2 and W6.close.

| Scenario | Processes replaced: PSS sum today | After | Ratio | Frame p99 today | After |
|---|---|---|---|---|---|
| Idle session, installed packages 0.1.0-202611162246, 4K at scale 1.5 | 179 692 kB (dock 53 868, launcher 42 164, menubar 28 916, notifications 19 477, files `--desktop` 14 772, controlcenter 9 989, osd 9 675, windowmanager 831); login, polkit agent and keyring prompter are not dumpable, read as root | | | not yet recorded | |

The server measured 161 185 kB in the same session. Frame timings need a session started with `CUPERTINO_PROFILE` set.

Processes replaced by the shell host: `cupertino-dock`, `-menubar`, `-launcher`, `-notifications`, `-controlcenter`,
`-osd`, `-windowmanager`, `cupertino-files --desktop`, and the session-agent part of `cupertino-login`. Replaced by
`cupertino-prompts`: `cupertino-polkit-agent`, `cupertino-keyring-prompter`.

## Glossary

- **Component**: one part of the shell running in the shell host as a Kit instance on its own thread.
- **Shell host**: the `cupertino-shell` process.
- **Kit instance**: one runtime object of the toolkit with its UI thread and connections.
- **Transaction**: changes from one or more participants that the compositor starts on the same frame.
- **Hole**: an area of a component's layer where the compositor draws a window's content.
- **Hold**: the server keeping a dropped component's persistent surfaces on screen.
- **Facade**: the one crate through which a higher layer reaches the toolkit (`cupertino-kit`, `cupertino-kit-shell`).
- **Remaining-work list**: `crates/cupertino-layers/remaining.txt`, the generated list of violations still to fix.
- **Program test**: a test that starts whole processes (headless server, shell host, programs) and checks what they do.

## Decision log

One line per change to a decision or approval of a design: date, number, what, who.

- 2026-11-17: first version.
- 2026-11-17: rewritten after an independent review: one render thread per process (D4), shared cache forms (D5), one
  concurrency model (D6), privileged interface in separate processes (D10), hang handling by isolation (D17, D18), holds
  per component (D19), identity per component (D20), remaining-work list (D22).
- 2026-11-17: owner: licences are not a constraint (D28); tests at public boundaries only, everything else removed
  (D29); keyboard layouts from libxkbcommon (D31); Smithay patches documented now, offered upstream later (D30).
- 2026-11-17: when a crate may exist (D32); thirteen more merges, 75 crates at the end (72 once the test crates were deleted).
- 2026-11-17: lead: a `shell` or `apps` crate gets its `clippy.toml` (D6) when its banned calls are gone; until then
  the missing file is a remaining-work line (D22), so clippy does not fail on code a later task removes.
- 2026-11-17: lead: the gate already keeps test code out of `src/`, so `cupertino-layers` has no test-placement check
  and the `T` tasks only make sure removed behaviour is covered by public or program tests.
- 2026-11-17: lead: the portal keeps `xkbcommon` as a remaining-work line until it becomes a service (W6.11); a
  server whose environment layout does not compile at start uses a US keymap (D31 has no last layout then).
- 2026-11-17: lead: the test suite was restarted from nothing before wave 1, so each `T` task writes the public tests
  of its crate's documented behaviour.
- 2026-11-17: lead: the window activation value is a field of `ColorTokens` that the runtime sets while colours cross,
  not a thread-local in `cupertino-ui-core` (D3: no per-thread state a second instance would share); the design tokens
  are `cupertino_theme::ui`.
- 2026-11-17: lead, after the review of waves 0 and 1: `standard::menu_bar` leaves out the groups an application's
  features leave empty (found by W1.T; no consumer depended on them). D6's rule that `system` crates start no threads
  becomes a check; the eight thread starts found (the radios and peer watchers hand work to the bus connection's
  executor, which D6 removes with W4.5's bus integration) are remaining-work lines, so `system` leaves `ENFORCED` until W4.5
  moves them onto Kit's task API and closes it again. The remaining-work list may only lose lines against the base
  (`cargo xtask remaining`), except lines of a rule the base's check did not have.
- 2026-11-17: second independent review: statics allowed under D3, render-thread commits and CPU drawing (D4), two
  pools and a structural ban on blocking calls (D6), file operations in Kit (D8), the session agent's and the
  portal's dialogs placed (D9, D10, D11, D13), audio started on use (D15), self-restart and hang hand-back (D16, D17),
  component identity (D20), the server split's direction and measured groups (D25), the host bus across hosts (D26),
  licence checks (D28), the Smithay patch form (D30); task order of W5.5 and W6.1 to W6.3, the component list schema,
  checks implementers can pass.
- 2026-11-17: owner: while the refactor runs, a change may add remaining-work lines, and temporary reaches across
  layers (a facade re-export ahead of its API, a direct external crate ahead of its service) are expected, not owner
  decisions. `cargo xtask remaining` reports added lines and passes (`GROWTH_FAILS` in `xtask/src/remaining.rs`); the
  last wave sets it and the list only shrinks from then. D22 amended.
- 2026-11-17: lead, after the review of wave 2: the evdev framer undoes a discarded partial frame's key and switch
  transitions, so a resync reports what consumers missed (a behaviour change); the pipewire tests' stand-in program is a
  Rust example, not shell; `TableCurve::new` refuses empty or non-positive entries, `mask_bounds` inverted corners; the
  font tests of `glyph` live in `cupertino-ui-text`. Left: `cupertino-effects`' sample-image example writes files, which
  the purity check does not see while the helper lives in `tests/common`; test file names mix kebab-case and
  snake_case; the shake test's slope check depends on orientation (the owner decides whether it should; no change until then).
- 2026-11-17: lead, wave 3: `cupertino-semantics` is `cupertino_app_protocols::semantics`, `cupertino-protocol-app` is
  `cupertino_wayland_client::app`. `cupertino-control` reaching `cupertino-app-protocols` is a new `shell facade`
  remaining-work line (it reached `cupertino-semantics` before). Messages without fields are written `{}` variants so
  that serde refuses unknown fields; the wire form is unchanged. Wire tests use `wayland-server` as a dev-dependency.
- 2026-11-17: lead, after the review of wave 3: the layer protocol's limits are one module of named constants,
  `cupertino_app_protocols::layers`, used by the client, the server and ui-platform, and a test checks the protocol
  XML states each; the client checks spring response and damping as the 24.8 fixed point carries them and refuses a
  lens above 4, so no curve or glass it sends ends the connection.
- 2026-11-17: owner, after waves 1 to 3 (3414 tests in 36 of 75 crates, on course for the 8428 the restart removed):
  tests are one per behaviour, table-driven across cases, with no tests of constants, names, versions or type
  plumbing, and never restored from history (26 tests of `cupertino-diagnostic-events` were). A consolidation task
  rewrites the existing suites to that rule before wave 4's `T` tasks.
