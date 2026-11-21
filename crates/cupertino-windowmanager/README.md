# cupertino-windowmanager

The window manager of the Cupertino desktop: the stage policy, on the `cupertino_wm::stage` model, and the stage strip.

The crate is a library: one component, `WindowManager`, which the shell host (`cupertino-shell`) runs as
`windowmanager` of the server's component table. The policy (`controller`) is pure; the strip's surface (`strip`) is
drawn by CupertinoKit; the component (`component`) speaks to the server only through `cupertino-kit-shell`: the window
manager role's feed and requests, holes and transactions. The server owns the frame clock and draws every window,
including the ones in the strip's holes; it draws no strip of its own.

| Item | Contents |
|---|---|
| `WindowManager` | The component: a `Controller` per output, the strip's surface while the stage is on |
| `Controller` | The stage policy of one output. Groups of window events (`Change`) come in from the server; the windows to hide or show again and the window to activate (`Request`) go out, then one commit; `strip` gives the thumbnails' resting rectangles |
| `strip` | The strip's Kit window: a hole per thumbnail, the pointer on the thumbnails alone |
| `parse_conf`, `WindowManagerFile`, `conf::read_stage` | `windowmanager.conf` |

Layer: `shell`. Depends on `cupertino-wm` (its `stage` module), `cupertino-kit`, `cupertino-kit-shell` and
`cupertino-defaults`.

## Running

The server lists `windowmanager` to the shell host, which runs it; nothing starts it on its own. `compositor.json` names
it among the `components` of its `shell` section (`docs/architecture/shell-host.md`), and `alone` runs it in a host of
its own (`cupertino-shell --only windowmanager`). A development build of the host makes it panic on request
(`cupertino shell crash windowmanager --at paint|dispatch|helper`).

| Setting | Where | Default |
|---|---|---|
| Config file | `windowmanager.conf` in `$XDG_CONFIG_HOME/cupertino/`; `$CUPERTINO_WINDOWMANAGER_CONFIG` overrides the path | no file means stage off |
| `stage` | `on` or `off` | `off` |

The file holds `key = value` lines, and `#` starts a comment. A line the parser does not understand is reported on stderr and
skipped. The component reads it once, on Kit's I/O pool, as it starts.

```
cargo run -p cupertino-windowmanager --example windowmanager_capture -- windowmanager/strip --appearance both
cargo run -p cupertino-windowmanager --example windowmanager_capture -- windowmanager/full
```

A capture has no server: each hole is painted a flat grey where the server would draw the window.

## What a restart loses

A failed component is started again alone by its host (D16, D21). The strip's surface is persistent (`strip`): while
the component starts again the server keeps showing it, with its holes and the windows they claim, and keeps the
windows it hid hidden (D19). The new instance hears that the strip is held, turns the stage on whatever the file says,
lays its strip out at once and commits its placements in the update that first shows it, so it takes the strip and the
placements over on one frame with nothing moving. When the server gives the component up, or it is turned off, the
windows go back to their own frames along the server's return motion. Lost with a restart:

- the stage turned off or on with the shortcut since the start, when the stage was off at the crash: the new instance
  reads the file again;
- the order of the strip's groups, which the new instance builds again from the windows' order;
- a swap under way: the new instance lays the strip out where the swap would have ended.

The windows, their frames and which is the key window are the server's, so a restart changes none of them.

## How it works

- **Stage off** (the default). The component places nothing and shows no strip, so every window is drawn and hit as
  without it.
- **Stage on** (`stage = on`, or the `toggle-stage` action, which has no default chord). The windows of the most recently
  active application are on the stage. The windows of the other applications are thumbnails in the strip on the left,
  one group per application, in the layout of `cupertino_wm::stage`: 170 pt slots, 4 to 6 rows. The groups beyond the
  rows are hidden.
- **The strip** is a bottom layer surface along the output's left edge, as wide as the strip's column, which takes the
  pointer only on its thumbnails. Each thumbnail is a hole that claims its window: the server draws the window there,
  fitted to the hole with its own corners and shadow, and nowhere else, and does not hit it.
- **A press on a thumbnail** brings its group to the stage, sends the previous group to the top of the strip and
  activates the front window of the group. A window focused any other way (the switcher, the dock, a new window) brings
  its group to the stage the same way.
- **A change is one transaction.** The strip's surface is staged with its holes' layer animations, and the placements
  and the activation are window changes of the same part, so the server starts them on one frame. A window going to the
  strip is taken over by a hole laid out where it shows, which moves to its slot along `SpringPreset::STAGE_SWAP`
  (critically damped, 0.38 s); a window coming back moves in its hole to its own frame, and the hole goes when the
  motion ended. A thumbnail fades in or out along `SpringPreset::STAGE_FADE` (0.30 s) as its group gains or loses a
  row, the server hiding the window under it. Under Reduce Motion the holes move at once, and a window going to the
  strip fades in there.
- **A window that closes** in the strip: the server fades its hole out and says so; a window minimized or moved to
  another desktop keeps its hole 500 ms (`DEPARTED_GRACE`). The strip closes up around it.
- The toggle is not written back to `windowmanager.conf`.

**Accessibility.** The strip is a group named "Stage Strip"; each thumbnail at rest is a button named by its
application.

## Open issues

Not built yet:

- Moving windows out of the strip column. A window wider than the stage area overlaps the strip.
- The strip's hide and edge reveal.
- Dragging windows and groups between the strip and the stage.
- The app-icon overlays and the strip's 3D tilt.
- The desktop click that empties the stage.
- Tiling requests. The tile shortcuts run in the server's model.
