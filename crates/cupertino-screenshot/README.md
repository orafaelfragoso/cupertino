# cupertino-screenshot

The screenshot tool of the Cupertino desktop. It stands in for the design's capture tool. It captures the whole
screen, a window or a selection. Before the capture it shows the crosshair overlay, and the toolbar on request. After the
capture it shows the floating thumbnail, then writes `Screenshot <date> at <time>.png` or puts the PNG on the clipboard.

It has no Smithay dependency. CupertinoKit draws every surface. The crate splits so a host can drive or test the UI
without a display:

- the pure model: `session`, `selection`, `pick`, `thumbnail`, `filename`, `settings`, `toolbar` (the Options menu's
  rows) and `image` (captured pixels to upright images and PNG);
- with `app`: `view`, the overlay, toolbar, Options menu, thumbnail and clipboard pixel as Kit windows, and
  `controller`, one run from the overlay to the delivered capture. The tests drive both through Kit's headless window
  system.

Its values come from the analysis (register,
``). The values marked approximate below are
authored and replaceable.

Layer: `shell`. Depends on:

- `cupertino-animation`, `cupertino-defaults` and `cupertino-geometry` (the turned-output math, `transform`);
- with `app`, also `cupertino-kit`, `cupertino-modifiers`, `cupertino-ui-wayland` (the Kit window system, with layer-surface
  roles), `cupertino-shell-client`, `cupertino-wayland-client` and `cupertino-protocols`.

## Running

Feature `app` builds the binary. The server starts one process per screenshot shortcut, as the design spawns its capture
tool per hot key. The process is not supervised. It exits when its capture is delivered or cancelled.

```
cargo build -p cupertino-screenshot --features app
cupertino-screenshot --mode screen|selection|toolbar [--clipboard]
```

`--mode` is required. An unknown argument is an error.

| Shortcut (default) | Server action | Arguments | Reference |
|---|---|---|---|
| Super+Shift+3 | `screenshot` | `--mode screen` | `-pd` |
| Super+Ctrl+Shift+3 | `screenshot-clipboard` | `--mode screen --clipboard` | `-pcd` |
| Super+Shift+4 | `screenshot-area` | `--mode selection` | `-pdi` |
| Super+Ctrl+Shift+4 | `screenshot-area-clipboard` | `--mode selection --clipboard` | `-pdic` |
| Super+Shift+5 | `screenshot-toolbar` | `--mode toolbar` | `-pdiU` |

