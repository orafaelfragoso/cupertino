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
- Notifications, the control panel and the volume and brightness overlays.
- The file manager and the settings app.
- Window management: move, resize, tile, full screen, minimise, close, switch windows.
- Screenshots of the screen or a selection (`Super+Shift+3` and `Super+Shift+4`).

Each has rough edges; the list above means the feature exists, not that it is finished.

## Known problems

These are open defects with priority p0 or p1. The issue has the current state and any workaround:

- Pointer acceleration with a mouse: [#1](https://github.com/orafaelfragoso/cupertino/issues/1).
- Mouse wheel scrolling is slow: [#2](https://github.com/orafaelfragoso/cupertino/issues/2).
- The natural-scrolling setting does not affect a mouse: [#3](https://github.com/orafaelfragoso/cupertino/issues/3).
- Popup menus are not dismissed when the pane that opened them closes: [#5](https://github.com/orafaelfragoso/cupertino/issues/5).
- The menu bar shows the app name and Window, not the app's own menus: [#6](https://github.com/orafaelfragoso/cupertino/issues/6).
- No way to change the wallpaper: [#7](https://github.com/orafaelfragoso/cupertino/issues/7).
- Text fields have no caret, and clicks do not place one: [#16](https://github.com/orafaelfragoso/cupertino/issues/16).
- An installed session starts no shell processes: [#17](https://github.com/orafaelfragoso/cupertino/issues/17).
- Open menus ignore the keyboard: [#18](https://github.com/orafaelfragoso/cupertino/issues/18).

Other open issues are in the [issue list](https://github.com/orafaelfragoso/cupertino/issues).

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

One problem per issue. Do not attach screenshots that show icons, fonts or wallpapers from a proprietary local asset pack. Report security problems privately, as [SECURITY.md](https://github.com/orafaelfragoso/cupertino/blob/main/SECURITY.md) describes, not in an issue.
