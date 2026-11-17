# Architecture

Cupertino is a Wayland desktop with separate compositor, shell, applications and services. The ISO and installer ship NetworkManager and BlueZ for network and Bluetooth service. Settings, the control panel and the menu bar select those backends when Cupertino's own daemons are absent. The in-tree daemons remain development implementations; replacing the shipped services requires feature parity, including enterprise Wi-Fi, VPN and captive-portal handling for networking.

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
| `cupertino-keyring-prompter` | the keyring's password prompt, drawn as the authentication prompt is (a second binary of the `cupertino-polkit-agent` crate, which has no crate of its own) |
| `cupertino-portal` | the xdg-desktop-portal backend: file chooser, screen sharing, screenshots and colour picking, app chooser, appearance settings |
| `cupertino-control` | desktop control for local agents: output-scoped observation, capture, pointer and keyboard runs, each with the person's consent per app, and its MCP stdio bridge (`--mcp`) |
| `cupertino-files` | the file manager, and with `--desktop` the items of the Desktop folder on the desktop |
| `cupertino-settings` | the settings app |
| `cupertino-netd` | links, Wi-Fi through iwd, DHCPv4, `resolv.conf`, `org.cupertino.Network1`, on the system bus |
| `cupertino-btd` | adapters, discovery, pairing with a UI agent, key store, `org.cupertino.Bluetooth1`, on the system bus |
| `cupertino-powerd` | batteries, mains, display and keyboard backlight, lid; the `org.freedesktop.UPower` subset apps call and `org.cupertino.Power1` |

The server starts the shell processes listed in `/etc/cupertino/compositor.json` and four built-in processes from their installed paths, each with one role: `portal`, `polkit-agent`, `keyring-prompter` and `desktop-control` (`cupertino-control`). `cupertino shell list` shows them all.

## Rendering

