# cupertino-assets

The asset packs every Cupertino process draws its text, icons, cursors and wallpaper from, and the `cupertino-assets`
tool that checks a pack. A pack is a directory with a `manifest.json`; packs live outside the repository. With no
pack installed, the built-in fallback (the embedded Inter subset, procedural cursors and placeholder icons) is used.

## Manifest

Version 2, a superset of version 1 (a version-1 manifest is read as the version-2 entries it means):

```json
{
  "version": 2,
  "name": "my-pack",
  "credits": "CREDITS.md",
  "fonts": { "ui": { "family": "Inter", "files": ["fonts/Inter-Variable.ttf"] } },
  "fallback_fonts": ["fonts/NotoSans-Regular.ttf"],
  "icons": {
    "folder.documents": { "raster": { "32": "icons/documents-32.png", "64": "icons/documents-64.png" } },
    "document.pdf": { "svg": "icons/pdf.svg" },
    "symbol.wifi": { "symbol": "symbols/wifi.svg" },
    "symbol.battery": { "theme": ["battery-symbolic", "battery-full-symbolic"] }
  },
  "icon_theme": { "name": "Adwaita", "map": "icon-map.toml" },
  "cursors": { "theme": "my-cursors", "path": ["cursors", "$XCURSOR_PATH", "~/.local/share/icons", "/usr/share/icons"] },
  "wallpapers": { "default": { "light": "wallpapers/day.jpg", "dark": "wallpapers/night.jpg" } }
}
```

| Key | Meaning |
|---|---|
| `fonts` | per role (`ui`, `ui-rounded`, `ui-compact`, `mono`, `serif`): the family as stored in the font's name table and its files. Version 1: `"ui": "Family"`, every font file of `fonts/ |
| `fallback_fonts` | faces loaded after the roles' files, for the characters those lack |
| `icons` | per logical name, sources tried in order: `raster` (pixel size to PNG; version 1: a size list, files `icons/<name>/<px>.png`), `svg`, `symbol` (a template SVG the caller tints, `symbol.*` names only; version 1: `true`, file `icons/<name>/symbol.svg`), `theme` (freedesktop names in the host icon theme) |
| `icon_theme` | the host theme for `theme` sources (`name`; the user's theme when omitted) and a default map `map`, TOML `"<logical name>" = ["<freedesktop name>", ...], for the names the pack does not list |
| `cursors` | the XCursor theme and its search directories (pack-relative, absolute, `~/…`, `$XCURSOR_PATH`; default `cursors`). Version 1: the theme name |
| `wallpapers` | the default picture, light and optional dark. Version 1: `"wallpaper": "<path>" |
| `name`, `credits` | the pack's name and a credits file |

Paths are pack-relative with plain components. An unknown key, an invalid entry or a missing file is reported and
skipped; only invalid JSON or an unsupported version makes the pack unusable.

## Resolution

`Assets::discover()` reads the packs of `search_path()`, in order: `$CUPERTINO_ASSET_DIR` (when set and not empty),
`$XDG_DATA_HOME/cupertino/assets` (`~/.local/share/cupertino/assets` when unset), `/usr/share/cupertino/assets`, then the
built-in fallback. Resolution is per logical name: the first pack that provides a name wins. Nothing here stops a process.

| Kind | Built-in fallback |
|---|---|
| Fonts | the embedded subset of Inter (`fonts/InterVariable-subset.ttf`, family `Inter Variable`, SIL Open Font License 1.1 in `fonts/OFL.txt`; rebuilt by), always loaded last |
| Icons, symbols | a placeholder rounded square drawn by `cupertino-icons` (filled grey for an icon, an outline in the tint for a symbol) |
| Cursors | the procedural cursor set of `cupertino-cursor`, drawn by the server |
| Wallpaper | the procedural wallpaper |

## API

| Item | Contents |
|---|---|
| `Assets::discover | the packs of `search_path`; never fails. `Assets::from_dirs(&[dir])` reads given directories the same way; `Assets::builtin has no pack |
| `Assets::read_pack(dir) | one pack and the problems it reports; fails only on an unreadable manifest, invalid JSON or an unsupported version |
| `Assets::open(dir) | the strict check of one pack: no problem at all |
| `Assets::icon_sources(&name, px) | every source of the packs for the name, with its pack, in trial order: `IconSource::{Raster, Svg, Symbol, Theme} |
| `Assets::fonts | `FontSource::File` of every pack in order, then `FontSource::Builtin`; `Assets::font_family(role) is the family of the first pack that assigns the role |
| `Assets::cursor_theme | `Some(CursorTheme { name, search_dir }) of the first pack whose theme is found |
| `Assets::wallpaper_path(dark) | the first pack's picture for the appearance |
| `Assets::credits`, `Assets::roots | the packs' directories, names and credits files |
| `RASTER_NAMES`, `SYMBOL_NAMES`, `CURSOR_NAMES` | the logical names the shell looks up |
| `AssetError` | `Read`, `Manifest`, `Font` (raised by the text renderers when no usable face is left) |

## Checking a pack

```sh
cargo run -p cupertino-assets -- check <dir>
```

It prints the problems of the pack, then per logical name what provides it (the pack's source or the built-in fallback)
and the list of names that fall back. It exits 1 only when the manifest cannot be read, is not valid JSON or has an
unsupported version, never because assets are missing.

## Open issues

Every process draws the system's icons from this crate by logical name and reports the names no pack provides at
startup (`SystemIcons::required`); they are drawn as placeholders. A public default pack (an open icon theme map) is not
in this tree yet. `volume.internal` and `volume.external` are not in the directory (the `IOStorageFamily`
icons were never extracted) and `document.script` has no payload icon (scripts use `document.text`). The semantic icon name
list is not checked against the contract's list, only its shape.

## Tests

`cargo test -p cupertino-assets`. The tests build a fixture directory (manifest and placeholder files) in a temporary directory.
Crates that draw text need a pack with the real fonts for their tests: set `CUPERTINO_ASSET_DIR` (without it they draw with the
built-in font and their pixel expectations fail).
