# cupertino-controlcenter

The control panel of the Cupertino desktop. It opens at the top right, below the menu bar: separate glass tiles for Wi-Fi,
Bluetooth, the microphone, now playing, dock hiding, the screen lock, mute, the keyboard backlight and focus mode, wide
display and sound tiles with their levels, and an Edit Controls capsule under them, which lets the person choose which
of these controls show and in which order. A press or a level drag acts on the system services. Wi-Fi, Bluetooth, display and sound open a detail view with their networks, devices, keyboard backlight
or outputs. A click outside the tiles closes the panel.

The crate is a library: one component, `ControlCenter`, which the shell host (`cupertino-shell`) runs on a thread of
its own as `controlcenter` of the server's component table. The model (`model`, `arrangement`, `layout`, `lifecycle`,
`motion`, `controller`) is pure, with injected time; the effects (`effects`) and the bus interface (`control`) use
zbus's async API on the component's UI loop and the toolkit's I/O pool; the windows (`view`) are drawn by CupertinoKit.

## Running

The server lists `controlcenter` to the shell host, which runs it; nothing starts it on its own. `compositor.json` names it
among the `components` of its `shell` section (`docs/architecture/shell-host.md`), and `alone` runs it in a host of its
own (`cupertino-shell --only controlcenter`). A development build of the host makes it panic on request
(`cupertino shell crash controlcenter --at paint|dispatch|helper`). The server's `control-panel` key binding and hot-corner
action call `Toggle`; the menu bar's status item opens it over the host bus (`TogglePanel`).

```
cargo run -p cupertino-controlcenter --example controlcenter_capture -- controlcenter/panel --sheet   # every view, light and dark
```

The panel follows the session's appearance: light glass with dark text and symbols in the light appearance, dark glass
with light ones in the dark appearance. It follows the display options as they change: Increase
Contrast takes the high-contrast catalog and makes each tile a solid grey with a rim (mid grey with a darker rim in the
dark appearance, light grey with a mid grey rim in the light one); Reduce Transparency makes
the tiles opaque on one opaque panel; Reduce Motion replaces the growth of a detail view with a cross-fade. Icons are the
symbols of the asset directory (`symbol.*`), loaded by the kit.

## What a restart loses

A failed component is started again alone by its host (D16, D21). The control panel keeps nothing of its own across a
restart but the controls the person chose, which are stored in `control-panel.conf` as Done is pressed:

- an open panel goes, with the detail view open in it and an edit of the controls not yet done; the next toggle opens
  the panel on its grid;
- a level being dragged stops where its last write left it;
- the readings it holds, which it takes again as it starts, the pictures of the shadow's bands, which it draws again as
  it next opens, and its bus connections;
- a `Toggle` of `org.cupertino.ControlPanel1` made while the name has no owner (from the failure until the new instance
  owns it) is refused by the bus, so the key press or the click does nothing.

