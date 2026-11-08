# cupertino-controlcenter

The control panel of the Cupertino desktop. It stands in for the design's control panel process: a 298 pt card that opens at
the top right, below the menu bar, with Wi-Fi, Bluetooth, focus mode, display brightness and volume controls, the battery, and
tiles for the modules whose service does not exist yet. A toggle, or a slider drag, acts on the system daemons. A click outside
the card closes it.

The crate has no Smithay dependency. The model (`model`, `layout`, `lifecycle`, `controller`), the backends (`backend`,
`system`), the bus interface (`control`) and the CupertinoKit view (`view`) are separate, so a host can drive the panel without
the process. The `cupertino-controlcenter` process is that host for the desktop. Sizes, timings, slider rules and the battery
glyph follow the analysis. The
items marked or approximate below are authored and replaceable.

## Running

Feature `app` builds the binary: the card as a CupertinoKit (`cupertino-kit`) window on a layer surface, as a client of
`cupertino-server`, as `cupertino-osd` does. `cupertino-shell-client` gives the windows their layer surfaces through the
`SurfaceRoles` of `cupertino-ui-wayland`.
The server starts and restarts it through the `shell` key of its configuration, and a `control-panel` key binding and hot-corner
action call `Toggle()`. The entry is `{"name": "controlcenter", "command": ["cupertino-controlcenter"]}` in the list below.

```json
{"shell": [{"name": "controlcenter", "command": ["cupertino-controlcenter"]}]}
```

```
cargo build -p cupertino-controlcenter --features app
cupertino-controlcenter [--fake-backends] [--alsa-card N]
```

| Option or variable | Default | Effect |
|---|---|---|
| `--fake-backends` | off | In-memory radios, volume, backlight and battery instead of the machine's. For trying the panel and for headless runs. |
| `--alsa-card N` | the default PipeWire sink, else the best master of all cards (rule in `cupertino-alsa`) | Use the master volume of sound card `N`. |

The session's appearance and display options change the look through the kit's theme tokens, which the kit follows
while the process runs: Increase Contrast takes the high-contrast catalog and Reduce Transparency the opaque material.
The module toggles, tiles, level sliders and battery meter are the controls of `cupertino-ui-controls`, which read every
colour and metric from the theme. Icons are the template symbols of the asset directory (`symbol.*`), loaded by the kit.

## How it works

**Windows.** `view::panel` is the card: a window of the card's size (`layout::card_size`, 298 x 370 pt, or 298 x 408 pt
with the battery row) with `WindowBackground::Surface` on the `Menu` surface, 16 pt radius. The kit asks the server for the
material as a backdrop, and the backdrop's alpha and the controls (one group) follow the fade. `view::catcher` is a
transparent window with no drawing that takes a left or right press anywhere on it. The process's `PanelRoles` gives the
card (`view::APP_ID`) an overlay layer surface anchored top and right, with an exclusive zone of -1 and margins that put the
card 6 pt from the right edge and 6 pt below the bar. The bar reserves no layer zone, so the top margin adds the theme's menu
bar height. The catcher (`view::CATCHER_APP_ID`) gets a top layer surface just below it, over the whole output.

**Session.** A hidden panel has no window. A toggle opens the card and the catcher. A dismissal closes the catcher at once
and fades the card, whose window closes when the fade ends. A battery that comes or goes opens the card again at its new
size. While the panel is shown the card asks for **exclusive** keyboard interactivity, so it is the key window as soon as it
opens, from the menu bar, a shortcut or a hot corner (the design panel can become key). It asks for it again when a toggle
during the fade shows the panel again. It asks for none from the start of the fade. Another exclusive surface (the launcher)
that takes the keyboard from the shown card dismisses the panel. On `locked` a shown panel starts closing too.

**Outputs.** The panel opens on the first output the server describes, chosen each time it opens with no surface left,
and the card and the catcher stay on that output while either exists (a toggle during the fade reopens on the card's
output). When that output goes (a display unplugged), the panel is **dismissed**, not moved: the server closes its
surfaces, or the output leaves the list the process sees (`Request::OutputGone`), and either hides the panel at once,
without the fade, dropping a drag in progress (`Controller::vanish`). The server takes back the card's keyboard for the
key window. The next toggle opens the panel on an output that exists. The process does not need an output to start or to
keep running: a toggle with no output at all opens surfaces the server closes again at once, so nothing shows. Moving the
open panel to the remaining output was not chosen: it would need new surfaces placed while the person's attention is on
the display change, and a dismissal is what any other loss of the panel's place does (a lock, another exclusive panel).

