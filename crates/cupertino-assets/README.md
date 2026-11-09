# cupertino-assets

The asset packs every Cupertino process draws its text, icons, cursors and wallpaper from, and the `cupertino-assets`
tool that checks a pack. A pack is a directory with a `manifest.json`. The original first-party icon pack lives in
`data/assets/cupertino` (GPL-3.0-or-later); third-party packs live outside the repository. Its full-colour artwork uses
1024-unit source canvases, glass app tiles, sky-blue folders with recessed marks, folded white documents and neutral ribbed bins.
The pack supplies 16/32-pixel optical editions and 64–1024-pixel master renditions. Lettering is outlined geometry.
`data/assets/cupertino/CREDITS.md` describes regeneration through the same native SVG loader the shell uses. With no
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
`$XDG_DATA_HOME/cupertino/assets` (`~/.local/share/cupertino/assets` when unset), `/usr/share/cupertino/assets/cupertino`,
`/usr/share/cupertino/assets`, then the
built-in fallback. Resolution is per logical name: the first pack that provides a name wins. Nothing here stops a process.

| Kind | Built-in fallback |
|---|---|
| Fonts | the embedded subset of Inter (`fonts/InterVariable-subset.ttf`, family `Inter Variable`, SIL Open Font License 1.1 in `fonts/OFL.txt`; rebuilt as [The embedded font](#the-embedded-font) says), always loaded last |
| Icons, symbols | a placeholder rounded square drawn by `cupertino-icons` (filled grey for an icon, an outline in the tint for a symbol) |
| Cursors | the procedural cursor set of `cupertino-cursor`, drawn by the server |
| Wallpaper | the procedural wallpaper |

## The embedded font

`fonts/InterVariable-subset.ttf` is `InterVariable.ttf` from the Inter 4.1 release
(<https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip>, sha256
`9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`; the font inside, version
`4.001;git-9221beed3`, sha256 `4989b125924991b90d05b2d16e0e388c48f7d5bb8b30539bbf9c755278d0ccaf`) cut down with
`pyftsubset` from fontTools 4.67.0. It keeps both variation axes (`opsz` 14–32, `wght` 100–900) and the features
`calt`, `case`, `ccmp`, `frac`, `locl`, `tnum`, `kern`, `mark` and `mkmk`. With the code points below saved as
`unicodes.txt`:

```sh
pyftsubset InterVariable.ttf --unicodes-file=unicodes.txt --name-IDs='*' --name-languages='*' \
  --layout-features=calt,case,ccmp,frac,locl,tnum,kern,mark,mkmk --notdef-outline \
  --output-file=InterVariable-subset.ttf
```

The result is byte-identical to the committed file, sha256
`8082ab1587b6b1a0ee14e44287b204f3aae7f2021583ca76beb1f717a093dcea`, which `ci/gate.py` pins. The 1216 code points:

```text
U+0000, U+0020-007E, U+00A0-00AC, U+00AE-0148, U+014A-01C3, U+01C5-024F, U+0300-0304, U+0306-030A, U+030C, U+030F,
U+0313, U+0315, U+031B, U+0323, U+0326-0328, U+032C, U+0337-0338, U+0342-0343, U+0346-036F, U+0374-0376,
U+037A-037F, U+0384-038A, U+038C, U+038E-03A1, U+03A3-03D7, U+03DC-03DD, U+03F0-03F6, U+03F9-03FA, U+03FC-045F,
U+1E00-1E9B, U+1E9D-1EFF, U+2000-200B, U+2010-2027, U+202F-2055, U+2057, U+205F, U+20A0-20AF, U+20B1-20B5,
U+20B8-20BA, U+20BC-20C0, U+2100-2101, U+2103, U+2105-2106, U+2109, U+2113, U+2116-2117, U+211E-2122, U+2126,
U+212A-212B, U+212E, U+2132, U+213B, U+214D, U+2190-2199, U+21A9-21AA, U+21B0-21B1, U+21B3-21B5, U+21BA-21BB,
U+21D0, U+21D2, U+21D4, U+21DE-21DF, U+21E4-21E5, U+21E7, U+21EA, U+2212, U+221E, U+2248, U+2260, U+2264-2265,
U+2303, U+2318, U+2325-2326, U+232B, U+238B, U+25CB, U+25CF
```

Inter has no keypad Enter symbol (U+2324), so the menu bar names that key `Enter`.

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
unsupported version, never because assets are missing. `PackReport` is that report; `cupertino appearance assets check
<dir>` (crate `cupertino-cli`) prints the same text, or the report as JSON with `--format json`, and replaces it:
`cupertino-assets check` first prints one line on stderr naming it, and will be removed in a later release
(`tests/deprecation.rs`).

## Open issues

Every process draws the system's icons from this crate by logical name and reports the names no pack provides at
startup (`SystemIcons::required`). The packaged original icon pack provides all raster and symbol names; the
built-in fallback stays available to isolated tests. `volume.internal` and `volume.external` are not in the directory (those
icons do not exist yet) and `document.script` has no payload icon (scripts use `document.text`). The semantic icon name
list is not checked against the contract's list, only its shape.

## Tests

`cargo test -p cupertino-assets`. The tests build a fixture directory (manifest and placeholder files) in a temporary directory.
Tests run with the built-in font; set `CUPERTINO_ASSET_DIR` only to try another pack.
