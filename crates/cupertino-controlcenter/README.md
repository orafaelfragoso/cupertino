# cupertino-controlcenter

The control panel of the Cupertino desktop. It opens at the top right, below the menu bar: separate glass tiles for Wi-Fi,
Bluetooth, the microphone, now playing, dock hiding, the screen lock, mute, the keyboard backlight and focus mode, wide
display and sound tiles with their levels, and an Edit Controls capsule under them. A press or a level drag acts on the
system services. Wi-Fi, Bluetooth, display and sound open a detail view with their networks, devices, keyboard backlight
or outputs. A click outside the tiles closes the panel.

The crate has no Smithay dependency. The model (`model`, `layout`, `lifecycle`, `motion`, `controller`), the backends
(`backend`, `system`), the bus interface (`control`) and the CupertinoKit view (`view`) are separate, so a host can drive
the panel without the process. The `cupertino-controlcenter` process is that host for the desktop.

## Running

Feature `app` builds the binary: the panel as a CupertinoKit (`cupertino-kit`) window on a layer surface, as a client of
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
| `--fake-backends` | off | In-memory radios, microphone, dock setting, keyboard backlight, focus mode, brightness, volume, outputs and media player instead of the machine's, and a lock and settings launcher that only record what they are asked. For trying the panel and for headless runs. |
| `--alsa-card N` | the default PipeWire sink, else the best master of all cards (rule in `cupertino-alsa`) | Use the master volume of sound card `N`. |

The panel follows the session's appearance: light glass with dark text and symbols in the light appearance, dark glass
with light ones in the dark appearance. It follows the display options as they change: Increase
Contrast takes the high-contrast catalog and makes each tile a solid grey with a rim (mid grey with a darker rim in the
dark appearance, light grey with a mid grey rim in the light one); Reduce Transparency makes
the tiles opaque on one opaque panel; Reduce Motion replaces the growth of a detail view with a cross-fade. Icons are the
symbols of the asset directory (`symbol.*`), loaded by the kit.

## How it works

**Windows.** `view::panel` is one transparent surface (`layout::SURFACE`) with no background of its own: the 308 x 500 pt window (`layout::WINDOW`) inside a transparent margin of 122 pt left and below it (24 pt above, to the output's edge on the right) where the soft shadow around the tiles is drawn, a darkening of about 9 % at the tiles' edge that fades out over 130 pt. Only the window's rectangle takes input, and nothing while the panel fades out.
Each tile asks the compositor for module glass under its own outline (`PaintCx::glass`, `Glass::module`): capsules and circles with round
ends, the media tile and an open detail view with 32 pt corners, the display and sound tiles with 24 pt corners. The tiles
join one glass container with an 8 pt blend, less than the 12 pt between them, so tiles at rest stay apart and a growing
detail view blends where it passes them. The glass alpha follows the fade; the content fades as one layer. Module glass
keeps the wallpaper's colour: over white a tile comes out at 244 in the light appearance and 207 in the dark one. The
Edit Controls capsule is glass as the tiles are.
With Reduce Transparency each tile fills grey 0.114 on one grey 0.302 panel of 40 pt corners (0.96 on 0.82 in the light
appearance), 8 pt around the grid and the Edit Controls capsule, closing in to an open detail view as it opens. With
Increase Contrast each tile fills grey 0.463 inside a 0.5 pt rim of grey 0.341 (0.86 inside 0.45 in the light
appearance), with no glass. A tile the window cannot show glass under fills a translucent grey of its appearance
instead. `view::catcher` is a transparent window with no drawing
(`WindowBackground::INPUT_ONLY`: one transparent pixel stretched over the output) that takes a left or right press anywhere on it. The process's `PanelRoles` gives the panel (`view::APP_ID`) an overlay
layer surface anchored top and right, with an exclusive zone of -1 and margins that put the window 6 pt from the right edge
and 12 pt below the bar, so the tiles sit 14 pt from the edge and 20 pt below the bar, 44 pt from the top of the output. The bar reserves no layer zone, so
the top margin adds the theme's menu bar height. The catcher (`view::CATCHER_APP_ID`) gets a top layer surface just below
it, over the whole output.

**Tiles.** A capsule shows a 36 pt badge 14 pt in from its leading and top edges, white with the symbol in the accent
colour when the module is on, a veil when it is off (white at 0.108 in the dark appearance, black at 0.102 in the light
one), and its title (13 semibold) over its state (11 regular),
wrapping onto a second line where the column is too narrow. The sizes are the theme's body and caption sizes; the panel
sets its labels itself (`view::Label`), since the kit's static text sets every label in its role's size. Over the glass a
tile's title is the primary label colour and its state the secondary one, so the state reads as a quieter line under the
name; the kit draws both as the glass's foreground (black in the light appearance, white in the dark one, the
secondary label at the strength that shifts the face by −0.4 or +0.3). The display and sound tiles' titles and the Edit
Controls label (13 medium) are primary labels, and a detail's heading is 11 semibold in the secondary label colour.
Other veils (a pressed control, a row under the pointer, the artwork placeholder, a separator) are white in the dark
appearance and black in the light one. The badge is the
module's switch. On Wi-Fi and Bluetooth the text is a second control that opens the detail view. A circular tile is a
switch or button as a whole, white when on. The media tile shows an artwork placeholder, what plays (or "Not Playing")
and the previous, play or pause and next buttons,
disabled while no player can take them. The display and sound tiles show their title, which opens the detail view, and a
level between their symbols: a 24 pt track of track glass of its own (`Glass::track`, no blur, bending with its height,
a ring shadow inside its outline) over the tile's glass, filled in white up to the level, without a knob; with Reduce
Transparency or Increase Contrast, or where the window shows no glass, the track is a veil; the sound tile also has the output button that
opens its detail view. A press on a tile away from its controls does nothing; a press between the tiles dismisses the
panel.