**Dismissal.** A left or right press outside the card (mouse monitor mask `0xa`), losing the keyboard focus
(resign-key observer) and Escape. Escape is **not** in the design: it has no cancel handler on its panel
window. It is added here (approximate). The menu-tracking observers of the design have no
counterpart yet.

**Menu bar.** A click on the control panel slot calls `Toggle. The panel anchors to the top right and does not take the
slot's position (places it from the item's frame).

## Protocols / D-Bus it speaks

The process owns `org.cupertino.ControlPanel1` at `/org/cupertino/ControlPanel1` on the session bus. A second panel cannot take
the name and exits with failure. The interface lives in `control` (`BUS_NAME`, `OBJECT_PATH`, `serve`).

| Method | Effect |
|---|---|
| `Toggle | Opens the panel (reading every backend first), or starts closing it when it is open. |

```
gdbus call --session --dest org.cupertino.ControlPanel1 --object-path /org/cupertino/ControlPanel1 --method org.cupertino.ControlPanel1.Toggle
```

The backends below also speak `org.cupertino.Network1`, `org.cupertino.Bluetooth1`, `org.cupertino.Power1` and
`org.freedesktop.UPower.Device` on the system bus, and `org.cupertino.NotificationCenter1` and `org.cupertino.Osd1` on the
session bus.

### Backends

A failure disables the tile with the reason and logs it on stderr as `cupertino-controlcenter: <what>: <reason>`. There is no
silent default. The panel reads again every second while open. It logs a failure when it starts or its reason changes, and
`<what>: available again` when it recovers, not on every retry. Each connection is made on first use and again after a
failure. Properties are read from the service every time, never from a cache.

