# cupertino-controlcenter

The control panel of the Cupertino desktop. It opens at the top right, below the menu bar: separate glass tiles for Wi-Fi,
Bluetooth, the microphone, now playing, dock hiding, the screen lock, mute, the keyboard backlight and focus mode, wide
display and sound tiles with their levels, and an Edit Controls capsule under them, which lets the person choose which
of these controls show and in which order. A press or a level drag acts on the system services. Wi-Fi, Bluetooth, display and sound open a detail view with their networks, devices, keyboard backlight
or outputs. A click outside the tiles closes the panel.

The crate has no Smithay dependency. The model (`model`, `arrangement`, `layout`, `lifecycle`, `motion`, `controller`), the backends
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

**Windows.** `view::panel` is one transparent surface (`layout::SURFACE`) with no background of its own: the 308 x 500 pt window (`layout::WINDOW`) inside a transparent margin of 122 pt left and below it (24 pt above, to the output's edge on the right) where the soft shadow around the tiles is drawn, a darkening of about 9 % at the tiles' edge that fades out over 130 pt
to the sides and below, and above the tiles within the 20 pt up to the menu bar's edge, so nothing of it reaches into the
bar and it shows no edge where the bar begins. Between the tiles it keeps the 9 %; under a fading tile it lies at the
share the tile's glass lets through, so it follows a detail view's motion without a jump. The shadow's 26 bands are drawn once, while the panel rests, into a picture of the window's device pixels that the process keeps (two at most: with no view and with one open) and every later frame and opening lays over one texel per pixel, within 2/255 of the bands filled in place; a detail view's moving outline is filled in place. A whole opening frame at 1.5 takes some 3 ms on the new renderer each opening makes, against 14 ms with the bands filled. Only the window's rectangle takes input, and nothing while the panel fades out.
Each tile asks the compositor for module glass under its own outline (`PaintCx::glass`, `Glass::module`): capsules and circles with round
ends, the media tile and an open detail view with 32 pt corners, the display and sound tiles with 24 pt corners. The tiles
join one glass container with an 8 pt blend, less than the 12 pt between them, so tiles at rest stay apart and a
moving detail view blends where it passes them. Every tile keeps that glass and container through a detail view's
motion, only its alpha changing, so nothing changes look the moment a motion starts, turns or ends. The glass alpha
follows the fade; the content fades as one layer. Module glass
keeps the wallpaper's colour: over white a tile comes out at 244 in the light appearance and 207 in the dark one. The
Edit Controls capsule is glass as the tiles are.
With Reduce Transparency each tile fills grey 0.114 on one grey 0.302 panel of 40 pt corners (0.96 on 0.82 in the light
appearance), 8 pt around the grid and the Edit Controls capsule, closing in to an open detail view as it opens. With
Increase Contrast each tile fills grey 0.463 inside a 0.5 pt rim of grey 0.341 (0.86 inside 0.45 in the light
appearance), with no glass. A tile the window cannot show glass under fills a translucent grey of its appearance
instead. `view::catcher` (Kit's `PressCatcher`, which the notification centre uses too) is a transparent window with no drawing
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
(the glass lies under all of the window's content, so the growing tile cannot hide what it passes over), and the tile's
content gives way to the view's at halfway. Closing runs the same way back, from wherever the motion is. With Reduce
Motion the tile fades out in place and the view fades in at its place over 0.2 s. The view shows the module's switch or level, a heading, the list
(Wi-Fi networks with a check on the connected one and a lock on secured ones; Bluetooth devices; the keyboard backlight;
sound outputs with a check on the current one) and a row that opens the module's settings page. Escape or a press
outside the view closes it; a second Escape dismisses the panel, also while the view is still closing. When the keyboard or assistive technology opened the
view, the focus moves into it once it is open; when they closed it, the focus goes back to the module's control once it has
closed. A click leaves the focus where it was, so no focus ring appears (`view::Via`).

**Editing the controls.** Edit Controls puts the panel itself into an editing state; no other window opens. Every tile
keeps its place and look, its content drawn at 0.55 and no longer acting (it is left out of the Tab order and of what
assistive technology reads), and carries a remove mark, an 18 pt disc with a bar 4 pt in from its leading top corner
(on the display and sound tiles, whose title starts there, from the trailing one). Cancel (62 x 24) and Done (49 x 24)
take the place of Edit Controls, 12 pt apart and centred on the grid, Done under Edit Controls' key so the focus stays
on it. The controls not shown are offered under them: round 36 pt chips with the control's symbol and a small disc with
a cross, 12 pt apart in centred rows of six, 16 pt below the capsules, in the margin under the window that its shadow
takes (the window's input region grows to hold them). A press on a mark removes its tile and the others close up; a
press on a chip adds its control after the others; a drag of a tile (3 pt of movement holds it) carries it under the
pointer, drawn over the others, while they make room: the order puts the held tile in the place nearest to where it is
held, and letting go drops it there. Nothing moves on its own: a tile is where the order puts it at once, so there is
no motion for Reduce Motion to take away. Done stores the edit and shows it; Cancel, Escape, a press outside the panel
and every other way the panel goes (a toggle, the lock, its output going) drop it, and nothing is written before Done.
A press between the tiles does nothing while editing, so a press that misses loses no edit. The marks are white with a
dark sign on the dark tiles and dark with a white sign on the light tiles of Reduce Transparency in a light session;
the chips and capsules are tiles like the others, glass in the one container, opaque with Reduce Transparency and
rimmed with Increase Contrast.

**The order.** `arrangement::Arrangement` is the list of the shown modules. The tiles are laid on the grid in that
order, each in the first free place that holds it, reading the grid row by row in steps of one circle (four across),
so a small tile fills a place a larger one before it left open; every module in reading order gives the grid as it
always was. Edit Controls follows 16 pt under the last row. A move that would need a seventh row is refused: a drag
then leaves the tile's place as it was, and an arrow key passes on to the next place that fits. The layout is stored per
person as the modules' names in order (`controls` in `control-panel.conf`, `cupertino-defaults`); nothing stored is
every module in reading order. Reading is tolerant: a name no module has and a repeated name are skipped, a module not
named is not shown, an empty value shows none, and an order that does not fit the grid (written by hand) is shown in
reading order. The panel reads the file as it opens and with every reading of the backends while open, so a layout
changed by another process shows within a second; while its controls are edited it shows the edit.

**Keyboard and assistive technology.** Every enabled control is a Tab stop and names itself: badges and circles are
switches with their state (Lock Screen is a button), the openers are buttons that say whether their view is open, levels
are sliders (0 to 1, a sixteenth per step), list rows are list items with their place and the current one selected. Space
and Return press; the arrow keys move the focus to the nearest control in their direction (Left and Right step a level
instead); Home and End take a level to its ends. Each tile is a group named for its module.

While the controls are edited, each tile is one control named for its module whose value is its place in the order (a
slider from 1 to the number of tiles, described as "Place 2 of 11. The arrow keys move it, Delete removes it."): Left
and Up move it a place earlier, Right and Down a place later (the Decrement and Increment actions), Delete or Backspace
removes it. The focus stays on a moved tile, goes to the tile that took a removed one's place (else the one before,
else Done) and to an added tile. Each remove mark ("Remove Wi-Fi") and each chip ("Add Wi-Fi") is a button of its own,
and Cancel and Done are buttons.

**Session.** A hidden panel has no window. A toggle opens the panel and the catcher, with no detail view open. A dismissal
closes the catcher at once and fades the panel, whose window closes when the fade ends. When the compositor presents
shell surfaces (`cupertino_layers_v1` version 2) it runs both fades on its clock as the card's presentation
(`lifecycle::OPEN_CURVE`, the same critically damped rise as a spring, and `lifecycle::CLOSE_CURVE`, the fall as a cubic
Bezier within a two-thousandth of it): the card draws at full opacity and commits no buffer while it fades. Otherwise
the card draws each step of the fade itself. While the panel is shown its window asks for **exclusive** keyboard interactivity, so it is the key window as soon as it
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

**Dismissal.** A left or right press outside the tiles, losing the keyboard focus and Escape (while the controls are
edited Escape cancels the edit and the panel stays). A detail view's settings row opens the settings application and
dismisses the panel.

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

The backends below also speak `org.cupertino.Network1` or NetworkManager, `org.cupertino.Bluetooth1` or BlueZ,
`org.cupertino.Power1`, `org.freedesktop.UPower.KbdBacklight` and `org.freedesktop.login1` on the system bus, and
`org.cupertino.NotificationCenter1`, `org.cupertino.Osd1` and `org.mpris.MediaPlayer2.*` on the session bus.

### Backends

A failure disables the tile with the reason and logs it on stderr as `cupertino-controlcenter: <what>: <reason>`. There is no
silent default. The panel reads again every second while open. It logs a failure when it starts or its reason changes, and
`<what>: available again` when it recovers, not on every retry. Each connection is made on first use and again after a
failure. Properties are read from the service every time, never from a cache.

| Module | Real backend | Fake (`--fake-backends`, tests) |
|---|---|---|
| Wi-Fi | `radios::RadioWifi`: the network service found on the system bus at each reading (`cupertino-radios`: `org.cupertino.Network1`, else NetworkManager), its radio switch (read, write) and the network of the first Wi-Fi device while it is connected; no service, no Wi-Fi device or a service that does not answer is a disabled tile with the reason; the view asks for a scan, lists the networks in range and joins with no passphrase, which only a network the service knows or an open one accepts (a new secured network needs the passphrase the Wi-Fi settings ask for, so its row is not pressable) | `MemoryWifi` |
| Bluetooth | `radios::RadioBluetooth`: the Bluetooth service found at each reading (`org.cupertino.Bluetooth1`, else BlueZ), its adapter (none, or a service that does not answer, is a disabled tile) and power switch (read, write); the view lists the paired devices by name and connects or disconnects one without waiting, since a service may answer only when the link is up | `MemoryBluetooth` |
| Microphone | `backend::PipewireMicrophone`: the mute of the default PipeWire source (`cupertino-pipewire`); on means it records | `MemorySwitch` |
| Now playing | `system::MprisPlayer`: the `org.mpris.MediaPlayer2.*` players on the session bus, the playing one first, else the first with a title; `Metadata` (`xesam:title`, `xesam:artist`), `PlaybackStatus`, `CanGoPrevious`, `CanGoNext`, `CanPlay`, `CanPause`; `Previous`, `PlayPause`, `Next`. Each call to a player waits at most 200 ms (`system::PLAYER_TIMEOUT`), so a player that hangs is passed over. No player is "Not Playing", not a failure | `MemoryMedia` |
| Dock hiding | `backend::DockHiding`: the `autohide` key of the dock's settings file (`cupertino_defaults::dock`), which the dock follows | `MemorySwitch` |
| Lock screen | `system::LogindLock`: `GetSessionByPID` of this process, then `Lock` on that session | `RecordingLock` |
| Mute | the volume backend's mute (see Volume); unmuting a level of 0 restores 0.25 | the volume's |
| Keyboard brightness | `system::UpowerKeyboard`: `org.freedesktop.UPower.KbdBacklight` (`GetBrightness`, `GetMaxBrightness`, `SetBrightness`, off or the maximum); a machine without one shows the tile disabled | `MemorySwitch` |
| Focus mode | `org.cupertino.NotificationCenter1` on the session bus (`cupertino-notifications`): the `DoNotDisturb` property and `SetDoNotDisturb(b)`; on, the capsule reads "Do Not Disturb" | `MemorySwitch` |
| Display brightness | `system::PowerdBrightness`: `org.cupertino.Power1` (`GetBrightness`, `GetMaxBrightness`, `SetBrightness`), raw value mapped linearly onto 0 to 1, never below raw 1 | `MemoryBrightness` |
| Volume | `backend::SessionVolume`: the default PipeWire sink through `cupertino-pipewire` (`wpctl`), the level `wpctl get-volume` reports, the sinks as outputs and `set-default` to choose one; when no PipeWire daemon answers a read, `backend::AlsaVolume`: `cupertino-alsa` master selection (`Master::open_default`, the same as the overlay), level and mute switch, with the card's master as the only output; opened on first use and again after a failure | `MemoryVolume` |
| Settings rows | `backend::SettingsLauncher`: starts `cupertino-settings --pane <page>` (`wifi`, `bluetooth`, `displays`, `sound`) | `RecordingLauncher` |
| Controls shown | `backend::StoredLayout`: the `controls` key of `control-panel.conf` (`cupertino_defaults::control_panel`, `$CUPERTINO_CONTROL_PANEL_CONF`), read at every reading and written by Done; a file that cannot be read keeps the controls shown and is logged, one that cannot be written drops the edit and is logged | `MemoryLayout` |
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
| Tiles | with every control shown: Wi-Fi, Bluetooth and the microphone capsules down the left column; now playing (140 x 140) beside the first two; dock hiding and lock screen circles beside the microphone; mute and keyboard brightness circles, then the focus capsule; display, then sound across both columns (`layout::frame`); otherwise the shown ones in their order, each in the first free place (`arrangement`) |
| Editing | content at 0.55; remove mark 18 pt, 4 pt in; Cancel 62 x 24 and Done 49 x 24, 12 pt apart; chips 36 pt, 12 pt apart, six in a row, 16 pt below the capsules, their mark 14 pt; a drag holds a tile after 3 pt (`layout::editing`) |
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

- Unplugging a real display with the panel open has not been tried yet; the hardware check belongs to the display hot-plug acceptance (#22).
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
| `arrangement` | `Arrangement`: the shown modules in order (`from_ids`, `ids`, `shown`, `available`), `add`, `remove`, `move_to`, `step`, `move_near`, and where the order puts each tile (`places`, `frame`, `rows`, `height`) |
| `layout` | the grid in points (`ROW`, `COLUMN`, `GAP`, `WINDOW`, `INSET`), each module's `frame` (with every module shown) and `outline`, the Edit Controls frame under the tiles, the parts of the editing state (`editing`), and the detail view's parts, `detail_frame` and `morph` |
| `lifecycle` | `Lifecycle`, `Phase`, `FADE_SECONDS` with injected time |
| `motion` | `spring`, `Expansion` (a detail view opening or closing, reversible, with Reduce Motion) and `visibility` with injected time |
| `backend` | the traits (`Wifi`, `Bluetooth`, `Switch`, `Brightness`, `Volume`, `MediaPlayer`, `SessionLock`, `Launcher`, `OverlayNotifier`, `LayoutStore`), `SessionVolume`, `AlsaVolume`, `PipewireMicrophone`, `DockHiding`, `StoredLayout`, `SettingsLauncher`, `Unavailable`, the `Memory*` stand-ins and `Recording` |
| `radios` (feature `dbus`) | `Radios` (the system bus the radios are read on, `on_change`, `waiting`), `Radio`, `RadioWifi`, `RadioBluetooth`, `FRESH_WAIT`, `SWITCH_WAIT` |
| `system` (feature `dbus`) | `NotificationFocusMode`, `PowerdBrightness`, `UpowerKeyboard`, `LogindLock`, `MprisPlayer`, `SessionOverlay` |
| `controller` | `Controller`: lifecycle, readings, presses, the open detail view and its list, joins, connections, outputs, media commands, settings, the controls shown and their editing (`edit_controls`, `edit_remove`, `edit_add`, `edit_step`, `edit_move_near`, `finish_edit`, `cancel_edit`), throttled level writes, refresh timer; `Backends` |
| `control` (feature `dbus`) | the `org.cupertino.ControlPanel1` interface (`serve`) |
| `view` (feature `app`) | the CupertinoKit windows: `arranged` (the tiles of an `Arrangement` at a `Presentation`; `panel` with every module), `Editing` and `Held` (the editing state and the tile the pointer holds), `catcher`, `Input` (what the user did), `Via`, `focus_key`, `edit_key`, `APP_ID`, `CATCHER_APP_ID`, `TITLE` |

The Wi-Fi and Bluetooth backends share one connection to the system bus (`radios::Radios`) and follow the services'
change signals on it (`cupertino_radios::watch`): a signal, or a service starting or stopping, has a shown panel read
that radio again at once (`Controller::radios_changed`), and no other backend, so a radio switched in Settings or a
device that connects shows without waiting for the periodic reading. A burst of signals asks for one reading, and a
Bluetooth device's signal strength while the adapter discovers asks for none.

The panel's thread never waits for a service. The connection is made, and every reading and switch carried out, on
threads of their own, and no lock is held across a call on the bus. A reading the panel asks for is waited for
`FRESH_WAIT` (100 ms), a switch's answer for `SWITCH_WAIT` (250 ms), which services that answer as they should need
less than. A reading that takes longer leaves the tile with its last reading (unavailable, "waiting for the network
service", while there is none) and has the panel read again when it arrives; one reading of a kind is under way at a
time, and a request that comes meanwhile has it taken once more when it ends. A service that does not answer fails
its reading after the bounded wait of `cupertino-radios` (3 s), and its tile is unavailable with "the network service
does not answer" until a reading succeeds.
