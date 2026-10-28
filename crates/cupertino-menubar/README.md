# cupertino-menubar

The menu bar of the Cupertino desktop. It stands in for the design's system menu bar process (menus, status items, clock).
The crate has no Smithay dependency. The `cupertino-menubar` process draws the bars and popups from the layouts this crate
computes, and feeds the crate pointer, key and timer events with absolute timestamps in seconds.

## Contents

| Module | Contents |
|---|---|
| `layout` | `BarContent` → `BarLayout`: bold app-name menu, menus, status items nearest the control panel first, control panel slot, clock. Items that do not fit are reported hidden. `layout_popup` lays out rows, separators, check / radio column, icon column, shortcut text, submenu arrows, flipping, clipping and scrolling. `TextMeasure` abstracts text measurement; `MonoMeasure` is a deterministic stand-in. |
| `controller` | `MenuController`: press-drag-release, click-sticky menus, hover switching between open menus, focus-bar shortcut, arrow / Home / End / Enter / Esc keys, type-ahead, submenus with open / close delays and a safe triangle, disabled / checked / separator / shortcut items, `AboutToShow` and `Activate` effects. |
| `clock` | The clock text (`clock_text`: weekday, day and short month, 12 / 24-hour time, seconds, day period, flashing separators) and its ticks (`next_tick`, `tick_interval`) from `ClockSettings`, which `cupertino_defaults::menubar` reads from and writes to `menubar.conf` (the `clock.* keys, bad lines reported with their numbers). |
| `extras` | The bar's own status items (Wi-Fi, battery, Bluetooth, Focus): which show, their icons, menus (`ExtraMenu`) and row actions (`ExtraAction`), from an `Extras` snapshot of the services; the system menu (`system_menu`, rows at fixed ids `SystemRow`) and its Recent Items submenu (`recent_items`). |
| `recent` | What Recent Items lists (`RecentItems`): `RecentSources::read` takes the launcher's applications in its order (`cupertino_apps::frecency`, ids without an installed visible desktop entry skipped) and the documents of `recently-used.xbel` newest first (`cupertino_apps::recent`, missing files skipped), at most 10 of each, leaving out what was used at or before the last Clear Menu (`RecentSources::clear`, the bar's own `$XDG_DATA_HOME/cupertino/menubar-recent-items-cleared`). It never writes the launcher's or the applications' files. |
| `item` | `MenuItem`: label, mnemonic, separator, enabled, visible, `Toggle`, `Shortcut`, icon name / data, children, the GMenuModel action and accelerator, and its `Origin` (the bar's or an exporter's), by which an activated item is routed, and a `detail`: secondary text drawn dimmed after the label (a search result's path). |
| `fallback` | Synthetic menus of an app that exports none: the app menu (About, Settings…, Hide, Hide Others, Show All, Quit; Settings… and Show All on the desktop), File (Close Window), Edit (Undo, Redo, Cut, Copy, Paste, Select All; enabled on a bar whose server takes `edit`, version 11, and disabled below), View (Enter Full Screen), Window (Minimize, Zoom, Move to Workspace, Tile Window, Bring All to Front, the window list of at most 1000 rows with the active window checked and minimized ones marked, Close Window) and Help (the search field, see *Help search*). Items of server actions show the chord the server binds (`Chords`), none while it sent none; the app's own convention chords use the primary modifier the server names. Ids are negative and stable; `action_for` maps activated ids back to `FallbackAction`. |
| `search` | The Help menu's search (`search`, `help_menu`): the enabled leaf items of the system menu, the App menu and the application's menus whose label holds the field's text, and the Help menu with them as rows (see *Help search*). |
| `merge` | `Export::merge`: an exporter's tree merged with the bar's App, Window and Help menus (roles, rebinding to the exporter's items, order, ids; see *Merged menus*); `Export::roles` for the routing, `label_role`, `accel_shortcut`. |
| `source` | `MenuSourceMap`: window → `MenuSource::{Kde, Gtk, Synthetic}`. A window gets only the menus it exported, never another window of its application. |
| `dbus::dbusmenu` | `DBusMenuClient`: `GetLayout` parsing (`parse_layout`), `Event`, `AboutToShow`, and `LayoutUpdated` / `ItemsPropertiesUpdated` / `ItemActivationRequested` as a `MenuUpdate` stream, property update helpers. |
| `dbus::sni` | `WatcherService` serving `org.kde.StatusNotifierWatcher` (the compositor is the host), `WatcherClient`, `fetch_item` (icons including ARGB pixmaps, status, tooltip, menu path), `load_menu` through DBusMenu, `activate_item` / `secondary_activate_item` / `context_menu_item` / `scroll_item`, `best_pixmap`. |
| `dbus::registrar` | The `com.canonical.AppMenu.Registrar` stub the worker serves after the watcher; the process asks it to own the name with the first bar and to release it with the last. |
| `dbus::gtk` | `GtkMenuClient`: an `org.gtk.Menus` + `org.gtk.Actions` reader (`Start`, sections flattened, `_` as the only mnemonic marker, namespaces, accelerators kept as text, enabled / toggle state from actions; the application menu read next to the menu bar and leading the tree under `GTK_APP_MENU`) and `Activate`. The worker loads such menus again when the exporter refuses with `AccessDenied` (100 ms, doubling to 1.6 s, then the refusal is reported): a CupertinoKit application answers only the bar's connection, which it learns over its own Wayland connection, possibly after the bar's first load. A load that fails is reported to the process (`Event::LayoutFailed`), which keeps the source's empty tree, so it does not ask in a loop, and asks again when a group announces the source again; the server sends the group again when it verifies the bar's connection. |

Feature `dbus` (default) enables the zbus-based layer. Without it the crate depends only on `cupertino-animation`. The D-Bus
layer is runtime agnostic (zbus with its `async-io` backend). Drive the futures with any executor or `zbus::block_on`.

```rust
use cupertino_menubar::controller::{Effect, MenuController};
use cupertino_menubar::layout::{BarContent, MonoMeasure};

let mut c = MenuController::new(output_rect, Box::new(MonoMeasure::default()));
c.set_content(BarContent { app_name: "Files".into(), menus, clock: "10:42".into(), ..Default::default() });
let out = c.pointer_press(pointer, now);
for effect in out.effects {
    match effect {
        Effect::AboutToShow { item, .. } => { /* dbusmenu AboutToShow, then set_content */ }
        Effect::Activate { item, .. } => { /* dbusmenu Event "clicked" */ }
        _ => {}
    }
}
```

## Running

Feature `app` builds the binary: a client of the display server that shows one menu bar per output and opens its menus as
popups. It runs on `cupertino-wayland-client` (popups with a grab), `cupertino-shell-client` (layer surfaces and the menu-bar
role) and `cupertino-protocol-app` (backdrops), with a `cupertino-ui-wayland` presenter per bar and per open menu level. The
server starts it from its `"shell"` configuration and restarts it after a failure; `cupertino-server`'s
`tests/shell_processes.rs` runs the real binary, including a menu popup opened, dismissed by a click outside and opened again.

```sh
cargo run -p cupertino-menubar --features app
```

| Setting | Effect |
|---|---|
| `$WAYLAND_DISPLAY` | The display to connect to. |
| `$DBUS_SESSION_BUS_ADDRESS` | Must be set. |
| `menubar.conf` in the configuration directory | The clock options. `CUPERTINO_MENUBAR_CONFIG` overrides the path. A missing file leaves every key unset. The bar watches the file and applies a change at once (the clock is redrawn and its tick rescheduled); a bad line is reported and keeps its last good value. |
| `CUPERTINO_ASSET_DIR` | The first asset pack (`cupertino-assets` resolves the others): the bar text's `ui` font and the symbols of the status items (`extras::SYMBOL_NAMES`). A symbol no pack has is reported at startup and its item is drawn without it. |

`menubar.conf` keys, with what an unset key reads as (the design's read rules):

| Key | Values | Default |
|---|---|---|
| `clock.show_seconds` | `on`, `off` | `off` |
| `clock.show_day_of_week` | `on`, `off` | `off` |
| `clock.show_date` | `when-space-allows`, `always`, `never` | `never` |
| `clock.show_am_pm` | `on`, `off` (12-hour cycle only) | `on` |
| `clock.flash_separators` | `on`, `off` | `off` |
| `clock.hour_cycle` | `12`, `24` | `12` |

The clock is redrawn 0.1 s after each minute, or each second with seconds or flashing separators.

Startup fails with a clear message when the server does not advertise `zwlr_layer_shell_v1`, `xdg_wm_base` (version 3 or
newer), `zxdg_output_manager_v1`, `cupertino_shell_v1` (backdrops and the menu-bar role, `cupertino-protocols`),
`xdg_activation_v1`, `wl_shm` or a seat. It also fails when another process owns
`org.kde.StatusNotifierWatcher` on the session bus.

## How it works

- **Surfaces.** A top-anchored layer surface per output (exclusive zone 0) with a `MenuBar` backdrop. The server reserves the
  space and owns hiding, revealing and dimming. Each open menu level is an `xdg_popup` with a `Menu` backdrop. The server
  draws the strip's material, so Reduce Transparency and Increase Contrast reach it there. The process keeps its palette
  under the display options (no recorded contrast palette for the menu bar text and status items).
- **Menus.** The server's `cupertino_menu_bar_v1` feed names the active application and how its window exports menus:
  DBusMenu or GMenuModel. A window without menus gets the synthetic menus, whose actions go back as `window_command` /
  `move_to_workspace` / `activate_window`. From version 10 the feed also lists the application's windows and sends the
  chords the server binds and its primary modifier; the synthetic menus are rebuilt when they change. A version 9 server
  sends neither, so the system items show no chord, and the bar does not send the commands it lacks. From version 11 the
  Edit items send `edit` before their menu closes, and the server types the chord into the key window once the keyboard
  is back there; below version 11 they stay disabled. The `engage` event focuses the bar's first
  title. Arrows, Return and Escape navigate. Type-ahead needs a keymap and is not available.
- **Merged menus (`merge`).** An exporter's tree is merged with the bar's own menus once it has arrived; nothing shows
  before. The server reports one address per window, the GMenuModel one when the window announced both.
  - Roles: about, preferences, quit, close and full screen. A GMenuModel item has the role of its action (`app.about`;
    `app.preferences` or `app.settings`; `app.quit`; `win.close`; `win.fullscreen`). A DBusMenu item has the role of its
    label, compared without mnemonic markers, a trailing "…" or "..." and case: about starts with "About"; quit is
    "Quit", "Quit" and anything, or "Exit"; preferences is "Preferences", "Settings", "Options" or "Configure <name>";
    close is "Close" or "Close Window"; full screen is "Full Screen" or "Enter Full Screen". English only; any other
    label stays as exported. `<name>` is the desktop entry's `Name` or the last component of the application id.
  - App menu: always the bar's. About, Settings… and Quit invoke the exporter's first item of their role (looked for in
    the application's own menu, File, Edit, then the other menus); without one About shows the About panel, Settings…
    is disabled and Quit closes every window. The exporter's about, preferences and quit items leave the application's
    own menu, File and Edit, and the separators left leading, trailing or doubled go; a menu the removal emptied goes.
    The rest of the application's own menu goes between About and Settings…, after a separator: the GMenuModel
    application menu (`app_menu_object_path`), or the top-level DBusMenu menu titled (any case) with `<name>`.
  - File, Edit and any other menu show as exported. View shows as exported, with a separator and Enter Full Screen
    appended when it has no full screen item and is loaded. Window is the bar's Window menu with the exporter's items
    that are no duplicates (by role close, or by label Minimize, Zoom, Maximize, Close, Close Window, Bring All to
    Front, Move to Workspace, Move to Desktop) below a separator at its end. Help is the bar's, with the exporter's items
    below the search row and a separator. Menus the exporter lacks are not added, except Window and Help. The order is
    File, Edit, View, the others, Window, Help; an application that exports only a GMenuModel application menu gets
    Window and Help.
  - Ids: the bar's items keep their fixed ids, all at most -1000 (-1107…-1110 for the separators the merge adds);
    the exporter's keep theirs, so a layout update leaves an open menu in place. Each item carries its origin
    (`item::Origin`, set to the exporter by the DBusMenu and GMenuModel readers), and an activated item is routed by the
    origin of the item shown in the menu that was open, never by its id: the bar's own items act through the server
    (rebound About, Settings… and Quit activate the exporter's item), the exporter's go to the exporter. The DBusMenu
    reader also leaves out an exporter's item whose id is in the bar's range (at most -1000), so ids never repeat in
    one menu; the GMenuModel reader numbers its items from 1. The bar's
    Window and Help menus send the exporter no `AboutToShow`, so items an exporter fills in lazily there do not show.
  - Accelerators: a GMenuModel item's `accel` names the physical keys the application receives; it is read with
    `cupertino_menu_model::Chord::from_accel` and the modifier map of the server's profile (the primary modifier of the
    shortcut group). The bar shows the keys the person presses. For a third-party application (an id that does not
    start with `cupertino-`, the server's rule) the server makes Ctrl chords of Super chords, so the bar shows the
    chord `cupertino_keyboard::rewrite::pressed_for` gives: under `cupertino`, `<Primary>q` shows as Super+Q (the
    command glyph), `<Primary>F5` as Ctrl+F5 (function keys, Escape, Tab, Space, Delete and the arrows are not
    rewritten); under `classic-pc` and for `cupertino-*` applications the accelerator shows as it is (`<Primary>q` is
    Ctrl+Q, `<Super>q` Super+Q). Accepted gaps: the bar does not know `rewrite_off`, so an application opted out of the
    rewrite shows Super+Q where only Ctrl+Q works; and it does not know `rewrite_terminals`, so a terminal is shown as
    an ordinary application.
- **Help search (`search`).** The Help menu's first row is a search field (`cupertino-text-field`, the editing chords of
  `keyboard.conf`). It takes the typing keys while the Help menu is open, through the popup's keyboard grab: Space, and the
  arrows, Home and End while it holds text (with no text Left and Right switch menus as elsewhere). Up and Down move
  through the rows, Return or a click chooses the selected one, and Esc clears the text first, then closes the menu.
  - Matching: a label holds the text, case ignored, the text trimmed. Only enabled leaf items count: separators,
    disabled items (headers among them), submenus themselves and hidden items are never results. The items searched are
    the system menu's, the App menu's and the application's menus as the bar shows them (merged), never the Help menu's;
    an exporter's submenu that has not been loaded yet is not searched.
  - Results: labels that start with the text first, then menu order, at most 12 (`fallback::MAX_SEARCH_RESULTS`), as rows
    below the field, the first selected. A row shows the label, the path to the item ("File ▸ Export", the system
    menu as "System") dimmed after it, and the item's chord. The path is cut at its end with an ellipsis to the room
    left before the chord; the label is never cut (a long one widens the menu). With nothing found the field has a
    disabled "No Results" row below it. A menu with the field is at least `MenuMetrics::search_width` wide.
  - Ids: the field is -1030, No Results -1031, and result `n` is -1050 - `n` (`fallback::search_result`), by position,
    so the ids stay while typing. Choosing a result is choosing the item it found in its own menu: the controller
    sends `Activate` with that menu's slot and the item's id, and routing goes by the item's origin as for a click on
    it (an exporter's item goes to the exporter, the bar's acts through the server).
  - Not yet: highlighting the found item in its real menu on hover, and a chord that focuses the field. The bar has no
    clipboard, so copy, cut and paste in the field only change its text where they edit it. The caret does not blink,
    and a text wider than the field is not scrolled.
- **Session.** The labels follow the appearance the server announces over `cupertino_session_v1`, read at start and redrawn on
  every change. The material follows it in the server. On `locked` the bars close every open menu and title focus.
- **System menu.** The first slot draws the system mark (`cupertino_icons::system_mark`, tinted with the label colour) and
  opens `Extras::system_menu`: About This Computer, Settings…, Recent Items, Force Quit…, then Sleep, Restart…, Shut Down…, Lock Screen, Log
  Out <name>… and, when the session agent can switch, Login Window…, with separators between the groups. Every row and
  separator has a fixed id in -4000…-4099 (`SystemRow`), so an open menu survives a rebuild. About This Computer starts
  `cupertino-settings --about` and Settings… starts `cupertino-settings`, both from `PATH` with an activation token (`XDG_ACTIVATION_TOKEN`, `DESKTOP_STARTUP_ID`); Force Quit…
  sends the `force_quit_dialog` command, which a version 9 server does not get. Force Quit…, Lock Screen and Log Out show
  the chord the server binds to them, none while it sent none. Without a session agent the menu is About This Computer, Settings…, Recent
  Items, Force Quit… and Lock Screen.
- **Recent Items.** The submenu is read (`recent` module) each time the system menu opens, never on a timer: a disabled
  Applications header and up to 10 applications by desktop entry name, a disabled Documents header and up to 10 documents by
  file name, a separator and Clear Menu. A section without rows keeps its header; with nothing to list the submenu is a
  disabled Clear Menu alone. Ids: the submenu -4009, the headers -4010 and -4011, Clear Menu -4012, its separators -4055
  and -4056, the application rows -4060…-4069 and the document rows -4070…-4079 by position; a click is resolved against
  the rows read when the menu opened. An application starts from its desktop entry with no files; a document opens with
  the default application of its type (`mimeapps.list`), its file in the `%f %F %u %U` codes; both get an activation
  token, through `cupertino_apps::launch` like Settings… and the dock. A document no installed application opens goes to
  `xdg-open` without a token, as the dock's folders do. An entry with `Terminal=true` is not started (the bar has no
  terminal). Clear Menu stores the time and hides what was used until then; problems are reported on standard error.
- **Session menu.** The system menu's session entries (`SessionAction`) are not D-Bus calls of the bar: the session agent answers only the server, so the bar sends `cupertino_menu_bar_v1.session_action` and the server calls the agent. The bar still reads the agent's `Locked`, `UserName` and `Capabilities` properties itself, but only from the owner of `org.cupertino.Login1` that is the process the server names in `cupertino_menu_bar_v1.session_agent` (checked with `cupertino-peer`; any owner in a development run); a client that took the name first is not read, and until the server names a process nothing is. Without a session agent Lock Screen still works: the server locks without the agent while it has named a login process.
- **Extras (`extras_bus`).** Wi-Fi, battery and Bluetooth read the system bus. The Focus item reads the do-not-disturb state
  and `FocusLifetimes` of `org.cupertino.NotificationCenter1`. The process reads them again after any signal on those
  objects, when a service's name changes owner, once a minute and after every action. A service that is not running, or has
  no device, hides its item. Any other failure hides it too and is reported on stderr once per reason. Without a system bus
  only the Focus item can show. The order next to the control panel is Wi-Fi, battery, Bluetooth, Focus, then the status
  notifier items.
  - Wi-Fi: a `Wi-Fi` switch row, then "Known Network(s)" and "Other Network(s)", each by name. A saved, open or OWE network is
    joined without a passphrase. The bar has no passphrase prompt, so other networks are listed disabled.
  - Battery: "Battery N%", "Power Source: Battery / Power Adapter", and "Charging" while charging (rows approximate).
  - Bluetooth: a `Bluetooth` switch row, then the paired devices by name, checked when connected. A row connects or
    disconnects.
  - Focus: shown only while do not disturb is on. A `Do Not Disturb` row turns it off, and the duration rows re-arm it.
- **Screen capture indicator.** While the server reports a capture session (`screen_capture`, menu bar version 3), a 6 pt
  indigo dot (size and colours) sits nearest the control panel. The enter / exit animation and
  minimum time are not played. Camera, microphone and system-audio indicators need the PipeWire graph and
  are not shown.

## Protocols / D-Bus it speaks

| Name | Role |
|---|---|
| `org.cupertino.NotificationCenter1` | Session bus. `ToggleCentre` on a click on the clock. Do-not-disturb state and `FocusLifetimes` for the Focus item. |
| `org.cupertino.ControlPanel1` | Session bus. `Toggle` on a click on the control panel slot. |
| `org.kde.StatusNotifierWatcher` | Owned by the process, which is the status item host. |
| `com.canonical.AppMenu.Registrar` | Owned by the process while it shows a bar and no other process owns the name; released with the last bar. Registers nothing. Qt, KDE Frameworks, Chromium and LibreOffice export their menus and hide their in-window menu bar only while the name has an owner. |
| `org.cupertino.Network1` | System bus. `Devices`, `WirelessEnabled`, `GetNetworks`, `Connect`. |
| UPower | System bus. The display device and `OnBattery`. |
| `org.cupertino.Bluetooth1` | System bus. `Available`, `Powered`, `Devices`, `Connect`, `Disconnect`. |


- **.** Behaviour of the design: bar structure, press-drag-release and
  click-sticky interaction, hover switching, keyboard navigation, type-ahead. The per-output presentation (dimmed inactive
  output, auto-hide in full screen) lives in `cupertino-wm::menu_bar`.
- **.** The selection highlight is a 4 pt continuous-corner rounded rectangle
  (`BarMetrics::selection_corner_radius`).
- **Clock.** The parts, the two-space join with a date and one space without, the unset-key reads, the
  1 s / 60 s tick 0.1 s after the boundary, the `remainder(t, 2) > 0` flash phase and its punctuation space are as designed. The
  English patterns stand in for the locale's skeleton step, and the hour cycle is a setting (/ approximate: no
  locale data). `when-space-allows` always shows the date; the compact fallback is open. The battery and Bluetooth rows (open), the symbol choice (one `symbol.wifi` / `symbol.battery` / `symbol.bluetooth` / `symbol.moon` each, drawn in the bar's text
  colour, dimmed to 0.4 when the radio is off or has no link; no signal-strength or charge-level variants) are approximate.
  Third-party status items keep their own icons (ARGB pixmap, absolute path or `hicolor`).
- **approximate.** Bar height (24 pt), paddings, popup metrics, submenu open / close delays, the safe triangle, the
  type-ahead timeout.

**Text.** The `ui` font is a variable font whose optical size follows the point size, so every `TextRenderer` owner calls `set_scale` with the output scale before it draws and measures (`BarText` sets it from the output's scale on every measure and draw): text is laid out in points at the same optical size it is drawn with, at every output scale.

## Open issues

Alt-click on the clock toggles do not disturb in the design. The bar has no reliable modifier state for a click:
the server sends keyboard modifiers only to the focused surface, and the bar holds the keyboard only while a title is
focused. A click on the clock therefore always opens the notification centre.

## Tests

```
cargo test -p cupertino-menubar            # unit, state machine and D-Bus tests
cargo clippy -p cupertino-menubar --all-targets
```

| Test | Proves |
|---|---|
| `tests/dbus_integration.rs` | Fake DBusMenu, StatusNotifier and GMenuModel peers work against the clients and services, on a private bus started with `dbus-daemon --session --print-address=1 --print-pid=1 --fork`. It skips with a message when `dbus-daemon` is not installed. |
| `tests/extras_bus.rs` (feature `app`) | Stand-ins for the network, power, Bluetooth and notification services, served on one bus used as both system and session bus. Checks what the extras read, that joining an open network calls `Connect` without a passphrase and is read back, and that a service leaving the bus hides its item. |
