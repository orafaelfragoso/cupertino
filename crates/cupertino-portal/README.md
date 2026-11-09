# cupertino-portal

The session's xdg-desktop-portal backend. Applications ask xdg-desktop-portal, and xdg-desktop-portal asks this
backend for the interfaces `data/cupertino.portal` lists: Settings, FileChooser, ScreenCast, AppChooser and Screenshot. The server sets `XDG_CURRENT_DESKTOP=Cupertino`, so
xdg-desktop-portal reads `cupertino-portals.conf` to pick the backend for each interface.

## Running

Feature `app` builds the `cupertino-portal` binary. The server starts and supervises it from its installed path. It
owns `org.freedesktop.impl.portal.desktop.cupertino` on the session bus and serves `/org/freedesktop/portal/desktop`.
There is no D-Bus activation file: an instance that the bus starts would lack the server's Wayland connection.

```
cargo build -p cupertino-portal --features app
```

It reads the session's appearance before it takes the bus name, so the first read after the server publishes the
session already has it. It exits with a failure (and the server restarts it) when the Wayland connection is lost, when a
change cannot be sent on the bus, or when another process holds the bus name.

## Settings

`org.freedesktop.impl.portal.Settings` version 1 serves the `org.freedesktop.appearance` namespace from the session's
appearance, as `cupertino_app_effects_v1` sends it:

| Key              | Value                                             |
| ---------------- | ------------------------------------------------- |
| `color-scheme`   | `1` (prefer dark) when dark, else `2` (prefer light) |
| `contrast`       | `1` (higher) with Increase Contrast, else `0`     |
| `reduced-motion` | `1` (reduced) with Reduce Motion, else `0`        |

`ReadAll` matches a namespace exactly or by a trailing `*`; an empty list or an empty pattern matches every namespace.
`Read` of any other key fails, so xdg-desktop-portal asks the next backend. A change of appearance emits
`SettingChanged` once for each key whose value changed.

## File chooser

`org.freedesktop.impl.portal.FileChooser` answers `OpenFile`, `SaveFile` and `SaveFiles` with a window drawn with
CupertinoKit: a sidebar of the home folder and the user directories, the open folder's list, and a bottom bar with the
filters, the choices, Cancel and the action button. A save adds a Save As field and asks before it replaces a file.
Each row wears the icon of its type from the asset packs and the host icon theme, typed by name with the session's MIME
database, as in Files (`cupertino_kit::file_icons`); the rows show no thumbnails and no special folder icons.

- Only the owner of `org.freedesktop.portal.Desktop` may call it or close a request; any other caller gets
  `AccessDenied`. Options are checked before a window shows: a wrong type, a relative folder, a `current_name` with a
  `/`, an unknown filter kind, a filter pattern over 256 bytes or a string or list over its bound is `InvalidArgs`. At
  most 16 dialogs are open at once, 4 for one app id.
- Glob filters match without regard to case, in at most pattern × name steps; MIME filters match by extension. Folders
  always show. The filter runs once for each listing, not on each redraw.
- The title, the action label, filter names and choice labels lose control and bidirectional-override characters.
- The answer is `0` with `uris` (`file://`, escaped), `choices` and `current_filter`; Cancel or closing the window is
  `1`; `Close` on the request takes the window down and answers `2`.
- A `parent_window` of `wayland:<handle>` makes the dialog a child of that window (xdg-foreign-unstable-v2, `wayland_parent`). An `x11:` window, an empty one, or a handle the server does not know leaves the dialog a window of its own, centred.
- A folder is read on a thread of its own, so one on a hung mount leaves the window responsive.

## Screen cast

`org.freedesktop.impl.portal.ScreenCast` version 5 shares one display or one window with an application as a PipeWire
video stream (`cupertino-screencast`). It is served only when the server offers the image capture globals
(ext-image-copy-capture-v1, ext-output-image-capture-source-v1, ext-foreign-toplevel-image-capture-source-v1 and
ext-foreign-toplevel-list-v1).

- `CreateSession` adds an `org.freedesktop.impl.portal.Session` object at the session handle. `SelectSources` takes
  `types` (monitor, window or both), `cursor_mode` (hidden or embedded), `persist_mode` (0, 1 or 2; another value is
  `InvalidArgs`) and `restore_data`; `multiple` is read but one source is shared.
