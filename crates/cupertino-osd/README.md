# cupertino-osd

The on-screen display of the Cupertino desktop. It stands in for the design's overlay helper: a 200 x 200 pt window
with a glyph and a 16-segment meter for volume, mute, brightness and keyboard backlight. It appears on a key press, holds,
and fades out.

The crate has no Smithay dependency. The model (`model`, `controller`, `sender`), the backends (`backend`, `system`), the bus
interface (`control`) and the CPU renderer (`layout`, `render`) are separate, so a host can drive the overlay without the
process. The `cupertino-osd` process is that host for the desktop. Window size, corner radius, position, meter geometry, the
partial-segment rule and the fade follow the analysis. The items marked approximate below are authored and replaceable.

## Running

Feature `app` builds the `cupertino-osd` binary: the overlay on a layer surface, as a client of `cupertino-server` on
CupertinoKit (`cupertino-kit`), with `cupertino-shell-client` for the layer surface. The server starts and restarts it through the `shell` key of its configuration.

```json
{"shell": [{"name": "osd", "command": ["cupertino-osd"]}]}
```

```
cargo build -p cupertino-osd --features app
cupertino-osd [--fake-backends] [--alsa-card N]
```

| Option | Default | Effect |
|---|---|---|
| `--fake-backends` | off | Change in-memory levels (0.5 each) instead of the machine's volume and backlights. For trying the overlay. |
| `--alsa-card N` | the default PipeWire sink, else the best master of all cards (rule in `cupertino-alsa`) | Use the master volume of sound card `N`. |

The overlay has no config file. The session's display options select the meter palette: Increase Contrast makes the light
track opaque black and the dark segment white, Reduce Transparency makes the dark track opaque, and both
take the opaque background.

## How it works

**Surface.** One overlay layer surface of 200 x 200 pt anchored to the bottom only, so it is centred horizontally, with a
bottom margin of 140 pt, no keyboard interactivity and an empty input region. The window is a CupertinoKit window
(`view::window`); `OsdRoles` in the process gives it that layer surface through `cupertino-ui-wayland`'s `SurfaceRoles`.
Its background is the HUD surface (`Surface::Hud`, the `LoginWindowControl` material) with the window's radius, and its
alpha follows the fade. The glyph (an `Icon`) and the meter draw over it as one layer at the same alpha. While hidden the
surface stays mapped, draws nothing and has no material. Glyphs are template symbols of the asset directory
(`symbol.speaker.slash`, `symbol.speaker.low`, `symbol.speaker`, `symbol.speaker.high`, `symbol.sun`, `symbol.keyboard-brightness` and `.off`, `symbol.mic` and `.slash`; `model::ICON_NAMES`),
in the control text colour (the label colour). A symbol no asset pack provides draws as Kit's placeholder.

**Session.** CupertinoKit follows the session's appearance and display options. With Reduce Transparency or Increase
Contrast the HUD surface takes no material and paints the theme's opaque window background in its shape. The overlay
takes no input and the server hides it while the session is locked, so a lock needs no reaction.

**Accessibility.** The window is named after the overlay ("Volume", "Volume muted", "Brightness", "Keyboard brightness",
"Microphone", "Microphone muted"), and the meter is a progress indicator of that name with the level from 0 to 1.

**Timing.** Commands from the bus thread arrive on a channel the application listens to. A timer fires at the end of the
hold, then once a frame (1/60 s) through the fade; a new command replaces the pending timer.

**Effects.** The volume and mute change the default PipeWire sink through `cupertino-pipewire` (`wpctl`), so the
overlay shows the level `wpctl get-volume` reports. When no PipeWire daemon answers a press, the press goes to the ALSA
master through `cupertino-alsa`, which the process opens on that press and again after a failure. Display
brightness and the keyboard backlight go through `cupertino-powerd` on the system bus, connected on the first press and again
after a failure. A failure (no sound card, no backlight, daemon absent) shows nothing and is logged on stderr with its reason.

- Levels map linearly onto the raw range of each backlight (no fixed curve to copy).
- **Platform deviation:** the display backlight never goes below raw value 1, although the design requests down to 0.0.
  A Linux backlight at raw 0 can blank the panel. The keyboard backlight reaches 0.
- A change of level always moves at least one raw unit, so a coarse raw range cannot swallow a press.
- A master without a mute switch counts as muted exactly at level 0. The volume keys work, and the mute key fails with its
  reason at any other level.

