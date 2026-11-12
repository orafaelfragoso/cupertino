# Using Cupertino every day

A short guide for the first days on a machine that uses Cupertino as its session. Everything here is taken from the crate READMEs and from changes that are on `main`; the server's README (`crates/cupertino-server/README.md`) has the detail. Cupertino is early (0.x): this page says what exists and how it is meant to be used, not that it is finished. Where a step has not been run on real hardware, the page says so.

## Starting and leaving a session

- **Start.** On a machine installed from the USB image (see [Install from a USB stick](install-usb.md)), `cupertino-login` starts the session on tty1: log in there. On other machines, the package installs the session entry `cupertino.desktop`, so GDM lists **Cupertino** in its session chooser; choose it, then log in. Do not turn on automatic login for this session while it is new: when the compositor ends for any reason, the session script ends and GDM shows its login screen again, and automatic login would start the session again at once.
- **Leave normally.** Log out from the menu bar's system menu or with `Super+Shift+Q`. Applications are asked to quit first, and a confirmation panel appears. While the login process is restarting after a crash, `Super+Shift+Q` ends the session without asking.
- **Reopen applications.** The log out, restart and shut down panels have **Reopen windows when logging back in**, on until you turn it off; the panel remembers your choice. With it on, the applications that were open start again once at the next login, through their desktop entries. Applications without a desktop entry and the shell's own programs are not saved. Each application opens as it does when you start it: getting its windows and documents back needs support in the application itself. After a crash or a power cut nothing is reopened.
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
| `Ctrl+Up`, `F3`, the overview key | overview (the same key closes it) |
| `Ctrl+Left`, `Ctrl+Right`, `Ctrl+1` to `Ctrl+9` | workspaces |
| `Super+Q`, `Super+W`, `Super+M`, `Super+H` | quit application, close window, minimize, hide application |
| `Super+Ctrl+F`, `Super+Ctrl+Left`, `Super+Ctrl+Right` | full screen, tile left, tile right |
| `Super+Alt+Escape` | Force Quit |
| `Super+Alt+S` | screen reader on or off, at the login screen and the lock screen too, the one shortcut that works on the lock screen besides switching consoles (the first of the installed readers, `/usr/bin/orca`; the same as **Settings > Accessibility > Screen reader**; no shortcuts file can take it while a reader is installed) |
| `Shift+Super+/` | Help menu with its search field focused |

To change a shortcut, add a `chord = action` line to `~/.config/cupertino/shortcuts.conf`, for example `Super+x = exec:foot`, or `Super+Tab = none` to free a chord. The file is applied on top of the defaults. `modifier_profile = classic-pc` in `~/.config/cupertino/keyboard.conf` switches to Windows and Linux habits (`Alt+Tab` switches, `Alt+F4` closes, `Ctrl` is the command key in Settings and text fields, and a tap of `Super` alone opens the launcher).

Under the default profile `Super` is the command key in every application. Other applications get `Super` chords as the `Ctrl` chords they bind, and a `Ctrl` chord on a letter, digit, punctuation key or `Return` reaches them with `Super` instead, so `Ctrl` copies, pastes and closes nothing. In a text field the line-editing chords `Ctrl+A`, `E`, `B`, `F`, `P`, `N`, `D` and `H` move and delete as their keys do. Terminals keep `Ctrl`. `client_rewrite = off` in `keyboard.conf`, or the application's id in `rewrite_off`, sends every chord as it is pressed.

## Layouts, Compose and dead keys

- **Layouts.** At login Cupertino reads the system layout (`X11Layout`, `X11Variant` and `X11Options` from `localed`). If `keyboard.conf` has no `input_sources`, those layouts become the input sources. To set them yourself, write `input_sources = us, de, fr(nodeadkeys)` in `~/.config/cupertino/keyboard.conf`; that file wins over `localed`.
- **Switching.** `Ctrl+Space` opens the input source list and steps through it (most recently used first); releasing the modifiers picks one. `Ctrl+Alt+Space` steps to the next source in menu order.
- **Option characters.** Both `Alt` keys act as Option (not under `modifier_profile = classic-pc`, where `Alt` stays `Alt`): `Alt+2` types ™, `Alt+Shift+2` types €, and `Alt+E` then `E` types é (on a US source, which uses the `us(mac)` symbols). Applications then receive no `Alt` from these keys, so their own `Alt` shortcuts do not work; `layout_level3 = right-alt` in `keyboard.conf` gives the left key back as `Alt`.
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