- `Start` shows a window, "“<app>” wants to share your screen. Choose a display or a window.", the app named and drawn
  by its desktop entry (`<app id>.desktop`: `Name=` and an `Icon=` that is an icon name) else shown as its id, or as
  "An application" for an empty id. It lists the displays and the windows, by title and app id, with nothing
  selected. When the app asked to persist and has an id, a check box "Let “<app>” share it again without asking" is
  offered, unticked.
- The window takes no answer that grants for 500 ms after its first frame is on screen (the frame callback that
  answers its first committed frame; a window the server shows late starts the delay late, and one it never shows
  stays unarmed): clicks, row picks and keys do nothing
  until then, and a key held since before keeps doing nothing, since Return shares only on a fresh press (its
  repeats are ignored). Share is the default button and is enabled only once armed and a row is picked; Escape cancels at once.
- The keyboard focus starts on Cancel, and a screen reader announces Cancel first. Until the window is armed, Return,
  Enter and Space press no button at all (not even Cancel), so a key typed for the app neither shares nor denies; Tab
  moves the focus, but Share and the check box are no Tab stops until armed. Once armed, Return and Space press the
  focused button: Cancel denies, Share shares; with the focus in the list Return presses Share. Escape, a click on
  Cancel and closing the window deny, armed or not.
- The answer to `Start` is `0` with one entry in `streams`: the node id, `source_type`, `size` (logical for a display,
  in pixels for a window) and `position` for a display. It has `persist_mode` and `restore_data` when the person
  ticked the box, else `persist_mode` 0. Cancel or closing the window is `1`; a `Close` on the request or the session
  while the window shows takes it down and answers `2`, as does the app leaving the bus (the frontend closes the
  session).
- Remembering. `restore_data` is `("cupertino", 1, <grant id>)`, the id 32 random hexadecimal digits; it means
  nothing without this backend's record. A window is remembered only until the app quits (`persist_mode` 1 at most):
  its identifier lasts no longer. A display the app may keep (`persist_mode` 2) is written to `screen-sharing.conf`
  (`cupertino-defaults::screen_sharing`, mode `0600`) as `grant = <id> <app id> display <connector>`, read, changed
  and written under an advisory `flock` on `.screen-sharing.conf.lock` beside it, which Settings' Forget takes too, so
  neither drops nor brings back the other's change; a transient
  grant is kept in memory, at most 64. A later `SelectSources` with that `restore_data` and the same app id shares the
  same source again without a window, as long as it is still there; otherwise the window shows. The grant is the
  frontend's app id's alone: it never restores for another id or an empty one.
- Forgetting. Settings › Privacy › Screen Sharing lists each remembered display by app, with Forget, which reads the
  file again under its lock and takes out that one line, never writing back the list the pane read; the next request
  shows the window again. A share running then goes on until it is stopped. Forget is the only way a remembered
  display is taken back: Stop Sharing ends the share and leaves `screen-sharing.conf` alone, so the app may share the
  display again without asking until the person forgets it in Settings. A remembered window is not listed in Settings
  and lasts only until the app quits; Stop Sharing drops it with the share, so the app asks again.
- While a share runs the portal shows a status item in the menu bar (StatusNotifierItem
  `org.kde.StatusNotifierItem-<pid>-<n>` with a `com.canonical.dbusmenu` menu, registered with
  `org.kde.StatusNotifierWatcher` and again whenever a watcher appears): tooltip and first menu row "“<app>” is sharing
  “<source>”", then Stop Sharing, which ends the share and closes the session (`Closed`), and keeps a remembered
  display (see Forgetting). The menu's events (`Event`, `EventGroup`) are taken only from the connection that owns
  `org.kde.StatusNotifierWatcher` when they arrive, the menu bar the item registered with; any other caller gets
  `org.freedesktop.DBus.Error.AccessDenied` and the share runs on, so another application cannot end a share it did not
  start. Without a menu bar nobody can press Stop Sharing; the share still ends with the app or its session. The item goes
  when the share ends for any reason. The server's privacy dot in the menu bar shows for any capture besides.
- Frames are copied with an image copy session straight into the stream's memfds through a `wl_shm` pool, the cursor
  painted in when it is embedded. A frame is captured only while PipeWire has a free buffer, at most at the framerate
  the consumer fixed, and carries the copy session's damage. A resized source renegotiates the stream.
- `Session.Close` and the frontend leaving the bus stop the share. A closed window or a source that fails ends the
  share, which closes the session and emits `Closed`.