| Setting | Effect |
|---|---|
| `screenshot` key of the server configuration | Names the program. Default `["cupertino-screenshot"]. |
| `screenshot.conf` in the Cupertino settings directory | The capture settings (see Settings). |
| `CUPERTINO_SCREENSHOT_CONFIG` | Redirects `screenshot.conf`. |
| `WAYLAND_SOCKET` | The server passes the process its own connection as a socket pair. |

While the overlay or toolbar is mapped (layer namespace `cupertino-screenshot`), a further shortcut starts nothing.

Only the server-passed connection sees the capture globals (`ext-image-capture-source-v1`, `ext-image-copy-capture-v1`,
`cupertino_screenshot_v1`). Started by hand on the public socket, the process fails at start with "binding …: was the process
started by the server?".

## How it works

**Screen.** Captures every output at once, one file each. The second and later files get ` (2)`, ` (3)`, … on the shared
time-stamped name.

**Selection.** A full-output overlay on every output.

- A drag spans the selection: gray 0.53 at 0.25. A size readout follows the pointer.
- Releasing the button captures the selection. A click without a drag captures nothing.
- Space switches to the window pick. The window under the pointer is highlighted, and a click captures that window with its
  decorations, without its shadow.
- Escape cancels.
- Control held at the release or click sends the capture to the clipboard.
- The overlay draws the crosshair and hides the system pointer.
- The first output's overlay takes the keyboard (see Keyboard).

**Toolbar.** The overlay plus the toolbar, a 560 × 48 pt window on the heads-up material, centred 40 pt above the bottom
of the first output and above the dock's exclusive zone. It takes the keyboard; the overlays take none.

- Close, the three mode buttons (a segmented control: entire screen, window, selection), Options and Capture. This tool
  does not record, so the recording buttons are left out.
- In selection mode the selection stays under a dim (black 0.5) with a clear hole, a black 1 pt and white dashed [4, 4]
  border, and eight handles.
- Drag inside the selection to move it, drag a handle to resize it, drag elsewhere to span a new one.
- Capture or Return takes it. In screen mode, a click on an output takes that output. In window mode, a click takes the
  window. Control held while Capture is clicked sends the capture to the clipboard.
- Options opens the menu below the button (above it when there is no room): save destination, timer (none, 5 s,
  10 s), show the floating thumbnail, remember the last selection, show the mouse pointer. The section titles are rows
  that cannot be chosen. Escape closes the menu before it cancels the run.
- A timer counts down on the Capture button before the capture.
- The mode, and the selection while "Remember Last Selection" is on, are saved for the next run. The selection is saved
  when it is captured.

**Keyboard.** The surface with the keyboard (the direct shortcut's first overlay, or the toolbar) takes these keys; on
the toolbar, Tab moves through the controls and Space or Return presses the focused one, and the keys below apply while
no control has the focus.

- Escape cancels; Space switches between selection and window pick; Return captures (Control+Return to the clipboard).
- In selection, an arrow key spans a selection of half the output in its middle when there is none, then moves it a
  point (Shift: 10 points); Alt with an arrow grows or shrinks it from the bottom-right corner.
- In the window pick, the arrows go through the windows front to back and the chosen one is highlighted.

**Assistive technology.** The overlay is a group named "Screenshot" whose description lists the keys and whose value says
what a capture takes now ("Selection 200 × 150 at 100, 100", "Window …", "No selection"). Its Click action captures, its
Focus action takes the keyboard focus. The toolbar is a toolbar of Kit buttons and a segmented control, each with its
own actions. The thumbnail is a button whose Click commits the capture.

**Leaving.** Escape, Close and a capture all take the overlay and the toolbar away. The overlay first shows the arrow (a
frame, or at most 0.1 s), so the pointer is not left hidden over what was under it; then the surfaces are destroyed, the
compositor gives the keyboard back, and only then is the capture copied, so it never contains them.

**Turned and mixed-scale outputs.** The capture comes as the output's buffer, turned and mirrored as the output is, at its
scale. A selection in logical points is cut from that buffer with `cupertino_geometry::area_in_buffer` and turned upright
(`image::area`); a whole output or a window is turned upright (`image::upright`). Each output keeps its own scale.

**Floating thumbnail.** The thumbnail sits in the bottom-right corner.

- Size: a tenth of the first output along the image's long axis.
- Inset: 16 pt from the corner. It stays above the dock's exclusive zone.
- It commits after 5 s, on a swipe right past 30 pt, or on a click.
- On commit the file is written, then the thumbnail slides twice its width to the right and fades over 0.5 s,
  ease-in-ease-out. After a swipe it moves faster, by the release velocity.

**The file** appears whole: the PNG is written to a hidden `.<name>.<pid>.part` in the same directory and linked to its name (`image::write_new`),
so a watcher that sees `Screenshot … .png` reads all of it, and a file of that name that appeared meanwhile is never replaced. Without hard links (FAT, exFAT) the hidden file is renamed with `RENAME_NOREPLACE`; where that is missing too the PNG is written to the name directly (`create_new`), which is not atomic but never replaces a file.

**Clipboard.** The capture is offered as `image/png`. Wayland needs a focused surface to set the selection, so a transparent
1 x 1 surface takes the keyboard for the moment of the offer, with the serial of its keyboard enter. The process then
serves pastes until another selection replaces it.

**Threads.** The Kit loop runs the windows. A capture thread owns a second event queue on the same connection: the
capture globals, the window feed, the clipboard source and a keyboard that counts enters. Both read the connection as
two libwayland readers do: every wait of the thread (its loop, a capture, a frame, a roundtrip) prepares its read,
then dispatches what the other reader already put in its queue, and only then sends and polls, so an answer the Kit
loop read for it is never left waiting. The roles hand a capture to the thread once the last overlay or toolbar surface is
destroyed, so the destroy requests reach the server first.

## Settings

`screenshot.conf` lives in the Cupertino settings directory. The toolbar writes it back. A bad line is reported on stderr and
its setting keeps the default.

```
location = desktop          # desktop, documents, pictures, clipboard or an absolute directory
timer = 0                   # 0, 5 or 10
show-thumbnail = true
show-pointer = false
remember-selection = true
mode = selection            # the toolbar's last mode: screen, window or selection
selection = 100,80,640,400  # the last selection, global points
```

The values shown are the defaults, except that `selection` is unset by default and `location` defaults to desktop.

Desktop, Documents and Pictures resolve through `$XDG_CONFIG_HOME/user-dirs.dirs`. The fallbacks are the xdg-user-dirs
defaults, `$HOME/Desktop` and so on. The directory is created when missing. The `user-dirs.dirs` parser
duplicates the one in `cupertino-files`, because a shell crate cannot depend on an application crate.

Each written path is printed on stdout.


**.** All are listed in `values`:

- thumbnail size, expiry, dismiss, swipe and drag thresholds;
- overlay dim, border, handles and tint;
- label metrics;
- crosshair geometry;
- toolbar size, radius, bottom margin, padding and spacing;
- the keyboard's selection size and large step;
- timer choices;
- file-name pattern.

** and approximate.** The first group is derived from verified facts, the second is authored, not from the design.

| Value | Basis |
|---|---|
| Thumbnail shadow outset 56 pt | Section 6 of the, not a register measurement |
| Toolbar 40 pt above the bottom | |
| Selection tint role | |
| Date/time split of `YYYY-MM-dd--h-mm-ss-a` at `-- | Pattern `%@ %@ at %@ |
| The dock's exclusive zone stands in for the design's dock rectangle | Thumbnail placement, |
| Window pick: Highlight alpha 0.35; Pointer: the system arrow | |
| Crosshair: Centre gap: ±3 px | |
| Handles: Drawn as 9 pt dots | |
| Size readout: Text `W × H` in points; Backdrop colour; 12 pt offset from the pointer, flipped at the edges | |
| Toolbar and menu: Kit's controls and menu, with text labels instead of glyphs | |
| Keyboard: the arrow keys, Alt resize, half-output first selection, 10 pt large step | |
| Toolbar: Stacked above the overlay; the design's window levels say otherwise | |
| Thumbnail: Shadow drawn as Kit's drop shadow, blur 12; Border colour; No hover pause; A click commits instead of opening markup; Only the first image is shown when several screens were captured; It never moves left of its place | |
| File name: ` (n) numbering of taken names; Day period separated by a space; Calendar year instead of week-based year | |
| Settings: Defaults other than `remember-selection` | |
| Clipboard: First output only for a multi-output screen capture | |
| Timer: Countdown drawn as the seconds on the Capture button (TimerAttachmentCell geometry unknown) | |

