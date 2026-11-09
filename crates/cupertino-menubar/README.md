# cupertino-menubar

The menu bar of the Cupertino desktop. It stands in for the design's system menu bar process (menus, status items, clock).
The crate has no Smithay dependency. The `cupertino-menubar` process draws the bars and popups from the layouts this crate
computes, and feeds the crate pointer, key and timer events with absolute timestamps in seconds.

## Contents

| Module | Contents |
|---|---|
| `layout` | `BarContent` → `BarLayout`: bold app-name menu, menus, status items nearest the control panel first, control panel slot, clock. Items that do not fit are reported hidden. `layout_popup` lays out rows, separators, check / radio column, icon column, shortcut text (the Mac modifier glyphs while Super is the command key, the PC key names such as `Alt+` and `F4`, `Ctrl+Super+` and `Z` while Ctrl is: `MenuMetrics::shortcut_style`, set from the server's primary modifier), submenu arrows, flipping, clipping and scrolling. `TextMeasure` abstracts text measurement; `MonoMeasure` is a deterministic stand-in. |
| `controller` | `MenuController`: press-drag-release, click-sticky menus, hover switching between open menus, focus-bar shortcut, arrow / Home / End / Enter / Esc keys, type-ahead, submenus with open / close delays and a safe triangle, disabled / checked / separator / shortcut items, `AboutToShow` and `Activate` effects. Releasing a press, the delays and the way into a submenu are `cupertino-menu-model`'s `tracking`, which Kit's menus share: a press on a title dragged onto a row chooses it on release; released on the title within 0.5 s the menu stays open; released there later, held or dragged out and back, it closes. The way into a submenu holds while each move stays in the triangle and the pointer does not rest 0.3 s (`tick` then gives the row under it the highlight). Type-ahead is `cupertino-typeahead` (a one-second pause restarts it, and it restarts in another menu level). |
| `clock` | The clock text (`clock_text`: weekday, day and short month, 12 / 24-hour time, seconds, day period, flashing separators) and its ticks (`next_tick`, `tick_interval`) from `ClockSettings`, which `cupertino_defaults::menubar` reads from and writes to `menubar.conf` (the `clock.* keys, bad lines reported with their numbers). |
| `extras` | The bar's own status items (Wi-Fi, battery, Bluetooth, Focus): which show, their icons, menus (`ExtraMenu`) and row actions (`ExtraAction`), from an `Extras` snapshot of the services; the system menu (`system_menu`, rows at fixed ids `SystemRow`) and its Recent Items submenu (`recent_items`). |
| `recent` | What Recent Items lists (`RecentItems`): `RecentSources::read` takes the launcher's applications in its order (`cupertino_apps::frecency`, ids without an installed visible desktop entry skipped) and the documents of `recently-used.xbel` newest first (`cupertino_apps::recent`, missing files skipped), at most 10 of each, leaving out what was used at or before the last Clear Menu (`RecentSources::clear`, the bar's own `$XDG_DATA_HOME/cupertino/menubar-recent-items-cleared`). It never writes the launcher's or the applications' files. |
| `item` | `MenuItem`: label, mnemonic, separator, enabled, visible, `Toggle`, `Shortcut`, icon name / data, children, the GMenuModel action and accelerator, and its `Origin` (the bar's or an exporter's), by which an activated item is routed, and a `detail`: secondary text drawn dimmed after the label (a search result's path). |
| `fallback` | Synthetic menus of an app that exports none: the app menu (About, Settings…, Hide, Hide Others, Show All, Quit; Settings… and Show All on the desktop), File (Close Window), Edit (Undo, Redo, Cut, Copy, Paste, Select All; enabled on a bar whose server takes `edit`, version 11, and disabled below), View (Exit Full Screen while the active window is in full screen, always enabled then; else Enter Full Screen, enabled only while the window can zoom, since the server refuses the person's full screen elsewhere; with a server before shell version 16, which never says full screen, Enter Full Screen stays enabled), Window (Minimize and Zoom, each disabled while the active window's is (`WindowState::can_minimize`, from shell version 15, and `can_zoom`), Move to Workspace, Tile Window, Bring All to Front, the window list of at most 1000 rows with the active window checked and minimized ones marked, Close Window; with Alt held Minimize All, Zoom All, Arrange in Front (a cascade) and Close All, see *Alt variants*) and Help (the search field, see *Help search*). Items of server actions show the chord the server binds (`Chords`), none while it sent none; the app's own convention chords use the primary modifier the server names. Ids are negative and stable; `action_for` maps activated ids back to `FallbackAction`. |
| `search` | The Help menu's search (`search`, `help_menu`): the enabled leaf items of the system menu, the App menu and the application's menus, Help included, whose label or submenu title holds the field's text, and the Help menu with them as rows (see *Help search*). |
| `merge` | `Export::merge`: an exporter's tree merged with the bar's App, Window and Help menus (roles, rebinding to the exporter's items, order, ids; see *Merged menus*); `Export::roles` for the routing, `label_role`, `accel_shortcut`. |
| `source` | `MenuSourceMap`: window → `MenuSource::{Kde, Gtk, Synthetic}`. A window gets only the menus it exported, never another window of its application. |
| `dbus::dbusmenu` | `DBusMenuClient`: `GetLayout` parsing (`parse_layout`), `Event`, `AboutToShow`, and `LayoutUpdated` / `ItemsPropertiesUpdated` / `ItemActivationRequested` as a `MenuUpdate` stream, property update helpers. |
| `dbus::sni` | `WatcherService` serving `org.kde.StatusNotifierWatcher` (the compositor is the host), `WatcherClient`, `fetch_item` (icons including ARGB pixmaps, status, tooltip, menu path), `load_menu` through DBusMenu, `activate_item` / `secondary_activate_item` / `context_menu_item` / `scroll_item`, `best_pixmap`. |
| `dbus::registrar` | The `com.canonical.AppMenu.Registrar` stub the worker serves after the watcher; the process asks it to own the name with the first bar and to release it with the last. |
| `dbus::gtk` | `GtkMenuClient`: an `org.gtk.Menus` + `org.gtk.Actions` reader (`Start`, sections flattened, `_` as the only mnemonic marker, namespaces, accelerators kept as text, enabled / toggle state from actions; the application menu read next to the menu bar and leading the tree under `GTK_APP_MENU`) and `Activate`. Both interfaces' `Changed` signals keep the cache current (`GtkMenuClient::changes`, subscribed before the load): menu records are spliced into the menus loaded (a record for any other menu is ignored, and a menu keeps at most `MAX_MENU_ITEMS` items, since any application can send them), and the actions' added, removed, enabled and state changes update their states. The worker reads the signals from the moment it subscribes, before the menus load, so a burst cannot fill the connection's signal queue and stop it; a change that comes while the menus load is not applied but loads them once more (twice at most). It applies each later change as it comes, one change of an exporter waiting at most, and sends the rebuilt tree at most every 50 ms. A tree is built from at most `MAX_TREE_ITEMS` entries, links included, and a link back to a menu being built builds nothing; an exporter keeps at most `MAX_ACTIONS` actions. Activation takes the item's action from the client and leaves the client with the worker. The worker loads such menus again when the exporter refuses with `AccessDenied` (100 ms, doubling to 1.6 s, then the refusal is reported): a CupertinoKit application answers only the bar's connection, which it learns over its own Wayland connection, possibly after the bar's first load. A load that fails is reported to the process (`Event::LayoutFailed`), which keeps the source's empty tree, so it does not ask in a loop, and asks again when a group announces the source again; the server sends the group again when it verifies the bar's connection. |

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
popups. It is a CupertinoKit application (`cupertino-kit`, `src/view.rs`): each bar is a Kit window whose layer surface,
menu-bar object and `MenuBar` backdrop come from `cupertino-shell-client` and `cupertino-protocol-app` through its surface
roles, and each open menu level is a Kit popup with a grab, on the `Menu` material, placed where the controller laid it out. The
server starts it from its `"shell"` configuration and restarts it after a failure; `cupertino-server`'s
`tests/shell_processes.rs` runs the real binary, including a menu popup opened, dismissed by a click outside and opened again,
and dismissed by a click on empty bar space, after which the keyboard is back on the window that had it.

**A click on empty bar space while a menu is open** closes the menu and does nothing else: no item is chosen and the press
reaches no client. The server takes that press as outside the menu's grab (only the popups belong to the grab, not the bar
they hang from), ends the grab, and the bar hears `popup_done` (`Input::Dismissed`); the keyboard goes to the key window,
not to the bar, which asks for it only while a title is focused by the shortcut. The controller does the same for a press it
receives on empty bar space (`Hit::Bar`). As with any outside press, a click on another title while a menu is open closes
the menu without opening the other one; hover and Left / Right switch menus.

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

- **Surfaces.** A top-anchored layer surface per output (exclusive zone 0). The server reserves the space and owns hiding,
  revealing and dimming. The bar draws nothing of its own: the wallpaper shows through it as it is, with no strip, fill or
  blur, and only its text and symbols follow what is behind them (*Backdrop* below). Increase Contrast keeps it
  see-through. Under Reduce Transparency it is a solid strip: the bar's `MenuBar` backdrop, which the server draws with
  the material's opaque recipe (grey 0.1255 dark, 0.8549 light), covers the bar then and has no area otherwise
  (`tone::solid_bar`). Each open menu level is an `xdg_popup` with a `Menu` backdrop. The process keeps its palette under
  the display options (no recorded contrast palette for the menu bar text and status items).
- **Menus.** The server's `cupertino_menu_bar_v1` feed names the active application and how its window exports menus:
  DBusMenu or GMenuModel. A window without menus gets the synthetic menus, whose actions go back as `window_command` /
  `move_to_workspace` / `activate_window`. From version 10 the feed also lists the application's windows and sends the
  chords the server binds and its primary modifier; the synthetic menus are rebuilt when they change. A version 9 server
  sends neither, so the system items show no chord, and the bar does not send the commands it lacks. From version 11 the
  Edit items send `edit` before their menu closes, and the server types the chord into the key window once the keyboard
  is back there; below version 11 they stay disabled. The `engage` event focuses the bar's first
  title; from version 12 `engage_help` opens the Help menu with its search field focused (see *Help search*). Arrows,
  Return and Escape navigate, and letters typed without Ctrl, Alt or the primary modifier select the first row they
  start (type-ahead; in the Help menu they go to its search field).
- **Alt variants.** While Alt is held, the open menus show the alternate of each of the bar's items that has one in its
  place: in the Window menu Minimize becomes Minimize All (`minimize_all`, enabled even when the active window cannot
  minimize), Zoom becomes Zoom All (`zoom_all`, enabled even when the active window cannot zoom), Close Window becomes Close All (`quit_app`, every window of the application)
  and Bring All to Front becomes Arrange in Front, which cascades the windows (`cascade_all`; a Cupertino label, as the
  design names none); in the system menu Force Quit… becomes Force Quit <app name> (see *System menu*). The bar holds the keyboard while its
  menus are open and reads Alt from the modifiers it was last sent, once per batch of events, so an Alt press and a
  click that arrive in the same batch choose what the menu showed before the swap; a menu opened with Alt held shows
  the alternates at once. A click or Return chooses the item the row shows at that moment, and the
  highlight stays on the row across a swap. A row is measured for the wider of its item and its alternate, so the menu
  keeps its size. Alternates have fixed ids of their own (-1025…-1028) and show no chord: the server binds none to
  minimize all, zoom all or cascade, and Close All runs the bound quit action, whose chord next to it would mislead. An exporter's items have no alternates.
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
- **Help search (`search`).** The Help menu's first row is a search field (`cupertino-ui-text`'s `TextBuffer`, the editing chords of
  `keyboard.conf`). It takes the typing keys while the Help menu is open, through the popup's keyboard grab: Space, and the
  arrows, Home and End while it holds text (with no text Left and Right switch menus as elsewhere). Up and Down move
  through the rows, Return or a click chooses the selected one, and Esc clears the text first, then closes the menu.
  - Matching: a label holds the text, case ignored, the text trimmed, or the title of a submenu on the item's way does
    ("Export" finds "File ▸ Export ▸ PDF"; the menu titles of the bar themselves do not count, so "File" does not list
    the whole File menu). Only enabled leaf items count: separators, disabled items (headers among them), submenus
    themselves and hidden items are never results. The items searched are the system menu's, the App menu's and every
    menu of the application as the bar shows them (merged), the Help menu's own items included (the application's and
    the bar's); only the bar's search row, No Results row and result rows are left out. An exporter's submenu that has
    not been loaded yet is not searched.
  - Results: labels that start with the text first, then labels that hold it, then items found by a submenu title
    alone, each group in menu order, at most 12 (`fallback::MAX_SEARCH_RESULTS`), as rows below the field, the first
    selected. A row shows the item's check or radio mark, the label, the path to the item ("File ▸ Export", the system
    menu as "System") dimmed after it, and the item's chord. The path is cut at its end with an ellipsis to the room
    left before the chord; the label is never cut (a long one widens the menu). With nothing found the field has a
    disabled "No Results" row below it. A menu with the field is at least `MenuMetrics::search_width` wide.
  - Ids: the field is -1030, No Results -1031, and result `n` is -1050 - `n` (`fallback::search_result`), by position,
    so the ids stay while typing. Choosing a result is choosing the item it found in its own menu: the controller
    sends `Activate` with that menu's slot and the item's id, and routing goes by the item's origin as for a click on
    it (an exporter's item goes to the exporter, the bar's acts through the server).
  - The empty field shows "Search" (`fallback::SEARCH_PLACEHOLDER`, the row's label) dimmed. The row shows the chord
    of the server's `help_search` binding (version 12) right of the field, none while the server sent none.
  - The Help search shortcut: on `engage_help` (version 12) the bar opens the Help menu with the keyboard in the field
    (`MenuController::focus_help_search`). It takes the keyboard through `engage`'s path: the Help title takes keyboard
    focus, the bar asks for exclusive keyboard interactivity, and the menu opens when the keyboard arrives
    (`keyboard_arrived`), its popup grab holding it from then on; there is no second keyboard-interactivity switch.
    With the keyboard on the bar already (title focus) or a menu open, Help opens at once; with Help open, its submenus
    close and the field keeps its text. Esc clears the field, then closes the menu and the keyboard goes back; a lock
    closes it as every menu. A version 11 server never sends the event, so the chord does nothing there.
  - Not yet: highlighting the found item in its real menu on hover. The bar has no clipboard, so copy, cut and paste in
    the field only change its text where they edit it. The caret does not blink, and a text wider than the field is not
    scrolled.
  - Known limit: dead keys and the Compose key do not compose in the field. The field gets the character of each key
    press as the bar's keymap translates it (`cupertino-wayland-client`), and the keyboard library of the project
    (`cupertino-xkbcommon`, pure Rust) has no compose support: a dead key types nothing and the next letter comes
    unaccented, so "é" cannot be typed through a dead key (a layout that has the character on a key of its own types
    it). There is no input method in the bar.
- **Session.** The labels follow the appearance the server announces over `cupertino_session_v1`, read at start and redrawn on
  every change. The solid strip follows it in the server. On `locked` the bars close every open menu and title focus.
- **Backdrop.** From shell version 14 the server sends each bar the lightness of its output's wallpaper under the strip
  (`backdrop_luminance`, CIE L*/100 in thousandths), and each bar picks its own tone (`tone::BarTone`): dark text at
  550 and above, light text at 450 and below, the last tone held in between. The first value, and the first after the
  appearance or the display options changed, is decided afresh, splitting the band at 500. What follows the tone: the
  labels, the application name, the clock, the system mark, the control panel glyph, the extras' symbols (drawn by the
  toolkit in the text colour of the bar's tone), the dot of a status item without
  an icon and the open title's highlight. What keeps the appearance: the menus and the screen capture indicator's
  indigo. Third-party status items keep their own icons. Before the first value, below version 14, and while Reduce
  Transparency makes the bar solid, the whole bar keeps the appearance's palette. The server measures the wallpaper's
  strip as it is, since nothing lies between it and the text, as the mean of each pixel's L* (a few bright lines over a dark strip leave it dark). Outside the band the chosen text is the one that reads
  better, at 4.5:1 or more below 400 and above 600; inside it neither colour reaches 4.5:1 (about 4.0:1 at 500). A
  strip's mean can hide light and dark parts side by side, so a slot over a busy part of the wallpaper keeps a soft halo
  of the opposite tone under its text, symbol or mark (`tone::halo`, `tone::Halo`). From shell version 21 the server
  also sends how unevenly light the wallpaper is under the strip, band by band across the output (`backdrop_spread`,
  the standard deviation of L*, from the same samples as the lightness); each slot takes the most of the bands it
  covers (`tone::spread_under`) and its halo follows it: none at all up to `CALM_SPREAD` (6 L*), so over a calm
  wallpaper nothing shows, then growing smoothly to full at `BUSY_SPREAD` (25 L*), black at 0.5625 under light text and
  white at 0.75 under dark text, its blur from 0.5 to 3 pt, over a dense core that hugs the strokes first (black 0.5,
  white 0.6, 0.75 pt, at the same share of full), so a thin glyph keeps a solid edge where stripes cross it. Before the
  first spread, and from an older server, every slot draws the halo at 0.8 of full without the core (black 0.45, white
  0.6, 2.5 pt), as all slots did before. The session's Reduce
  Transparency is the one source of both the solid strip and the halo (`tone::solid_bar`, `BarScene::solid`): a solid
  bar draws no halo, whatever the theme's own option says. A bar redraws when its tone or a slot's halo changes.
- **System menu.** The first slot draws the system mark (`cupertino_icons::system_mark`, tinted with the label colour) and
  opens `Extras::system_menu`: About This Computer, Settings…, Recent Items, Force Quit…, then Sleep, Restart…, Shut Down…, Lock Screen, Log
  Out <name>… and, when the session agent can switch, Login Window…, with separators between the groups. Every row and
  separator has a fixed id in -4000…-4099 (`SystemRow`), so an open menu survives a rebuild. About This Computer starts
  `cupertino-settings --about` and Settings… starts `cupertino-settings`, both from `PATH` with an activation token (`XDG_ACTIVATION_TOKEN`, `DESKTOP_STARTUP_ID`); Force Quit…
  sends the `force_quit_dialog` command, which a version 9 server does not get. With Alt held and an active application,
  Force Quit… shows its alternate Force Quit <app name> (-4013, `SystemRow::ForceQuitApp`), with the name the bold App menu
  title shows, no ellipsis and no chord; a name over 64 characters (`MAX_FORCE_QUIT_NAME_CHARS`, the cap of the other
  names in the system menu) is cut at its end with an ellipsis. It sends `force_quit_app` with the app id the label was
  built for, and the server ends that application only while it is still the active one. Unlike Force Quit…, it asks for
  no confirmation: choosing the item that names the application with Alt held is the confirmation. It is disabled below version 13
  (`MenuBar::can_force_quit_app`) and absent on the desktop. Force Quit…, Lock Screen and Log Out show
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
- **Session menu.** The system menu's session entries (`SessionAction`) are not D-Bus calls of the bar: the session agent answers only the server, so the bar sends `cupertino_menu_bar_v1.session_action` and the server calls the agent. The bar still reads the agent's `Locked`, `UserName` and `Capabilities` properties itself (an item whose capability is not reported is disabled; a read of the property that fails keeps the items as the previous read gave them, every one offered before any read, and the agent refuses what is not allowed), but only from the owner of `org.cupertino.Login1` that is the process the server names in `cupertino_menu_bar_v1.session_agent` (checked with `cupertino-peer`; any owner in a development run); a client that took the name first is not read, and until the server names a process nothing is. Without a session agent Lock Screen still works: the server locks without the agent while it has named a login process. A system menu that is open when the agent goes away closes.
- **Extras (`extras_bus`).** Wi-Fi, battery and Bluetooth read the system bus. The Focus item reads the do-not-disturb state
  and `FocusLifetimes` of `org.cupertino.NotificationCenter1`. The process reads them again after any signal on those
  objects, when a service's name changes owner, once a minute and after every action. A service that is not running, or has
  no device, hides its item. Any other failure hides it too and is reported on stderr once per reason. Without a system bus
  only the Focus item can show. The order next to the control panel is Wi-Fi, battery, Bluetooth, Focus, then the status
  notifier items. The task's queue is bounded (`QUEUE`, 16): any peer can send the process signals, so a refresh is queued
  only when none waits (the read a waiting refresh causes starts after the new signal, so it sees what it announced), the
  session agent is a value the next read takes and never queued or lost, and an action that finds the queue full (the task
  stuck on a service that does not answer) is dropped and reported on stderr instead of queued without bound.
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
- **approximate.** Bar height (24 pt), paddings, popup metrics, the backdrop thresholds (550 and 450, split at 500 when decided afresh), what the tone covers.

**Text.** The controller measures with the toolkit's text system (`view::KitMeasure`), the one the windows draw with, so the slots and rows it lays out hold the text the windows draw, at every output scale.

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
| `tests/controller.rs` | The controller's pointer and key behaviour; among it, a press on empty bar space closes a sticky or press-drag menu, is consumed and chooses nothing on the press or the release; pointer paths for a drag onto a row, a quick click and a hold on the title, a diagonal into a submenu across other rows, a rest and a slow path that give the submenu up, and type-ahead with Return and Escape. |
| `tests/dbus_integration.rs` | Fake DBusMenu, StatusNotifier and GMenuModel peers work against the clients and services, on a private bus from `cupertino-test-bus` (a `dbus-daemon` child that dies with its test). It skips with a message when `dbus-daemon` is not installed. |
| `src/tone.rs` tests | Outside the band every tone the bar can hold is the text colour that reads better, at 4.5:1 or more away from the band; the text is light over a dark blue wallpaper and dark over a pale one; only Reduce Transparency makes the bar solid and keeps the appearance's text, Increase Contrast leaves it adapting. The halo's strength is none up to the calm spread (and under 2 % of full just past it), grows smoothly and never falls to full at the busy spread, its blur and its core growing with it; a solid bar has none; with no measure it is the halo slots drew before, without a core; a slot takes the most spread of the bands it covers. |
| `tests/app-menus.rs` (feature `app`) | The menus Files and Settings publish with `cupertino-ui-menus`, read by the bar's worker over a private bus, open from their title and choose an item: the application hears that item's action. It builds the menus from `cupertino_files::menu_bar` and `cupertino_settings::menu_bar` themselves, so it needs these two apps-layer crates as dev-dependencies, which are the layer graph's dev-only exceptions for this crate. |
| `tests/bar-view.rs` (feature `app`) | The bar, the bar with the File menu open, and the open menu as CupertinoKit draws them match the goldens in `tests/goldens`: the frame at 1x (x86_64 only, within 2/255 a channel) and its focus and accessibility tree. A press on a title opens its menu as a popup of the bar, and a click on a row chooses the row's item; a screen reader does both through the titles' and rows' Click action. Every settled frame passes `cupertino-ui-test`'s accessibility audit: each menu title and row says its place and its bar or menu its count (separators and the Help search field not counted). The session's Reduce Transparency gives the solid strip and no halo over the busiest wallpaper, the theme's own option taking none away; over a calm wallpaper no slot draws a halo, and over one busy under the bar's left eighth only the slots there do. |
| `src/extras_bus.rs` tests | 10 000 refreshes in a burst wait as one; a queue filled with actions stays at its bound, a refresh then is not needed, the newest session agent is kept, and a task that is gone is reported. |
| `tests/extras_bus.rs` (feature `app`) | Stand-ins for the network, power, Bluetooth and notification services, served on one bus used as both system and session bus. Checks what the extras read, that joining an open network calls `Connect` without a passphrase and is read back, and that a service leaving the bus hides its item. |
