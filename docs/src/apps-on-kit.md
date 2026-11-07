# What runs on CupertinoKit

CupertinoKit is the toolkit that Cupertino's own apps are moving onto. This page says which of them draw their windows with it on `main`, and which do not yet. Assessed at `0a928c70` on 2026-11-04. A program counts as migrated only when its normal entry point draws through Kit. A dependency or a parallel implementation alone does not complete migration.

| Program | On Kit today | Not on Kit yet |
|---|---|---|
| `cupertino-osd` | The whole overlay: a Kit window on a layer surface for volume, brightness and keyboard backlight. | |
| `cupertino-settings` | The Settings window: sidebar, back and forward toolbar with the search field, the panes' sections as Kit controls, sheets as dialogs, and menus on the desktop's menu bar. Privacy > Screen Sharing lists each display an app may share again without asking, with Forget. | The old painter and window modules are deleted. Accessibility > Screen reader turns the session's reader on and off through the server (tested against a fake session and a fake reader); a run with a real screen reader and full app accessibility acceptance remain open. |
| `cupertino-files` | The whole app: sidebar with removable media (mount, unlock, eject), toolbar with the animated search field and "No Results", list and icon views, path and status bars, menus and context menus, live folder updates, file operations, Copy and Paste Item between folders, multiple windows, and items dragged out of the icon view with an image of the item or a stack with its count. | The old window is deleted. Kit windows take no drops, so nothing can be dropped onto a folder or a place; items wear symbols, not file-type icons or thumbnails; the toolbar has no view, Tags or Action popups. See the crate README's open issues. |
| `cupertino-portal` | Its dialogs (file chooser, screen cast chooser, app chooser, screenshot consent). See [Portals](portals.md). | The colour picker overlay and the screen-sharing menu bar item are drawn without Kit (a frozen screen with a loupe; a status item the menu bar draws). |
| `cupertino-controlcenter` | The whole panel: a Kit window on a layer surface with module toggles, tiles, the battery meter and level sliders. It is dismissed when its display is unplugged and opens on a remaining or reconnected one. | Real display hot-plug is not yet tried (#22). |
| `cupertino-menubar` | Bar and menus on Kit layer surfaces. A click on empty bar space closes the open menu and the keyboard goes back to the window. | |
| `cupertino-launcher` | Search panel, catcher and hidden windows on Kit layer surfaces. | See the crate README for interaction and styling limits. |
| `cupertino-notifications` | Banners and notification centre on Kit layer surfaces. | Application icons from image data, image paths and desktop entries are not drawn yet. |
| `cupertino-screenshot` | The overlay with its selection, window pick, size readout and crosshair, the toolbar and its Options menu, the floating thumbnail and the clipboard pixel, all Kit windows on layer surfaces. | Screen recording and markup are not implemented. |
| `cupertino-dock` | The window-manager UI: the overview chrome (workspaces bar, window titles, selection) and the application switcher panel as Kit windows (`overview_kit`, `switcher_kit`), tested headless. | The dock itself, and the dock process's drawing path for the overview and switcher: wiring them in comes with the Dock's move to Kit, which deletes the old painters. |
| `cupertino-login` | The lock screen (Kit windows as session-lock surfaces, one per output), the greeter's picker, name and password screens and power menu, the log out, restart, shut down and power-button panels with the "Reopen windows" checkbox, and the interrupt panel. The password field is Kit's secure field. | Real suspend, console switches and PAM logins are checked by hand (the crate README lists the checks). |

## The OSD

`cupertino-osd` is one 200 by 200 point layer surface at the bottom of the screen, with no keyboard focus and no input. It shows the volume (the default PipeWire sink, else the ALSA master), the display brightness and the keyboard backlight, and the microphone mute. It fades after a short hold. With Reduce Transparency or Increase Contrast it draws an opaque background. The session starts and restarts it through the `shell` key of the server's configuration. Run `cupertino-osd --fake-backends` to see it without touching the machine's volume. The crate's README has the details.

## The Control Center

`cupertino-controlcenter` opens below the menu bar at the top right as a Kit window on a layer surface, built from the panel controls of `cupertino-ui-controls`: module toggles, tiles, a battery meter and level sliders. It takes the keyboard while it is shown; Escape or a press outside it closes it. With Reduce Transparency or Increase Contrast it draws an opaque background. Run `cupertino-controlcenter --fake-backends` to try it without touching the machine's radios, volume and backlight. The crate's README has the details.

## The screenshot tool

`cupertino-screenshot` puts a Kit window on a layer surface over each output, and with the toolbar shortcut a Kit toolbar at the bottom of the first output, with its Options menu as a Kit menu. Everything works from the keyboard: Escape cancels, Space switches between selection and window pick, the arrow keys span, move and (with Alt) resize the selection or go through the windows, Return captures, and on the toolbar Tab reaches each control. Assistive technology reads what a capture would take and can capture, cancel and press every toolbar button. Escape, Close and a capture remove the overlay and the toolbar and give the keyboard back before anything is copied, so a capture never contains them. Selections are cut correctly from turned, mirrored and mixed-scale outputs. The crate's README has the details.

## Menus

A Kit window publishes its menus to the desktop's menu bar, and an item chosen there sends its message back to the app. A Kit app can also ask for an activation token for another application, and the Dock and the window switcher show the icon that a window sets for itself.

## What this means for you

Nothing you have to change. The programs above run as before, with the same shortcuts. Where the table says "not yet", the old window is still what you get.