**Text.** The size readout and the toolbar's labels are shaped by Kit's text system, in points, at every output scale.

## Open issues

Not implemented:

- Screen recording.
- Markup.
- The thumbnail's context menu and drag-out.
- Stacking of several thumbnails (unknown in the design).
- The thumbnail's opacity during a drag: 0.25, whose role is not established.

## Tests

```
cargo nextest run -p cupertino-screenshot --all-features
```

| Test | Proves |
|---|---|
| Session unit tests | Direct and toolbar selection, window pick, Space, Escape, Control, timer, remembered selection. |
| Selection unit tests | The selection geometry and readout. |
| Window pick unit tests | The window pick. |
| Thumbnail unit tests | The thumbnail's size, expiry, click, swipe and drag. |
| File-name unit tests | The file name and numbering. |
| Settings unit tests | The settings file and the user-dirs resolution. |
| Toolbar unit tests | The Options menu's rows and what they change. |
| Image unit tests | Shm-to-image conversion; a selection cut from the buffer at scales 1 and 2 and from a buffer turned by each of the eight transforms comes out upright; downscaling. |
| `cupertino-geometry` `transform` tests | Each transform's point, rectangle and pixel mapping, its inverse, and fractional and mixed scales. |
| View and controller unit tests | App ids, the overlay's summary, and the phases from the overlay to the delivered capture. |
| `tests/surfaces.rs` (Kit headless) | A drag spans and captures; the arrow keys and Return capture; Escape and Close leave no window and no capture; the toolbar's controls are named and its buttons choose the mode and capture; Space and Return reach the overlay from the toolbar; Options opens a popup under the toolbar; the thumbnail commits once; the clipboard pixel goes after the offer. |
| `cupertino-server` `tests/shell_processes.rs`, selection shortcut | The overlay maps, and a drag writes a 200 x 150 PNG. |
| `tests/shell_processes.rs`, toolbar | The toolbar maps at its place and takes the keyboard, and Return captures the screen. |
| `tests/shell_processes.rs`, Escape | Escape removes the overlay and toolbar and the keyboard returns to the window that had it; nothing is captured. |
| `tests/shell_processes.rs`, clipboard shortcut | The process offers a PNG of the output's size. |
| `cupertino-server` `tests/capture.rs` | The capture protocols. |
| QEMU check `26-screenshot` | A full-screen capture in the guest. |