The server composites and draws its materials (backdrop blur, glass) on the GPU with OpenGL ES through Smithay's GLES renderer; with no hardware renderer, Mesa's software renderer draws instead ([Displays and GPUs](displays-and-gpus.md)). CupertinoKit applications and shell processes draw their windows with the renderer of their class, repainting only the damage: Files windows, Settings, the dock and the launcher with the GPU renderer (`cupertino-ui-gpu`, over the thin GPU layer `cupertino-gpu`), handed to the server as dmabufs; the small, rarely repainted and must-draw surfaces on the CPU (`cupertino-ui-render`) in shared memory; other applications on the CPU until a window is large or busy. The CPU renderer is the standard the GPU renderer follows and its fallback. [GPU rendering](gpu-rendering.md) has the design, `docs/architecture/rendering.md` the rules every rendering change keeps, and `docs/plans/gpu-rendering.md` the steps and their checklist ([#155](https://github.com/orafaelfragoso/cupertino/issues/155)).

## Rules

1. **The server does only what a window server does.** The server restarts a shell process that exits with failure, at most once per second.
2. **Standard protocols first:** layer-shell, `ext-foreign-toplevel-list`, `ext-session-lock`, `ext-idle-notify`, `ext-image-copy-capture`, `xdg-activation`.
3. **A private protocol covers the gaps.** The public `cupertino_app_effects_v1` carries backdrop materials by recipe and glass backdrops joined per container (regular, clear, or the dimmer style of large floating panels; each told whether its face is dark or light when it is small enough to follow what lies behind it), backdrops of a subsurface drawn between it and the rest of its window, so content scrolls under a bar's glass, the title bar style, the appearance, whether the session is locked and where a window's menus are on the session bus; the public `cupertino_layers_v1` lets a window hand the compositor its presentation and layers of its own surface (place, size, opacity, scale, glass, content blur) to animate on the compositor's clock; the private `cupertino_shell_v1`, offered only to the shell processes the server starts, carries dock tile rectangles for the minimize effect, overview and workspace transforms driven by the dock, menu bar overrides and how light and how uneven the wallpaper is under the menu bar, application activation and grouping, gesture events, and session lock and appearance notifications. The public `cupertino_semantics_v1` lets a Kit application publish the semantic tree of its own surfaces (roles, names, states, bounds and actions) for desktop control, and `cupertino_scroll_v1` gives the scroll count of a series of flicks beside the pointer's axis events; the private `cupertino_control_v1` and `cupertino_control_authority_v1` are offered only to the desktop controller and to the session agent that asks the person for consent.
4. **One settings store:** `cupertino-defaults`. Files live in `$XDG_CONFIG_HOME/cupertino/`, each with its own environment override. Writes are atomic and change notification is an inotify watch coalesced until a burst is quiet.
5. **Privileges stay in the daemons.** They authorise callers by D-Bus peer credentials plus "is this the active seat user". There is no polkit. The session runs a polkit authentication agent for other software's actions; no Cupertino daemon asks polkit. System daemons ship unit files; that is packaging, not a code dependency.
6. **IPC:** Wayland between the server and the shell; D-Bus between the shell, apps and the daemons.

## Crate layers

A crate's layer is declared in its manifest (`[package.metadata.cupertino] layer`). The table, from the bottom, says
which layers each layer may depend on, in normal, build and dev dependencies; every crate may also have `testing` crates
as dev-dependencies, and only dev-dependencies point at a `testing` crate. The crates column is what each crate declares.

| Layer | Crates | May depend on |
|---|---|---|
| `foundation` (values, maths and data models with no I/O) | `cupertino-animation`, `cupertino-geometry`, `cupertino-keyboard`, `cupertino-menu-model`, `cupertino-theme` | `foundation` |
| `graphics` (the one definition of the drawing maths, and the GPU device) | `cupertino-gpu`, `cupertino-raster-math` | `foundation`, `graphics` |
| `models` (desktop policy as pure logic) | `cupertino-control-model`, `cupertino-displays`, `cupertino-effects`, `cupertino-input`, `cupertino-scene`, `cupertino-wm` | `foundation`, `models` |
| `system` (user-space operating-system access) | `cupertino-apps`, `cupertino-assets`, `cupertino-defaults`, `cupertino-diagnostic-events`, `cupertino-icons`, `cupertino-peer`, `cupertino-radios`, `cupertino-trash` | `foundation`, `system` |
| `platform` (kernel interfaces that more than one program needs) | `cupertino-alsa`, `cupertino-bluetooth`, `cupertino-devices`, `cupertino-netlink`, `cupertino-pipewire`, `cupertino-screencast`, `cupertino-seat` | `foundation`, `platform` |
| `protocol` (wire schemas) | `cupertino-app-protocols`, `cupertino-login-ipc`, `cupertino-protocols` | `foundation`, `protocol` |
| `transport` (client connections without widgets) | `cupertino-protocol-app`, `cupertino-shell-client`, `cupertino-wayland-client` | `foundation`, `system`, `platform`, `protocol`, `transport` |
| `render` (the compositor's own drawing) | `cupertino-cursor`, `cupertino-decorations`, `cupertino-glass` | `foundation`, `graphics`, `system`, `platform`, `render` |
| `ui` (the toolkit) | `cupertino-kit`, `cupertino-ui-abi`, `cupertino-ui-atspi`, `cupertino-ui-controls`, `cupertino-ui-core`, `cupertino-ui-gpu`, `cupertino-ui-layout`, `cupertino-ui-menus`, `cupertino-ui-platform`, `cupertino-ui-render`, `cupertino-ui-test`, `cupertino-ui-text`, `cupertino-ui-wayland` | `foundation`, `graphics`, `system`, `protocol`, `transport`, `ui` |
| `shellkit` (the shell's extension of the toolkit) | none | `foundation`, `system`, `protocol`, `transport`, `ui` |
| `core` (the compositor) | `cupertino-server` | `foundation`, `graphics`, `models`, `system`, `platform`, `protocol`, `render`, `core` |
| `services` (processes with no interface of their own) | `cupertino-btd`, `cupertino-netd`, `cupertino-powerd`, `cupertino-sessiond` | `foundation`, `models`, `system`, `platform`, `protocol`, `transport`, `services` |
| `shell` | `cupertino-control`, `cupertino-controlcenter`, `cupertino-dock`, `cupertino-launcher`, `cupertino-login`, `cupertino-menubar`, `cupertino-notifications`, `cupertino-osd`, `cupertino-polkit-agent`, `cupertino-portal`, `cupertino-screenshot`, `cupertino-windowmanager` | `foundation`, `models`, `system`, `protocol`, `ui`, `shellkit`, `shell` |
| `apps` | `cupertino-files`, `cupertino-settings` | `foundation`, `models`, `system`, `protocol`, `ui` |
| `tools` | `cupertino-cli`, `cupertino-diagnostics`, `cupertino-gpu-shaders`, `cupertino-gpu-slice`, `cupertino-layers`, `xtask` | every layer except `core`, `shell`, `services`, `apps`, `testing` |
| `testing` (test harnesses, tests that start programs from more than one crate) | `cupertino-capture`, `cupertino-session-tests` | every layer |

Further rules:

- No crate reaches a higher layer through a chain of dependencies, also not through `tools` or `testing`.
- `foundation` and `models` sources use no `std::fs`, `std::net`, `std::process`, `std::env`, `std::thread`, no
  `unsafe` and no FFI. Build scripts and `tests/` are exempt.
- A `shell` crate never depends on another `shell` crate, except the shell host on its components.
- `cupertino-wayland-client` never reaches `cupertino-protocols` or the wlr, misc or plasma protocol crates;
  `cupertino-app-protocols` never reaches `cupertino-protocols`; `cupertino-kit` and the `cupertino-ui-*` crates never
  reach `cupertino-protocols` or `cupertino-shell-client`; `cupertino-ui-render` never reaches `cupertino-ui-wayland` or
  `cupertino-wayland-client`.
- `shell` and `apps` crates reach the toolkit only through `cupertino-kit`, and `cupertino-shell-client` and
  `cupertino-protocols` only through `cupertino-kit-shell` (for `shell` crates).
- External crates stay in their layers: Smithay in `core` only; the image, Wayland client, `xkbcommon`,
  `wayland-server`, `zbus` and system-call crates in the layers `cupertino-layers` lists for them.
- Every `shell` and `apps` crate holds the `clippy.toml` that `cupertino-layers` writes, which disallows starting
  threads, `block_on`, `std::fs` and blocking bus calls.
- Every crate declares a licence.

`cargo run -p cupertino-layers` checks them all. The problems that crates still have are listed one per line in
`crates/cupertino-layers/remaining.txt`: the check fails on a problem the list does not hold and on a line that is no
longer a problem, and `cargo run -q -p cupertino-layers -- --write-remaining` writes it again. Until the layered
refactor's last wave, a change may add lines (`cargo xtask remaining` reports them); after it the list only shrinks.
The layers in `ENFORCED` in `cupertino-layers` are closed: none of their problems may be listed. Test code outside a
crate's `tests/` is refused by the gate of `cargo xtask ci`. Keyboard layouts come from the system's libxkbcommon
through the registry `xkbcommon` crate, the one Smithay uses too; the server keeps the last keymap that compiled when a
new layout does not.
`crates/cupertino-layers/graph/layers.dot` is a DOT snapshot of the layers (`cargo run -p cupertino-layers -- --graph`
writes it).

## Contracts registry

[`docs/architecture/contracts.yaml`](../architecture/contracts.yaml) lists every Wayland global the server advertises, every D-Bus interface a Cupertino process serves and every binary a workspace crate builds. Each entry has its version, owner crate and area, threat scope (`public`, `shell-private` or `system`), and the file that defines it with a short text (`source_match`) found in it; the file's header defines the fields. No tool checks the registry against the code: keeping it true is part of the change that touches an entry. `unknown` marks a value that is not measured yet.

The registry records the state of the code; it does not grant access. It covers D-Bus interfaces that Cupertino serves, not the interfaces it calls on other services. When a global, a served interface or a binary is added, removed or moved, change its entry in the same commit.

## In-tree Linux interfaces

| Replaces | Component | Kernel interface |
|---|---|---|
| libudev | `cupertino-devices` | `NETLINK_KOBJECT_UEVENT`, `/sys` |
| libinput | `cupertino-devices` (`evdev`) and `cupertino-input` | evdev |
| libseat, logind client | `cupertino-seat` | logind's D-Bus protocol (`TakeControl`, `TakeDevice`, pause and resume, VT switch) |
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
