# cupertino-ui-menus

Publishes a CupertinoKit application's menus to the desktop menu bar over the session bus. MPL-2.0. Depends on
`cupertino-menu-model` and zbus; never reaches the private protocols (`cupertino-layers` denies `cupertino-protocols`
and `cupertino-shell-client`).

## Public API

| Item | Contents |
|---|---|
| `Exporter` | one per application and bus connection. `export(window, &MenuBar)` publishes `MenuBar::exported` and returns the window's `DbusProperties`; `set_enabled(window, action, enabled)`; `set_modifier_map` rewrites every accelerator for another profile; `remove(window)` withdraws a closed window's objects |
| `DbusProperties` | where a window's menus are, which it passes to `cupertino_app_menu_v1.set_address` |
| `Activation`, `WindowId` | a chosen item: the action, and the window for a `win.` action |

## How it works

The application object (`/org/example/Notes` for `org.example.Notes`) serves `org.gtk.Actions` with every `app.`
action of the open windows and, at `<app>/menus/appmenu`, `org.gtk.Menus` with the application menu (About,
Settings… and Quit when the application handles them; the menu bar adds Hide, Hide Others and Show All). Each window
serves its `win.` actions at `<app>/window/<n>` and its menu bar at `<app>/window/<n>/menus/menubar`. Every menu is in
group 0; sections and submenus are menus of their own linked with `:section` and `:submenu`. Labels carry no
mnemonic: each `_` goes out doubled. A change replaces each
menu that differs in one `Changed` record. Actions are stateless and take no parameter: `Activate` of a disabled one
does nothing, of an unknown one or with a parameter is `InvalidArgs`; `SetState` is `NotSupported`. Any session-bus
peer may activate an enabled action, as for every GTK application.

## Tests

```
cargo test -p cupertino-ui-menus
```

Unit tests cover the menu encoding, the change records and the object paths. `tests/bus.rs` starts a private
`dbus-daemon` (skipped when it is not installed) and reads the menus as the menu bar does: the published objects and
paths, activation, a disabled action, a profile change and a closed window. `cupertino-menubar`'s `tests/kit_export.rs`
reads an export through the bar's own `GtkMenuClient`.
