# cupertino-icons

The icons of the system and of applications for the server and the shell processes, and loading of PNG and SVG files into
premultiplied RGBA pixels. The system's own icons (folders, documents by type, the trash, badges, template symbols) come from
the asset packs (`cupertino-assets`) by logical name: a pack's raster, SVG or symbol file, or freedesktop names resolved in a
host icon theme (a pack's `theme` sources and the default map of its `icon_theme`: the theme the pack names, else
`Adwaita`, then its `Inherits` chain, then `hicolor`). A name no pack provides is drawn as a
placeholder rounded square. An application's icon comes only from its own `Icon=` entry: a name of the packs'
`application.` namespace (the session's own applications, `application.file-manager`, `application.settings`), an absolute
path or the `hicolor` theme. Depends on `cupertino-assets`, `cupertino-defaults` and `cupertino-apps`.

## Public API

| Item | Contents |
|---|---|
| `SystemIcons` | `new(&Assets)`; `required(&Assets, rasters, symbols)` the same, reporting on stderr every name no pack provides; `raster(name, px)` a raster icon (`folder`, `trash.full`, ...), `symbol(name, px, color)` a template symbol (`symbol.*`) tinted by the caller, both cached, the placeholder when no source loads; `has_raster(name, px) whether a source loads; `pack_has_raster(name)` whether a pack lists a raster or SVG file of its own for the name (the icon theme not counted) |
| `IconLookup` | an application's icon file: an `application.` name from the asset directory, an absolute path as it is, else the `hicolor` theme (`index.theme` size matching), then the flat `~/.icons` and pixmaps directories. `new(assets)` searches the asset directory, `~/.icons`, `<data dir>/icons` for every XDG data directory and `/usr/share/pixmaps`; `with_roots(assets, bases)` takes explicit roots; `find(name, size, scale) |
| `ThemeIcons` | the icons of applications, not of the system: `load(name, px)` gives a `tiny_skia::Pixmap` for an application icon name, path or `file://` URI (through `IconLookup`); `load_for_entry(entry, px)` the `Icon=` of the installed desktop entry with that id (hidden entries and stock names give `None`) |
| `load_icon(path, px) | decodes a PNG or renders an SVG (resvg) to an `IconImage` of `px` × `px` pixels |
| `system_mark(px, color)` | the system menu's mark (`data/system-menu-mark.svg`, built in, the project's own drawing) rendered to fit `px` × `px` pixels and tinted like a template symbol |
| `IconImage` | `width`, `height` and premultiplied RGBA bytes |
| `from_argb(width, height, argb) | converts the ARGB pixmaps of StatusNotifier items to an `IconImage` |
| `document` | file icons: `document_icon_name(db, mime)`, `corner_badge`, `Badge`, `badge_rect`, and `FileIcons` (document, folder and badge pixmaps by path, type and size) |

## How it works

### File icons

`document_icon_name` is the one place that maps a MIME type to a semantic name, on the shared MIME database of `cupertino-apps`
(mapping, after: the type's icon, the nearest ancestor's, then the generic document icon): `folder` for a
directory, then by the type or its nearest ancestor `document.pdf`, `document.image` / `.audio` / `.video` (by the media
prefix), `document.archive` (not for an office document packed in an archive: a type whose generic icon is an
`x-office-` one, such as a spreadsheet, falls through), `document.text`, `document.executable`, else `document.generic`. Scripts use `document.text`
because the payload has no `document.script` icon. `FileIcons` also holds the folders that wear an icon of their own (the home
folder and the user directories: `folder.home`, `folder.documents`, ...), which the file manager passes in from its places.
`FileIcons::pack_has(name)` says whether a pack ships its own file for an icon name. `FileIcons::with_system` takes the `SystemIcons` to draw with, so a test or a capture can use `SystemIcons::without_host`
and never read the icon themes of the machine.

Badges: the design's bottom-left slot shows the alias badge, else the lock badge: `alias.badge` for a
symbolic link, `locked.badge` for an item the user cannot write (counterpart). The other badges of the design
(caution, unsupported, file-provider states) have no file-system counterpart and are not drawn.
`badge_rect(size) places the badge with the recipe's rectangles (16 and 32 pt share the ratios 3/32, 6/32, 1/2;
512 pt has 44, 91, 256; values, pairing with the field); in between the ratios are interpolated
linearly (approximate).

### Template symbols

`symbol(name, px, color)` renders the symbol's SVG (black, tight view box) to fit `px` on its longer side and replaces the
colour, keeping the coverage; the caller chooses the colour and the size. A name the directory lacks gives `None`; a listed
file that cannot be decoded is reported on stderr.

### Application icons

`hicolor` is the base theme of the icon theme specification, where applications install their own icons, so it is the only
theme searched. A theme's stock icon names (`folder`, `audio-volume-high-symbolic`) find nothing there.

`cargo run -p cupertino-icons --example resolve -- <icon name>...` prints where this machine's `hicolor` resolves each name.

The `render-artwork` example takes an SVG path, an output PNG path and a pixel size. It uses `load_icon`, including
its native SVG filters and premultiplied pixel handling, so the first-party pack generator can bake exactly the
renditions the shell renders. Build it with `cargo build --locked -p cupertino-icons --example render-artwork`;
then run `python3 -B data/assets/cupertino/generate.py`. The generator leaves logical names and raster sizes unchanged.

## Open issues

- `volume.internal` / `volume.external` are not in the asset directory, so no code path draws a volume icon.
- Raster icons are scaled by the callers when the nearest asset size differs from the wanted one.

## Tests

`cargo test -p cupertino-icons` (an in-test asset directory; no environment needed).
