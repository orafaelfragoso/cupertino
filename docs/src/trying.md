# Trying Cupertino on your machine

Cupertino is early (0.x). This page is for testers: what to expect, what does not work yet, and how to report it.

## Before you start

- A current stable Rust toolchain, OpenGL ES drivers (Mesa), and a Wayland session to run nested in, or a free text console with logind and a DRM/KMS device.
- Cupertino serves Wayland clients. X11 programs run when `xwayland-satellite` (0.8 or later) and `Xwayland` are installed; the satellite starts with the first X11 program.
- Run either nested (a window in your current session) or on a text console; steps for both are in [Building and running](building.md). The TTY session has run in a virtual machine, not yet on real hardware.
- Nothing is installed and no service is enabled by these steps; they run from a checkout.

## What works today

- The compositor starts nested and on a TTY, and starts its shell processes with it.
- The dock, menu bar, launcher, application switcher, overview, workspaces and hot corners.
- Notifications, the control panel, and the volume and brightness overlays. The volume keys change the default PipeWire output; choosing the device and media controls are still open, see [#21](https://github.com/orafaelfragoso/cupertino/issues/21).
- The file manager and the settings app.
- Window management: move, resize, tile, full screen, minimise, close, switch windows, and Force Quit for an application that does not answer.
- The lock screen, and log out, restart and shut down from the menu bar.
- Screenshots of the screen, a window or a selection (`Super+Shift+3`, `Super+Shift+4`, `Super+Shift+5` for the toolbar).
- The portal backend: file chooser, screen sharing, screenshots and colour picking for apps ([Portals](portals.md)).

Each has rough edges; the list above means the feature exists, not that it is finished.

## Selected known problems

A selection of the open p1 problems a tester hits first. Each issue has the current state and any workaround; the live list is [open p1 issues](https://github.com/orafaelfragoso/cupertino/issues?q=is%3Aissue+is%3Aopen+label%3A%22priority%3A+p1%22).

- The installed-session quality backlog: menus and the pointer, sidebars, the Control Center, notifications, Trash, quick preview, scrollbars and materials. [Desktop quality and open issues](desktop-quality.md) lists each item with its issue.
- Applications installed during a session are missing from the launcher until it restarts: [#190](https://github.com/orafaelfragoso/cupertino/issues/190).
- The launcher and the overview stutter as they close: [#158](https://github.com/orafaelfragoso/cupertino/issues/158).
- Audio: [#21](https://github.com/orafaelfragoso/cupertino/issues/21).
- Multiple monitors: [#22](https://github.com/orafaelfragoso/cupertino/issues/22).

## Reporting a bug

Open the [bug report form](https://github.com/orafaelfragoso/cupertino/issues/new?template=bug-report.yml). It asks for:

- the commit (`git rev-parse --short HEAD`), your distribution and kernel, and your GPU and driver;
- how Cupertino was running: TTY (`--udev`), nested (`--winit`) or headless (`--headless`);
- the display resolution and scale;
- steps to reproduce, what you expected and what happened.

Attach the log:

- TTY run: `~/cupertino-tty.log` (the script prints this path).
- Nested run: the output of `cupertino-server` in the terminal that started it.
- Headless capture: the output of `capture.sh` or `scripts/dev/shot`.

`cupertino diagnose export --output FILE` writes the versions, the machine check and the recorded errors into one file you can attach; `--include-logs` adds the scrubbed server and session logs ([The cupertino command](cupertino-command.md)).

One problem per issue. Do not attach screenshots that show icons, fonts or wallpapers from a local asset pack. Report security problems privately, as [SECURITY.md](https://github.com/orafaelfragoso/cupertino/blob/main/SECURITY.md) describes, not in an issue.
