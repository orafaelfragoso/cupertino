# cupertino-osd

The on-screen displays of the Cupertino desktop. They stand in for the design's overlay helper: a 200 x 200 pt window
with a glyph and a 16-segment meter for volume, mute, brightness, keyboard backlight and microphone. It appears on a key
press, holds, and fades out.

The crate is a library: one component, `Osd`, which the shell host (`cupertino-shell`) runs on a thread of its own as
`osd` of the server's component table. The model (`model`, `sender`, `controller`) is pure; the effects (`effects`)
and the bus interface (`control`) use zbus's async API on the component's UI loop; the window (`view`, `layout`) is
drawn by CupertinoKit. Window size, corner radius, position, meter geometry, the partial-segment rule and the fade follow
the analysis. The items marked approximate below are authored and replaceable.

## Running

The server lists `osd` to the shell host, which runs it; nothing starts it on its own. `compositor.json` names it among
the `components` of its `shell` section (`docs/architecture/shell-host.md`), and `alone` runs it in a host of its own
(`cupertino-shell --only osd`). A development build of the host makes it panic on request
(`cupertino shell crash osd --at paint|dispatch|helper`).

To see it without touching the machine's levels, call `Show` (below): it changes nothing.

```
cargo run -p cupertino-osd --example osd_capture -- osd/overlay --sheet   # the overlay of each kind, light and dark
```

## What a restart loses

A failed component is started again alone by its host (D16, D21). The osd keeps nothing of its own across a restart:

- the overlay on screen goes, and shows again at the next press;
- the presses accepted and not yet carried out are dropped, and a call of `org.cupertino.Osd1` made while the name has
  no owner (from the failure until the new instance owns it) is refused by the bus: the server's key press does
  nothing;
- its bus connections and the hold or fade under way.

The levels themselves live in `cupertino-audiod`, the power daemon and UPower, so a restart changes none of them.

## How it works

**Surface.** One overlay layer surface of 200 x 200 pt anchored to the bottom only, so it is centred horizontally, with a
bottom margin of 140 pt, no keyboard interactivity and an empty input region. The window is a CupertinoKit window
(`view::window`); the component's `Program::surface` gives it that layer surface (`cupertino-kit-shell`), on the
server's first output; with no output there is no window, and a window the server closed is made anew.
Its background is the HUD surface (`Surface::Hud`, the `LoginWindowControl` material) with the window's radius, and its
alpha follows the fade. The glyph (an `Icon`) and the meter draw over it as one layer at the same alpha. While hidden the
surface stays mapped, draws nothing and has no material. Glyphs are template symbols of the asset directory
(`symbol.speaker.slash`, `symbol.speaker.low`, `symbol.speaker`, `symbol.speaker.high`, `symbol.sun`, `symbol.keyboard-brightness` and `.off`, `symbol.mic` and `.slash`; `model::ICON_NAMES`),
in the control text colour (the label colour). A symbol no asset pack provides draws as Kit's placeholder.

**Session.** CupertinoKit follows the session's appearance and display options. With Reduce Transparency or Increase
Contrast the HUD surface takes no material and paints the theme's opaque window background in its shape. The overlay
takes no input and the server hides it while the session is locked, so a lock needs no reaction.

**Accessibility.** The window is named after the overlay ("Volume", "Volume muted", "Brightness", "Keyboard brightness",
"Microphone", "Microphone muted"), and the meter is a progress indicator with the level from 0 to 1, named after the
overlay and, unless it shows a mute, the level in whole percent ("Volume 50%"). The meter is a polite live region, so a
screen reader says each new level without the overlay taking the focus: the bridge announces a live node only as it
appears or its name changes, which is why the name holds the level.

**Timing.** The calls `org.cupertino.Osd1` accepts arrive on a channel the component's loop listens to, and the
commands are carried out one at a time, in order. A timer of the UI loop (`Command::After`) ends the hold; through the
fade a pacer in the window asks for each of the display's frames, and the alpha is read from the model's clock. A new
show restarts the hold, and the end of an earlier one is ignored.

**Effects.** The output volume and mute and the microphone go through `org.cupertino.Audio1` of `cupertino-audiod`
on the session bus (`GetVolume`, then `SetVolume` with the level and mute the press asks for; the service answers
the level it reached), so the overlay shows what the service reports; the service, bus-activated, picks the PipeWire
default sink or source, else the ALSA master. Display brightness goes through `org.cupertino.Power1` and the keyboard
backlight through `org.freedesktop.UPower.KbdBacklight` on the system bus, connected on the first backlight press
and again after the connection closed. A failure (no service, no sound device, no backlight) shows nothing and is logged
on stderr with its reason.

