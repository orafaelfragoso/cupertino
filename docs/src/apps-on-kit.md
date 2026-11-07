# What runs on CupertinoKit

CupertinoKit is the toolkit that Cupertino's own apps are moving onto. This page says which of them draw their windows with it on `main`, and which do not yet. Assessed at `0a928c70` on 2026-11-04. A program counts as migrated only when its normal entry point draws through Kit. A dependency or a parallel implementation alone does not complete migration.

| Program | On Kit today | Not on Kit yet |
|---|---|---|
| `cupertino-osd` | The whole overlay: a Kit window on a layer surface for volume, brightness and keyboard backlight. | |
| `cupertino-settings` | The Settings window: sidebar, back and forward toolbar with the search field, the panes' sections as Kit controls, sheets as dialogs, and menus on the desktop's menu bar. Privacy > Screen Sharing lists each display an app may share again without asking, with Forget. | The old painter and window modules are deleted. Accessibility > Screen reader turns the session's reader on and off through the server (tested against a fake session and a fake reader); a run with a real screen reader and full app accessibility acceptance remain open. |
| `cupertino-files` | The whole app: a live sidebar on the sidebar material with removable media (mount, unlock, eject; Primary+E ejects the row the keyboard is on) and Trash once something is trashed, read over AT-SPI; toolbar with the View, Tags and Action pop-up buttons and the animated search field; icon view and list view with disclosure triangles, both with a rubber band that scrolls at the edges, click-to-rename, Ctrl+click menus and Tab by name; a clickable path bar and the status bar; menus and context menus, live folder updates, file operations, Copy and Paste Item between folders (off the window's thread, with progress and Stop; nothing pasted on read-only media), multiple windows, items dragged out of either view with an image of the item, the row or a stack with its count, files dropped from any application onto folders, sidebar places, the Trash and the folder shown, highlighted while over them, moved within a device and copied across devices, and items that wear their type's icon from the icon theme (special folders their own, links the alias badge) and pictures their thumbnail, made off the UI thread in the freedesktop cache. | The old window is deleted. Folders do not spring open under a drag; only PNG, JPEG and WebP pictures get thumbnails; no Group By. See the crate README's open issues. |
| `cupertino-portal` | Its dialogs (file chooser, screen cast chooser, app chooser, screenshot consent). See [Portals](portals.md). | The colour picker overlay and the screen-sharing menu bar item are drawn without Kit (a frozen screen with a loupe; a status item the menu bar draws). |
| `cupertino-controlcenter` | The whole panel: a Kit window on a layer surface with module toggles, tiles, the battery meter and level sliders. It is dismissed when its display is unplugged and opens on a remaining or reconnected one. | Real display hot-plug is not yet tried (#22). |
| `cupertino-menubar` | Bar and menus on Kit layer surfaces. A click on empty bar space closes the open menu and the keyboard goes back to the window. | |
| `cupertino-launcher` | Search panel, catcher and hidden windows on Kit layer surfaces. | See the crate README for interaction and styling limits. |
| `cupertino-notifications` | Banners and notification centre on Kit layer surfaces. | Application icons from image data, image paths and desktop entries are not drawn yet. |
| `cupertino-screenshot` | The overlay with its selection, window pick, size readout and crosshair, the toolbar and its Options menu, the floating thumbnail and the clipboard pixel, all Kit windows on layer surfaces. | Screen recording and markup are not implemented. |
| `cupertino-dock` | The bar, its context menus, the dragged tile, the application grid, the Force Quit dialog and the edge strips that move the bar to another display, as Kit windows on layer surfaces, with keyboard navigation (Focus Dock) and accessible names and actions on every tile. The application switcher panel and the overview chrome (workspaces bar, window titles, selection), one of each per display. Everything is drawn through Kit; the process paints nothing itself. | Multi-monitor acceptance on hardware is open (#22). |
| `cupertino-login` | The lock screen (Kit windows as session-lock surfaces, one per output), the greeter's picker, name and password screens and power menu, the log out, restart, shut down and power-button panels with the "Reopen windows" checkbox, and the interrupt panel. The password field is Kit's secure field, busy while an attempt is checked, invalid after a wrong password until the next edit, and capped at PAM's 512 bytes. | Real suspend, console switches and PAM logins are checked by hand (the crate README lists the checks). |

## The OSD

`cupertino-osd` is one 200 by 200 point layer surface at the bottom of the screen, with no keyboard focus and no input. It shows the volume (the default PipeWire sink, else the ALSA master), the display brightness and the keyboard backlight, and the microphone mute. It fades after a short hold. With Reduce Transparency or Increase Contrast it draws an opaque background. The session starts and restarts it through the `shell` key of the server's configuration. Run `cupertino-osd --fake-backends` to see it without touching the machine's volume. The crate's README has the details.

## The Control Center

`cupertino-controlcenter` opens below the menu bar at the top right as a Kit window on a layer surface, built from the panel controls of `cupertino-ui-controls`: module toggles, tiles, a battery meter and level sliders. It takes the keyboard while it is shown; Escape or a press outside it closes it. With Reduce Transparency or Increase Contrast it draws an opaque background. Run `cupertino-controlcenter --fake-backends` to try it without touching the machine's radios, volume and backlight. The crate's README has the details.

## The screenshot tool

`cupertino-screenshot` puts a Kit window on a layer surface over each output, and with the toolbar shortcut a Kit toolbar at the bottom of the first output, with its Options menu as a Kit menu. Everything works from the keyboard: Escape cancels, Space switches between selection and window pick, the arrow keys span, move and (with Alt) resize the selection or go through the windows, Return captures, and on the toolbar Tab reaches each control. Assistive technology reads what a capture would take and can capture, cancel and press every toolbar button. Escape, Close and a capture remove the overlay and the toolbar and give the keyboard back before anything is copied, so a capture never contains them. Selections are cut correctly from turned, mirrored and mixed-scale outputs. The crate's README has the details.

## Menus

A Kit window publishes its menus to the desktop's menu bar, and an item chosen there sends its message back to the app. A Kit app can also ask for an activation token for another application, and the Dock and the window switcher show the icon that a window sets for itself.

## Frame time

Files and Settings each have a repaint report that times every frame of their common interactions with the window
filling a 3840×2160 display at scales 2, 1.5 and 1: `repaint_budget_report` in `crates/cupertino-files/tests/kit-repaint.rs`
and in `crates/cupertino-settings/src/shell/tests/repaint.rs`. Each frame is timed from its input to the presented
frame (update, view, layout, paint, raster on the CPU and the copy of the damage), and the report gives p50, p95, p99,
maximum, the frames over 8 ms and the share of the window each frame repainted. The tests are `#[ignore]`d, as wall
time depends on the machine; the crates' READMEs say how to run them. Tests that do run pin what each interaction
repaints instead (`crates/cupertino-files/tests/kit-damage.rs`, the damage tests of the Settings report's module).

Measured on 2026-11-07 on an AMD Ryzen 7 5800X (8 cores, 16 threads) in the release profile, one test thread, Files
with a folder of 1500 files (p99 at scales 2 / 1.5 / 1, in ms):

| Interaction | p99 | Frames over 8 ms |
|---|---|---|
| Files: arrow-key selection among the rows in view | 6.2 / 6.3 / 7.7 | none |
| Files: hover over the rows | 2.9 / 3.9 / 4.7 | none |
| Files: the caret's blink in the search field | 5.1 / 3.0 / 4.9 | none |
| Files: the search field opening | 7.0 / 6.4 / 9.4 | all at scale 1 |
| Files: the search field closing | 7.0 / 7.6 / 9.9 | all at scale 1 |
| Files: typing a new name | 4.1 / 5.3 / 8.8 | 1 of 12 at scale 1 |
| Files: typing a search (each key filters the listing) | 34.8 / 34.2 / 38.8 | 1 of 6 / 1 of 6 / 6 of 6 |
| Files: opening the folder of 1500 files | 27.2 / 23.8 / 45.2 | the first frames |
| Files: wheel scrolling the listing | 61.4 / 66.0 / 71.2 | all |
| Files: an arrow key past the last row in view (scrolls a row) | 63.4 / 68.3 / 71.8 | all |
| Settings: switching panes | 5.3 / 3.8 / 2.6 | none |
| Settings: toggling a switch, dragging a slider, the caret | under 0.5 | none |
| Settings: scrolling a pane (at its default size) | 6.0 / 3.5 / 2.2 | none |
| Settings: typing a search (the results pop up) | 15.8 / 11.0 / 7.6 | the first key at scales 2 and 1.5 |

Before this work every frame of Files but the caret's and a hover's repainted 85–95 % of the window and took
57–96 ms, and switching Settings panes repainted 96–98 % in 16–21 ms. A frame now repaints what its nodes'
operations changed, and a change that moves nothing keeps the layout. A scroll still repaints everything the listing
shows, at about 9 ns a pixel: up to 7.6 million pixels a frame at scale 1. Meeting 8 ms there needs the renderer to move the pixels it has, or the GPU composition
planned for Phase 6.

## What this means for you

Nothing you have to change. The programs above run as before, with the same shortcuts. Where the table says "not yet", the old window is still what you get.
