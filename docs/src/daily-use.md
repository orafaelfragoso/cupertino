# Using Cupertino every day

A short guide for the first days on a machine that uses Cupertino as its session. Everything here is taken from the crate READMEs and from changes that are on `main`; the server's README (`crates/cupertino-server/README.md`) has the detail. Cupertino is early (0.x): this page says what exists and how it is meant to be used, not that it is finished. Where a step has not been run on real hardware, the page says so.

## Starting and leaving a session

- **Start.** On a machine installed from the USB image, `cupertino-login` starts the session on tty1: log in there. On other machines, the package installs the session entry `cupertino.desktop`, so GDM lists **Cupertino** in its session chooser; choose it, then log in. Do not turn on automatic login for this session while it is new: when the compositor ends for any reason, the session script ends and GDM shows its login screen again, and automatic login would start the session again at once.
- **Leave normally.** Log out from the menu bar's system menu or with `Super+Shift+Q`. Applications are asked to quit first, and a confirmation panel appears. While the login process is restarting after a crash, `Super+Shift+Q` ends the session without asking.
- **Lock.** `Super+Ctrl+Q`.
- **Leave at once.** `Ctrl+Alt+Backspace` ends the compositor. `Ctrl+Alt+F1` to `F12` switch to a text console. Both are reserved: no application can take them over.
- **Bound the first login.** Write a whole number of seconds into `~/.config/cupertino/session-limit`, for example `echo 600 > ~/.config/cupertino/session-limit`. The compositor stops after that long. Remove the file for a normal session.
- **From a checkout, without GDM.** On a free text console, `./run-cupertino.sh`; see [Building and running](building.md).

Not verified: how GDM treats the session on a real machine (the virtual console, and the restart after a crash). The first login on the hardware is the test.

## Keyboard shortcuts

| Keys | Action |
|---|---|
| `Super+Space` | launcher |
| `Super+Return` | terminal (the first of `foot`, `alacritty`, `kitty`, `wezterm` and `xterm` that is installed) |
| `Super+Tab`, `Super+Shift+Tab` | application switcher (release `Super` to pick) |
| `Ctrl+Up` | overview |
| `Ctrl+Left`, `Ctrl+Right`, `Ctrl+1` to `Ctrl+9` | workspaces |
| `Super+Q`, `Super+W`, `Super+M`, `Super+H` | quit application, close window, minimize, hide application |
| `Super+Ctrl+F`, `Super+Ctrl+Left`, `Super+Ctrl+Right` | full screen, tile left, tile right |
| `Super+Alt+Escape` | Force Quit |
| `Shift+Super+/` | Help menu with its search field focused |

To change a shortcut, add a `chord = action` line to `~/.config/cupertino/shortcuts.conf`, for example `Super+x = exec:foot`, or `Super+Tab = none` to free a chord. The file is applied on top of the defaults. `modifier_profile = classic-pc` in `~/.config/cupertino/keyboard.conf` switches to Windows and Linux habits (`Alt+Tab` switches, `Alt+F4` closes, `Ctrl` is the command key in Settings and text fields, and a tap of `Super` alone opens the launcher).

## Layouts, Compose and dead keys

