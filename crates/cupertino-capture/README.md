# cupertino-capture

The shared part of the look-check capture command: the flags of every app's capture example, the names of the PNGs it
writes and the contact sheet. MPL-2.0; a dev-dependency of `cupertino-settings`, `cupertino-files` and `cupertino-kit`,
whose `examples/<app>_capture.rs` draw their surfaces with their own painters on sample data.

```
ci/capture.sh APP/SURFACE [--scale N] [--appearance light|dark|both] [--increase-contrast] [--reduce-transparency]
              [--size WxH] [--out DIR] [--sheet] [--STATE-FLAG VALUE]...
ci/capture.sh APP          # lists the surfaces of APP
```

| Flag | Effect |
|---|---|
| `--scale N` | device pixels per point (default 1) |
| `--appearance` | `light`, `dark` or `both` (default) |
| `--increase-contrast`, `--reduce-transparency` | the session's display options |
| `--size WxH` | the window size in points instead of the surface's default |
| `--out DIR` | where the PNGs go (default `target/captures`, created) |
| `--sheet` | every state the app lists for the surface, and one contact sheet: states as columns, appearances as rows |
| `--STATE-FLAG VALUE` | a state of the surface; each app names its own (`--login saving`, `--view list`) |

One PNG per state and appearance: `<app>-<surface>[-<flag>-<value>]...-<light|dark>[-hc][-rt]@<scale>x.png`, the value in
lower case with other characters than letters and digits as `-` (`settings-accessibility-login-saving-dark@2x.png`).
The sheet is `<app>-<surface>-sheet[-hc][-rt]@<scale>x.png`. The command prints each path it writes. It exits 2 with a
message for an unknown surface, flag or state value and when no asset pack is found: a capture with the built-in font
alone does not show the surfaces as they look.

| Surface | States |
|---|---|
| `settings/<pane-key>` (every pane), `settings/wallpaper-chooser`, `settings/search-results` | `--login none\|saving\|refused` (Accessibility), `--mice one\|two` (Mouse) |
| `files/window`, `files/sidebar` (the sidebar column of the window) | `--view icon\|list`, `--drop-target NAME` (a sidebar place, else an item), `--selected NAME` |
| `kit/controls`, `kit/icons` | none |

The sidebar material is the server's, so a flat fill of the window background stands in for it. The live readings,
the home folder and the pictures are example data, not the machine's.
