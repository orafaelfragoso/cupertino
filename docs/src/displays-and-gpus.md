# Displays and GPUs

How Cupertino treats monitors, the GPU driver and X11 programs on the text-console (DRM) backend, as the code on `main` does it. Where a statement is about a program Cupertino starts but does not contain (the NVIDIA driver, `xwayland-satellite`), the page says so. For a machine you can reach, [`cupertino diagnose`](diagnose.md) shows most of this at once.

## Monitors

**Start-up state.** A display that connects gets its stored state if `displays.conf` has one for it. Otherwise it starts at its preferred mode, at the default scale for its size, enabled and to the right of the displays already there. The `--scale` option of the server replaces the scale in both cases.

- A stored mode the display no longer offers is replaced by its preferred mode, unless the display accepts custom modes.
- A display stored as off is turned on when no other display is on.
- When the last display that is on goes away, the first remaining display is turned on.

**Default scale.** For a display without a stored scale, the scale comes from its mode in pixels and its physical size in millimetres. It is 1 when the physical size is unknown, or when the display is too small to have a list of choices.

- The machine's own panel (an `eDP`, `LVDS` or `DSI` connector) offers five sizes: its default size, three with larger text and one with more space. The default is the size at about 127 points per inch, at a whole scale when one is within 5 % of that. A 14-inch 3024 × 1964 panel looks like 1024 × 665, 1147 × 745, 1352 × 878, 1512 × 982 (the default, scale 2) or 1800 × 1169.
- Any other display offers up to five sizes around the one at 110 points per inch, from scales between 1 and 3 in eighths, and starts at that one.
- Settings shows these choices for each display under Displays, from larger text to more space, and the server sets the chosen one's exact scale, so the desktop has exactly the size the choice names.

**`displays.conf`.** The file keeps one section per display, keyed by make, model and serial, or by the connector name when there is no serial. The default path is `displays.conf` in the Cupertino configuration directory; `CUPERTINO_DISPLAYS_CONFIG` overrides it. A display that is unplugged and plugged in again gets its arrangement back, or a place right of the others when another display took its place meanwhile. An arrangement is applied whole or not at all: displays may touch but not overlap (there is no mirroring), and one that fails part way leaves the previous arrangement as it was.

**Unplugging a display.** Its windows move to the remaining display, inside its usable area; a full-screen window there becomes a normal window, an open menu of a window on it closes, an overview on it ends, the pointer comes onto the remaining display and a locked session stays locked. When the same display (same make, model and serial, or connector) comes back, the windows that left it go back to the same place, unless you moved them meanwhile.

**Where things open.** New windows, the launcher, notifications, the control panel and the overview open on the display you are working on: the one the pointer last moved onto, or the one of the window you last switched to. Each display keeps its own scale and work area.

**Mode fallback.** If a display cannot show the mode it starts in (a link or dock too slow for it, or a mode the driver refuses), the server tries other modes of that display in this order: the same size at lower refresh rates, then smaller sizes, each from its highest refresh rate. It never tries a mode larger or faster than the wanted one. The first mode that works is shown and announced as the current mode, and the log says `the display shows a fallback mode`. The fallback is not stored: the next time the display is plugged in, the wanted mode is tried again. If no mode works, the display stays off and the log says why. No other mode is tried when the failure is "device busy" or "permission denied", because another mode cannot fix those.

**Hotplug.** On the text console the server scans the connectors of a card:

- at start,
- on each change event of that card,
- when the session becomes active again,
- after the event queue overflowed.

A paused session does not scan. The scan compares the displays with every connector that has a CRTC, not only with the one that changed.

**When a display appears to clients.** An output is announced to applications only once its display shows it. A display that fails to turn on announces nothing. An output that goes away is withdrawn at once, but its global stays bindable for 5 seconds before it is destroyed, so a client that binds it late is not disconnected. Panels on it are closed first, and none of them keeps the keyboard: an open control panel is dismissed and the window that had the keyboard gets it back.

**Changing displays from a program.** The server offers `zwlr_output_manager_v1`, which tools such as `wlr-randr` and `kanshi` use. A configuration the display cannot carry out fails and the previous state is restored. A sandboxed client does not see this protocol.

**More than one GPU.** Every DRM card is opened. One card draws every frame: the one named by `CUPERTINO_DRM_DEVICE`, otherwise the primary one. The value is a card node such as `/dev/dri/card1`, its render node such as `/dev/dri/renderD129`, or a `/dev/dri/by-path` link to either. If the variable names no GPU, the log warns and the primary one renders; an empty value counts as unset. A display on another card shows a copy of what the render card drew. If the render card goes away, the session cannot go on drawing.

