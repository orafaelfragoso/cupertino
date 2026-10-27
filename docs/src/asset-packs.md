# Asset packs

Fonts, icons, cursors and wallpapers are not hard-coded. Every process loads them from **asset packs**. You can replace the whole look or one part of it without rebuilding anything.

## Where packs are searched

A pack is a directory with a `manifest.json`. The packs are searched in this order:

1. `$CUPERTINO_ASSET_DIR` (when set and not empty)
2. `$XDG_DATA_HOME/cupertino/assets` (`~/.local/share/cupertino/assets` when unset)
3. `/usr/share/cupertino/assets`
4. the built-in fallback

Resolution is **per asset**: for each logical name the first pack that provides it wins, and a name no pack provides comes from the built-in fallback. Nothing here stops a process: a missing pack, file or name is logged once and drawn as a placeholder. Only invalid JSON or an unsupported `version` makes one pack unusable, and the search goes on with the next.

| Kind | Built-in fallback |
|---|---|
| Fonts | an embedded subset of Inter (SIL Open Font License 1.1), always loaded last |
| Icons, symbols | a placeholder rounded square |
| Cursors | the procedural cursor set of `cupertino-cursor` |
| Wallpaper | a procedural wallpaper |

The default pack uses Inter, Cupertino's built-in cursors, the host system's freedesktop icon theme and wallpapers chosen for the project. Packs are data and are not part of the source tree.

## Manifest, version 2

```json
{
  "version": 2,
  "name": "my-pack",
  "credits": "CREDITS.md",
  "fonts": {
    "ui":   { "family": "Inter", "files": ["fonts/Inter-Variable.ttf"] },
    "mono": { "family": "Noto Sans Mono", "files": ["fonts/NotoSansMono.ttf"] }
  },
  "fallback_fonts": ["fonts/NotoSans-Regular.ttf", "fonts/NotoSansArabic.ttf"],
  "icons": {
    "folder.documents":    { "raster": { "32": "icons/documents-32.png", "64": "icons/documents-64.png" } },
    "document.pdf":        { "svg": "icons/pdf.svg" },
    "symbol.wifi":         { "symbol": "symbols/wifi.svg" },
    "symbol.battery":      { "theme": ["battery-symbolic", "battery-full-symbolic"] },
    "application.generic": { "theme": ["application-x-executable"] }
  },
  "icon_theme": { "name": "Adwaita", "map": "icon-map.toml" },
  "cursors": { "theme": "my-cursors", "path": ["cursors", "$XCURSOR_PATH", "~/.local/share/icons", "/usr/share/icons"] },
  "wallpapers": { "default": { "light": "wallpapers/day.jpg", "dark": "wallpapers/night.jpg" } }
}
```

A version-1 manifest keeps working: it is read as the version-2 entries it means.

Rules:

- **Paths** are relative to the pack directory, plain components only (no `..`, no absolute path). The one exception is a cursor `path` entry, which may be an absolute directory or use `$XCURSOR_PATH` or `~`.
- **Icon sources**, one per name, tried in this order when several are present: `raster` (pixel size to PNG; the smallest size at least the request wins, else the largest), `svg` (full-colour vector), `symbol` (a single-colour template SVG that the caller tints; only for names that start with `symbol.`), `theme` (an ordered list of freedesktop icon names resolved in the host icon theme and its `Inherits` chain, then `hicolor`). A name with none of these is "not provided" and falls through to the next pack.
- **`icon_theme`** supplies a default for every name the pack does not list: `name` is the host theme to search (omit it to use the user's theme) and `map` is a TOML file of `"<logical name>" = ["<freedesktop name>", ...]`.
- **Fonts:** `ui` is the role the code asks for first; the roles `ui-rounded`, `ui-compact`, `mono` and `serif` are optional. The family is the name stored in the font's name table, not the file name. Weights 400, 500, 600 and 700 should be present (a variable font with named instances, or static faces). `fallback_fonts` are loaded after the roles' files for characters those lack.
- **Wallpapers:** PNG or JPEG; `light`, and `dark` as an option. `credits` names a file the About window can show.
- **Unknown keys** are ignored and reported once, so a newer pack still loads on an older shell.

## Logical names

A pack uses the logical names the shell looks up: 26 raster icon names (such as `folder.documents` and `document.pdf`), 50 `symbol.*` names, the font roles, 34 cursor names and the wallpaper. The lists are the constants `RASTER_NAMES`, `SYMBOL_NAMES` and `CURSOR_NAMES` in `cupertino-assets` (`crates/cupertino-assets/src/name.rs`). A name no code looks up may be left out.

## Build your own pack

1. Create a directory, for example `~/.local/share/cupertino/assets/my-pack/`, and make it the pack: point `CUPERTINO_ASSET_DIR` at it, or install it directly as `~/.local/share/cupertino/assets`.
2. Copy `manifest.json` from the example above; set `version` to 2 and a `name`.
3. **Fonts:** put your UI family in `fonts/` and set `fonts.ui.family` to its family name. Add fallback faces for the scripts you need (text is shaped left to right only).
4. **Icons:** for each logical name you want to replace, add an entry (`raster`, `svg`, `symbol` or `theme`). Rasters are RGBA PNGs at the sizes the shell asks for. Symbols are single-colour SVGs on a transparent background with a square `viewBox` and no hard-coded colour; the shell tints them. Names you skip come from the host theme.
5. **Cursors:** install or copy a freedesktop cursor theme (a directory with `index.theme` and `cursors/`) that provides the 34 CSS cursor names (`default`, `pointer`, `text`, `grab`, `not-allowed`, the `*-resize` names, and so on; aliases may be symlinks) and set `cursors.theme` to its directory name. `default` or `left_ptr` is the only mandatory name.
6. **Wallpapers:** add `wallpapers.default` (`light`, optionally `dark`), large enough for your displays, and credit the author in your credits file.
7. **Check it:**

   ```bash
   cargo run -p cupertino-assets -- check <dir>
   ```

   It prints the problems of the pack, then for every logical name which pack or fallback provides it, and lists the names that fall back. It exits 1 only when the manifest cannot be read, is not valid JSON or has an unsupported version, never because assets are missing. Then start a nested session with `CUPERTINO_ASSET_DIR=<dir>` and look at the dock, the file manager, the settings app and the control panel.
8. **Share it (optional):** ship the directory as a package that installs to `/usr/share/cupertino/assets/<name>/`. A pack that contains other people's artwork must keep their licence files next to it.

## Licences

The pack's content has its own licence; it is separate from the licence of the source code (`GPL-3.0-or-later`). Do not add third-party assets to this repository; distribute them as a pack.
