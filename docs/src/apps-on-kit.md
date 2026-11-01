# What runs on CupertinoKit

CupertinoKit is the toolkit that Cupertino's own apps are moving onto. This page says which of them draw their windows with it on `main`, and which do not yet. A program counts as on Kit here when its `Cargo.toml` depends on `cupertino-kit`.

| Program | On Kit today | Not on Kit yet |
|---|---|---|
| `cupertino-osd` | The whole overlay: a Kit window on a layer surface for volume, brightness and keyboard backlight. | |
| `cupertino-settings` | The Settings window: sidebar, back and forward toolbar with the search field, the panes' sections as Kit controls, sheets as dialogs, and menus on the desktop's menu bar. | The roadmap lists more work on Settings. |
| `cupertino-files` | A browser window, started beside the existing one: the sidebar, the toolbar, the file list as a table, the path and status bars, the menu bar and live folder updates. | Menu items the Kit window cannot do yet are disabled. The existing window stays until the Kit one does everything it does. |
| `cupertino-portal` | Its dialogs (file chooser, screen cast chooser, app chooser, screenshot consent). See [Portals](portals.md). | |
| Dock, menu bar, launcher, notifications, control panel, login and lock screen | | All of them. The control panel has its controls on Kit (below), not its window. |

## The OSD

`cupertino-osd` is one 200 by 200 point layer surface at the bottom of the screen, with no keyboard focus and no input. It shows the volume (the default PipeWire sink, else the ALSA master), the display brightness and the keyboard backlight, and the microphone mute. It fades after a short hold. With Reduce Transparency or Increase Contrast it draws an opaque background. The session starts and restarts it through the `shell` key of the server's configuration. Run `cupertino-osd --fake-backends` to see it without touching the machine's volume. The crate's README has the details.

## Control Center controls

The control panel's building blocks exist in `cupertino-ui-controls`: module toggles, tiles, a battery meter and a level slider for a panel. The `cupertino-controlcenter` process does not use them yet; it still draws its own window.

## Menus

A Kit window publishes its menus to the desktop's menu bar, and an item chosen there sends its message back to the app. A Kit app can also ask for an activation token for another application, and the Dock and the window switcher show the icon that a window sets for itself.

## What this means for you

Nothing you have to change. The programs above run as before, with the same shortcuts. Where the table says "not yet", the old window is still what you get.
