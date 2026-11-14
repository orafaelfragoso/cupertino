# Architecture

Cupertino is a Wayland desktop with separate compositor, shell, applications and services. The ISO and installer ship NetworkManager and BlueZ for network and Bluetooth service. Settings selects those backends when Cupertino's own daemons are absent. The in-tree daemons remain development implementations; replacing the shipped services requires feature parity, including enterprise Wi-Fi, VPN and captive-portal handling for networking.

## What we own, speak and keep

| | Scope | Examples |
|---|---|---|
| Own | Userspace libraries and policy daemons | device discovery, input, seat handling, keymaps, network, Bluetooth and power daemons, notification and tray services |
| Speak | Wire protocols, implemented in Rust without a C library | Wayland; the D-Bus interfaces apps call (notifications, status items, menus); kernel interfaces (DRM/KMS, evdev, netlink, nl80211, Bluetooth management sockets, sysfs) |
| Keep | External, each behind one crate boundary | the kernel; Mesa GPU drivers; the D-Bus broker; Smithay for Wayland protocol handling; NetworkManager and BlueZ in the installed distribution; iwd when using cupertino-netd; PipeWire for application audio and screen-sharing streams |

## Processes

A crash in a shell process never takes the desktop down.

| Process | Owns |
|---|---|
| `cupertino-server` | compositing, window policy, input routing, cursor, materials (backdrop rendering), the menu bar layer and its auto-hide, window snapping, supervision of the shell processes |
| `cupertino-dock` | the bar, application switcher, overview, workspaces, hot corners, trackpad gestures |
| `cupertino-menubar` | status items and application menus (DBusMenu, StatusNotifierItem, GMenuModel) |
| `cupertino-controlcenter` | the control panel and its modules |
| `cupertino-osd` | volume, brightness and keyboard-backlight overlays |
| `cupertino-notifications` | banners, the notification list, `org.freedesktop.Notifications` |
| `cupertino-launcher` | the search and launch panel, and the application browser |
| `cupertino-windowmanager` | the stage and tiling policy (off by default) |
| `cupertino-login` | the login screen, the lock screen (`ext-session-lock`) and the session agent |
| `cupertino-sessiond` | the root-owned half of login: the login screen's socket, PAM in a worker per attempt, logind sessions and virtual terminals, fast user switching, power, auto login |
| `cupertino-screenshot` | screen, window and selection capture, toolbar, floating thumbnail |
| `cupertino-polkit-agent` | the session's polkit authentication agent and its prompt |
| `cupertino-keyring-prompter` | the keyring's password prompt, drawn as the authentication prompt is (built from the `cupertino-polkit-agent` crate) |
| `cupertino-portal` | the xdg-desktop-portal backend: file chooser, screen sharing, screenshots and colour picking, app chooser, appearance settings |
| `cupertino-control` | desktop control for local agents: output-scoped observation, capture, pointer and keyboard runs, each with the person's consent per app, and its MCP stdio bridge (`--mcp`) |
| `cupertino-files` | the file manager, and with `--desktop` the items of the Desktop folder on the desktop |
| `cupertino-settings` | the settings app |
| `cupertino-netd` | links, Wi-Fi through iwd, DHCPv4, `resolv.conf`, `org.cupertino.Network1`, on the system bus |
| `cupertino-btd` | adapters, discovery, pairing with a UI agent, key store, `org.cupertino.Bluetooth1`, on the system bus |
| `cupertino-powerd` | batteries, mains, display and keyboard backlight, lid; the `org.freedesktop.UPower` subset apps call and `org.cupertino.Power1` |

The server starts the shell processes listed in `/etc/cupertino/compositor.json` and four built-in processes from their installed paths, each with one role: `portal`, `polkit-agent`, `keyring-prompter` and `desktop-control` (`cupertino-control`). `cupertino shell list` shows them all.

## Rendering