- Levels map linearly onto the raw range of each backlight (no fixed curve to copy).
- **Platform deviation:** the display backlight never goes below raw value 1, although the design requests down to 0.0.
  A Linux backlight at raw 0 can blank the panel. The keyboard backlight reaches 0.
- A change of level always moves at least one raw unit, so a coarse raw range cannot swallow a press.
- A master without a mute switch counts as muted exactly at level 0 (the service's rule). The volume keys work, and
  the mute key fails with its reason at any other level.

## Protocols / D-Bus it speaks

The component owns `org.cupertino.Osd1` at `/org/cupertino/Osd1` on its own session bus connection (`bus::session`
of `cupertino-kit-shell`, announced to the server, D20). When another connection owns the name the component ends,
and its host reports a failure. The interface lives in `control` (`BUS_NAME`, `OBJECT_PATH`, `serve`).

Callers: `VolumeStep`, `ToggleMute`, `ToggleMicrophoneMute`, `BrightnessStep` and `KeyboardBrightnessStep` change
levels, so only the server may call them: the connection that owns `org.cupertino.Session1` (`bus::caller`),
from which the server sends them. Any other caller, another connection of the server's process included, gets `org.freedesktop.DBus.Error.AccessDenied` and nothing changes. `Show` only draws the overlay, so it
stays open to every caller (the control panel uses it). A component may also publish `ShowOsd` on the host bus, which
shows the same way.

| Method | Effect |
|---|---|
| `VolumeStep(i32 direction, bool fine)` | Raises (`direction` > 0) or lowers the volume one press (a level of 0 mutes, a level above 0 unmutes) and shows the level. |
| `ToggleMute` | Mutes or unmutes the output (at volume 0 the result is always muted). Shows the muted glyph with an empty meter, or the level. |
| `ToggleMicrophoneMute` | Mutes or unmutes the default source. Shows the slashed microphone with an empty meter, or the microphone at the input level. |
| `BrightnessStep(i32 direction, bool fine)` | Display brightness one press, through `org.cupertino.Power1` on the system bus. |
| `KeyboardBrightnessStep(i32 direction, bool fine)` | Keyboard backlight one press, through `org.freedesktop.UPower.KbdBacklight` on the system bus. |
| `Show(s kind, d level)` | Shows `volume`, `mute`, `brightness`, `keyboard-brightness`, `microphone` or `microphone-mute` at `level` (0 to 1) without changing anything, holding 4 s. For the control panel, which changes levels itself. |

A direction of 0, an unknown kind or a level outside 0 to 1 is answered with `org.freedesktop.DBus.Error.InvalidArgs` and
shows nothing.

```
gdbus call --session --dest org.cupertino.Osd1 --object-path /org/cupertino/Osd1 --method org.cupertino.Osd1.Show volume 0.5
```

The server calls the interface for these keys. Shift+Alt held makes the press fine.

| Key | Call |
|---|---|
| `XF86AudioRaiseVolume` | `VolumeStep(1, fine)` |
| `XF86AudioLowerVolume` | `VolumeStep(-1, fine)` |
| `XF86AudioMute` | `ToggleMute` |
| `XF86AudioMicMute` | `ToggleMicrophoneMute` |
| `XF86MonBrightnessUp` | `BrightnessStep(1, fine)` |
| `XF86MonBrightnessDown` | `BrightnessStep(-1, fine)` |
| `XF86KbdBrightnessUp` | `KeyboardBrightnessStep(1, fine)` |
| `XF86KbdBrightnessDown` | `KeyboardBrightnessStep(-1, fine)` |

## Values applied

| Item | Value |
|---|---|
| Window | 200 x 200 pt, horizontally centred, 140 pt above the bottom edge, no shadow, ignores the mouse |
| Corner radius | 18 pt |
| Meter | 16 segments of 9 x 6 pt at pitch 10, in a 161 x 8 container at (20, 20) bottom-left, so segment `i` is at x = 20 + 10 i + 1, y = 173 top-left |
| Fill | `v = level * 16`; `floor(v)` full segments; the next one `(int)(frac * 9 + 0.5) points wide (0 is empty, 9 is full) |
| Fade (`model::fade`) | the catalogue's `Timing::OSD_FADE`, constant speed over 0.7 s: alpha `1 - t / 0.7`, after a hold the sender supplies; a new show restarts at alpha 1 |
| Step of one press (`sender::COARSE_STEPS`, `FINE_STEPS`, `step_level`) | snap `q = floor((v + 1/128) * 64) / 64`, then up `floor(16 (q + 1/16)) / 16`, down `ceil(16 (q - 1/16)) / 16`; clamp 0 to 1; all three kinds |
| Step with Shift+Alt held (`sender::FINE_STEPS`) | the same formulas with 1/64: up `floor(64 (q + 1/64)) / 64`, down `ceil(64 (q - 1/64)) / 64` |
| Volume and mute | a new level <= 0 mutes; a volume key unmutes when the new level is > 0; the mute key toggles but at volume 0 the result is muted; muted shows the mute glyph with an empty meter, unmuted the level glyph |
| Hold (`sender::HOLD_CHANGED_MSEC`, `HOLD_UNCHANGED_MSEC`, `hold_seconds`) | the sender passes 4000 ms when the press changed a value, 1000 ms when it was refused or changed nothing (up at 1.0); the helper holds `floor(msec / 1000)` whole seconds, then the fade. `effects::apply` says whether a press changed a value (the level, or the mute state) |
| Hold of `Show` | 4 s: the control panel changed the level itself |
| Bounds | volume and keyboard backlight reach 0; display brightness is requested down to 0.0 |
| Material | `LoginWindowControl` through `Surface::Hud`; with Reduce Transparency or Increase Contrast, the theme's opaque window background in the window's shape |
| Track and segment colours | picked by the menu bar's darkness (the session's appearance stands in). Light: track = secondary label colour (black 0.498), segments white. Dark: track = black 0.25, segments = secondary label colour (white 0.549). The track is the 161 x 8 container drawn behind the segments; empty segments show only it |
| Priority | all four overlays share one priority (the design sends 500 for all), so a show always replaces a visible one |
| Muted glyph below level 0.001; speaker tiers at 1/3 and 2/3 | |
| Display floor (`sender::DISPLAY_FLOOR_RAW`) | raw value 1; platform deviation, see Effects |
| Glyph colour | the label colour (`ColorRole::ControlText`) |
| Glyph size and top (`layout::GLYPH_SIZE`, `GLYPH_TOP`) | 96 pt, 28 pt from the top |
| Label top (`layout::LABEL_TOP`) | 132 pt |

## Modules

| Module | Content |
|---|---|
| `model` | `Kind`, `Overlay`, `segments` (the fill rule), `Hud` (show / hold / fade with injected time), `fade`, `Phase`, `View` |
| `sender` | `step_level`, `hold_seconds` and the step and hold constants; the display floor |
| `controller` | `Command`; the step and mute rules as pure functions of the level before the press (`volume_step`, `mute_toggle`, `microphone_toggle`, `backlight_step`, `linear_raw`) and the overlay they show; `Shown`, `EffectError` |
| `effects` | `apply`: a command carried out through `Audio1` and the backlight daemons, async (`Buses`) |
| `control` | the `org.cupertino.Osd1` interface (`serve`) and its caller check |
| `layout` | window and meter geometry in points, top-left origin |
| `view` | the overlay's CupertinoKit window: `window`, `MeterColors`, `accessible_name`, `APP_ID` |
| `Osd` | the component: its model, its messages, its layer surface and the host-bus `ShowOsd` |

## Tests

`crates/cupertino-session-tests/tests/shell.rs` runs the component in `cupertino-shell`'s host against the real server,
headless, on a session bus of its own: `Show` draws, a caller other than the server cannot step a level, and fifty
panics injected as it paints, as its loop dispatches and on a helper thread each restart it alone while the dock keeps
drawing and the host's memory stays flat. `examples/osd_capture.rs` draws the overlay for look checks.

## Open issues

Not adopted:

- The design's saturated keyboard-backlight glyphs (codes 27 and 28). The off glyph (code 26) is shown at level 0
  (`symbol.keyboard-brightness.off`).
- The beep after an unmute and the settings-pane shortcut of Alt alone.

unknown:

- The label text for the four overlays. gives its font and frame only, and `Overlay::label` returns `None`.
- The glyph colour and artwork: the OSD names `Volume`, `Mute`, and are as designed but their pixels
  were not decoded, so the symbols above are approximate stand-ins from the asset catalog.