**Detail views.** The tile grows into the view on a critically damped spring (response 0.35 s, settled by 0.6 s,
`motion::spring`): its frame moves from its cell to the top of the grid, 292 pt wide and as high as its list (at most the
grid's 444 pt), its corners go to 32 pt, the other tiles and Edit Controls fade out over the first quarter of the motion
(the glass lies under all of the window's content, so the growing tile cannot hide what it passes over) on subdued
module glass (`Glass::subdued_module`: twice the blur, a flat, quieter face), since they lie behind the view and take
no presses while it is open, and the tile's
content gives way to the view's at halfway. Closing runs the same way back, from wherever the motion is. With Reduce Motion the tile fades out
in place on subdued glass and the view fades in at its place over 0.2 s. The view shows the module's switch or level, a heading, the list
(Wi-Fi networks with a check on the connected one and a lock on secured ones; Bluetooth devices; the keyboard backlight;
sound outputs with a check on the current one) and a row that opens the module's settings page. Escape or a press
outside the view closes it; a second Escape dismisses the panel, also while the view is still closing. When the keyboard or assistive technology opened the
view, the focus moves into it once it is open; when they closed it, the focus goes back to the module's control once it has
closed. A click leaves the focus where it was, so no focus ring appears (`view::Via`).

**Keyboard and assistive technology.** Every enabled control is a Tab stop and names itself: badges and circles are
switches with their state (Lock Screen is a button), the openers are buttons that say whether their view is open, levels
are sliders (0 to 1, a sixteenth per step), list rows are list items with their place and the current one selected. Space
and Return press; the arrow keys move the focus to the nearest control in their direction (Left and Right step a level
instead); Home and End take a level to its ends. Each tile is a group named for its module.

**Session.** A hidden panel has no window. A toggle opens the panel and the catcher, with no detail view open. A dismissal
closes the catcher at once and fades the panel, whose window closes when the fade ends. While the panel is shown its window asks for **exclusive** keyboard interactivity, so it is the key window as soon as it
opens, from the menu bar, a shortcut or a hot corner. It asks for it again when a toggle
during the fade shows the panel again. It asks for none from the start of the fade. Another exclusive surface (the launcher)
that takes the keyboard from the shown panel dismisses the panel. On `locked` a shown panel starts closing too.

**Outputs.** The panel and the catcher are always on the same output. A panel that opens with nothing open names no
output: the server opens both surfaces on its active output, the one the person works on (where the pointer last moved or
the keyboard last went; the menu bar clicked is on it), in the same pass. The surfaces are then told that output
(`wl_surface.enter`), and a surface that opens while the other is still open names it (`SharedOutput` in `src/app.rs`):
a toggle during the fade opens the new catcher on the fading card's output, even when the person has moved to another
output meanwhile. When that output goes (a display unplugged), the panel is **dismissed**, not moved: the server closes its
surfaces, or the output leaves the list the process sees (`Request::OutputGone`), and either hides the panel at once,
without the fade, dropping a drag in progress (`Controller::vanish`). The server takes back the panel's keyboard for the
key window. The next toggle opens the panel on an output that exists. The process does not need an output to start or to
keep running: a toggle with no output at all opens surfaces the server closes again at once, so nothing shows. Moving the
open panel to the remaining output was not chosen: it would need new surfaces placed while the person's attention is on
the display change, and a dismissal is what any other loss of the panel's place does (a lock, another exclusive panel).

**Dismissal.** A left or right press outside the tiles, losing the keyboard focus and Escape. Edit Controls and a
detail view's settings row open the settings application and dismiss the panel.

**Menu bar.** A click on the control panel slot calls `Toggle`. The panel anchors to the top right and does not take the
slot's position.

## Protocols / D-Bus it speaks

The process owns `org.cupertino.ControlPanel1` at `/org/cupertino/ControlPanel1` on the session bus. A second panel cannot take
the name and exits with failure. The interface lives in `control` (`BUS_NAME`, `OBJECT_PATH`, `serve`, whose `Served`
publishes the properties). The process's surface roles publish them before every wait of the loop, from whether the
catcher is open and which output the panel's surfaces were told. A process that ends drops the name, which the menu bar
takes as not shown.

| Method | Effect |
|---|---|
| `Shown` property (`b`) | Whether the panel is shown: from the toggle that opens it until its dismissal starts the fade, whatever dismissed it. `PropertiesChanged` announces every change. The menu bar highlights its control panel item while it is true. |
| `Output` property (`s`) | The connector name (`wl_output.name`) of the output the shown panel is on, so the menu bar highlights the item on that output's bar; empty while hidden or before the panel's surfaces are told their output. |
| `Toggle` | Opens the panel (reading every backend first), or starts closing it when it is open. Refused to a Flatpak, a Snap and a caller the bus cannot name (`org.cupertino.Error.FlatpakCaller`, `SnapCaller`, `AnonymousCaller`; `cupertino_peer::check_call`). Any unsandboxed process of the person may call it: the menu bar is a caller, and it is no relation of the panel (the server started both), so it cannot be told apart from another process of the person, as for `org.cupertino.NotificationCenter1`. |

```
gdbus call --session --dest org.cupertino.ControlPanel1 --object-path /org/cupertino/ControlPanel1 --method org.cupertino.ControlPanel1.Toggle
```

The backends below also speak `org.cupertino.Network1`, `org.cupertino.Bluetooth1`, `org.cupertino.Power1`,
`org.freedesktop.UPower.KbdBacklight` and `org.freedesktop.login1` on the system bus, and
`org.cupertino.NotificationCenter1`, `org.cupertino.Osd1` and `org.mpris.MediaPlayer2.*` on the session bus.

### Backends

A failure disables the tile with the reason and logs it on stderr as `cupertino-controlcenter: <what>: <reason>`. There is no
silent default. The panel reads again every second while open. It logs a failure when it starts or its reason changes, and
`<what>: available again` when it recovers, not on every retry. Each connection is made on first use and again after a
failure. Properties are read from the service every time, never from a cache.

| Module | Real backend | Fake (`--fake-backends`, tests) |
|---|---|---|
| Wi-Fi | `system::NetdWifi`: `org.cupertino.Network1` on the system bus, property `WirelessEnabled` (read, write) and `Devices` (the SSID of the first connected Wi-Fi device); the view runs `Scan`, lists `GetNetworks` and joins with `Connect(ssid, "")`, which only a network the service knows or an open one accepts (a new secured network needs the passphrase the Wi-Fi settings ask for, so its row is not pressable) | `MemoryWifi` |
| Bluetooth | `system::BtdBluetooth`: `org.cupertino.Bluetooth1`, properties `Available` (false is a disabled tile) and `Powered` (read, write); the view lists the paired `Devices` by name and toggles one with `Connect` or `Disconnect` | `MemoryBluetooth` |
| Microphone | `backend::PipewireMicrophone`: the mute of the default PipeWire source (`cupertino-pipewire`); on means it records | `MemorySwitch` |
| Now playing | `system::MprisPlayer`: the `org.mpris.MediaPlayer2.*` players on the session bus, the playing one first, else the first with a title; `Metadata` (`xesam:title`, `xesam:artist`), `PlaybackStatus`, `CanGoPrevious`, `CanGoNext`, `CanPlay`, `CanPause`; `Previous`, `PlayPause`, `Next`. Each call to a player waits at most 200 ms (`system::PLAYER_TIMEOUT`), so a player that hangs is passed over. No player is "Not Playing", not a failure | `MemoryMedia` |
| Dock hiding | `backend::DockHiding`: the `autohide` key of the dock's settings file (`cupertino_defaults::dock`), which the dock follows | `MemorySwitch` |
| Lock screen | `system::LogindLock`: `GetSessionByPID` of this process, then `Lock` on that session | `RecordingLock` |
| Mute | the volume backend's mute (see Volume); unmuting a level of 0 restores 0.25 | the volume's |
| Keyboard brightness | `system::UpowerKeyboard`: `org.freedesktop.UPower.KbdBacklight` (`GetBrightness`, `GetMaxBrightness`, `SetBrightness`, off or the maximum); a machine without one shows the tile disabled | `MemorySwitch` |
| Focus mode | `org.cupertino.NotificationCenter1` on the session bus (`cupertino-notifications`): the `DoNotDisturb` property and `SetDoNotDisturb(b)`; on, the capsule reads "Do Not Disturb" | `MemorySwitch` |
| Display brightness | `system::PowerdBrightness`: `org.cupertino.Power1` (`GetBrightness`, `GetMaxBrightness`, `SetBrightness`), raw value mapped linearly onto 0 to 1, never below raw 1 | `MemoryBrightness` |
| Volume | `backend::SessionVolume`: the default PipeWire sink through `cupertino-pipewire` (`wpctl`), the level `wpctl get-volume` reports, the sinks as outputs and `set-default` to choose one; when no PipeWire daemon answers a read, `backend::AlsaVolume`: `cupertino-alsa` master selection (`Master::open_default`, the same as the overlay), level and mute switch, with the card's master as the only output; opened on first use and again after a failure | `MemoryVolume` |
| Edit Controls, settings rows | `backend::SettingsLauncher`: starts `cupertino-settings --pane <page>` (`control-panel`, `wifi`, `bluetooth`, `displays`, `sound`) | `RecordingLauncher` |
| Overlay | `system::SessionOverlay`: `org.cupertino.Osd1.Show(kind, level)` on the session bus, after a slider drag ends (`volume`, `mute` for a muted output, `brightness`) | `RecordingOverlay` |

The modules are the ones a service backs. Sharing and screen mirroring have no service, and the session refuses an
appearance change from anyone but the settings application, so the panel has no tiles for them; the battery lives in
the menu bar.

### Behaviour

- Opening reads every backend; while open the panel reads them again every second, and the open detail view's list too,
  so a radio that follows its switch late, or a level changed by a key, shows up. No reading happens during a level drag.
- A switch press flips the switch in the backend and shows the new state at once. A refused write disables the tile with
  the service's error. A disabled tile ignores presses.
- A level is a zero-distance drag: the press puts the level under the pointer (it reaches 4 pt past each end of the track)
  and the drag follows. The model follows every motion; the backend is written at most every 16.7 ms, the latest value
  winning. Releasing writes the last value, then tells the overlay.
- The volume is linear, 0 to 1; a level below 0.001 mutes, any higher level unmutes. A card without a mute switch is
  silenced by level 0 alone.
- Dismissing during a drag drops the unwritten value.

## Values applied

| Item | Value |
|---|---|
| Grid | two columns of 140 pt and six rows of 64 pt, 12 pt apart: 292 x 444 pt (`layout`) |
| Tiles | Wi-Fi, Bluetooth and the microphone capsules down the left column; now playing (140 x 140) beside the first two; dock hiding and lock screen circles beside the microphone; mute and keyboard brightness circles, then the focus capsule; display, then sound across both columns (`layout::frame`) |
| Corners | capsules and circles round, now playing and an open view 32 pt, display and sound 24 pt |
| Window | 308 x 500 pt: the grid 8 pt in, Edit Controls (94 x 24) centred 16 pt below it; 6 pt from the right edge and 12 pt below the bar (the tiles 44 pt from the top); the surface adds the shadow's margin (`layout::SURFACE`) |
| Capsule content | 36 pt badge 14 pt in; text from 56 pt; 13 pt semibold title, 11 pt state |
| Level | 4 pt track centred 42 pt below the tile's top; display track 36 to 254 pt, sound 33 to 213 pt; 26 pt output button centred at 265 pt |
| Media tile | 40 pt artwork 14 pt in; buttons centred at 30, 70 and 110 pt across and 111 pt down |
| Glass | regular, one container, 8 pt blend; in a light session black 0.27 over the glass so it reads as the dark glass does; the rim and glow are the same in both sessions |
| Reduce Transparency | tiles grey 0.114, panel grey 0.302 with 40 pt corners and 8 pt padding |
| Increase Contrast | tiles grey 0.463, 0.5 pt rim grey 0.341 |
| Detail view | 64 pt header, 28 pt heading, 32 pt rows (at most 9), 9 pt separator, 32 pt settings row, 8 pt bottom (`layout::detail`) |
| Detail motion | critically damped spring, response 0.35 s, settled at 0.6 s; content hands over at halfway; Reduce Motion: 0.2 s linear cross-fade (`motion`) |
| Fade | in over 0.23 s on a critically damped curve, with no scale or movement; out over 0.18 s, ease-out; with Reduce Motion both are instant; input stops at the start of the fade-out |
| Lifecycle | hidden, shown, hiding, hidden; a toggle while hiding reopens; the output going hides at once |
| Volume writes | linear 0 to 1, mute below 0.001, throttle 16.7 ms with the latest value kept |
| Level keys | a sixteenth per arrow step |
| Display floor | raw value 1 (a slider never turns the screen off) (`system`) |
| Refresh while open | 1 s (`controller::REFRESH_INTERVAL`) |

## Open issues

- Unplugging a real display with the panel open has not been tried yet; the headless output tests cover it, and the
  hardware check belongs to the display hot-plug acceptance (#22).
- The tiles have no drop shadow: the server draws shadows for windows only, not for layer surfaces.
- The light-session tint is drawn by the panel over the compositor's light glass. A per-region dark recipe in the app
  effects protocol would let the compositor draw dark glass there itself. The compositor does not see the tint when it
  picks the text tone of small glass, so the labels stay white only because all tiles join one container too large to
  follow its backdrop; a tile split into a container of its own under 64 pt would get dark labels over a light page.
- The media tile shows a placeholder, not the track's artwork.

## Modules

| Module | Content |
|---|---|
| `model` | `Module` (reading order), `Detail`, `Slider`, `Reading` (value or reason), `Controls`, `TileState`, `VolumeState`, `Media`, `MediaCommand`, `Network`, `Device`, `Output`, `speaker_icon` |
| `layout` | the grid in points (`ROW`, `COLUMN`, `GAP`, `WINDOW`, `INSET`), each module's `frame` and `outline`, the Edit Controls frame, and the detail view's parts, `detail_frame` and `morph` |
| `lifecycle` | `Lifecycle`, `Phase`, `FADE_SECONDS` with injected time |
| `motion` | `spring`, `Expansion` (a detail view opening or closing, reversible, with Reduce Motion) and `visibility` with injected time |
| `backend` | the traits (`Wifi`, `Bluetooth`, `Switch`, `Brightness`, `Volume`, `MediaPlayer`, `SessionLock`, `Launcher`, `OverlayNotifier`), `SessionVolume`, `AlsaVolume`, `PipewireMicrophone`, `DockHiding`, `SettingsLauncher`, `Unavailable`, the `Memory*` stand-ins and `Recording` |
| `system` (feature `dbus`) | `NetdWifi`, `BtdBluetooth`, `NotificationFocusMode`, `PowerdBrightness`, `UpowerKeyboard`, `LogindLock`, `MprisPlayer`, `SessionOverlay` |
| `controller` | `Controller`: lifecycle, readings, presses, the open detail view and its list, joins, connections, outputs, media commands, settings, throttled level writes, refresh timer; `Backends` |
| `control` (feature `dbus`) | the `org.cupertino.ControlPanel1` interface (`serve`) |
| `view` (feature `app`) | the CupertinoKit windows: `panel` (the tiles at a `Presentation`), `catcher`, `Input` (what the user did), `Via`, `focus_key`, `APP_ID`, `CATCHER_APP_ID`, `TITLE` |

## Tests

```
cargo test -p cupertino-controlcenter --features app
```

| Test | Proves |
|---|---|
| Model, layout and motion unit tests | The readings behind the tiles, every tile's cell and outline with no two tiles touching, the detail view's size and morph, the spring without overshoot, a reversal from where the motion was, the Reduce Motion fade and the content hand-over. |
| Lifecycle unit tests | Full opacity, the fade, reopening while hiding, hiding at once from any phase, at literal instants. |
| Backend unit tests | The dock hiding file, the microphone and the session outputs against `cupertino-pipewire`'s fake `wpctl`, the ALSA and session volume, and the stand-ins. |
| Controller unit tests | Switches, mute, lock, Edit Controls and settings, media commands, the detail lists, a refused join, the 16.7 ms write throttle, mute below 0.001, a tile disabled by a failing read or write with the reason, the overlay told after a drag, the refresh timer, a vanish that hides at once, drops a drag and reads again on reopening. |
| `tests/panel.rs` | The panel as the kit draws it against `tests/goldens` (`shown`, `reduce-transparency`, `increase-contrast`, `light-session`, `wifi-detail`: the 1x frame and the focus and accessibility tree of every control); every tile's frame; badge, opener, circle, media, output and Edit Controls presses; a disabled module; a level drag and release; Escape and a press between the tiles dismissing, or collapsing an open view; the Wi-Fi view's list, join and settings row; Tab, Space and the arrow keys across the tiles; the catcher dismissing on a left or right press only; the tiles staying one glass container too large for its text to follow the backdrop, at rest, in a light session, fading and with a view opening or open; Edit Controls glass as the tiles are, in a dark and a light session. The `layout` unit tests hold the tiles 44 pt from the top of the output and 14 pt from its right edge. |
| Process unit tests (`src/app.rs`) | The windows per phase (panel over catcher when shown, the panel alone while fading, none when hidden), the lock, input ignored during the fade, a toggle during the fade then Escape, the timer generations, the detail view opening, closing and reversing, a second Escape while it closes dismissing, with the focus moved at its ends only after the keyboard, Reduce Motion and the light session from the session, and the panel's keyboard: asked for whenever shown, given back for the fade, and losing it while shown dismisses. A window the server closes, or the output going, hides the panel at once, in the shown and the fading phase, and a toggle reopens a panel that acts; the panel hears when its output goes; a toggle during the fade opens the catcher on the card's output after the active output changed. |
| `tests/control.rs` | `Toggle` against a private `dbus-daemon` with in-memory backends: it opens, then closes, the panel, and a second panel cannot take the name; another process (a `dbus-send` the test starts, as the menu bar is to the panel) may toggle, and a connection over anonymous TCP (no process the bus can name) is refused as `AnonymousCaller`. `Shown` and `Output` say what the panel published, hidden until then, and each change is announced once with `PropertiesChanged`. |

`cupertino-server`'s `tests/shell_processes.rs` runs the process with `--fake-backends` through output removal: open on an
output that is unplugged while another remains, then on the last output, then after a display is plugged in again. Each
time the panel's surfaces go, the keyboard goes back to the window, the process is not restarted, and the reopened panel's
Bluetooth switch and sound level draw their new state through a refresh of the backends.

Nothing here touches a real radio, adapter, volume or backlight.