The server composites and draws its materials (backdrop blur, glass) on the GPU with OpenGL ES through Smithay's GLES renderer; with no hardware renderer, Mesa's software renderer draws instead ([Displays and GPUs](displays-and-gpus.md)). CupertinoKit applications and shell processes rasterize their windows on the CPU (`cupertino-ui-render`) and hand them to the server in shared memory, repainting only the damage. Drawing large windows on the GPU, with the CPU rasterizer kept as the standard it is tested against and as the fallback, is planned: [GPU rendering](gpu-rendering.md) has the design, `docs/architecture/rendering.md` the rules every rendering change keeps, and `docs/plans/gpu-rendering.md` the steps and their checklist ([#155](https://github.com/orafaelfragoso/cupertino/issues/155)).

## Rules

1. **The server does only what a window server does.** The server restarts a shell process that exits with failure, at most once per second.
2. **Standard protocols first:** layer-shell, `ext-foreign-toplevel-list`, `ext-session-lock`, `ext-idle-notify`, `ext-image-copy-capture`, `xdg-activation`.
3. **A private protocol covers the gaps.** The public `cupertino_app_effects_v1` carries backdrop materials by recipe and glass backdrops joined per container (regular, clear, or the dimmer style of large floating panels; each told whether its face is dark or light when it is small enough to follow what lies behind it), backdrops of a subsurface drawn between it and the rest of its window, so content scrolls under a bar's glass, the title bar style, the appearance, whether the session is locked and where a window's menus are on the session bus; the private `cupertino_shell_v1`, offered only to the shell processes the server starts, carries dock tile rectangles for the minimize effect, overview and workspace transforms driven by the dock, menu bar overrides and how light and how uneven the wallpaper is under the menu bar, application activation and grouping, gesture events, and session lock and appearance notifications. The public `cupertino_semantics_v1` lets a Kit application publish the semantic tree of its own surfaces (roles, names, states, bounds and actions) for desktop control, and `cupertino_scroll_v1` gives the scroll count of a series of flicks beside the pointer's axis events; the private `cupertino_control_v1` and `cupertino_control_authority_v1` are offered only to the desktop controller and to the session agent that asks the person for consent.
4. **One settings store:** `cupertino-defaults`. Files live in `$XDG_CONFIG_HOME/cupertino/`, each with its own environment override. Writes are atomic and change notification is an inotify watch coalesced until a burst is quiet.
5. **Privileges stay in the daemons.** They authorise callers by D-Bus peer credentials plus "is this the active seat user". There is no polkit. The session runs a polkit authentication agent for other software's actions; no Cupertino daemon asks polkit. System daemons ship unit files; that is packaging, not a code dependency.
6. **IPC:** Wayland between the server and the shell; D-Bus between the shell, apps and the daemons.

## Crate layers

A crate's layer is declared in its manifest (`[package.metadata.cupertino] layer`). `cargo run -p cupertino-layers` fails on any dependency between layers that the table does not allow.

| Layer | Crates | May depend on |
|---|---|---|
| `foundation` | `cupertino-animation`, `cupertino-apps`, `cupertino-assets`, `cupertino-build-info`, `cupertino-capture`, `cupertino-defaults`, `cupertino-diagnostic-events`, `cupertino-filepicker`, `cupertino-geometry`, `cupertino-glyph`, `cupertino-icons`, `cupertino-key-bindings`, `cupertino-keyboard`, `cupertino-menu-model`, `cupertino-modifiers`, `cupertino-test-bus`, `cupertino-test-pam`, `cupertino-test-pam-shim`, `cupertino-theme`, `cupertino-thumbnails`, `cupertino-trash`, `cupertino-typeahead`, `xkbcommon` | `foundation` |
| `graphics` (the one definition of the drawing maths) | `cupertino-raster-math` | `foundation`, `graphics` |
| `models` (pure logic, no I/O) | `cupertino-control-model`, `cupertino-displays`, `cupertino-effects`, `cupertino-input`, `cupertino-overview`, `cupertino-render-api`, `cupertino-scene`, `cupertino-stage`, `cupertino-wm` | `foundation`, `models` |
| `platform` (kernel interfaces, and the PipeWire boundary) | `cupertino-alsa`, `cupertino-bluetooth`, `cupertino-devices`, `cupertino-evdev`, `cupertino-netlink`, `cupertino-peer`, `cupertino-pipewire`, `cupertino-screencast`, `cupertino-seat` | `foundation`, `platform` |
| `protocol` | `cupertino-app-protocols`, `cupertino-login-ipc`, `cupertino-protocols`, `cupertino-semantics` | `foundation`, `protocol` |
| `transport` | `cupertino-protocol-app`, `cupertino-shell-client`, `cupertino-wayland-client` | `foundation`, `platform`, `protocol`, `transport` |
| `render` | `cupertino-cursor`, `cupertino-decorations`, `cupertino-glass` | `foundation`, `graphics`, `platform`, `render` |
| `ui` (the toolkit) | `cupertino-kit`, `cupertino-ui-abi`, `cupertino-ui-atspi`, `cupertino-ui-controls`, `cupertino-ui-core`, `cupertino-ui-layout`, `cupertino-ui-menus`, `cupertino-ui-platform`, `cupertino-ui-render`, `cupertino-ui-test`, `cupertino-ui-text`, `cupertino-ui-theme`, `cupertino-ui-wayland` | `foundation`, `graphics`, `protocol`, `transport`, `ui` |
| `core` | `cupertino-server` | `foundation`, `graphics`, `models`, `platform`, `protocol`, `render` |
| `shell` | `cupertino-control`, `cupertino-controlcenter`, `cupertino-dock`, `cupertino-launcher`, `cupertino-login`, `cupertino-menubar`, `cupertino-notifications`, `cupertino-osd`, `cupertino-polkit-agent`, `cupertino-portal`, `cupertino-screenshot`, `cupertino-windowmanager` | `foundation`, `graphics`, `models`, `platform`, `protocol`, `transport`, `render`, `ui` |
| `services` | `cupertino-btd`, `cupertino-netd`, `cupertino-powerd`, `cupertino-sessiond` | `foundation`, `models`, `platform`, `protocol`, `services` |
| `apps` | `cupertino-files`, `cupertino-settings` | `foundation`, `graphics`, `models`, `protocol`, `transport`, `ui` |
| `tools` | `cupertino-cli`, `cupertino-diagnostics`, `cupertino-layers`, `cupertino-testclient` | all but `core`, `shell`, `services`, `apps` |

`shell`, `services`, `apps` and `tools` crates never depend on `core` or on each other. Each shell component is one crate: its pure model plus a binary behind an `app` feature. Only `cupertino-server` and `cupertino-seat` (which implements Smithay's session trait) depend on Smithay. The `xkbcommon` package of the workspace is `cupertino-xkbcommon`, a pure-Rust drop-in for the C-binding crate, selected through `[patch.crates-io]`.

An edge the table forbids that exists today is listed as an exception in `cupertino-layers` (permanent, or migration debt with its issue), and any dependency chain from `ui` to `core`, `shell` or `services` fails even when its edges are excused. `crates/cupertino-layers/graph/layers.dot` is a DOT snapshot of the layers, checked by a test (`cargo run -p cupertino-layers -- --graph`).

## Contracts registry

[`docs/architecture/contracts.yaml`](../architecture/contracts.yaml) lists every Wayland global the server advertises, every D-Bus interface a Cupertino process serves and every binary a workspace crate builds. Each entry has its version, owner crate and area, threat scope (`public`, `shell-private` or `system`), test file, and the file that defines it with a short text (`source_match`) that must occur in it; the file's header defines the fields. `unknown` marks a value that is not measured yet.

Three checks keep it complete:

| Check | Command | Fails when |
|---|---|---|
| Wayland globals, at run time | `cargo nextest run -p cupertino-server --test suite contracts::` | the globals that an ordinary client, the compositor's own connection, or a connection for any one shell role sees differ from the entries of that scope, with and without an accent table |
| D-Bus interfaces and `org.cupertino.*` names | `python3 -I ci/contracts.py .` | a `#[zbus::interface]` (also `#[interface]`, and an attribute that spans lines) or a Cupertino bus name in `crates/*/src` has no entry, an attribute's name is not a string literal, or an entry points at a line that no longer defines it |
| Binaries | `python3 -I ci/contracts.py .` | a binary target in `cargo metadata` has no entry, or `shipped` differs from `BINARIES` in `packaging/release.py` |

`ci/contracts.py` also fails when the number of entries with `test_owner: unknown` changes from the count it records, so the number can only go down. The checker's own tests run with `python3 -I ci/test_contracts.py`. The run-time test skips with a message when the machine has no EGL device; CI sets `CUPERTINO_EGL_REQUIRED=1`, which makes that a failure. The registry records the state of the code; it does not grant access. The D-Bus scan covers interfaces that Cupertino serves, not the interfaces it calls on other services.

## In-tree Linux interfaces

| Replaces | Component | Kernel interface |
|---|---|---|
| libudev | `cupertino-devices` | `NETLINK_KOBJECT_UEVENT`, `/sys` |
| libinput | `cupertino-evdev` and `cupertino-input` | evdev |
| libseat, logind client | `cupertino-seat` | logind's D-Bus protocol (`TakeControl`, `TakeDevice`, pause and resume, VT switch) |
| xkbcommon | `cupertino-xkbcommon` over `xkbcommon-rs` (vendored in `third-party/`) | none |
| alsa-lib | `cupertino-alsa` | `/dev/snd/controlC*` ioctls |
| UPower, backlight helpers | `cupertino-powerd` | `/sys/class/power_supply`, `/sys/class/backlight`, lid switch over evdev |
| NetworkManager (development alternative) | `cupertino-netd` | rtnetlink, nl80211; iwd for authentication |
| BlueZ daemon (development alternative) | `cupertino-btd` | Bluetooth management socket, L2CAP |

## Known limits

- X11 programs need `xwayland-satellite` (0.8 or later) and `Xwayland` installed; without them the session serves Wayland clients only. Drag and drop between X11 and Wayland programs and per-output scales for X11 windows depend on the satellite (it uses one scale for all X11 windows), and none of it has been tested with real applications on hardware.
- `org.cupertino.Session1` authorizes mutating calls by process identity. Settings changes require the trusted Settings executable, the supervised session agent or the server. Application quitting requires the session agent or server. Listing and restarting the supervised shell processes requires an unsandboxed process of the session's own user. This does not make arbitrary same-user code a security boundary (see `SECURITY.md`).
- VM evidence covers the TTY backend. The full real-hardware acceptance matrix remains open; see [Roadmap](roadmap.md).
- Audio is not owned: applications use PipeWire. The volume overlay, the control panel and the Sound pane change the default
  PipeWire sink through `cupertino-pipewire` (WirePlumber's `wpctl`), and the ALSA mixer only when no PipeWire daemon answers.
