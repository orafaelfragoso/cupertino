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
| `extras` | The bar's own status items (Wi-Fi, battery, Bluetooth, Focus): which show, their icons, menus (`ExtraMenu`) and row actions (`ExtraAction`), from an `Extras` snapshot of the services. |
| `item` | `MenuItem`: label, mnemonic, separator, enabled, visible, `Toggle`, `Shortcut`, icon name / data, children. |
| `fallback` | Synthetic app menu (About, Hide, Hide Others, Show All, Quit) and Window menu (Minimize, Zoom, Move to Workspace, Close Window). `action_for` maps activated ids back to `FallbackAction`. |
| `source` | `MenuSourceMap`: window / app identity → `MenuSource::{Kde, Gtk, Synthetic}, service-vanished cleanup. |
| `dbus::dbusmenu` | `DBusMenuClient`: `GetLayout` parsing (`parse_layout`), `Event`, `AboutToShow`, and `LayoutUpdated` / `ItemsPropertiesUpdated` / `ItemActivationRequested` as a `MenuUpdate` stream, property update helpers. |
| `dbus::sni` | `WatcherService` serving `org.kde.StatusNotifierWatcher` (the compositor is the host), `WatcherClient`, `fetch_item` (icons including ARGB pixmaps, status, tooltip, menu path), `load_menu` through DBusMenu, `activate_item` / `secondary_activate_item` / `context_menu_item` / `scroll_item`, `best_pixmap`. |
| `dbus::gtk` | `GtkMenuClient`: an `org.gtk.Menus` + `org.gtk.Actions` reader (`Start`, sections flattened, namespaces, accelerators, enabled / toggle state from actions) and `Activate`. |

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
  DBusMenu or GMenuModel. A window without menus gets the synthetic app and
  Window menus, whose actions go back as `window_command` / `move_to_workspace`. The `engage` event focuses the bar's first
  title. Arrows, Return and Escape navigate. Type-ahead needs a keymap and is not available.
- **Session.** The labels follow the appearance the server announces over `cupertino_session_v1`, read at start and redrawn on
  every change. The material follows it in the server. On `locked` the bars close every open menu and title focus.
- **Session menu.** The system menu's entries (`SessionAction`) are not D-Bus calls of the bar: the session agent answers only the server, so the bar sends `cupertino_menu_bar_v1.session_action` and the server calls the agent. The bar still reads the agent's `Locked`, `UserName` and `Capabilities` properties itself, but only from the owner of `org.cupertino.Login1` that is the process the server names in `cupertino_menu_bar_v1.session_agent` (checked with `cupertino-peer`; any owner in a development run); a client that took the name first is not read, and until the server names a process nothing is. Lock Screen is always in the system menu: without a session agent it is the only entry, and the server locks without the agent.
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
