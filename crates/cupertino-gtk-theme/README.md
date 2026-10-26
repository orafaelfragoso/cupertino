# cupertino-gtk-theme

Generator (library and binary) that turns the `cupertino-theme` design tokens into toolkit theme files:

* GTK 3 and GTK 4 theme directories (light and dark),
* libadwaita overrides for `~/.config/gtk-4.0/{gtk,gtk-dark}.css`,
* a KDE colour scheme (`.colors`) and `qt5ct`/`qt6ct` palette schemes (no Kvantum theme),
* a fontconfig snippet that maps the system UI font to the substitute family (Inter),
* `settings.ini` files for font and theme name, the double-click time (500 ms) and distance (5 px) and the drag threshold (5 px) from `cupertino-input`.


* colour catalog storage; window radius 12 and menu selection radius 4.
* colours' sRGB interpretation; control heights from the control alignment table.
* approximate, as fields of `Metrics` / `Palette`: the token-to-role mapping, control radii (button/entry 6, popover 10), focus ring width (3 px), paddings.

## Usage

```
cargo run -p cupertino-gtk-theme -- [options]

  --output DIR        output directory (default: prototypes/tools/cupertino-gtk-theme/output/)
  --accent NAME       blue (default) purple pink red orange yellow green graphite
  --size SIZE         control size: regular (default), small, large
  --high-contrast     use the high-contrast catalogs
  --name NAME         theme name (default: Cupertino; letters, digits, - and _)
  --map-sans-serif    also map the generic sans-serif family to the UI font
  --spec-dir DIR      specification directory (default $CUPERTINO_SPEC_DIR or the repository's)
  --install           additionally copy the files into the user's XDG directories
  --force             with --install: replace differing files (old ones are kept as <name>.bak)
  --dry-run           with --install: print only
  --home DIR          with --install: use DIR instead of $HOME
  --list-accents
```

Nothing is written below the home directory without `--install`, which never overwrites a differing file without `--force`.

| Generated (relative to the output directory) | Installed to |
|---|---|
| `themes/Cupertino[-dark]/… | `$XDG_DATA_HOME/themes/` (`~/.local/share/themes/) |
| `config/gtk-4.0/{gtk,gtk-dark}.css`, `config/gtk-{3,4}.0/settings.ini` | `$XDG_CONFIG_HOME/` (`~/.config/) |
| `qt/color-schemes/Cupertino{Light,Dark}.colors` | `$XDG_DATA_HOME/color-schemes/ |
| `qt/qt{5,6}ct/colors/*.conf` | `$XDG_CONFIG_HOME/qt{5,6}ct/colors/ |
| `fontconfig/conf.d/60-cupertino-ui-font.conf` | `$XDG_CONFIG_HOME/fontconfig/conf.d/ |
| `qt/qt6ct/qt6ct.conf.snippet`, `README.txt` | not installed (settings to merge by hand) |

Library:

```rust
use cupertino_gtk_theme::{generate, write_files, Accent, GeneratorOptions};
use cupertino_gtk_theme::install::{install, InstallOptions, Targets};

let theme = cupertino_theme::Theme::load()?;
let files = generate(&theme, &GeneratorOptions { accent: Accent::Purple, ..Default::default() });
// Vec<GeneratedFile { path (relative), contents }>
write_files(&files, std::path::Path::new("out"))?;
// explicit targets only; the compositor/session setup decides when this runs
let targets = Targets::from_env(None).unwrap();
install(&files, &targets, InstallOptions::default())?;
```

Lower-level pieces: `Palette::from_theme(theme, dark, high_contrast, accent)`, `Metrics::from_theme`,
`css::{libadwaita_override, gtk4_theme, gtk3_theme}`, `qt::{kde_colors, qtct_scheme}`,
`fontconfig::snippet`. For live accent changes the compositor can call `generate` again and rewrite
only `config/gtk-4.0/gtk*.css`.

## Design

* libadwaita (1.10 here) derives its CSS variables from `@define-color` names, so the override defines every name the
  installed library defines (53 names including `accent_*`, `window_*`, `view_*`, `headerbar_*`, `card_*`, `popover_*`,
  `sidebar_*`, `secondary_sidebar_*`) plus `cupertino_*` helper colours. Accent selection changes `accent_bg_color`/`accent_color`
  and `theme_selected_bg_color`.
* The GTK 3 theme imports `resource:///org/gtk/libgtk/theme/Adwaita/gtk-contained[-dark].css` and
  overrides its `theme_*` names. The GTK 4 theme imports
  `resource:///org/gtk/libgtk/theme/Default/Default-{light,dark}.css`. Both resource paths exist in the
  installed GTK 3.24 and GTK 4.24 libraries.
* Rules: `button`, `entry`, `spinbutton`, `check`, `radio`, `switch`, `scale`, `scrollbar slider`,
  `headerbar`, window rounding (`decoration` / `window.csd`), popovers and menus, selection
  (focused and backdrop), links, tooltips, table headers, rows. The focus ring is a 3 px outline in the
  keyboard-focus token colour. `min-height` is the alignment height minus the 1 px borders.
* `themes/Cupertino/…/gtk-dark.css` is the dark appearance for applications that ask for dark with the light theme selected. `themes/Cupertino-dark` is dark in both files.

## Tests

`cargo test -p cupertino-gtk-theme` (41 tests) covers:

* CSS well-formedness with the crate's own tokenizer (`cssck`): balanced blocks, terminated strings and comments, `name: value;`, `@define-color`, `@import`, every `@colour` used is defined, every colour value parses;
* the libadwaita name list, colour values against the tokens for every accent and both appearances, control heights, radii and focus ring against the metrics, and the theme imports;
* the KDE scheme (parsed back, all keys of all sections present and opaque) and the qt5ct/qt6ct palettes (21 roles in `ColorRole` order, token-derived values, inactive/disabled variants);
* the fontconfig snippet, installation rules (no overwrite, `.bak`, dry run), the CLI (default output only, install into a fake home, error codes) and the absence of vendor names in all output.

Manual validation, done once: all generated GTK style sheets load in `Gtk.CssProvider` of GTK 3.24 and GTK 4.24 without a
parsing error (a deliberately broken sheet reports errors), and `kreadconfig6` reads the KDE scheme
(`Colors:Selection/BackgroundNormal` = `0,122,255`).

Visual results in running applications were not evaluated.
