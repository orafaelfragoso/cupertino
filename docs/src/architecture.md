# Architecture

Cupertino is a Wayland desktop that depends on no other userspace library or policy daemon of the Linux desktop stack for what it does itself. Every part ships on its own: the server boots a desktop, and the dock, menu bar, launcher and the rest install separately.

## What we own, speak and keep

| | Scope | Examples |
|---|---|---|
| Own | Userspace libraries and policy daemons | device discovery, input, seat handling, keymaps, network, Bluetooth and power daemons, notification and tray services |
| Speak | Wire protocols, implemented in Rust without a C library | Wayland; the D-Bus interfaces apps call (notifications, status items, menus); kernel interfaces (DRM/KMS, evdev, netlink, nl80211, Bluetooth management sockets, sysfs) |
| Keep | External, each behind one crate boundary | the kernel; Mesa GPU drivers; the D-Bus broker; Smithay for Wayland protocol handling; iwd for Wi-Fi authentication; PipeWire for application audio |

## Processes

A crash in a shell process never takes the desktop down.

| Process | Owns |
|---|---|
| `cupertino-server` | compositing, window policy, input routing, cursor, materials (backdrop rendering), the menu bar layer and its auto-hide, window snapping, supervision of the shell processes |
| `cupertino-dock` | the bar, application switcher, overview, app grid, workspaces, hot corners, trackpad gestures |
| `cupertino-menubar` | status items and application menus (DBusMenu, StatusNotifierItem, GMenuModel) |
| `cupertino-controlcenter` | the control panel and its modules |
| `cupertino-osd` | volume, brightness and keyboard-backlight overlays |
| `cupertino-notifications` | banners, the notification list, `org.freedesktop.Notifications` |
| `cupertino-launcher` | the search and launch panel |
| `cupertino-windowmanager` | the stage and tiling policy (off by default) |
| `cupertino-login` | the login screen, the lock screen (`ext-session-lock`) and the session agent |
| `cupertino-sessiond` | the root-owned half of login: the login screen's socket, PAM in a worker per attempt, logind sessions and virtual terminals, fast user switching, power, auto login |
| `cupertino-screenshot` | screen, window and selection capture, toolbar, floating thumbnail |
| `cupertino-files` | the file manager |
| `cupertino-settings` | the settings app |
| `cupertino-netd` | links, Wi-Fi through iwd, DHCPv4, `resolv.conf`, `org.cupertino.Network1`, on the system bus |
| `cupertino-btd` | adapters, discovery, pairing with a UI agent, key store, `org.cupertino.Bluetooth1`, on the system bus |
| `cupertino-powerd` | batteries, mains, display and keyboard backlight, lid; the `org.freedesktop.UPower` subset apps call and `org.cupertino.Power1` |

## Rules

1. **The server does only what a window server does.** The server restarts a shell process that exits with failure, at most once per second.
2. **Standard protocols first:** layer-shell, `ext-foreign-toplevel-list`, `ext-session-lock`, `ext-idle-notify`, `ext-image-copy-capture`, `xdg-activation`.
3. **A private protocol covers the gaps.** `cupertino_shell_v1` carries backdrop materials by recipe, dock tile rectangles for the minimize effect, overview and workspace transforms driven by the dock, menu bar overrides, application activation and grouping, gesture events, and session lock and appearance notifications.
4. **One settings store:** `cupertino-defaults`. Files live in `$XDG_CONFIG_HOME/cupertino/`, each with its own environment override. Writes are atomic and change notification is an inotify watch coalesced until a burst is quiet.
5. **Privileges stay in the daemons.** They authorise callers by D-Bus peer credentials plus "is this the active seat user". There is no polkit. System daemons ship unit files; that is packaging, not a code dependency.
6. **IPC:** Wayland between the server and the shell; D-Bus between the shell, apps and the daemons.

## Crate layers

A crate's layer is declared in its manifest (`[package.metadata.cupertino] layer`). `cargo run -p cupertino-layers` fails on any dependency between layers that the table does not allow.

| Layer | Crates | May depend on |
|---|---|---|
| `foundation` | cupertino-animation, -apps, -assets, -defaults, -filepicker, -geometry, -theme, -thumbnails, -typeahead, -xkbcommon | `foundation` |
| `models` (pure logic, no I/O) | cupertino-displays, -effects, -input, -keyboard, -login-ipc, -overview, -stage, -text-field, -wm | `foundation`, `models` |
| `platform` (kernel interfaces only) | cupertino-alsa, -bluetooth, -devices, -evdev, -netlink, -seat | `foundation`, `platform` |
| `render` | cupertino-client, -cursor, -decorations, -glass, -icons, -protocols | `foundation`, `platform`, `render` |
| `core` | cupertino-server | `foundation`, `models`, `platform`, `render` |
| `shell` | cupertino-controlcenter, -dock, -launcher, -login, -menubar, -notifications, -osd, -screenshot, -windowmanager | `foundation`, `models`, `platform`, `render` |
| `services` | cupertino-btd, -netd, -powerd, -sessiond | as `shell` |
| `apps` | cupertino-files, -settings | as `shell` |
| `tools` | cupertino-gtk-theme, -layers, -testclient | as `shell` |

`shell`, `services`, `apps` and `tools` crates never depend on `core` or on each other. Each shell component is one crate: its pure model plus a binary behind an `app` feature. Only `cupertino-server` and `cupertino-seat` (which implements Smithay's session trait) depend on Smithay. The `xkbcommon` package of the workspace is `cupertino-xkbcommon`, a pure-Rust drop-in for the C-binding crate, selected through `[patch.crates-io]`.

The migration moves to twelve layers (adding `protocol`, `transport` and `ui`). `cupertino-layers` also checks every crate's target layer, with each edge that the target forbids today listed as a tracked exception; its README has the target table, and `crates/cupertino-layers/graph/` holds DOT snapshots of both views (`cargo run -p cupertino-layers -- --graph current|target`).

## Replacing the Linux stack

| Replaces | Component | Kernel interface |
|---|---|---|
| libudev | `cupertino-devices` | `NETLINK_KOBJECT_UEVENT`, `/sys` |
| libinput | `cupertino-evdev` and `cupertino-input` | evdev |
| libseat, logind client | `cupertino-seat` | logind's D-Bus protocol (`TakeControl`, `TakeDevice`, pause and resume, VT switch) |
| xkbcommon | `cupertino-xkbcommon` over `xkbcommon-rs` (vendored in `third-party/`) | none |
| alsa-lib | `cupertino-alsa` | `/dev/snd/controlC*` ioctls |
| UPower, backlight helpers | `cupertino-powerd` | `/sys/class/power_supply`, `/sys/class/backlight`, lid switch over evdev |
| NetworkManager | `cupertino-netd` | rtnetlink, nl80211; iwd for authentication |
| BlueZ daemon | `cupertino-btd` | Bluetooth management socket, L2CAP |

## Known limits

- Wayland clients only: there is no X11 or XWayland support.
- The settings setters on `org.cupertino.Session1` trust any client on the same user's session bus (see `SECURITY.md`).
- The TTY backend has run in a virtual machine, not on real hardware.
- Audio is not owned: applications use PipeWire, and the volume overlay reads the ALSA mixer.