- **Layouts.** At login Cupertino reads the system layout (`X11Layout`, `X11Variant` and `X11Options` from `localed`). If `keyboard.conf` has no `input_sources`, those layouts become the input sources. To set them yourself, write `input_sources = us, de, fr(nodeadkeys)` in `~/.config/cupertino/keyboard.conf`; that file wins over `localed`.
- **Switching.** `Ctrl+Space` opens the input source list and steps through it (most recently used first); releasing the modifiers picks one. `Ctrl+Alt+Space` steps to the next source in menu order.
- **Compose key.** `compose_key = right-alt` in `keyboard.conf` (also `right-ctrl`, `menu`, `caps-lock`, `scroll-lock`, or `off`) sets the compose key of the system layouts. Unset, it keeps the one `localed` holds.
- **Dead keys and Compose sequences.** Applications built on Cupertino's own Wayland client (CupertinoKit apps) read the locale's compose table (`XCOMPOSEFILE`, then `~/.XCompose`, then the locale's file), so layouts with dead keys such as `us(intl)` and `de(deadkeys)` type their characters. Other toolkits do their own compose handling.
- **Accents.** Holding a letter key opens the accent picker, but only with `accent_table` set and only in applications with text input support.

## Screenshots

| Keys | What happens |
|---|---|
| `Super+Shift+3` (also `Super+Shift+S`) | the whole screen |
| `Super+Shift+4` | a selection |
| `Super+Shift+5` | the toolbar |
| `Super+Ctrl+Shift+3`, `Super+Ctrl+Shift+4` | the screen or a selection, to the clipboard |

Files go to the Desktop folder unless `~/.config/cupertino/screenshot.conf` sets `location` to `documents`, `pictures`, `clipboard` or a directory. The toolbar writes the file back. Screen recording is not implemented.

## Logs and crash reasons

`cupertino-diagnose` checks the GPU driver, the session, the portal, the services and the last crash in one go, and names
the fix for what is wrong; `cupertino-diagnose --json` gives the same as JSON.

| File | Holds |
|---|---|
| `~/.local/state/cupertino/session.log` | one line per session start and end: a normal end, the exit status, the signal that ended the compositor, or the watchdog. When the compositor ends with an error, the end line also has the reason: a panic on its main thread or a lost rendering context. An abort or a fault (SIGABRT, SIGSEGV) is named as the reason, and `server.log` has its backtrace, as it has a panic's. Other signals leave no reason; the line names the signal. |
| `~/.local/state/cupertino/server.log` | everything the compositor logs, including the GPU, the driver, the kernel and any panic. The previous run is `server.log.1`. An error that stops the start is its last line, `the server stopped: ...`. |
| `~/cupertino-tty.log` | what the processes print when started with `run-cupertino.sh`, including a start that fails before `server.log` opens. |

`RUST_LOG` changes the log filter, and `CUPERTINO_LOG_FILE` moves `server.log`. After a panic or a lost rendering context, the teardown may take 2 s, then the server exits.

## If it hangs

- **One application does not answer.** The compositor pings every application window once a second. With no answer 2 s after a ping, the pointer over its window shows the busy cursor and the dock marks it Not Responding. Press `Super+Alt+Escape`, or use Force Quit in the menu bar. An application that does not answer is killed at once; any other gets a close request first and is killed 3 s later if it is still there.
- **A shell process fails** (dock, menu bar, launcher and so on). The compositor starts it again, at most once per second. A process that exits successfully stays stopped.
- **The compositor hangs.** `Ctrl+Alt+Backspace` and `Ctrl+Alt+F<n>` need a live compositor. From another console or over ssh, `pkill cupertino-server` ends it. As a last resort, with `sudo sysctl kernel.sysrq=1` set beforehand, `Alt+SysRq+R`, `E`, `I` returns the keyboard to the kernel and ends all processes. Save your work in other sessions first.
- **After a panic, an abort, a fault or a lost rendering context.** The login screen comes back within seconds. `session.log` has the reason and `server.log.1` (after the next login) the backtrace.

## Known limits

The server's README has the full list under "Daily-driver known limits". The ones to expect in a first week:

- A real session on the NVIDIA driver with GDM has not been run yet, and explicit sync has not run on NVIDIA.
- The portal backend serves the appearance settings, the file chooser and screen sharing. A video call shares one display or one window at a time, chosen again on each share (nothing is remembered), without sound. There is no screenshot portal interface.
- The volume and mute keys change the default PipeWire output and the microphone mute key toggles the default input. With no PipeWire daemon, volume and mute fall back to the ALSA master mixer.
- X11 apps run through `xwayland-satellite`. Scale 2 and the clipboard were checked in a virtual machine only. With monitors of different scales, every X11 window gets the smallest scale.
- No terminal is shipped; the launcher falls back to `xterm`.
- A server crash ends the session; windows are not reopened.

To report a bug, see [Trying Cupertino](trying.md).
