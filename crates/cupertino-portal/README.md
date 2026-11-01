# cupertino-portal

The session's xdg-desktop-portal backend. Applications ask xdg-desktop-portal, and xdg-desktop-portal asks this
backend for the interfaces `data/cupertino.portal` lists. The server sets `XDG_CURRENT_DESKTOP=Cupertino`, so
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
- `parent_window` is not followed yet: the dialog is a window of its own, centred.
- A folder is read on a thread of its own, so one on a hung mount leaves the window responsive.

## Installing

- `bin/cupertino-portal`
- `data/cupertino.portal` to `/usr/share/xdg-desktop-portal/portals/`
- `data/cupertino-portals.conf` to `/usr/share/xdg-desktop-portal/`: Settings and FileChooser from this backend
  first, then the GTK backend for what it does not serve; every other interface from the GTK backend