What the tiles show lives in the services (the radios, `cupertino-audiod`, the power daemon, UPower, the media players,
the dock's settings and do not disturb in the notifications), so a restart changes none of it.

## How it works

**Windows.** `view::panel` is one transparent surface (`layout::SURFACE`) with no background of its own: the 308 x 500 pt window (`layout::WINDOW`) inside a transparent margin of 122 pt left and below it (24 pt above, to the output's edge on the right) where the soft shadow around the tiles is drawn, a darkening of about 9 % at the tiles' edge that fades out over 130 pt
to the sides and below, and above the tiles within the 20 pt up to the menu bar's edge, so nothing of it reaches into the
bar and it shows no edge where the bar begins. Between the tiles it keeps the 9 %; under a fading tile it lies at the
share the tile's glass lets through, so it follows a detail view's motion without a jump. The shadow's 26 bands are drawn once, while the panel rests, into a picture of the window's device pixels that the component keeps (two at most: with no view and with one open) and every later frame and opening lays over one texel per pixel, within 2/255 of the bands filled in place; a detail view's moving outline is filled in place. A whole opening frame at 1.5 takes some 3 ms on the new renderer each opening makes, against 14 ms with the bands filled; the picture is drawn on the toolkit's CPU pool (`BandsWanted::draw`), at the scale of the card's output, or at each output's while the server places the card. Only the window's rectangle takes input, and nothing while the panel fades out.
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
(`WindowBackground::INPUT_ONLY`: one transparent pixel stretched over the output) that takes a left or right press anywhere on it. The component's `Program::surface` gives the card (`view::PANEL_KEY`) an overlay
layer surface anchored top and right, with an exclusive zone of -1 and margins that put the window 6 pt from the right edge
and 12 pt below the bar, so the tiles sit 14 pt from the edge and 20 pt below the bar, 44 pt from the top of the output. The bar reserves no layer zone, so
the top margin adds the theme's menu bar height, which the component reads from the theme on the I/O pool as it starts (`cupertino_kit_shell::design::menu_bar`); a toggle before it is known opens the panel once it is. The catcher (`view::CATCHER_KEY`) gets a top layer surface just below
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

**Detail views.** The tile grows into the view on the catalogue's critically damped spring of the control panel's detail views
(`SpringPreset::CONTROL_PANEL_DETAIL`: response 0.35 s, settled by 0.6 s; `motion::spring`): its frame moves from its cell to the top of the grid, 292 pt wide and as high as its list (at most the
grid's 444 pt), its corners go to 32 pt, the other tiles and Edit Controls fade out over the first quarter of the motion
(the glass lies under all of the window's content, so the growing tile cannot hide what it passes over), and the tile's
content gives way to the view's at halfway. Closing runs the same way back, from wherever the motion is. With Reduce
Motion the tile fades out in place and the view fades in at its place over 0.2 s (`Timing::CONTROL_PANEL_DETAIL_FADE`, linear; `motion::fade`). The view shows the module's switch or level, a heading, the list
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
closes the catcher at once and fades the panel, whose window closes when the fade ends. The compositor runs both fades on
its clock as the card's presentation (`WindowRequest::Present`), on the catalogue's `SpringPreset::CONTROL_PANEL_OPEN`
(`lifecycle::open`, a critically damped rise) and `Timing::CONTROL_PANEL_CLOSE` (`lifecycle::close`, quick at first and
slowing towards the end): the card draws at full opacity and commits no buffer while it fades. With Reduce Motion it
appears and goes at once. While the panel is shown the card's surface asks for **exclusive** keyboard interactivity, so it
is the key window as soon as it opens, from the menu bar, a shortcut or a hot corner. It asks for it again when a toggle
during the fade shows the panel again, and for none from the start of the fade. The card's content holds the focus while
the panel is shown (`view::PANEL_ROOT`), so another exclusive surface (the launcher) that takes the keyboard from it
dismisses the panel. When the session locks, a shown panel starts closing too.

**Outputs.** The panel and the catcher are always on the same output. A `TogglePanel` publication of the host bus names
the output to open on; a `Toggle` call names none, and the server opens both surfaces on its active output, the one the
person works on (where the pointer last moved or the keyboard last went; the menu bar clicked is on it), in the same
pass. A toggle during the fade shows the panel again on the fading card's output. When that output goes (a display
unplugged), the panel is **dismissed**, not moved: the server closes its surfaces, or the output leaves the server's list,
and either hides the panel at once, without the fade, dropping a drag in progress (`Controller::vanish`). The next toggle
opens the panel on an output that exists. The component does not need an output to start or to keep running. Moving the
open panel to the remaining output was not chosen: it would need new surfaces placed while the person's attention is on
the display change, and a dismissal is what any other loss of the panel's place does (a lock, another exclusive panel).

**Dismissal.** A left or right press outside the tiles, losing the keyboard focus and Escape (while the controls are
edited Escape cancels the edit and the panel stays). A detail view's settings row opens the settings application and
dismisses the panel.

**Menu bar.** A click on the control panel slot publishes `TogglePanel` on the host bus with its output. The panel anchors to the top right and does not take the
slot's position.

## Protocols / D-Bus it speaks

The component owns `org.cupertino.ControlPanel1` at `/org/cupertino/ControlPanel1` on its own session bus connection
(D20). A second panel cannot take the name, and the component stops with failure. The interface lives in `control`
(`BUS_NAME`, `OBJECT_PATH`, `serve`, whose `Served` announces the properties); the component tells it, and the host
bus (`PanelShown`), whether the panel is shown and on which output whenever that changes. A component that stops drops
the name, which the menu bar takes as not shown.

| Method | Effect |
|---|---|
| `Shown` property (`b`) | Whether the panel is shown: from the toggle that opens it until its dismissal starts the fade, whatever dismissed it. `PropertiesChanged` announces every change. The menu bar highlights its control panel item while it is true. |
| `Output` property (`s`) | The connector name (`wl_output.name`) of the output the shown panel is on, so the menu bar highlights the item on that output's bar: the output a `TogglePanel` named, else the server's only output; empty while hidden, and while the server places the panel on one of several outputs. |
| `Toggle` | Opens the panel, or starts closing it when it is open. Only the server may call it: the connection that owns `org.cupertino.Session1` (D20), from which the server sends it; the check asks the bus daemon who owns that name and reads nothing else. Any other caller, another connection of the server's process and the components of the shell host included, is refused with `org.cupertino.Error.UntrustedCaller` (`cupertino_peer::CallerError`) and nothing changes. |

A call from a shell (`gdbus call ... --method org.cupertino.ControlPanel1.Toggle`) is refused for that reason; use the
`control-panel` key binding, or pass `--action 2:control-panel` to a scripted server.

The effects below also speak `org.cupertino.Network1` or NetworkManager, `org.cupertino.Bluetooth1` or BlueZ,
`org.cupertino.Power1`, `org.freedesktop.UPower.KbdBacklight` and `org.freedesktop.login1` on the system bus, and
`org.cupertino.Audio1` and `org.mpris.MediaPlayer2.*` on the session bus. On the host bus the component follows
`TogglePanel` for `ControlPanel` and `DoNotDisturbChanged`, and publishes `PanelShown`, `SetDoNotDisturb` and `ShowOsd`.

### Backends

A failure disables the tile with the reason and logs it on stderr as `cupertino-controlcenter: <what>: <reason>`. There is no
silent default. The component reads every source as it starts, as the panel opens, and again every second while it is
open, since the sound service sends no change signal. It logs a failure when it starts or its reason changes, and
`<what>: available again` when it recovers, not on every retry. One reading of a source is under way at a time; a
reading that started before a change of its source, or that is asked for while the change is under way, waits for the
change to end, so a tile never shows what the service had before. The system bus connection is made as the component
starts and again on the next use after it closed. Properties are read from the service every time, never from a cache.

| Module | Through |
|---|---|
| Wi-Fi | the network service found on the system bus at each reading (`cupertino-radios`: `org.cupertino.Network1`, else NetworkManager), its radio switch (read, write) and the network of the first Wi-Fi device while it is connected; no service, no Wi-Fi device or a service that does not answer is a disabled tile with the reason; the view asks for a scan, lists the networks in range and joins with no passphrase, which only a network the service knows or an open one accepts (a new secured network needs the passphrase the Wi-Fi settings ask for, so its row is not pressable) |
| Bluetooth | the Bluetooth service found at each reading (`org.cupertino.Bluetooth1`, else BlueZ), its adapter (none, or a service that does not answer, is a disabled tile) and power switch (read, write); the view lists the paired devices by name and connects or disconnects one; the device's row shows the outcome when the call ends, which BlueZ answers only once the link is up or has failed |
| Microphone | `org.cupertino.Audio1`: `GetVolume("input")`, on when the default source is not muted; a press sets its mute with `SetVolume` at the level it has |
| Now playing | the `org.mpris.MediaPlayer2.*` players on the session bus (`ListNames`, then `GetAll` of each player's `org.mpris.MediaPlayer2.Player`, all at once), the playing one first, else the first by name with a title; `Metadata` (`xesam:title`, `xesam:artist`), `PlaybackStatus`, `CanGoPrevious`, `CanGoNext`, `CanPlay`, `CanPause`; `Previous`, `PlayPause`, `Next`. A reading or a button ends after 400 ms (`effects::PLAYER_DEADLINE`), so a player that hangs is passed over. No player is "Not Playing", not a failure |
| Dock hiding | the `autohide` key of the dock's settings file (`cupertino_defaults::dock`), which the dock follows; read and written on the I/O pool |
| Lock screen | logind: `GetSessionByPID` of the shell host, then `Lock` on that session |
| Mute | the volume's mute (see Volume); unmuting a level of 0 restores 0.25 |
| Keyboard brightness | `org.freedesktop.UPower.KbdBacklight` (`GetBrightness`, `GetMaxBrightness`, `SetBrightness`, off or the maximum); a machine without one shows the tile disabled |
| Focus mode | do not disturb of the notifications component, on the host bus: the retained `DoNotDisturbChanged` shows it, and a press publishes `SetDoNotDisturb` with no duration; on, the capsule reads "Do Not Disturb"; while the notifications component is stopped the capsule is disabled |
| Display brightness | `org.cupertino.Power1` (`GetBrightness`, `GetMaxBrightness`, `SetBrightness`), raw value mapped linearly onto 0 to 1, never below raw 1 |
| Volume | `org.cupertino.Audio1` of `cupertino-audiod` (`GetVolume("output")`, `SetVolume`, which answers the level reached); the Sound view's outputs are `ListDevices("output")` and `SetDefault` chooses one. The service picks the PipeWire default sink, else the ALSA master |
| Settings rows | starts `cupertino-settings --pane <page>` (`wifi`, `bluetooth`, `displays`, `sound`) through the host, which reaps it |
| Controls shown | the `controls` key of `control-panel.conf` (`cupertino_defaults::control_panel`, `$CUPERTINO_CONTROL_PANEL_CONF`), read at every reading and written by Done, on the I/O pool; a file that cannot be read keeps the controls shown and is logged, one that cannot be written drops the edit and is logged |
| Overlay | `ShowOsd` on the host bus after a slider drag ends (`Volume`, `Mute` for a muted output, `Brightness`) |

The modules are the ones a service backs. Sharing and screen mirroring have no service, and the session refuses an
appearance change from anyone but the settings application, so the panel has no tiles for them; the battery lives in
the menu bar. The panel has no power controls: restart, shut down, sleep and log out are the session menu's.

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
| Detail motion | `SpringPreset::CONTROL_PANEL_DETAIL`, critically damped, response 0.35 s, settled at 0.6 s; content hands over at halfway; Reduce Motion: `Timing::CONTROL_PANEL_DETAIL_FADE`, 0.2 s linear cross-fade (`motion`) |
| Fade | in on `SpringPreset::CONTROL_PANEL_OPEN`, a critically damped curve, with no scale or movement; out on `Timing::CONTROL_PANEL_CLOSE` over 0.18 s; with Reduce Motion both are instant; input stops at the start of the fade-out |
| Lifecycle | hidden, shown, hiding, hidden; a toggle while hiding reopens; the output going hides at once |
| Volume writes | linear 0 to 1, mute below 0.001, throttle 16.7 ms with the latest value kept |
| Level keys | a sixteenth per arrow step |
| Display floor | raw value 1 (a slider never turns the screen off) (`effects`) |
| Refresh while open | 1 s (`controller::REFRESH_INTERVAL`) |

## Tests

`tests/` drives the public interface without a service: `controller.rs` the state machine (what an opening reads, one
reading of a source at a time, a reading from before a change put off until the change ended, refused changes, the
throttled writes of a dragged level and the overlay on release, presses, the detail views' lists, editing and the
stored layout, the refresh and the fade-out's end), `lifecycle.rs` the fades and the detail motion on the catalogue's
curves and their reversals and `arrangement.rs` the grid's order.
`crates/cupertino-session-tests/tests/shell-controlcenter.rs` runs the component in `cupertino-shell`'s host against
the real server, headless, on a bus of its own: the component owns `org.cupertino.ControlPanel1` on a connection of its
own, a `Toggle` from another connection of the server's process is refused, the server's toggle opens the panel on the
sole output where it draws, and fifty panics injected as it paints, as its loop
dispatches and on a helper thread each restart it alone and closed while the dock keeps drawing and the host's memory
stays flat. `examples/controlcenter_capture.rs` draws the panel and its detail views for look checks.

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
| `lifecycle` | `Lifecycle`, `Phase`, the fades `open` and `close`, with injected time |
| `motion` | `spring`, `fade`, `Expansion` (a detail view opening or closing, reversible, with Reduce Motion) and `visibility` with injected time |
| `controller` | `Controller`: lifecycle, readings, presses, the open detail view and its list, joins, connections, outputs, media commands, settings, the controls shown and their editing (`edit_controls`, `edit_remove`, `edit_add`, `edit_step`, `edit_move_near`, `finish_edit`, `cancel_edit`), throttled level writes and the refresh, as the `Effect`s (`Read` of a `Source`, `Change`) it asks for and the outcomes it is given (`read`, `changed`) |
| `effects` (private) | what the effects do on the buses and the I/O pool |
| `control` | the `org.cupertino.ControlPanel1` interface (`serve`, `Served`, `Presence`, `Call`, `judge`) |
| `view` | the CupertinoKit windows: `arranged` (the tiles of an `Arrangement` at a `Presentation`; `panel` with every module), `bands_wanted` and `BandPictures` (the pictures of the shadow's bands), `Editing` and `Held` (the editing state and the tile the pointer holds), `catcher`, `Input` (what the user did), `Via`, `focus_key`, `edit_key`, `PANEL_KEY`, `CATCHER_KEY`, `PANEL_ROOT`, `TITLE` |
| `component` (private) | `ControlCenter`, the component |

The Wi-Fi and Bluetooth readings follow the services' change signals on the component's system bus connection
(`cupertino_radios::watch`): a signal, or a service starting or stopping, has a shown panel read that radio again at once
(`Controller::radios_changed`), and no other source, so a radio switched in Settings or a device that connects shows
without waiting for the next reading. A burst of signals asks for one reading, and a Bluetooth device's signal strength
while the adapter discovers asks for none.

The UI thread never waits for a service. The bus calls are futures of the UI loop; the radios' calls, which wait for
their answers, and the settings files run on the toolkit's I/O pool. A service that does not answer fails its reading
after the bounded wait of `cupertino-radios` (3 s), and its tile is unavailable with "the network service does not
answer" until a reading succeeds.
