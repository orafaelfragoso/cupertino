# cupertino-screenshot

The screenshot tool of the Cupertino desktop. It stands in for the design's capture tool. It captures the whole
screen, a window or a selection. Before the capture it shows the crosshair overlay, and the toolbar on request. After the
capture it shows the floating thumbnail, then writes `Screenshot <date> at <time>.png` or puts the PNG on the clipboard.

It has no Smithay dependency. The crate splits in two so a host can drive or test the UI without a display:

- the pure model: `session`, `selection`, `pick`, `thumbnail`, `filename`, `settings` and `toolbar`;
- the drawing: `render` and `image`.

Its values come from the analysis (register,
``). The values marked approximate below are
authored and replaceable.

Layer: `shell`. Depends on:

- `cupertino-animation`, `cupertino-defaults`, `cupertino-geometry`, `cupertino-theme` and `cupertino-decorations`;
- with `app`, also `cupertino-wayland-client`, `cupertino-shell-client`, `cupertino-protocol-app`, `cupertino-ui-wayland` (a
  presenter per surface) and `cupertino-protocols`.

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

**Toolbar.** The overlay plus the toolbar, centred 12 pt above the bottom of the first output.

- The mode buttons choose entire screen, window or selection. The recording buttons are drawn disabled, because this tool
  does not record.
- In selection mode the selection stays under a dim (black 0.5) with a clear hole, a black 1 pt and white dashed [4, 4]
  border, and eight handles.
- Drag inside the selection to move it, drag a handle to resize it, drag elsewhere to span a new one.
- Capture or Return takes it. In screen mode, a click on an output takes that output. In window mode, a click takes the
  window.
- Options opens the menu: save destination, timer (none, 5 s, 10 s), show the floating thumbnail, remember the last
  selection, show the mouse pointer.
- A timer counts down on the Capture button before the capture.
- The mode, and the selection while "Remember Last Selection" is on, are saved for the next run.

**Floating thumbnail.** The thumbnail sits in the bottom-right corner.

- Size: a tenth of the first output along the image's long axis.
- Inset: 16 pt from the corner. It stays above the dock's exclusive zone.
- It commits after 5 s, on a swipe right past 30 pt, or on a click.
- On commit the file is written, then the thumbnail slides twice its width to the right and fades over 0.5 s,
  ease-in-ease-out. After a swipe it moves faster, by the release velocity.

**The file** appears whole: the PNG is written to a hidden `.<name>.<pid>.part` in the same directory and linked to its name (`image::write_new`),
so a watcher that sees `Screenshot … .png` reads all of it, and a file of that name that appeared meanwhile is never replaced.

**Clipboard.** The capture is offered as `image/png`. Wayland needs a focused surface to set the selection, so a transparent
1 x 1 surface takes the keyboard for the moment of the offer. The process then serves pastes until another selection replaces
it.

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
- toolbar window, background, item positions, radius and material;
- timer choices;
- file-name pattern.

** and approximate.** The first group is derived from verified facts, the second is authored, not from the design.

| Value | Basis |
|---|---|
| Toolbar item x is relative to the background view | Capture then ends at the background's 560 pt edge |
| Mode buttons fill the background's height | Nib geometry |
| Thumbnail shadow outset 56 pt | Section 6 of the, not a register measurement |
| Toolbar 12 pt above the bottom | |
| Selection tint role | |
| Date/time split of `YYYY-MM-dd--h-mm-ss-a` at `-- | Pattern `%@ %@ at %@ |
| The dock's exclusive zone stands in for the design's dock rectangle | Thumbnail placement, |
| Window pick: Highlight alpha 0.35; Pointer: the system arrow | |
| Crosshair: Centre gap: ±3 px | |
| Handles: Drawn as 9 pt dots | |
| Size readout: Text `W × H` in points; Backdrop colour; 12 pt offset from the pointer, flipped at the edges | |
| Toolbar and menu: Glyph artwork; 13 pt text; Menu row metrics; Selected-button fill; Popup arrow placement | |
| Toolbar: Stacked above the overlay; the design's window levels say otherwise | |
| Thumbnail: Shadow drawn as a Gaussian, sigma 12; Border colour; No hover pause; A click commits instead of opening markup; Only the first image is shown when several screens were captured; It never moves left of its place | |
| File name: ` (n) numbering of taken names; Day period separated by a space; Calendar year instead of week-based year | |
| Settings: Defaults other than `remember-selection` | |
| Clipboard: First output only for a multi-output screen capture | |
| Timer: Countdown drawn as the seconds on the Capture button (TimerAttachmentCell geometry unknown) | |

**Text.** The `ui` font is a variable font whose optical size follows the point size, so every `TextRenderer` owner calls `set_scale` with the output scale before it draws and measures (`measure` works in points, `text_in_band` at the render scale): text is laid out in points at the same optical size it is drawn with, at every output scale.

## Open issues

Not implemented:

- Screen recording.
- Markup.
- The thumbnail's context menu and drag-out.
- Stacking of several thumbnails (unknown in the design).
- The thumbnail's opacity during a drag: 0.25, whose role is not established.

## Tests

```
cargo test -p cupertino-screenshot
```

| Test | Proves |
|---|---|
| Session unit tests | Direct and toolbar selection, window pick, Space, Escape, Control, timer, remembered selection. |
| Selection unit tests | The selection geometry and readout. |
| Window pick unit tests | The window pick. |
| Thumbnail unit tests | The thumbnail's size, expiry, click, swipe and drag. |
| File-name unit tests | The file name and numbering. |
| Settings unit tests | The settings file and the user-dirs resolution. |
| Toolbar unit tests | The toolbar layout and menu. |
| Image unit tests | Shm-to-image conversion and selection cropping. |
| Render unit tests | The overlay's dim and hole, and drawing at sub-pixel positions and fractional scales. |
| `cupertino-server` `tests/shell_processes.rs`, selection shortcut | The overlay maps, and a drag writes a 200 x 150 PNG. |
| `tests/shell_processes.rs`, toolbar | The toolbar maps at its place, and Return captures the screen. |
| `tests/shell_processes.rs`, clipboard shortcut | The process offers a PNG of the output's size. |
| `cupertino-server` `tests/capture.rs` | The capture protocols. |
| QEMU check `26-screenshot` | A full-screen capture in the guest. |