## Protocols / D-Bus it speaks

The process owns `org.cupertino.Osd1` at `/org/cupertino/Osd1` on the session bus. A second overlay cannot take the name and
exits with failure. The interface lives in `control` (`BUS_NAME`, `OBJECT_PATH`, `serve`).

Callers: `VolumeStep`, `ToggleMute`, `ToggleMicrophoneMute`, `BrightnessStep` and `KeyboardBrightnessStep` change hardware levels, so only the server
(this process's parent) and this process itself may call them (`cupertino-peer`). An anonymous, sandboxed (Flatpak, Snap) or
unrelated caller gets `org.cupertino.Error.AnonymousCaller`, `FlatpakCaller`, `SnapCaller` or `UntrustedCaller`, one warning
is logged and nothing changes. `Show` only draws the overlay, so it stays open to every caller (the control panel uses it).

| Method | Effect |
|---|---|
| `VolumeStep(i32 direction, bool fine) | Raises (`direction` > 0) or lowers the volume one press (a level of 0 mutes, a level above 0 unmutes) and shows the level. |
| `ToggleMute | Mutes or unmutes the output (at volume 0 the result is always muted). Shows the muted glyph with an empty meter, or the level. |
| `ToggleMicrophoneMute` | Mutes or unmutes the default PipeWire source. Shows the slashed microphone with an empty meter, or the microphone at the input level. Without a PipeWire daemon it fails and shows nothing. |
| `BrightnessStep(i32 direction, bool fine) | Display brightness one press, through `org.cupertino.Power1` on the system bus. |
| `KeyboardBrightnessStep(i32 direction, bool fine) | Keyboard backlight one press, through `org.freedesktop.UPower.KbdBacklight` on the system bus. |
| `Show(s kind, d level) | Shows `volume`, `mute`, `brightness`, `keyboard-brightness`, `microphone` or `microphone-mute` at `level` (0 to 1) without changing anything, holding 4 s. For the control panel, which changes levels itself. |

A direction of 0, an unknown kind or a level outside 0 to 1 is answered with `org.freedesktop.DBus.Error.InvalidArgs` and
shows nothing.

```
gdbus call --session --dest org.cupertino.Osd1 --object-path /org/cupertino/Osd1 --method org.cupertino.Osd1.VolumeStep 1 false
```

The server calls the interface for these keys. Shift+Alt held makes the press fine.

| Key | Call |
|---|---|
| `XF86AudioRaiseVolume` | `VolumeStep(1, fine) |
| `XF86AudioLowerVolume` | `VolumeStep(-1, fine) |
| `XF86AudioMute` | `ToggleMute |
| `XF86AudioMicMute` | `ToggleMicrophoneMute` |
| `XF86MonBrightnessUp` | `BrightnessStep(1, fine) |
| `XF86MonBrightnessDown` | `BrightnessStep(-1, fine) |
| `XF86KbdBrightnessUp` | `KeyboardBrightnessStep(1, fine) |
| `XF86KbdBrightnessDown` | `KeyboardBrightnessStep(-1, fine) |

## Values applied

| Item | Value |
|---|---|
| Window | 200 x 200 pt, horizontally centred, 140 pt above the bottom edge, no shadow, ignores the mouse |
| Corner radius | 18 pt |
| Meter | 16 segments of 9 x 6 pt at pitch 10, in a 161 x 8 container at (20, 20) bottom-left, so segment `i` is at x = 20 + 10 i + 1, y = 173 top-left |
| Fill | `v = level * 16`; `floor(v)` full segments; the next one `(int)(frac * 9 + 0.5) points wide (0 is empty, 9 is full) |
| Fade | linear alpha `1 - t / 0.7`, 0.7 s, after a hold the sender supplies; a new show restarts at alpha 1 |
| Step of one press (`sender::COARSE_STEPS`, `FINE_STEPS`, `step_level`) | snap `q = floor((v + 1/128) * 64) / 64`, then up `floor(16 (q + 1/16)) / 16`, down `ceil(16 (q - 1/16)) / 16`; clamp 0 to 1; all three kinds |
| Step with Shift+Alt held (`sender::FINE_STEPS`) | the same formulas with 1/64: up `floor(64 (q + 1/64)) / 64`, down `ceil(64 (q - 1/64)) / 64` |
| Volume and mute | a new level <= 0 mutes; a volume key unmutes when the new level is > 0; the mute key toggles but at volume 0 the result is muted; muted shows the mute glyph with an empty meter, unmuted the level glyph |
| Hold (`sender::HOLD_CHANGED_MSEC`, `HOLD_UNCHANGED_MSEC`, `hold_seconds`) | the sender passes 4000 ms when the press changed a value, 1000 ms when it was refused or changed nothing (up at 1.0); the helper holds `floor(msec / 1000)` whole seconds, then the fade. `Controller` knows whether a press changed a value (the level, or the mute state) |
| Hold of `Show` | 4 s: the control panel changed the level itself |
| Bounds | volume and keyboard backlight reach 0; display brightness is requested down to 0.0 |
| Material | `LoginWindowControl` through `Surface::Hud`; with Reduce Transparency or Increase Contrast, the theme's opaque window background in the window's shape |
| Track and segment colours | picked by the menu bar's darkness (the session's appearance stands in). Light: track = secondary label colour (black 0.498), segments white. Dark: track = black 0.25, segments = secondary label colour (white 0.549). The track is the 161 x 8 container drawn behind the segments; empty segments show only it |
| Priority | all four overlays share one priority (the design sends 500 for all), so a show always replaces a visible one |
| Muted glyph below level 0.001; speaker tiers at 1/3 and 2/3 | |
| Display floor (`sender::DISPLAY_FLOOR_RAW`) | raw value 1; platform deviation, see Effects |
| Glyph colour (`render::Palette::from_theme`) | the label colour |
| Glyph size and top (`layout::GLYPH_SIZE`, `GLYPH_TOP`) | 96 pt, 28 pt from the top |
| Label top (`layout::LABEL_TOP`) | 132 pt |

## Modules

| Module | Content |
|---|---|
| `model` | `Kind`, `Overlay`, `segments` (the fill rule), `Hud` (show / hold / fade with injected time), `Phase`, `View` |
| `sender` | `step_level`, `hold_seconds` and the step and hold constants; the display floor |
| `backend` | `Volume` and `Backlight` traits (read and write a level, mute); `SessionVolume` (PipeWire, else ALSA), `AlsaVolume`, `MemoryVolume`, `MemoryBacklight`; `linear_raw` |
| `system` (feature `dbus`) | `DisplayBacklight`, `KeyboardBacklight` over the system bus |
| `controller` | `Command`, `Controller`: applies a command (step rules, mute rules) through the backends, then shows the result with the hold of its effect; `EffectError` |
| `control` (feature `dbus`) | the `org.cupertino.Osd1` interface (`serve`) |
| `layout` | window and meter geometry in points, top-left origin |
| `view` (feature `app`) | the overlay's CupertinoKit window: `window`, `MeterColors`, `accessible_name`, `APP_ID` |

## Open issues

Not adopted:

- The design's saturated keyboard-backlight glyphs (codes 27 and 28). The off glyph (code 26) is shown at level 0
  (`symbol.keyboard-brightness.off`).
- The beep after an unmute and the settings-pane shortcut of Alt alone.

unknown:

- The label text for the four overlays. gives its font and frame only, and `Overlay::label` returns `None`.
- The glyph colour and artwork: the OSD names `Volume`, `Mute`, and are as designed but their pixels
  were not decoded, so the symbols above are approximate stand-ins from the asset catalog.

## Tests

```
cargo test -p cupertino-osd --features app
```

| Test | Proves |
|---|---|
| Model unit tests | The fill rule and the lifetime at literal instants. |
| Sender and controller unit tests | The step formulas, the mute rules, and the 4 s and 1 s holds. |
| Layout and view unit tests | The meter geometry, its colours in each appearance, its pixels at 2x, its accessible name and level, and the HUD background at the fade's alpha. |
| `tests/control.rs` | The bus interface against a private `dbus-daemon` with in-memory backends; a host process (`dbus-send`) is refused every step method with `UntrustedCaller`, and `Show` stays open. |

Screenshots of the process on the headless compositor are in `output/` (`osd-volume.png`, `osd-brightness.png`,
`osd-fade.png`). They were taken with `--fake-backends`: a server config with the `osd` shell entry on a private session bus,
`VolumeStep 1 false` and then `BrightnessStep -1 false` through `gdbus`, and `--shot` / `--screenshot` timed 1 s after the
first press, 1 s after the second and 0.4 s into its fade.
