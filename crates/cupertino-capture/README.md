# cupertino-capture

The shared part of the look-check capture command: the flags of every app's capture example, the names of the PNGs it
writes and the contact sheet. MPL-2.0; a dev-dependency of `cupertino-files` and `cupertino-kit`,
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
| `--out DIR` | where the PNGs go (created); `ci/capture.sh` defaults to `captures` in `$CARGO_TARGET_DIR`, else in the workspace's `target` |
| `--sheet` | every state the app lists for the surface, and one contact sheet: states as columns, appearances as rows; refused with a state flag |
| `--STATE-FLAG VALUE` | a state of the surface; each app names its own (`--login saving`, `--view list`) |

One PNG per state and appearance: `<app>-<surface>[-<flag>-<value>]...-<light|dark>[-hc][-rt]@<scale>x.png`, the value in
lower case with other characters than letters and digits as `-` (`settings-accessibility-login-saving-dark@2x.png`).
The sheet is `<app>-<surface>-sheet[-hc][-rt]@<scale>x.png`. The command prints each path it writes. It exits 2 with a
message for an unknown surface, flag or state value and when no asset pack is found: a capture with the built-in font
alone does not show the surfaces as they look.

| Surface | States |
|---|---|
| `files/window`, `files/sidebar` (the sidebar column of the window) | `--view icon\|list`, `--selected NAME`, `--devices removable` (a mounted, a busy and an unmounted device in the sidebar) |
| `kit/controls` (the row 167a controls sheet), `kit/icons` (with the chevrons along the reading direction and a missing symbol's placeholder) | `--direction ltr\|rtl` |
| `kit/gallery` (every control family; its sheet shows each state) | `--state rest\|held-button\|held-switch\|held-slider\|held-segment\|disabled`, `--control-size mini\|small\|regular\|large\|extra-large`, `--compact on\|off`, `--backdrop flat\|busy`, `--direction ltr\|rtl` |
| `kit/scroll` (an overlay scroll view: at rest, pulled past its top edge, its bar shown, fading and hidden after a wheel step) | `--direction ltr\|rtl`, `--phase rest\|stretch\|shown\|fading\|hidden` |
| `kit/under-toolbar` (a page that scrolls under a toolbar row drawn in a layer over it, set off by the scroll edge: at rest, just after a wheel step and after the bar has faded) | `--direction ltr\|rtl`, `--phase rest\|shown\|hidden` |
| `overview/all` (the server headless with test windows, `ci/capture-overview.sh`; no `--sheet`) | `--windows N` (1 to 6, default 2) |

The sidebar material is the server's, so a flat fill of the window background stands in for it. The home folder and
the pictures are example data, not the machine's. Files draws at a fixed clock (2026-01-01 15:00
UTC), so a command gives the same pixels on every run.
