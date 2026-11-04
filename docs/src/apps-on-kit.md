# What runs on CupertinoKit

CupertinoKit is the toolkit that Cupertino's own apps are moving onto. This page says which of them draw their windows with it on `main`, and which do not yet. Assessed at `0a928c70` on 2026-11-04. A program counts as migrated only when its normal entry point draws through Kit. A dependency or a parallel implementation alone does not complete migration.

| Program | On Kit today | Not on Kit yet |
|---|---|---|
| `cupertino-osd` | The whole overlay: a Kit window on a layer surface for volume, brightness and keyboard backlight. | |
| `cupertino-settings` | The Settings window: sidebar, back and forward toolbar with the search field, the panes' sections as Kit controls, sheets as dialogs, and menus on the desktop's menu bar. | The old painter and window modules are deleted; full app accessibility acceptance remains open. |
| `cupertino-files` | A parallel browser implementation: sidebar, toolbar, list and icon views, path and status bars, menus, live folder updates, file operations and multiple windows. | Menu items the Kit window cannot do yet are disabled. The existing window stays until the Kit one does everything it does. |
| `cupertino-portal` | Its dialogs (file chooser, screen cast chooser, app chooser, screenshot consent). See [Portals](portals.md). | |
| `cupertino-controlcenter` | The whole panel: a Kit window on a layer surface with module toggles, tiles, the battery meter and level sliders. | |
| `cupertino-menubar` | Bar and menus on Kit layer surfaces. | Outside-click dismissal on empty bar space has an open review finding. |
| `cupertino-launcher` | Search panel, catcher and hidden windows on Kit layer surfaces. | See the crate README for interaction and styling limits. |
| `cupertino-notifications` | Banners and notification centre on Kit layer surfaces. | Material, body layout and centre scrolling still have review findings. |
| Dock, login and lock screen, screenshot UI, window-manager UI | | Normal entry points still use their existing drawing paths. |

## The OSD

`cupertino-osd` is one 200 by 200 point layer surface at the bottom of the screen, with no keyboard focus and no input. It shows the volume (the default PipeWire sink, else the ALSA master), the display brightness and the keyboard backlight, and the microphone mute. It fades after a short hold. With Reduce Transparency or Increase Contrast it draws an opaque background. The session starts and restarts it through the `shell` key of the server's configuration. Run `cupertino-osd --fake-backends` to see it without touching the machine's volume. The crate's README has the details.

## The Control Center

`cupertino-controlcenter` opens below the menu bar at the top right as a Kit window on a layer surface, built from the panel controls of `cupertino-ui-controls`: module toggles, tiles, a battery meter and level sliders. It takes the keyboard while it is shown; Escape or a press outside it closes it. With Reduce Transparency or Increase Contrast it draws an opaque background. Run `cupertino-controlcenter --fake-backends` to try it without touching the machine's radios, volume and backlight. The crate's README has the details.

## Menus

A Kit window publishes its menus to the desktop's menu bar, and an item chosen there sends its message back to the app. A Kit app can also ask for an activation token for another application, and the Dock and the window switcher show the icon that a window sets for itself.

## What this means for you

Nothing you have to change. The programs above run as before, with the same shortcuts. Where the table says "not yet", the old window is still what you get.