Files go to the Desktop folder unless `~/.config/cupertino/screenshot.conf` sets `location` to `documents`, `pictures`, `clipboard` or a directory. The toolbar writes the file back. On the overlay, Escape cancels, Space switches to picking a window, the arrow keys make and move a selection (Alt with an arrow resizes it) and Return captures. The screenshot tool has no built-in video recorder. External recorders can use the ScreenCast portal: in OBS choose Screen Capture (PipeWire), then select a display or window. The OBS black-capture buffer-negotiation regression is fixed in `fe28a392`; the local recording verified desktop frames after replacing the portal. See [Portals](portals.md#screen-sharing) for scope and remaining limitations.

## Apps that control the desktop

The first time an app asks to see your windows and their controls and use the pointer and keyboard, or to take pictures of the screen, you are asked, and your answer is remembered for that app in `~/.config/cupertino/desktop-control.conf`. **Settings > Privacy > Desktop Control** and **Settings > Privacy > Screen Capture** list the apps that asked, each by the name of its app or, for a program that runs without a window of its own, by its file name and full path. Turning an app's switch off stops it at once, even in the middle of what it is doing; turning it on allows it again. Forget removes the app, which then asks again the next time. The two pages are separate: an app can be allowed to take pictures of the screen and not to control the desktop. The file must be yours and writable by no one else; otherwise the pages say why and change nothing.

## Saved passwords

Apps keep saved passwords and tokens in the keyring: gnome-keyring, which the packages install. Logging in with your password unlocks it, and logging out locks it again. After an automatic login, or after your password was changed with `passwd`, it stays locked: the first app that needs it opens a window that asks for the keyring's password, and Cancel there refuses the app. Unlocking the lock screen with your password also unlocks the keyring.

There is no Cupertino window for saved passwords yet. In a terminal, `secret-tool search --all ATTRIBUTE VALUE` shows the matching items (with their secrets), `secret-tool clear ATTRIBUTE VALUE` deletes them, and the Seahorse app ("Passwords and Keys", installed separately) lists and deletes them in a window. `cupertino-sessiond`'s README ("Secret Service") has the details and the commands that list every item.

Not verified: none of this has been run in an installed session yet; the README lists the checks.

## Logs and crash reasons

`cupertino-diagnose` checks the GPU driver, the session, the portal, the services and the last crash in one go, and names
the fix for what is wrong; `cupertino-diagnose --json` gives the same as JSON.

| File | Holds |
|---|---|
| `~/.local/state/cupertino/session.log` | one line per session start and end: a normal end, the exit status, the signal that ended the compositor, or the watchdog. When the compositor ends with an error, the end line also has the reason: a panic on its main thread or a lost rendering context for which the server could not make a new renderer. A GPU reset that the server survives is one line of its own (see [A GPU reset](#a-gpu-reset)), and so is a session that has only a software renderer (see [No GPU acceleration](displays-and-gpus.md)). An abort or a fault (SIGABRT, SIGSEGV) is named as the reason, and `server.log` has its backtrace, as it has a panic's. Other signals leave no reason; the line names the signal. |
| `~/.local/state/cupertino/server.log` | everything the compositor logs, including the GPU, the driver, the kernel and any panic. The previous run is `server.log.1`. An error that stops the start is its last line, `the server stopped: ...`. |
| `~/cupertino-tty.log` | what the processes print when started with `run-cupertino.sh`, including a start that fails before `server.log` opens. |

`RUST_LOG` changes the log filter, and `CUPERTINO_LOG_FILE` moves `server.log`. After a panic, or a lost rendering context for which the server could not make a new renderer, the teardown may take 2 s, then the server exits.

## A GPU reset

On a display, the server asks the render GPU whether it was reset before each round of frames, and a frame that fails with the context lost counts as a reset too. On a reset the server makes a new renderer for each card and draws again: the windows stay, the clients stay connected and nothing restarts. A driver without the robustness extension reports no reset, so only a frame that fails with the context lost starts this.

Each reset is one line in `session.log`:

```text
<time> the GPU was reset: gpu=<card> reason=<who caused it>; <outcome>
```

The outcome is `renderer remade, the clients stayed connected` or `renderer not remade (<error>), the session ends`.

The limits: a fourth reset within 60 s, or a renderer that cannot be made, ends the session. The login screen comes back, as after any lost rendering context. A client's buffer that the reset lost shows what the driver left in it until the client draws again.

Not verified on hardware: the new renderer has run only through the offscreen test hook (`tests/gpu-reset.rs`), not against a real reset.

To investigate a reset with an AI assistant:

1. Run `cupertino-diagnose --area crash --json`. The `crash.gpu_reset` check is `warn` when `session.log` has a reset, with the facts `resets`, `remade`, `last_time`, `last_gpu`, `last_reason` and `last_outcome`.
2. Copy the lines of `~/.local/state/cupertino/server.log` around `last_time` (`server.log.1` if the session has restarted since).
3. Give both to an AI assistant, or to the driver's bug tracker. Paste them by hand; no Cupertino service does this yet.

## If it hangs

- **One application does not answer.** The compositor pings every application window once a second. With no answer 2 s after a ping, the pointer over its window shows the busy cursor and the dock marks it Not Responding. Press `Super+Alt+Escape`, or use Force Quit in the menu bar. An application that does not answer is killed at once; any other gets a close request first and is killed 3 s later if it is still there.
- **A shell process fails** (dock, menu bar, launcher and so on). The compositor starts it again, at most once per second. A process that exits successfully stays stopped.
- **The compositor hangs.** `Ctrl+Alt+Backspace` and `Ctrl+Alt+F<n>` need a live compositor. From another console or over ssh, `pkill cupertino-server` ends it. As a last resort, with `sudo sysctl kernel.sysrq=1` set beforehand, `Alt+SysRq+R`, `E`, `I` returns the keyboard to the kernel and ends all processes. Save your work in other sessions first.
- **After a panic, an abort, a fault, or a lost rendering context with no new renderer.** On a display the server first makes a new renderer and the windows stay ([A GPU reset](#a-gpu-reset)). The login screen comes back within seconds. `session.log` has the reason and `server.log.1` (after the next login) the backtrace.

## Known limits

The 2026-11-10 installed-session review and follow-up inspection found or recorded 32 open interaction and presentation issues. See [Desktop quality and open issues](desktop-quality.md) for the complete list, evidence status and acceptance checks. In particular, menu pointer ownership, sidebar bounds, Control Center readability/connectivity, Trash emptying, quick preview, input/cursor behavior and glass/scrollbar consistency are not accepted as finished. Newly installed applications also remain absent from the launcher until its process restarts (DQ-30); the backlog records the workaround. ChatGPT Desktop cannot be moved by dragging through XWayland; launching it with `--ozone-platform=wayland` restores dragging in the maintainer's test (DQ-33). Electron applications that draw their own titlebar, such as ChatGPT and Claude Desktop, keep it instead of Cupertino's by design (DQ-31, closed). The backlog records the app-specific flags file and remaining checks. ChatGPT status-menu actions can also stop working because the menu bar retains obsolete action IDs (DQ-32). Some entries are maintainer reports still awaiting focused reproduction.

The server's README has the full list under "Daily-driver known limits". The ones to expect in a first week:

- A real session on the NVIDIA driver with GDM has not been run yet, and explicit sync has not run on NVIDIA.
- The portal backend serves the appearance settings, the file chooser, the app chooser, screen sharing, screenshots and colour picking; every other portal goes to the GTK backend ([Portals](portals.md)). A video call shares one display or one window at a time, without sound. A display you let an app share again without asking is listed in **Settings > Privacy > Screen Sharing**, where Forget removes it; a window is remembered only until the app quits. The screenshot portal asks for consent and retries a failed capture, and its colour picking takes the colour of the pixel you click.
- The volume and mute keys change the default PipeWire output and the microphone mute key toggles the default input. With no PipeWire daemon, volume and mute fall back to the ALSA master mixer.
- X11 apps run through `xwayland-satellite`. Scale 2 and the clipboard were checked in a virtual machine only. With monitors of different scales, every X11 window gets the smallest scale.
- The ISO and installer ship `foot`. Other installations need one of the terminals listed under keyboard shortcuts.
- A server crash ends the session; windows are not reopened. Applications are reopened only after a confirmed log out, restart or shut down with the checkbox on.

To report a bug, see [Trying Cupertino](trying.md).
