# cupertino-screenshot

The screenshot tool of the Cupertino desktop. It stands in for the design's capture tool. It captures the whole
screen, a window or a selection. Before the capture it shows the crosshair overlay, and the toolbar on request. After the
capture it shows the floating thumbnail, then writes `Screenshot <date> at <time>.png` or puts the PNG on the clipboard.

It is also the portal's colour picker and its chooser of what a screen share shows: the server starts it with
`--serve` when the portal asks (`org.cupertino.Session1.StartScreenshot`), and the portal calls
`org.cupertino.Screenshot1` on it.

It is a CupertinoKit program on `cupertino-kit-shell` (`cupertino_kit_shell::run`): Kit draws every surface,
kit-shell gives them their layer roles and captures the outputs and windows. The crate splits so a host can drive or
test the UI without a display:

- the pure model: `session`, `selection`, `pick` (the window under the pointer), `thumbnail`, `filename`,
  `settings`, `toolbar` (the Options menu's rows);
- `view`, the overlay, toolbar, Options menu, thumbnail and clipboard pixel as Kit windows, and `controller`, one run
  from the overlay to the delivered capture;
- `files`: the settings file and the PNG files, run on Kit's I/O pool;
- `shot`: the program of one capture (`--mode`);
- `color_picker`, `share_picker`: the colour pick and the sharing choice, model and windows;
- `control`: `org.cupertino.Screenshot1`; `serve`: the program of the portal's service (`--serve`).

Its values come from the analysis (register,
``). The values marked approximate below are
authored and replaceable.

Layer: `shell`. Depends on:

- `cupertino-kit` (its windows, images and task pools), `cupertino-kit-shell` (layer roles, outputs, capture, the
  session bus);
- `cupertino-defaults` (the settings directory, `user_dirs`, atomic writes), `cupertino-geometry`,
  `cupertino-diagnostic-events`, `cupertino-peer` (who calls `Screenshot1`);
- `zbus` and `async-channel` (the interface), `rustix` (the PNG file's no-replace link).

## Running

The server starts one process per screenshot shortcut, as the design spawns its capture tool per hot key. The process
is not supervised. It exits when its capture is delivered or cancelled.

```
cargo build -p cupertino-screenshot
cupertino-screenshot --mode screen|selection|toolbar [--clipboard]
cupertino-screenshot --serve
```

`--mode` or `--serve` is required. An unknown argument is an error.

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

Only the server-passed connection holds the capture role that kit-shell's capture needs. Started by hand on the
public socket, the process captures nothing and ends with the capture's error.

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

**Toolbar.** The overlay plus the toolbar, a 560 × 48 pt window on regular glass (as the reference draws its capture toolbar;
with Reduce Transparency, or where the desktop shows no glass, the opaque heads-up fill in its shape), centred 40 pt
above the bottom of the first output and above the dock's exclusive zone. It takes the keyboard; the overlays take none.

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

**Turned and mixed-scale outputs.** kit-shell's capture gives every image upright at its output's scale, a selection
cut where it is on the upright output. Each output keeps its own scale.

**Floating thumbnail.** The thumbnail sits in the bottom-right corner.

- Size: a tenth of the first output along the image's long axis.
- Inset: 16 pt from the corner. It stays above the dock's exclusive zone.
- It commits after 5 s, on a swipe right past 30 pt, or on a click.
- On commit the file is written, then the thumbnail slides twice its width to the right and fades over 0.5 s,
  ease-in-ease-out. After a swipe it moves faster, by the release velocity.

**The file** appears whole: the PNG is written to a hidden `.<name>.<pid>.part` in the same directory and linked to its name (`files::write_new`),
so a watcher that sees `Screenshot … .png` reads all of it, and a file of that name that appeared meanwhile is never replaced. Without hard links (FAT, exFAT) the hidden file is renamed with `RENAME_NOREPLACE`; where that is missing too the PNG is written to the name directly (`create_new`), which is not atomic but never replaces a file.

**Clipboard.** The capture is offered as `image/png` through Kit (`Command::SetClipboardData`). Wayland needs a focused
surface to set the selection, so a transparent 1 x 1 surface takes the keyboard, and the offer is made once it has it.
The process then serves pastes until another selection replaces it.

**Threads.** The Kit loop runs the windows and the capture's requests, which kit-shell sends once the view that drops
the overlays was reconciled, so their destruction reaches the server first. Reading and writing the settings and the
PNG files run on Kit's I/O pool, the thumbnail's picture and the PNG for the clipboard on its CPU pool.

## The portal's service

`cupertino-screenshot --serve` serves `org.cupertino.Screenshot1` at `/org/cupertino/Screenshot1` on its own session
bus connection, which owns the name. Only the program the server started as `portal` may call
(`Session1.ProgramOf`); any other caller gets `AccessDenied`. Each answer starts with a response: 0 success,
1 cancelled, 2 failed.

| Method | Answer |
|---|---|
| `PickColor(s handle)` | `(u response, (ddd) colour)`: red, green and blue in sRGB from 0 to 1 |
| `SelectSources(s handle, s app_name, s app_icon, s parent_window, b offer_remember, a(sss) sources)` | `(u response, u index, b remember)` |
| `Close(s handle)` | Takes the call's picker down unanswered |

**Colour picker.** Every output is copied when the call comes, and its copy is shown frozen under an overlay on it
(layer namespace `cupertino-screenshot`, the capture overlay's, so no screenshot shortcut starts during a pick). A
crosshair marks the hidden pointer, a loupe of 11 x 11 pixels follows it, and a click takes the colour of the pixel
under it. A click counts only once its overlay has been on screen 0.5 s, so a click meant for the app picks nothing.
Escape cancels. An output added, gone or moved ends the pick. One pick runs at a time.

**Sharing choice.** A window titled "Share Screen" (app id `org.cupertino.Portal.Prompt`) shows the app, the displays
and windows the portal lists, and, when the app asked to restore the share, a box to let it share again without asking.
Nothing can be chosen for 0.5 s after the window's first frame; the focus starts on Cancel; Escape cancels.
`sources` are `("display", connector, description)` or `("window", title, app id)`.

The service exits once it has had nothing to do for 30 s (`serve::IDLE_EXIT`) and when its bus connection goes; the
portal starts it again for its next call.

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

Desktop, Documents and Pictures resolve through `$XDG_CONFIG_HOME/user-dirs.dirs`, read by
`cupertino_defaults::user_dirs`. The fallbacks are the xdg-user-dirs defaults, `$HOME/Desktop` and so on. The
directory is created when missing.

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