| Module | Real backend | Fake (`--fake-backends`, tests) |
|---|---|---|
| Wi-Fi | `system::NetdWifi`: `org.cupertino.Network1` on the system bus, property `WirelessEnabled` (read, write) and `Devices` (the SSID of the first connected Wi-Fi device) | `MemoryWifi` |
| Bluetooth | `system::BtdBluetooth`: `org.cupertino.Bluetooth1`, properties `Available` (false is a disabled tile) and `Powered` (read, write) | `MemoryBluetooth` |
| Focus mode | `org.cupertino.NotificationCenter1` on the session bus (`cupertino-notifications`): the `DoNotDisturb` property and `SetDoNotDisturb(b); on, it stays on until turned off | `MemoryFocusMode` |
| Display brightness | `system::PowerdBrightness`: `org.cupertino.Power1` (`GetBrightness`, `GetMaxBrightness`, `SetBrightness`), raw value mapped linearly onto 0 to 1, never below raw 1 | `MemoryBrightness` |
| Volume | `backend::SessionVolume`: the default PipeWire sink through `cupertino-pipewire` (`wpctl`), the level `wpctl get-volume` reports; when no PipeWire daemon answers a read, `backend::AlsaVolume`: `cupertino-alsa` master selection (`Master::open_default`, the same as the overlay), level and mute switch; opened on first use and again after a failure | `MemoryVolume` |
| Battery | `system::PowerdBattery`: `org.freedesktop.UPower.Device` at `DisplayDevice` (`IsPresent`, `Percentage`, `State`); no battery hides the row, it is not a failure | `MemoryBattery` |
| Overlay | `system::SessionOverlay`: `org.cupertino.Osd1.Show(kind, level)` on the session bus, after a slider drag ends (`volume`, `mute` for a muted output, `brightness`) | `RecordingOverlay` |
| File sharing, stage, screen mirroring, now playing | none: shown, disabled, with a reason (`Module::unsupported`) | same |

### Behaviour

- Opening reads every backend; while open the panel reads them again every second (approximate), so a radio that follows its
  switch late, or a level changed by a key, shows up. No reading happens during a slider drag.
- A toggle press flips the switch in the backend and shows the new state at once. A refused write disables the tile with the
  daemon's error. A disabled tile ignores presses.
- A slider is a zero-distance drag: the press jumps the knob to the pointer and the drag follows. The model follows every
  motion; the backend is written at most every 16.7 ms, the latest value winning. Releasing writes the last value, then
  tells the overlay.
- The volume is linear, 0 to 1; a level below 0.001 mutes, any higher level unmutes. A card without a mute switch is silenced by
  level 0 alone.
- Dismissing during a drag drops the unwritten value.

## Values applied

| Item | Value |
|---|---|
| Card width | 298 pt (the per-language extra width is not applied: English is +0) |
| Padding, spacing, radius | 10 pt padding, 10 pt vertical spacing, 16 pt continuous corners |
| Column gap | 10 pt: `298 - 2 * 10 = 278 = 2 * 134 + 10` and `(134 - 10) / 2 = 62`, so the grid is made of integers only with a gap of 10 |
| Compact tile height | 62 pt |
| Connectivity block height | 134 pt (`2 * 62 + 10`) |
| Connectivity row | 40 pt high, 7 pt between rows: `3 * 40 + 2 * 7 = 134`, so the rows sit directly on the card with no padded tile |
| Optional first row | 28 pt; used for the battery (shown only with a battery) |
| Module order | left: Wi-Fi, Bluetooth, file sharing (the connectivity block); right: focus mode (the "Focus" tile), then stage and screen mirroring (62 x 62 each); then display, sound and now playing rows (278 x 62) |
| Row tile | the focus and now playing tiles keep the 10 pt card padding on every side, so a title never runs closer than that to the trailing edge |
| Fade-out | 0.238 s, linear alpha on the card and its backdrop |
| Lifecycle | hidden, shown, hiding, hidden; a toggle while hiding reopens; the output going hides at once |
| Slider track | 22 pt high (minimum width 120; the panel's track is 254 pt) |
| Slider knob travel and position | `f = clamp((x - minX - 11) / (w - 22), 0, 1); the knob's hit frame reaches 9 pt past the track on every side |
| Slider colours | track grey 0.569 (dark) or 0.463 (light), 3:1 or more against the module in every palette (`cupertino-ui-theme`'s `PanelTrack`), light outline black 0.102, white fill inset 1 pt, knob about 20 pt, disabled alpha 0.30 (dark) or 0.50 |
| Volume writes | linear 0 to 1, mute below 0.001, throttle 16.7 ms with the latest value kept |
| Fonts | titles system 12 medium |
| Battery glyph | fill `clamp(round(19 * level), 2, 19) at (2, 2, w, 8) with radius 1.5, 4 pt to the percentage, red at level 0.1 or below |
| Card shadow | none for now (see the open issues); the design's is opacity 0.25, radius 8, offset 4, sigma = radius / 2 |
| Edge | the card 6 pt from the right edge and below the bar (`layout::EDGE_MARGIN`) |
| Speaker glyph tiers | muted below 0.001, then thirds |
| Card material | the `Menu` recipe (the design's has no recipe of its own in the design tokens) (`view::panel`) |
| Opening | immediate: no open animation for the panel itself is recorded (is the module expansion spring) (`lifecycle`) |
| Tile and toggle colours | tile background, button off, on with a white glyph; found the literals but not their roles (the theme roles of `cupertino-ui-controls`' `ModuleToggle`) |
| Tile corner radius | 12 pt (`layout::TILE_RADIUS`) |
| Slider placement | 12 pt from the tile's sides, 10 pt from its bottom; title 8 pt from the top (`layout`) |
| Caption size and text gaps | 10 pt (the footnote style), 8 pt between button and text (`cupertino-ui-controls`) |
| Icon names and sizes | `symbol.*` names (`view`): Wi-Fi and Bluetooth are the names the design uses, the other tiles' symbols is not known; 18 pt in a button, 22 pt on a compact tile, 14 pt on a slider. The slider glyphs are `symbol.sun` (`sun.max.fill`) and the speaker tiers (`speaker.slash.fill`, `speaker.wave.3.fill`, names; the tiers use the static symbol with the same number of waves) |
| Display floor | raw value 1 (a slider never turns the screen off; the design maps 0.0 to the panel's device minimum) (`system`) |
| Brightness write throttle | the 16.7 ms of the volume (`controller::WRITE_INTERVAL`) |
| Refresh while open | 1 s (`controller::REFRESH_INTERVAL`) |
| Escape dismisses | not in the design (`view::panel`) |
| Wi-Fi tier glyph, expandable modules, toggle spring, slider spring, tick dots, low power mode colour, per-language width, right-to-left mirroring | not implemented |

## Open issues

- Unplugging a real display with the panel open has not been tried yet; the headless output tests cover it, and the
  hardware check belongs to the display hot-plug acceptance (#22).
- The card has no drop shadow. The server draws shadows for windows only, not for layer surfaces, and a kit backdrop covers
  its whole window, so shadow room on the surface would also get the material. Restoring it needs a backdrop inset (or a
  shadow) in the app effects protocol, the kit and the server.

## Modules

| Module | Content |
|---|---|
| `model` | `Module` (grid order), `Slider`, `Reading` (value or reason), `Controls`, `TileState`, `Battery`, `VolumeState`, `speaker_icon` |
| `layout` | the card's metrics in points (`PANEL_WIDTH`, `PADDING`, `SPACING`, `COLUMN`, row heights, slider insets, `EDGE_MARGIN`) and `card_size` |
| `lifecycle` | `Lifecycle`, `Phase`, `FADE_SECONDS` with injected time |
| `backend` | the traits (`Wifi`, `Bluetooth`, `FocusMode`, `Brightness`, `Volume`, `BatteryReader`, `OverlayNotifier`), `SessionVolume`, `AlsaVolume`, `Unavailable` and the `Memory* stand-ins |
| `system` (feature `dbus`) | `NetdWifi`, `BtdBluetooth`, `PowerdBrightness`, `PowerdBattery`, `SessionOverlay` |
| `controller` | `Controller`: lifecycle, readings, presses, throttled slider writes, refresh timer; `Backends` |
| `control` (feature `dbus`) | the `org.cupertino.ControlPanel1` interface (`serve`) |
| `view` (feature `app`) | the CupertinoKit windows: `panel` (the card), `catcher`, `Input` (what the user did), `APP_ID`, `CATCHER_APP_ID`, `TITLE` |

## Tests

```
cargo test -p cupertino-controlcenter --features app
```

| Test | Proves |
|---|---|
| Model and layout unit tests | The module order, the readings behind the tiles, the card's size with and without the battery row. |
| Lifecycle unit tests | Full opacity, the linear fade over 0.238 s, reopening while hiding, hiding at once from any phase, at literal instants. |
| Controller unit tests | Toggles and slider changes, the 16.7 ms write throttle, mute below 0.001, a tile disabled by a failing read or write with the reason, the overlay told after a drag, the refresh timer, a vanish that hides at once, drops a drag and reads again on reopening. |
| `tests/panel.rs` | The card as the kit draws it against `tests/goldens` (`shown`, `reduce-transparency`, `increase-contrast`: the 1x frame and the focus and accessibility tree of every control), its material and window per battery case, the grid frames, a switch press, a slider drag and release, Escape; the catcher dismissing on a left or right press only. |
| Process unit tests (`src/app.rs`) | The windows per phase (card over catcher when shown, the card alone while fading, none when hidden), the lock, input ignored during the fade, a toggle during the fade then Escape, the timer generations, and the card's keyboard: asked for whenever shown, given back for the fade, and losing it while shown dismisses. A window the server closes, or the output going, hides the panel at once, in the shown and the fading phase, and a toggle reopens a card that acts; the opening output is the first described one and the panel hears when its output goes. |
| `tests/control.rs` | `Toggle` against a private `dbus-daemon` with in-memory backends: it opens, then closes, the panel, and a second panel cannot take the name. |

`cupertino-server`'s `tests/shell_processes.rs` runs the process with `--fake-backends` through output removal: open on an
output that is unplugged while another remains, then on the last output, then after a display is plugged in again. Each
time the panel's surfaces go, the keyboard goes back to the window, the process is not restarted, and the reopened card's
Bluetooth switch and sound slider draw their new state through a refresh of the backends.

Nothing here touches a real radio, adapter, volume or backlight.

Screenshots of the process on the headless compositor are in `output/` (`panel-light.png`, `panel-dark.png`, and `panel-interact.png`
after clicks on Bluetooth, the focus mode tile and the volume slider), taken with `--fake-backends` (before the glyphs came from the assets)
and `Toggle()` over a private session bus:

```
cargo run -p cupertino-server -- --headless --size 1280x800 --dbus \
  --spawn "target/debug/cupertino-controlcenter --fake-backends" --spawn-delay 1 --shot 5:panel-light.png --run-for 7
# 3 s in, from another shell on the same bus:
gdbus call --session --dest org.cupertino.ControlPanel1 --object-path /org/cupertino/ControlPanel1 --method org.cupertino.ControlPanel1.Toggle
```
