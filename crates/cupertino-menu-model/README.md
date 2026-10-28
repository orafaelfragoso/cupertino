# cupertino-menu-model

Typed application menus, shared by CupertinoKit, which exports them, and the menu bar, which shows them. MPL-2.0,
foundation layer, one dependency (`cupertino-modifiers`).

## Public API

| Item | Contents |
|---|---|
| (`cupertino-modifiers`) | the logical modifiers (`Modifier`, `Mods`), the physical keys (`Physical`) and the `ModifierMap` between them, supplied by the caller as the active modifier profile; a chord holds `Mods` and has no modifier type of its own |
| `Key`, `Chord` | a key by keysym name and a key with logical modifiers. `display(map)` is menu text (`Shift+Super+Q` under `cupertino`, `Shift+Ctrl+Q` under `classic-pc`); `to_accel(map)` / `from_accel(accel, map)` are the GTK accelerator of the physical keys (`<Shift><Super>q`); GTK's `<Primary>` is the Control key. A modifier no key carries gives `None` |
| `Scope`, `ActionName`, `Role` | `app.` and `win.` actions; the roles the menu bar rebinds, by action: `app.about`, `app.preferences` (also `app.settings`), `app.quit`, `win.close`, `win.fullscreen` |
| `Menu`, `Entry`, `Item`, `Command`, `SystemCommand`, `MenuBar` | the menu tree: groups of entries, items naming an action or a desktop command (Hide, Hide Others, Show All); `MenuBar::exported` drops the desktop commands and whatever they leave empty |
| `standard::{Features, menu_bar}` | the App, File, Edit and View menus of the menu bar spec, with the items the application handles; items it does not handle are omitted, not disabled. The Window and Help menus are the desktop's |

## Tests

```
cargo test -p cupertino-menu-model
```

Display and accelerator text of the spec's chords under both profiles, accelerator round trips, roles, the standard
menus with every and with no optional feature, and the exported subset.
