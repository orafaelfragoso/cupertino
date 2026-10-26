# cupertino-controlcenter

The control panel of the Cupertino desktop. It stands in for the design's control panel process: a 298 pt card that opens at
the top right, below the menu bar, with Wi-Fi, Bluetooth, focus mode, display brightness and volume controls, the battery, and
tiles for the modules whose service does not exist yet. A toggle, or a slider drag, acts on the system daemons. A click outside
the card closes it.

The crate has no Smithay dependency. The model (`model`, `layout`, `lifecycle`, `controller`), the backends (`backend`,
`system`), the bus interface (`control`) and the CPU renderer (`render`) are separate, so a host can drive the panel without
the process. The `cupertino-controlcenter` process is that host for the desktop. Sizes, timings, slider rules and the battery
glyph follow the analysis. The
items marked or approximate below are authored and replaceable.

## Running

Feature `app` builds the binary: the card on a layer surface, as a client of `cupertino-server` (`render/cupertino-client`).
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
| `--alsa-card N` | the best master of all cards (rule in `cupertino-alsa`) | Use the master volume of sound card `N`. |

The session's display options change the look: Increase Contrast takes the high-contrast catalog and the contrast slider track,
gray 0.459 light / 0.612 dark, and Reduce Transparency takes the opaque material. Icons are the template
symbols of the asset directory (`symbol.*`) tinted by the palette; `Renderer::new` fails when the
directory lacks one of `render::ICON_NAMES`, so no glyph is ever left out or substituted while drawing. The text renderer's
optical size follows the surface scale (`TextRenderer::set_scale`, set on every render).

## How it works

**Surface.** One overlay layer surface anchored top and right, as large as the card with a tall battery row plus its shadow
room (330 x 430 pt), with an exclusive zone of -1 and margins that put the card 6 pt from the right edge and 6 pt below the
bar. The bar reserves no layer zone, so the top margin adds the theme's menu bar height. The material (`render::MATERIAL`) is a
server-rendered backdrop with the card's rectangle and 16 pt radius, and its alpha follows the fade. While hidden the surface
stays mapped, draws nothing and has an empty input region. While shown its input region is the card. A second, transparent layer
surface over the whole output sits just below it while the panel is shown and catches a press outside the card. While shown the
surface asks for **exclusive** keyboard interactivity, so it is the key window as soon as it opens, from the menu bar, a shortcut
or a hot corner (the design panel can become key). It asks for none otherwise. Another exclusive surface (the
launcher) takes the keyboard from it, which dismisses it.

