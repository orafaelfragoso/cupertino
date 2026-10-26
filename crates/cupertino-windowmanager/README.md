# cupertino-windowmanager

The window manager process of the Cupertino desktop. It stands in for the design's window manager process and owns the stage policy, on the `cupertino-stage` model. The server owns the frame clock: it
animates the windows to the placements this process commits and draws the strip.

| Item | Contents |
|---|---|
| `Controller` | The stage policy of one output. Groups of window events (`Change`) come in from the server and placement and activation requests (`Request`) go out. Only placements that changed are sent, then one commit. |
| `parse_conf`, `WindowManagerFile` | `windowmanager.conf` |
| `cupertino-windowmanager` (feature `app`) | The process: one `cupertino_window_manager_v1` per output of `cupertino-shell-v1`, a `Controller` each. |

Layer: `shell`. Depends on `cupertino-stage` and `cupertino-wm`. The process also depends on `cupertino-defaults`,
`cupertino-protocols` and `wayland-client`. It has no surface, so it does not use the `cupertino-client` toolkit.

## Running

The server starts it as a shell process (`"shell"` key of `compositor.json`):

```json
{"name": "windowmanager", "command": ["cupertino-windowmanager"]}
```

It exits with failure when the connection ends, so the server restarts it. The server puts every window back at its own frame
as soon as the process is gone.

| Setting | Where | Default |
|---|---|---|
| Config file | `windowmanager.conf` in `$XDG_CONFIG_HOME/cupertino/`; `$CUPERTINO_WINDOWMANAGER_CONFIG` overrides the path | no file means stage off |
| `stage` | `on` or `off` | `off` |

The file holds `key = value` lines, and `#` starts a comment. A line the parser does not understand is reported on stderr and
skipped.

## How it works

- **Stage off** (the default). The process sends nothing, so every window is drawn and hit as without it.
- **Stage on** (`stage = on`, or the `toggle-stage` action, which has no default chord). The windows of the most recently
  active application are on the stage. The windows of the other applications are thumbnails in the strip on the left, one
  group per application, in the layout of `cupertino-stage`: 170 pt slots, 4 to 6 rows,. Groups beyond the
  rows are hidden.
- A press on a thumbnail brings its group to the stage, sends the previous group to the top of the strip and activates the
  front window of the group. A window focused any other way (the switcher, the dock, a new window) brings its group to the
  stage the same way.
- The swap runs in the server with the stage model's springs, all damping 1.0: the frame with response 0.38 s, the
  alpha with 0.30 s and the corner radius, from the window's 12 pt to the strip's 8 pt, with 0.20 s. Hidden windows fade out.
- The toggle is not written back to `windowmanager.conf`, because `cupertino-defaults` has no writes yet.

## Open issues

Not built yet:

- Moving windows out of the strip column. A window wider than the stage area overlaps the strip, which stays shown.
- The strip's hide and edge reveal.
- Dragging windows and groups between the strip and the stage.
- The app-icon overlays and the strip's 3D tilt.
- The desktop click that empties the stage.
- Tiling requests. The tile shortcuts run in the server's model.

## Tests

```
cargo test -p cupertino-windowmanager
```

| Test | Proves |
|---|---|
| Controller and settings unit tests | The stage policy and the `windowmanager.conf` parser. |
| `cupertino-server` `tests/window_manager_v1.rs` | The `cupertino_window_manager_v1` protocol. |
| `tests/shell_processes.rs`, `killing_the_window_manager_process_puts_the_windows_back` | The real binary runs, and the server restores the windows when it dies. |
| QEMU check `18-stage` | Toggling the stage in a guest and switching groups by clicking the strip. |
