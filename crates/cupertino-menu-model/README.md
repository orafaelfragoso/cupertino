# cupertino-menu-model

Typed application menus, shared by CupertinoKit, which exports them, and the menu bar, which shows them. MPL-2.0,
foundation layer, one dependency (`cupertino-modifiers`). It also holds the pointer tracking every menu shares.

## Public API

| Item | Contents |
|---|---|
| (`cupertino-modifiers`) | the logical modifiers (`Modifier`, `Mods`), the physical keys (`Physical`) and the `ModifierMap` between them, supplied by the caller as the active modifier profile; a chord holds `Mods` and has no modifier type of its own |
| `Key`, `Chord` | a key by keysym name and a key with logical modifiers. `display(map)` is menu text (`Shift+Super+Q` under `cupertino`, `Shift+Ctrl+Q` under `classic-pc`); `to_accel(map)` / `from_accel(accel, map)` are the GTK accelerator of the physical keys (`<Shift><Super>q`); GTK's `<Primary>` is the Control key. A modifier no key carries gives `None` |
| `Scope`, `ActionName`, `Role` | `app.` and `win.` actions; the roles the menu bar rebinds, by action: `app.about`, `app.preferences` (also `app.settings`), `app.quit`, `win.close`, `win.fullscreen` |
| `Menu`, `Entry`, `Item`, `Command`, `SystemCommand`, `MenuBar` | the menu tree: groups of entries, items naming an action or a desktop command (Hide, Hide Others, Show All); `MenuBar::exported` drops the desktop commands and whatever they leave empty |
| `tracking::{Tracking, Under, Release}` | what the release of a press does: a press on a title dragged onto a row chooses it on release; a click on the title released there within `TRACKING_INTERVAL` (0.5 s) leaves the menu open, and released there later closes it; a menu opened under the pointer (a pop-up button's) chooses the row there only after the interval. A press inside an open menu chooses where it is let go |
| `tracking::{SubmenuAim, Edge, point_in_triangle}` | whether the pointer heads from a row into the submenu it opened: each move inside the triangle between the last point and the submenu's near edge keeps the submenu and the highlight; a move outside it, or a rest of `SUBMENU_AIM_PAUSE` (0.375 s), gives the submenu up to the row under the pointer, and the submenu closes at once. A submenu opens `SUBMENU_OPEN_DELAY` (0.35 s) after its row lights. Used by the menu bar and CupertinoKit's menus, so the file manager's context menus track the same way |
| `standard::{Features, menu_bar}` | the App, File, Edit and View menus of the menu bar spec, with the items the application handles; items it does not handle are omitted, not disabled. The Window and Help menus are the desktop's |

## Tests

```
cargo test -p cupertino-menu-model
```

Display and accelerator text of the spec's chords under both profiles, accelerator round trips, roles, the standard
menus with every and with no optional feature, and the exported subset; release decisions, aim along a diagonal, a
rest and a slow path, and the triangle.