**Session.** The palette comes from the appearance the server announces over `cupertino_session_v1`. It is read at start, and on
a change the renderer is rebuilt and the card drawn again. On `locked` a shown panel starts closing, as when it loses the
keyboard.

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
| Volume | `backend::AlsaVolume`: `cupertino-alsa` master selection (`Master::open_default`, the same as the overlay), level and mute switch; opened on first use and again after a failure | `MemoryVolume` |
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
| Module order | left: Wi-Fi, Bluetooth, file sharing (the connectivity block); right: focus mode, then stage and screen mirroring (62 x 62 each); then display, sound and now playing rows (278 x 62) |
| Fade-out | 0.238 s, linear alpha on the card and its backdrop |
| Lifecycle | hidden, shown, hiding, hidden; a toggle while hiding reopens |
| Slider track | 22 pt high (minimum width 120; the panel's track is 254 pt) |
| Slider knob travel and position | `f = clamp((x - minX - 11) / (w - 22), 0, 1); the knob's hit frame reaches 9 pt past the track on every side |
| Slider colours | track grey 0.137 (dark) or 0.910 (light), light outline black 0.102, white fill inset 1 pt, knob about 20 pt, disabled alpha 0.30 (dark) or 0.50 |
| Volume writes | linear 0 to 1, mute below 0.001, throttle 16.7 ms with the latest value kept |
| Fonts | titles system 12 medium |
| Battery glyph | fill `clamp(round(19 * level), 2, 19) at (2, 2, w, 8) with radius 1.5, 4 pt to the percentage, red at level 0.1 or below |
| Card shadow | opacity 0.25, radius 8, offset 4; sigma = radius / 2 |
| Shadow room and edge | surface insets (6, 16, 16, 16); the card 6 pt from the right edge and below the bar |
| Speaker glyph tiers | muted below 0.001, then thirds |
| Card material | the `Menu` recipe (the design's has no recipe of its own in the design tokens) (`render::MATERIAL`) |
| Opening | immediate: no open animation for the panel itself is recorded (is the module expansion spring) (`lifecycle`) |
| Tile and toggle colours | tile background, button off, on with a white glyph; found the literals but not their roles (`render::Palette`) |
| Tile corner radius | 12 pt (`layout::TILE_RADIUS`) |
| Slider placement | 12 pt from the tile's sides, 10 pt from its bottom; title 8 pt from the top (`layout`, `render`) |
| Caption size and text gaps | 10 pt (the footnote style), 8 pt between button and text (`render`) |
| Icon names and sizes | `symbol.*` names (`render::module_icon`): Wi-Fi and Bluetooth are the names the design uses, the other tiles' symbols is not known; 18 pt in a button, 22 pt on a compact tile, 14 pt on a slider. The slider glyphs are `symbol.sun` (`sun.max.fill`) and the speaker tiers (`speaker.slash.fill`, `speaker.wave.3.fill`, names; the tiers use the static symbol with the same number of waves) |
| Display floor | raw value 1 (a slider never turns the screen off; the design maps 0.0 to the panel's device minimum) (`system`) |
| Brightness write throttle | the 16.7 ms of the volume (`controller::WRITE_INTERVAL`) |
| Refresh while open | 1 s (`controller::REFRESH_INTERVAL`) |
| Escape dismisses | not in the design (`app`) |
| Wi-Fi tier glyph, expandable modules, toggle spring, slider spring, tick dots, low power mode colour, per-language width, right-to-left mirroring | not implemented |

## Open issues


## Modules

| Module | Content |
|---|---|
| `model` | `Module` (grid order), `Slider`, `Reading` (value or reason), `Controls`, `TileState`, `Battery`, `VolumeState`, `speaker_icon` |
| `layout` | frames of the card in points, slider tracks, `Hit`, `fraction_at` |
| `lifecycle` | `Lifecycle`, `Phase`, `FADE_SECONDS` with injected time |
| `backend` | the traits (`Wifi`, `Bluetooth`, `FocusMode`, `Brightness`, `Volume`, `BatteryReader`, `OverlayNotifier`), `AlsaVolume`, `Unavailable` and the `Memory* stand-ins |
| `system` (feature `dbus`) | `NetdWifi`, `BtdBluetooth`, `PowerdBrightness`, `PowerdBattery`, `SessionOverlay` |
| `controller` | `Controller`: lifecycle, readings, presses, throttled slider writes, refresh timer; `Backends` |
| `control` (feature `dbus`) | the `org.cupertino.ControlPanel1` interface (`serve`) |
| `render` | `Palette`, `Renderer`, `surface_size`, `card_origin` |

## Tests

```
cargo test -p cupertino-controlcenter --features app
```

| Test | Proves |
|---|---|
| Model and layout unit tests | The module order, the frames with and without the battery row, the slider track and position formula, hit testing (rows, tiles, sliders, the knob's hit frame, outside). |
| Lifecycle unit tests | Full opacity, the linear fade over 0.238 s, reopening while hiding, at literal instants. |
| Controller unit tests | Toggles and slider changes, the 16.7 ms write throttle, mute below 0.001, a tile disabled by a failing read or write with the reason, the overlay told after a drag, the refresh timer. |
| Render unit tests | The slider, disabled and battery pixels. |
| `tests/control.rs` | `Toggle` against a private `dbus-daemon` with in-memory backends: it opens, then closes, the panel, and a second panel cannot take the name. |

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
