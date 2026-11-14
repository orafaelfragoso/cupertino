# What runs on CupertinoKit

CupertinoKit is the toolkit that Cupertino's own apps draw with. This page says what each of them draws with it and what is still open. Assessed on 2026-11-07 at the end of the Phase 4 work. A program counts as migrated only when its normal entry point draws through Kit. A dependency or a parallel implementation alone does not complete migration. Kit rasterizes every window on the CPU and hands it to the compositor in shared memory; rendering on the GPU is planned ([#155](https://github.com/orafaelfragoso/cupertino/issues/155)): a process started with `CUPERTINO_RENDERER=gpu` already draws its windows with the GPU renderer and hands them over as dmabufs, but no app does so by default yet.

| Program | On Kit today | Still open |
|---|---|---|
| `cupertino-osd` | The whole overlay: a Kit window on a layer surface for volume, brightness and keyboard backlight. | |
| `cupertino-settings` | The Settings window in the full-height-sidebar style: the sidebar with the window control buttons at its top and a search field over tiled rows below them, a floating back and forward toolbar, each pane's header card over its sections as Kit grouped-form controls, sub-pages opened from navigation rows through the same history with their title in the toolbar, sheets as dialogs, and menus on the desktop's menu bar. Privacy > Screen Sharing lists each display an app may share again without asking, with Forget. Privacy > Desktop Control and Screen Capture list each app that asked to control the desktop or to take pictures of the screen, with a switch that allows or stops it at once and Forget, which makes it ask again; the page follows the file while it is open. Notifications sets whether notifications show on the lock screen and how much of them shows (Always, When Unlocked, Never), by default and per application. | The old painter and window modules are deleted. Accessibility > Screen reader turns the session's reader on and off through the server (tested against a fake session and a fake reader), and the shared accessibility audit checks the window after every step of its tests; speech and braille with a real screen reader are checked by hand in an installed session. |
| `cupertino-files` | The whole app: a live sidebar on the sidebar material with removable media (mount, unlock, eject; Primary+E ejects the row the keyboard is on) and Trash once something is trashed, read over AT-SPI; the full-height-sidebar window style with a floating glass toolbar: the view-mode choice, the View, Tags and Action menus and the search field that opens from a round button; icon view, list view with disclosure triangles and a column view (a chain of folders that scrolls sideways, Right and Left between them, resizable columns kept with the view state and a preview of the selected item at its end), all with a rubber band that scrolls at the edges (the list's starts on a row's blank part too), click-to-rename, Ctrl+click menus and Tab by name; a clickable path bar and the status bar; an accessibility audit (every control read and acted on by assistive technology and checked by the shared audit at every step, Tab and Shift+Tab through the whole window in mirror order, column headers included, the keyboard back where it was after menus, renames and a closed folder, a polite live region for transfers, search results and selections, selected rows in the selected text colour); menus and context menus, live folder updates, file operations as jobs of the process (copy, move, duplicate, New Folder with Selection and drops run off the window's thread and outlive the window that started them; a progress window lists each with its bar and Stop, opens after a second, from Window › Show Progress or from activating Files with no window open, and only hides when closed; a failure opens it; a failure, the end of an operation that ran past that second and an end nobody saw are each one notification, whose click shows the progress window again; nothing pasted on read-only media), Copy and Paste Item between folders, multiple windows of one process per session (a later launch from the dock, a terminal or a folder opened elsewhere hands its folder, or its wish for a window, to the running Files over the session bus), items dragged out of either view with an image of the item, the row or a stack with its count, files dropped from any application onto folders, sidebar places, path bar folders, the Trash and the folder shown, highlighted while over them, folders that spring open under a held drag and give the window back when it leaves or is cancelled, moved within a device and copied across devices, and items that wear their type's icon from the icon theme (special folders their own, links the alias badge) and pictures their thumbnail, made off the UI thread in the freedesktop cache; a preview window that Space opens on the selection (a picture, the start of a text file or the item's icon and details, the arrows through several items, an Open with button, a motion that Reduce Motion turns into a fade); tabs on one long track (Primary+T and Primary+W, dragged to reorder, each with its own history and view); and a tags popover that puts colour and named tags on the selection in `user.xdg.tags`, with the tags in use in the sidebar and found by the search. | The old window is deleted. Only PNG, JPEG and WebP pictures get thumbnails and previews (no PDF preview); no gallery view; no Group By; the list offers no AT-SPI Selection interface; when the row the keyboard is on goes away, a screen reader hears the focus on the table or frame around it until Tab or a click moves it on. See the crate README's open issues. |
| `cupertino-portal` | Its dialogs (file chooser, screen cast chooser, app chooser, screenshot consent). See [Portals](portals.md). | The colour picker overlay is drawn without Kit (a frozen screen with a loupe). The screen-sharing item is a status item that the menu bar draws. |
| `cupertino-polkit-agent`, `cupertino-keyring-prompter` | Nothing: the authentication prompt and the keyring's password prompt draw its password panel with `tiny-skia` and presents it on shared-memory buffers; its scrim is one pixel of its colour the compositor stretches over each output (`ShmPresenter::draw_solid`). | Both prompts are still to be moved to Kit. |
| `cupertino-controlcenter` | The whole panel: a Kit window on a layer surface with separate glass tiles, level sliders and detail views. It is dismissed when its display is unplugged and opens on a remaining or reconnected one. | Real display hot-plug is not yet tried (#22). |
| `cupertino-menubar` | Bar and menus on Kit layer surfaces. A click on empty bar space closes the open menu and the keyboard goes back to the window. | |
| `cupertino-launcher` | One glass panel for search (results beside a preview of the selected one and its actions, the preview enlarged on Space) and for browsing the applications (category filters, a grid or a list, the recently used first), which the Dock's app-grid action opens and Escape folds back into the search field; catcher and hidden windows on Kit layer surfaces. | See the crate README for interaction and styling limits. |
| `cupertino-notifications` | Banners and notification centre on Kit layer surfaces: glass cards with wrapped bodies, a centre that scrolls, groups that expand out of their stack, and banners a screen reader speaks with their app, title and body. While the session is locked banners wait in the centre and are not said; the lock screen gets only what the lock-screen policy allows, and nothing is said again at the unlock. | Application icons from image data, image paths and desktop entries are not drawn yet. |
| `cupertino-screenshot` | The overlay with its selection, window pick, size readout and crosshair, the toolbar and its Options menu, the floating thumbnail and the clipboard pixel, all Kit windows on layer surfaces. | Screen recording and markup are not implemented. |
| `cupertino-dock` | The bar, its context menus, the dragged tile, the Force Quit dialog and the edge strips that move the bar to another display, as Kit windows on layer surfaces, with keyboard navigation (Focus Dock) and accessible names and actions on every tile. The application switcher panel and the overview chrome (workspaces bar, window titles, selection), one of each per display. Everything is drawn through Kit; the process paints nothing itself. | Multi-monitor acceptance on hardware is open (#22). |
| `cupertino-login` | The lock screen (Kit windows as session-lock surfaces, one per output), the greeter's picker, name and password screens and power menu, the log out, restart, shut down and power-button panels with the "Reopen windows" checkbox, and the interrupt panel. The password field is Kit's secure field, busy while an attempt is checked, invalid after a wrong password until the next edit, and capped at PAM's 512 bytes. The lock screen shows the notifications the notification daemon lists for it, redacted unless the application's previews are always shown. | Real suspend, console switches and PAM logins are checked by hand (the crate README lists the checks). |

## The OSD

`cupertino-osd` is one 200 by 200 point layer surface at the bottom of the screen, with no keyboard focus and no input. It shows the volume (the default PipeWire sink, else the ALSA master), the display brightness and the keyboard backlight, and the microphone mute. It fades after a short hold. With Reduce Transparency or Increase Contrast it draws an opaque background. The session starts and restarts it through the `shell` key of the server's configuration. Run `cupertino-osd --fake-backends` to see it without touching the machine's volume. The crate's README has the details.

## The Control Center

`cupertino-controlcenter` opens below the menu bar at the top right as a transparent Kit window on a layer surface. Each module is its own glass tile, and Wi-Fi, Bluetooth, display and sound grow into a detail view with their networks, devices or outputs. It takes the keyboard while it is shown; every tile is reachable with Tab and the arrow keys. Escape or a press outside it closes it. Its fade in and out run on the compositor's clock, so opening it draws no frame after the first. With Reduce Transparency the tiles are opaque on one opaque panel. Run `cupertino-controlcenter --fake-backends` to try it without touching the machine's radios, volume and backlight. The crate's README has the details.

## The screenshot tool

`cupertino-screenshot` puts a Kit window on a layer surface over each output, and with the toolbar shortcut a Kit toolbar at the bottom of the first output, with its Options menu as a Kit menu. Everything works from the keyboard: Escape cancels, Space switches between selection and window pick, the arrow keys span, move and (with Alt) resize the selection or go through the windows, Return captures, and on the toolbar Tab reaches each control. Assistive technology reads what a capture would take and can capture, cancel and press every toolbar button. Escape, Close and a capture remove the overlay and the toolbar and give the keyboard back before anything is copied, so a capture never contains them. Selections are cut correctly from turned, mirrored and mixed-scale outputs. The crate's README has the details.

## Menus

A Kit window publishes its menus to the desktop's menu bar, and an item chosen there sends its message back to the app. Kit's own menus (pop-up buttons, context menus) follow the pointer and keys as the menu bar's do, through `cupertino-menu-model`'s `tracking`: a press dragged onto an item chooses it on release while a quick click leaves the menu open, a submenu stays open while the pointer heads into it, and typed letters find an item. A Kit app can also ask for an activation token for another application, and the Dock and the window switcher show the icon that a window sets for itself.

## Frame time

Files and Settings each have a repaint report that times every frame of their common interactions with the window
filling a 3840×2160 display at scales 2, 1.5 and 1: `repaint_budget_report` in `crates/cupertino-files/tests/kit-repaint.rs`
and in `crates/cupertino-settings/src/shell/tests/repaint.rs`. Each frame is timed from its input to the presented
frame (update, view, layout, paint, raster on the CPU and the copy of the damage), and the report gives p50, p95, p99,
maximum, the frames over 8 ms and the share of the window each frame repainted. The tests are `#[ignore]`d, as wall
time depends on the machine; the crates' READMEs say how to run them. Tests that do run pin what each interaction
repaints instead (`crates/cupertino-files/tests/kit-damage.rs`, the damage tests of the Settings report's module).

Measured on 2026-11-08 on an AMD Ryzen 7 5800X (8 cores, 16 threads) in the release profile, one test thread, with
other builds running on the machine, Files with a folder of 1500 files (p99 at scales 2 / 1.5 / 1, in ms):

| Interaction | p99 | Frames over 8 ms |
|---|---|---|
| Files: arrow-key selection among the rows in view | 5.6 / 6.2 / 8.0 | 1 of 30 at scale 1 |
| Files: hover over the rows | 3.2 / 4.2 / 6.3 | none |
| Files: the caret's blink in the search field | 3.6 / 4.4 / 7.6 | none |
| Files: the search field opening | 10.1 / 12.2 / 11.7 | 12 of 13 at scale 2, all 13 at 1.5 and 1 |
| Files: the search field closing | 8.9 / 9.1 / 11.5 | 8, 6 and 13 of 14 |
| Files: typing a new name | 4.8 / 6.1 / 8.6 | 10 of 12 at scale 1 |
| Files: typing a search | 28.9 / 31.9 / 38.5 | 1, 2 and 6 of 11 |
| Files: opening the folder of 1500 files | 21.1 / 19.1 / 24.8 | both frames |
| Files: wheel scrolling the listing | 27.3 / 29.2 / 38.1 | all |
| Files: an arrow key past the last row in view (scrolls a row) | 32.0 / 30.8 / 38.3 | all |
| Settings: switching panes | 11.9 / 6.9 / 5.0 | 3 of 8 at scale 2 |
| Settings: toggling a switch, dragging a slider, the caret | under 1 | none |
| Settings: scrolling a pane (at its default size) | 0.4 / 0.4 / 0.3 | none |
| Settings: typing a search (the page lists the results and the sidebar keeps the panes found) | 16.0 / 12.7 / 7.7 | 1 of 8 at scales 2 and 1.5 |

Before the first damage work every frame of Files but the caret's and a hover's repainted 85–95 % of the window and
took 57–96 ms. Now a node that draws nothing (a stack, a scroll view's content, the listing's gesture layer) damages
nothing, a node that draws damages its box, and a change that moves nothing keeps the layout. A folder opened and the
search results still repaint everything the listing shows. A scroll moves the listing's kept image and paints only
the rows that come into view; the icon grid builds only the cells near its viewport, keeps each one while the window
only scrolls (`Element::kept`) and makes an item's icon only for a cell it builds. A full-screen scroll still takes
over 8 ms a frame: the toolkit still lays out and paints its whole tree each frame and copies the moved viewport into
the shared-memory buffer, which the GPU rendering planned for toolkit applications removes
([Roadmap](roadmap.md#planned-gpu-rendering-in-applications)).

When the Settings pages became grouped forms on their own background, switching panes went from 10.2 / 7.1 / 4.9 ms
to 39.4 / 29.1 / 20.3 ms and typing a search from 15.7 / 11.9 / 7.7 to 60.6 / 46.1 / 30.5 ms: the CPU renderer
blended every layer pixel by pixel, the page's background over the window's, the groups' fills over the page's, at
about 10.5 ns a damaged pixel. The renderer now takes a shorter way for solid fills, with the same pixels to the
byte (`crates/cupertino-ui-render/README.md`):

- The pixels a solid rectangle or rounded rectangle covers fully are written as rows, copied when the colour is
  opaque; only its anti-aliased rim goes through an image of its own.
- A one-colour write waits until something else paints there, so a group's fill over the page's background over the
  window's becomes a single write.
- Nodes under an opaque fill that covers a whole damaged rectangle are not painted, and the rectangle is not cleared.

A damaged pixel of Settings now costs about 2.4 ns instead of 13.5 to 14, and a pixel the Files listing scrolls
about 4 instead of 7.8. With
these paths turned off, the same build on the same machine took 51.0 / 37.4 / 25.5 ms to switch panes, 75.1 / 55.5 /
37.3 ms to type a search, 54.9 / 57.3 / 62.2 ms to scroll the Files listing and 5.9 / 6.5 / 7.4 ms for an arrow key
among the rows in view. Opening and closing the Files search field takes as long either way; it is slower than on
2026-11-07 for reasons outside the renderer.

## What this means for you

Nothing you have to change. The programs above run as before, with the same shortcuts. Each of them draws its windows with Kit; the "Still open" column lists what it still lacks. The password prompts (polkit's and the keyring's) and the portal's colour picker are the only parts drawn without Kit.
