# cupertino-apps

The application registry shared by the Cupertino processes: desktop entries, their `Exec` lines, the shared MIME database
and default applications. Plain Rust over the freedesktop text formats, with no C library and no dependencies. Layer: `foundation`.

`cupertino-launcher` re-exports its `desktop` and `exec` modules, `cupertino-icons` uses it for document icons, and `cupertino-files` to open files.

## Public API

| Module | Contents |
|---|---|
| `entry` | Desktop Entry Specification: `parse_desktop_entry` (listings: `NoDisplay`, `Hidden`, `OnlyShowIn` / `NotShowIn` against `XDG_CURRENT_DESKTOP`, `TryExec`), `parse_handler` (MIME handlers: only `Hidden` and `TryExec` remove an entry, the `MimeType` list is kept), localised keys with the `lang_COUNTRY@MODIFIER` fallback, `index_dirs` with desktop file ID shadowing |
| `exec` | `Exec` quoting, field codes (`%f %F %u %U %i %c %k`), `CommandSpec` with the activation token variables and terminal wrapping |
| `mime` | `MimeDatabase::load(data_dirs)`: `globs2` (literal names first, then suffix and wildcard globs by weight, then pattern length; `:cs` case-sensitive globs; `__NOGLOBS__`), `aliases`, `subclasses` with the implicit `text/plain` and `application/octet-stream` parents, `icons` and `generic-icons`; `type_for_name`, `canonical`, `parents`, `ancestors`, `is_a`, `icon_name`, `generic_icon_name` |
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

`cargo test -p cupertino-apps`.
