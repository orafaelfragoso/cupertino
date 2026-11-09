# cupertino-apps

The application registry shared by the Cupertino processes: desktop entries, their `Exec` lines, the shared MIME database
and default applications, launching with an activation token, the recently-used files and the launcher's usage store. Plain
Rust over the freedesktop text formats and JSON, with no C library. Layer: `foundation`.

`cupertino-launcher` re-exports its `desktop` and `exec` modules and keeps its usage store in the `frecency` format,
`cupertino-icons` uses it for document icons, `cupertino-files` to open files and to list Recents, the dock to launch,
and the menu bar to launch and to list Recent Items. The dock and the launcher ask a resident application for a window
with `activate` (feature `dbus`, which adds `zbus`; without it the crate has no D-Bus code).

## Public API

| Module | Contents |
|---|---|
| `activate` (feature `dbus`) | `org.freedesktop.Application.Activate` on the bus name of a desktop file ID (`object_path`, `platform_data` with the activation token as `activation-token` and `desktop-startup-id`, `activate`), and `activate_in_thread`, the call on the session bus on a thread of its own, bounded by `ACTIVATE_TIMEOUT` (3 s), ending in whether it was answered |
| `entry` | Desktop Entry Specification: `dbus_activatable` / `dbus_activatable_file` (`DBusActivatable=true` in the main group), `parse_desktop_entry` (listings: `NoDisplay`, `Hidden`, `OnlyShowIn` / `NotShowIn` against `XDG_CURRENT_DESKTOP`, `TryExec`), `parse_handler` (MIME handlers: only `Hidden` and `TryExec` remove an entry, the `MimeType` list is kept), localised keys with the `lang_COUNTRY@MODIFIER` fallback, `index_dirs` with desktop file ID shadowing |
| `exec` | `Exec` quoting, field codes (`%f %F %u %U %i %c %k`), `CommandSpec` (`from_argv`) with the activation token variables and terminal wrapping |
| `launch` | What a shell process starts, `Launch::{Program, App, Open}`, and its command with an activation token (`Launch::command`: an application without files, a file through the `%f %F %u %U` codes); `open_with_default` (a file's default application, `inode/directory` for a folder, `application/octet-stream` for a name no glob matches); `spawn` (detached, the exit reaped on a thread and an unsuccessful one logged); `OPEN_COMMAND` (`xdg-open`, for what no installed application handles). The token request stays in each process, on its own Wayland connection. |
| `recent` | `recently-used.xbel` in the data home (`xbel_path`, `XBEL_NAME`), read only: `parse_xbel` (local files in file order, each once), `parse_recent_files` / `read_recent_files` (with the latest `added`, `modified` or `visited` time of each file) |
| `frecency` | The launcher's usage store `cupertino/launcher.json` in the data home (`store_path`, `default_path`): `Store` (versioned, a missing, corrupt or foreign file reads empty), `ItemRecord`, `decayed` (half-life `HALF_LIFE_SECS`, 14 days), `ranked`, `ranked_apps` (application keys `app:<id>` without their actions, in the launcher's order). The launcher writes it; others only read. |
| `mime` | `MimeDatabase::file_type` (the type a file opens as: `inode/directory` for a folder, its name's type, else `application/octet-stream`; no content sniffing); `MimeDatabase::load(data_dirs)`: `globs2` (literal names first, then suffix and wildcard globs by weight, then pattern length; `:cs` case-sensitive globs; `__NOGLOBS__`), `aliases`, `subclasses` with the implicit `text/plain` and `application/octet-stream` parents, `icons` and `generic-icons`; `type_for_name`, `canonical`, `parents`, `ancestors`, `is_a`, `icon_name`, `generic_icon_name` |
| `mimeapps` | `MimeApps::load(XdgDirs, MimeDatabase, Locale, Visibility)`: `default_for(mime)` and `open_with(mime) |

## How it works

`mimeapps.list` files are read in the order of the MIME Applications Associations specification 1.0.1. Each directory's
`<desktop>-mimeapps.list` (one per `XDG_CURRENT_DESKTOP` entry, lower-cased) comes before its `mimeapps.list`. The
directories are `$XDG_CONFIG_HOME`, `$XDG_CONFIG_DIRS`, `$XDG_DATA_HOME/applications` and `$XDG_DATA_DIRS/applications`.
Desktop-specific files only set defaults.

- **Associations of a type.** Walking the levels, each adds its `[Added Associations]`, blocks its `[Removed Associations]
  for the levels below, then adds the entries of its application directory that list the type. A level's changes only
  reach entries installed at that level or below.
- **Default.** For the type, then each ancestor nearest first, the first `[Default Applications] entry that is installed
  and associated with that type, else the first association. A handler of a more specific type wins over a default set
  for a broader one. Aliases resolve to their canonical type everywhere.
- **Open With.** The associations of the type and its ancestors, without `application/octet-stream`. That type joins only
  when the list is otherwise empty (the design's second, broader pass). The default comes first and the rest
  follow by name, case-insensitive with numbers by value. Equal names sort by desktop file ID (the
  reference breaks the tie on the bundle version, which a desktop entry lacks).

Out of scope:

- `mimeinfo.cache` is not read. The `MimeType` keys it is built from are.
- Types come from file names only. `magic` is not read, so a file whose name matches no glob has no type here, and the
  caller uses `application/octet-stream` for a regular file, as the shared-mime-info specification does.

## Tests

`cargo test -p cupertino-apps` (`--features dbus` adds the activation unit tests; the call against a private bus is
`cupertino-dock`'s `tests/activate.rs`).
