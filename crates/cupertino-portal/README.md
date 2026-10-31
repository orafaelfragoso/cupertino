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

## Installing

- `bin/cupertino-portal`
- `data/cupertino.portal` to `/usr/share/xdg-desktop-portal/portals/`
- `data/cupertino-portals.conf` to `/usr/share/xdg-desktop-portal/`: Settings from this backend first, then the GTK
  backend for the namespaces it does not serve; every other interface from the GTK backend until this one serves it