**How the desktop is drawn.** The server composites the windows and draws its materials with OpenGL ES on the render card. CupertinoKit applications and shell processes rasterize their own windows on the CPU and hand them over in shared memory; drawing large windows on the GPU is planned ([GPU rendering](gpu-rendering.md), [#155](https://github.com/orafaelfragoso/cupertino/issues/155)). The toolkit's GPU presenter is built but off: every process draws on the CPU unless it is started with `CUPERTINO_RENDERER=gpu`, and `CUPERTINO_RENDERER=cpu` keeps it there once the GPU path is turned on. Each process writes `renderer: <cpu|gpu> (<reason>)` to its standard error when its renderer changes, and `cupertino diagnose` lists them as `gpu.client_renderers`.

**Frame timing and depth.** On the text console, where a display's page flips report the driver's timestamps, the server starts each frame late after a vblank, as late as the recent frames' render times allow and at most half a refresh period, so a frame shows the latest pointer motion and input; `CUPERTINO_LATE_LATCH=0` makes every frame start at once after the vblank. Primary planes have 8 bits a channel; `"output_depth": 10` in `compositor.json` asks for 10 bits (`AB30`) from the next session on, and 8 bits stay where a display has no 10-bit format. The `output initialised` line in `server.log` names the format each display got. Screenshots, screen sharing and recordings are 8 bits a channel at either depth. `CUPERTINO_DRM_TRACE=1` logs each display's presentation-time branch, missed refreshes, planes, start delay and input-to-flip and pointer-to-flip latency, and `scripts/dev/drm-trace-summary` in the checkout summarises those lines.

**No GPU acceleration.** When the render card has only a software renderer (Mesa's llvmpipe, as on a virtual machine without 3D or a GPU whose driver is missing), another card with a hardware renderer draws instead. When no card has one, the CPU draws the desktop: it works, but more slowly and with more power. The server log warns and names the card, and `session.log` gets a line `<time> only a software renderer is available: gpu=<card>; the CPU draws the desktop`, which `cupertino diagnose` reports as `gpu.renderer`. A card named by `CUPERTINO_DRM_DEVICE` draws even when its renderer is a software one. On a virtual machine, a GPU with 3D (`virtio-vga-gl` or `virtio-gpu-gl` in QEMU) makes the desktop fast again.

## NVIDIA

Before the TTY backend starts, the server checks the machine. `cupertino-server --check-hardware` runs the same check and prints it; the server refuses to start when it finds a problem.

| Finding | Level | Fix the check gives |
|---|---|---|
| The `nvidia_drm` module is not loaded | problem | `modprobe nvidia_drm modeset=1` and set the option in `/etc/modprobe.d` |
| `nvidia_drm` `modeset` is not `Y` | problem | add `nvidia-drm.modeset=1` to the kernel command line and reboot |
| No `nvidia-drm_gbm.so` GBM backend found | warning | install the driver's GBM library (`nvidia-utils`) |

The check also notes the driver version and whether `nvidia-drm` `fbdev` is on. `cat /sys/module/nvidia_drm/parameters/modeset` prints `Y` when modesetting is on. A machine installed from the [USB image](install-usb.md) sets `nvidia_drm modeset=1 fbdev=1` for NVIDIA cards from the Turing generation on.

**Explicit sync.** When the render card's kernel can wait on syncobj timelines, the server offers `linux-drm-syncobj-v1` and logs `explicit sync (linux-drm-syncobj-v1) is offered`. Otherwise it logs `explicit sync is not offered: the kernel cannot wait on syncobj timelines`. A client that uses explicit sync says when its buffer is ready; any other dmabuf is waited for through its fences. Whether explicit sync works with the NVIDIA driver has not been run, according to [Using Cupertino every day](daily-use.md). On a new NVIDIA machine, check for the log line and that a browser starts and draws.

## X11 programs

X11 programs run through `xwayland-satellite`, which starts when the first X11 program connects.

- The server needs `xwayland-satellite` on `PATH`. Without it, or when no X display can be taken, the session serves Wayland programs only and the log says why.
- Each X11 window reaches the server as an ordinary window, so it follows the same window policy, focus and input as any other.
- A program the server starts gets `DISPLAY` and `XAUTHORITY` for the X display, and `XDG_SESSION_TYPE=wayland`.
- It also gets `QT_QPA_PLATFORM=wayland;xcb` and `ELECTRON_OZONE_PLATFORM_HINT=auto`, so Qt and Electron prefer Wayland and fall back to X11. A value the program or the session already set is kept. `SDL_VIDEODRIVER` is not set.
- When the satellite exits, the server waits for the next X11 program to start it again. After a failure, or a success within a second of the start, the restart waits for a back-off that starts at 1 second and doubles to 30 seconds, so a satellite that cannot run is not started in a loop. X11 windows that were open are lost when it exits.
- The satellite, not the server, decides the scale of X11 windows. With monitors of different scales, [Using Cupertino every day](daily-use.md) says every X11 window gets the smallest scale; that is the satellite's behaviour and is not checked here.

## Report a display problem

Run this in the session that has the problem, from a terminal:

```sh
cupertino diagnose --area gpu --area session --area crash --area logs
```

Add `--format json` before `diagnose` for a script, and attach the file `cupertino diagnose export --output FILE` writes. `gpu.driver` shows the driver and the findings above, `session.wayland` shows whether the compositor answers, `crash.last` shows how the last sessions ended, and `logs.paths` shows where the logs are. With the report, attach `~/.local/state/cupertino/session.log` and `server.log` (and `server.log.1` after a restart). The log lines named on this page (`display connected`, `the display shows a fallback mode`, `explicit sync`) are in `server.log`.
