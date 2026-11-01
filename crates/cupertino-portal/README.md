# cupertino-portal

The session's xdg-desktop-portal backend. Applications ask xdg-desktop-portal, and xdg-desktop-portal asks this
backend for the interfaces `data/cupertino.portal` lists: Settings, FileChooser and ScreenCast. The server sets `XDG_CURRENT_DESKTOP=Cupertino`, so
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
  `types` (monitor, window or both) and `cursor_mode` (hidden or embedded); `multiple` and `persist_mode` are read but
  one source is shared and nothing persists. `Start` shows a window that lists the displays and the windows, by title
  and app id, with nothing selected. Share is the default button and is enabled only once a row is picked, so keys
  typed for the app before the window took focus share nothing: Return then shares the picked source, and Escape
  cancels.
- The answer to `Start` is `0` with one entry in `streams`: the node id, `source_type`, `size` (logical for a display,
  in pixels for a window) and `position` for a display, and `persist_mode` 0. Cancel or closing the window is `1`; a
  `Close` on the request or the session while the window shows takes it down and answers `2`.
- Frames are copied with an image copy session straight into the stream's memfds through a `wl_shm` pool, the cursor
  painted in when it is embedded. A frame is captured only while PipeWire has a free buffer, at most at the framerate
  the consumer fixed, and carries the copy session's damage. A resized source renegotiates the stream.
- `Session.Close` and the frontend leaving the bus stop the share. A closed window or a source that fails ends the
  share, which closes the session and emits `Closed`.
- The same callers, bounds and sanitizing as the file chooser apply: only the frontend calls, at most 16 sessions and 4
  for one app id, and titles lose control and bidirectional-override characters.

Known limitations: no restore tokens, no cursor metadata, no region source, one source for each `Start`, shared memory
only (no dmabuf), and no audio.

## Installing

- `bin/cupertino-portal`
- `data/cupertino.portal` to `/usr/share/xdg-desktop-portal/portals/`
- `data/cupertino-portals.conf` to `/usr/share/xdg-desktop-portal/`: Settings and FileChooser from this backend
  first, then the GTK backend for what it does not serve; ScreenCast from this backend; every other interface from the
  GTK backend

The packages ship the binary and `cupertino.portal`, with `packaging/files/usr/share/xdg-desktop-portal/cupertino-portals.conf`
in place of the one above: ScreenCast from this backend and every other interface from the GTK backend, until the
Settings and FileChooser backends are routed here too.
