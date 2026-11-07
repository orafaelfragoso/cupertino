# What runs on CupertinoKit

CupertinoKit is the toolkit that Cupertino's own apps are moving onto. This page says which of them draw their windows with it on `main`, and which do not yet. Assessed at `0a928c70` on 2026-11-04. A program counts as migrated only when its normal entry point draws through Kit. A dependency or a parallel implementation alone does not complete migration.

| Program | On Kit today | Not on Kit yet |
|---|---|---|
| `cupertino-osd` | The whole overlay: a Kit window on a layer surface for volume, brightness and keyboard backlight. | |
| `cupertino-settings` | The Settings window: sidebar, back and forward toolbar with the search field, the panes' sections as Kit controls, sheets as dialogs, and menus on the desktop's menu bar. Privacy > Screen Sharing lists each display an app may share again without asking, with Forget. | The old painter and window modules are deleted. Accessibility > Screen reader turns the session's reader on and off through the server (tested against a fake session and a fake reader); a run with a real screen reader and full app accessibility acceptance remain open. |
| `cupertino-files` | The whole app: a live sidebar on the sidebar material with removable media (mount, unlock, eject; Primary+E ejects the row the keyboard is on) and Trash once something is trashed, read over AT-SPI; toolbar with the View, Tags and Action pop-up buttons and the animated search field; icon view and list view with disclosure triangles, both with a rubber band that scrolls at the edges, click-to-rename, Ctrl+click menus and Tab by name; a clickable path bar and the status bar; menus and context menus, live folder updates, file operations, Copy and Paste Item between folders (off the window's thread, with progress and Stop; nothing pasted on read-only media), multiple windows, and items dragged out of either view with an image of the item, the row or a stack with its count. | The old window is deleted. Kit windows take no drops, so nothing can be dropped onto a folder or a place; items wear symbols, not file-type icons or thumbnails; no Group By. See the crate README's open issues. |
| `cupertino-portal` | Its dialogs (file chooser, screen cast chooser, app chooser, screenshot consent). See [Portals](portals.md). | The colour picker overlay and the screen-sharing menu bar item are drawn without Kit (a frozen screen with a loupe; a status item the menu bar draws). |
| `cupertino-controlcenter` | The whole panel: a Kit window on a layer surface with module toggles, tiles, the battery meter and level sliders. It is dismissed when its display is unplugged and opens on a remaining or reconnected one. | Real display hot-plug is not yet tried (#22). |
| `cupertino-menubar` | Bar and menus on Kit layer surfaces. A click on empty bar space closes the open menu and the keyboard goes back to the window. | |
| `cupertino-launcher` | Search panel, catcher and hidden windows on Kit layer surfaces. | See the crate README for interaction and styling limits. |
| `cupertino-notifications` | Banners and notification centre on Kit layer surfaces. | Application icons from image data, image paths and desktop entries are not drawn yet. |
| `cupertino-dock` | The window-manager UI: the overview chrome (workspaces bar, window titles, selection) and the application switcher panel as Kit windows (`overview_kit`, `switcher_kit`), tested headless. | The dock itself, and the dock process's drawing path for the overview and switcher: wiring them in comes with the Dock's move to Kit, which deletes the old painters. |
| `cupertino-login` | The lock screen (Kit windows as session-lock surfaces, one per output), the greeter's picker, name and password screens and power menu, the log out, restart, shut down and power-button panels with the "Reopen windows" checkbox, and the interrupt panel. The password field is Kit's secure field. | Real suspend, console switches and PAM logins are checked by hand (the crate README lists the checks). |
| Screenshot UI | | Normal entry points still use their existing drawing paths. |

## The OSD

`cupertino-osd` is one 200 by 200 point layer surface at the bottom of the screen, with no keyboard focus and no input. It shows the volume (the default PipeWire sink, else the ALSA master), the display brightness and the keyboard backlight, and the microphone mute. It fades after a short hold. With Reduce Transparency or Increase Contrast it draws an opaque background. The session starts and restarts it through the `shell` key of the server's configuration. Run `cupertino-osd --fake-backends` to see it without touching the machine's volume. The crate's README has the details.

## The Control Center

`cupertino-controlcenter` opens below the menu bar at the top right as a Kit window on a layer surface, built from the panel controls of `cupertino-ui-controls`: module toggles, tiles, a battery meter and level sliders. It takes the keyboard while it is shown; Escape or a press outside it closes it. With Reduce Transparency or Increase Contrast it draws an opaque background. Run `cupertino-controlcenter --fake-backends` to try it without touching the machine's radios, volume and backlight. The crate's README has the details.

## Menus

A Kit window publishes its menus to the desktop's menu bar, and an item chosen there sends its message back to the app. A Kit app can also ask for an activation token for another application, and the Dock and the window switcher show the icon that a window sets for itself.

## What this means for you

Nothing you have to change. The programs above run as before, with the same shortcuts. Where the table says "not yet", the old window is still what you get.