- A display without logical geometry is sized by its mode turned by its transform over its scale (`geometry`, with
  `cupertino-geometry`'s `OutputTransform`, every `wl_output` transform tested).
- Stream frames are not turned upright. They go to the stream exactly as the server copies them, and the copy's
  `transform` event is ignored (`capture::frame_event`), unlike a screenshot's or a colour pick's. Cupertino's server
  always copies a share upright and reports it as not turned, so this is right on Cupertino; on a server that sends
  turned frames a turned display would be streamed turned.
- The same callers, bounds and sanitizing as the file chooser apply: only the frontend calls, at most 16 sessions and 4
  for one app id, and titles lose control and bidirectional-override characters.

Known limitations: no cursor metadata, no region source, one source for each `Start`, shared memory only (no dmabuf),
and no audio. A display is remembered by its connector name, so a different screen on the same connector restores
too. The app id of an app outside a sandbox is what the frontend reads from its systemd unit; such an app runs as the
person and could edit `screen-sharing.conf` itself, so a grant protects against sandboxed apps only.

## App chooser

`org.freedesktop.impl.portal.AppChooser` version 2 asks which app opens a link or a file when xdg-desktop-portal's
OpenURI needs the person to pick or confirm one. The window is the consent: nothing opens unless they choose an app.

- `ChooseApplication` shows a window, "Open “<file name or URI>” with:", that lists the `choices` by the name of their
  desktop entry (`<id>.desktop` in `$XDG_DATA_HOME/applications` and each `XDG_DATA_DIRS` entry); an id with no
  installed entry shows as the id. Each row shows the entry's `Icon=` as `cupertino-icons` resolves an application's
  icon (the session's `application.` icons, then the `hicolor` theme and the pixmaps of each data directory); an entry
  with no icon, an icon that does not resolve, or an `Icon=` with a `/` or a control character shows the applications
  symbol. `last_choice` opens selected when it is one of `choices`. Open is the default button: Return opens the
  selected app, and Escape cancels. Unlike the screen-sharing and screenshot windows it has no arming delay and opens
  with the focus in the list, on the selected app: it hands the app nothing of the person's (no pixels, no files),
  only which installed app opens what the app already named, so the arrows and Return work at once.
- The answer is `0` with `choice` and an `activation_token`: the caller's own, handed on unchanged, when it sent one;
  else one the window asks the server for when the person opens the app, made for that key press or click and naming
  the chosen app, so the app may take the focus (and the Dock bounces its tile). When the server gives none the answer
  has no `activation_token`. Cancel or closing the window is `1`; `Close` on the request takes the window down and
  answers `2`; neither carries a token. An answer that is not one of the choices offered at that moment is `2` with
  no `choice`.
- `UpdateChoices` replaces the list of the open window at `handle`; the selection stays on its app while that app is
  still offered. Any other handle is `InvalidArgs`.
- The same callers and bounds as the file chooser apply: only the frontend calls, at most 16 windows and 4 for one app
  id, at most 256 choices, an id with a `/` or a control character is refused, and shown text loses control and
  bidirectional-override characters.

Known limitations: an `Icon=` that is an absolute path is not shown.

## Screenshot

`org.freedesktop.impl.portal.Screenshot` version 3 takes a screenshot of every display for an application. It is served
with screen cast, only when the server offers the image capture globals. The window is the consent: nothing is
captured unless the person agrees.

- `Screenshot` shows a window, "Screenshot", that says "“<app>” wants to take a screenshot of your screen.", the app
  named and drawn by its desktop entry (`<app id>.desktop` in `$XDG_DATA_HOME/applications` and each `XDG_DATA_DIRS`
  entry), else by its id; an empty id shows as "An application". The keyboard focus starts on Cancel, which a screen
  reader announces first. Take Screenshot has no shortcut and is enabled (and a Tab stop) only 500 ms after the
  window's first frame is on screen; until then Return, Enter and Space press no button, Cancel included, so neither
  a key nor a click meant for the app can consent. Once armed, Return and Space press the focused button, so Tab to
  Take Screenshot and Return consents. Escape, a click on Cancel and closing the window deny, armed or not.
- `modal`, `interactive` and `permission_store_checked` are read but change nothing: the same window shows every time,
  even when the frontend checked the permission store. `AvailableTargets` is `1` (the whole screen); a `target` other
  than `1` is `InvalidArgs`.
- Once the window has had 300 ms to leave the screen, one frame of each display is copied with an image copy session
  into a memfd, without the pointer. Each is turned upright when its copy says it is turned, and the displays are joined
  as they sit, at the largest scale among them; what no display covers is transparent.
- The PNG goes to `XDG_PICTURES_DIR` of `user-dirs.dirs`, else `~/Pictures`, created when missing, as
  `Screenshot <date> at <time>.png` like the screenshot tool names it, then ` (2)`, ` (3)`, … when the name is taken. It
  is created new (never replacing a file) and readable by its owner only (`0600`).
- The answer is `0` with `uri` (`file://`, escaped); Cancel or closing the window is `1`; `Close` on the request takes
  the window down and answers `2`, and so does a capture or a write that fails.
- `PickColor` lets the person pick the colour of one pixel on any display. One frame of every display is copied at once
  and frozen under an overlay layer surface on each display (namespace `cupertino-portal-color-picker`), the pointer
  hidden. A crosshair marks the pixel under the pointer and a loupe beside it shows the 11 × 11 pixels round it, ten
  points each, the middle one outlined; it moves to the other side of the pointer near an edge. A click picks: its
  press must come on the overlay at least 500 ms after it appeared, and its release takes the pixel under the pointer
  then, both on overlays on screen for at least 500 ms, counted for each overlay from the frame callback of its first
  frame; so a button held from before or a click meant for what was under the pointer picks nothing. Escape cancels.
- Every display has an overlay for the whole pick. The pick does not start when the server has a display that was not
  copied (one plugged in after the copy), and any display added, unplugged or moved during the pick ends it with `2`:
  a display without an overlay would pass a click to the app under it. A second `PickColor` while one runs answers
  `2` at once, before any display is copied for it.
- The pick is the consent, so no window asks first: the app learns one colour and only of a pixel the person clicked.
  The answer is `0` with `color`, `(ddd)` red, green and blue in sRGB from 0 to 1 (the display's 8-bit value over
  255); Escape is `1`; `Close` on the request takes the overlay down and answers `2`, as do displays that change
  during the pick, a copy that fails, a pick while another one runs, and a server without a layer shell.
- The same callers and bounds as the file chooser apply: only the frontend calls, at most 16 screenshots and 4 for one
  app id are pending, the app id and the parent window are at most 1024 bytes, and the app's name loses control and
  bidirectional-override characters.

Known limitations: no window or area target and no pointer in the image. A picked colour is the copied pixel's value,
without the display's colour profile, and the picker has no keyboard movement of the pointer.

## Diagnostic events

The process records versioned diagnostic events to the user's event store (`$XDG_STATE_HOME/cupertino/events`), each
beside the line or answer it already gives; `main` installs the recorder before anything else. App ids, interfaces and
D-Bus error names are recorded as they are; a path in an error's text is recorded as its hash.

| Event | Severity | Fields | Where |
|---|---|---|---|
| `portal.request.failed` | warn | `interface`, `method`, `app_id` (when the call has one), `error` (the D-Bus error name) | a call of FileChooser, AppChooser, ScreenCast, Session, Screenshot or Request is refused (a caller other than the frontend, bad arguments, a limit) |
| `portal.screencast.failed` | error | `stage` (`start`: the stream never reached the app; `running`: a share failed after), `app_id` (for `start`), `error` | a share does not start or ends on a failure |
| `portal.permission.store_failed` | warn | `operation` (`save`, `load`), `app_id`, `error` | `screen-sharing.conf` cannot be written (the share is kept only while the app runs) or read (nothing is restored) |
| `portal.screenshot.failed` | error | `method` (`Screenshot`, `PickColor`), `app_id` (when known), `error` | a granted screenshot or colour pick cannot copy, write or pick |
| `portal.exit_error` | error | `error` | the process ends with an error |
| `portal.panic` | fatal | `panic`, `location` | any panic |

`cupertino diagnose events --component cupertino-portal` lists them.

## Installing

- `bin/cupertino-portal`
- `data/cupertino.portal` to `/usr/share/xdg-desktop-portal/portals/`
- `data/cupertino-portals.conf` to `/usr/share/xdg-desktop-portal/`: Settings and FileChooser from this backend
  first, then the GTK backend for what it does not serve; ScreenCast, AppChooser and Screenshot (which carries
  `PickColor`) from this backend only; Secret from gnome-keyring; every other interface from the GTK backend

The packages ship the binary, `cupertino.portal` and the same routing file as
`packaging/files/usr/share/xdg-desktop-portal/cupertino-portals.conf`. `tests/portals_conf.rs` reads the packaged file
and checks it against the D-Bus names of the interfaces this crate implements: each is routed here first, nothing else
is, `cupertino.portal` lists exactly those, and the crate's copy is the same file.
