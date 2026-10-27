# Trying Cupertino on your machine

Cupertino is early (0.x). This page is for testers: what to expect, what does not work yet, and how to report it.

## Before you start

- A current stable Rust toolchain, OpenGL ES drivers (Mesa), and a Wayland session to run nested in, or a free text console with logind and a DRM/KMS device.
- Cupertino serves Wayland clients only. There is no X11 or XWayland support.
- Run either nested (a window in your current session) or on a text console; steps for both are in [Building and running](building.md). The TTY session has run in a virtual machine, not yet on real hardware.
- Nothing is installed and no service is enabled by these steps; they run from a checkout.

## What works today

- The compositor starts nested and on a TTY, and starts its shell processes with it.
- The dock, menu bar, launcher, application switcher, overview, workspaces and hot corners.
- Notifications, the control panel, and the volume and brightness overlays. The volume keys drive the raw mixer: no device choice, see [#21](https://github.com/orafaelfragoso/cupertino/issues/21).
- The file manager and the settings app.
- Window management: move, resize, tile, full screen, minimise, close, switch windows. New windows take focus: see [#20](https://github.com/orafaelfragoso/cupertino/issues/20).
- Screenshots of the screen or a selection (`Super+Shift+3` and `Super+Shift+4`).

Each has rough edges; the list above means the feature exists, not that it is finished.

## Selected known problems

A selection of the open p0 and p1 defects a tester hits first. Each issue has the current state and any workaround; the live list is [open p0 and p1 issues](https://github.com/orafaelfragoso/cupertino/issues?q=is%3Aissue+is%3Aopen+label%3A%22priority%3A+p0%22%2Clabel%3A%22priority%3A+p1%22).

- Mouse: [#1](https://github.com/orafaelfragoso/cupertino/issues/1), [#2](https://github.com/orafaelfragoso/cupertino/issues/2), [#3](https://github.com/orafaelfragoso/cupertino/issues/3).
- Text fields: [#16](https://github.com/orafaelfragoso/cupertino/issues/16).
- Menus and the keyboard: [#18](https://github.com/orafaelfragoso/cupertino/issues/18).
- Session menu: [#19](https://github.com/orafaelfragoso/cupertino/issues/19).
- Focus: [#20](https://github.com/orafaelfragoso/cupertino/issues/20).
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
- Headless capture: the output of `capture.sh`.

One problem per issue. Do not attach screenshots that show icons, fonts or wallpapers from a local asset pack. Report security problems privately, as [SECURITY.md](https://github.com/orafaelfragoso/cupertino/blob/main/SECURITY.md) describes, not in an issue.
