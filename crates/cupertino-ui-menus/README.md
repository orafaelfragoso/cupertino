# cupertino-ui-menus

Publishes a CupertinoKit application's menus to the desktop menu bar over the session bus. MPL-2.0. Depends on
`cupertino-menu-model` and zbus; never reaches the private protocols (`cupertino-layers` denies `cupertino-protocols`
and `cupertino-shell-client`).

## Public API

| Item | Contents |
|---|---|
| `Exporter` | one per application and bus connection. `new(builder, app_id, map, on_activate)` builds the connection from a `zbus::blocking::connection::Builder` (`Builder::session()` on the desktop) with the application object already served, so zbus listens for method calls before it reads the socket: a call made right after an address is announced is never dropped unanswered. `export(window, &MenuBar)` publishes `MenuBar::exported` and returns the window's `DbusProperties`; `set_enabled(window, action, enabled)`; `set_modifier_map` rewrites every accelerator for another profile; `remove(window)` withdraws a closed window's objects; `set_reader(name)` names the only peer answered |
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
does nothing, of an unknown one or with a parameter is `InvalidArgs`; `SetState` is `NotSupported`. Unlike a GTK
application's, the objects answer only the reader, the menu bar connection the compositor verified
(`cupertino_app_menu_v1.menu_reader`, passed to `Exporter::set_reader`): every `org.gtk.Menus` and `org.gtk.Actions`
method of any other peer, and of every peer while there is no reader, is `AccessDenied`, and `Changed` is sent to the
reader alone. Labels carry document names, so reads are refused as well as activation.

## Tests

```
cargo test -p cupertino-ui-menus
```

Unit tests cover the menu encoding, the change records and the object paths. `tests/bus.rs` starts a private
`dbus-daemon` (skipped when it is not installed; a child of the test, with a deadline on its address, every call and every
signal) and reads the menus as the menu bar does: the published objects and
paths, activation, a disabled action, a profile change, a closed window, and the reader: another peer and every peer
without a reader refused, a new reader taking the right over, signals the other peer does not receive. `cupertino-menubar`'s `tests/kit_export.rs`
reads an export through the bar's own `GtkMenuClient`.
