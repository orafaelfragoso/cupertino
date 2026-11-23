# cupertino-shell-agent

The session agent of the Cupertino desktop: a component of the shell host (`cupertino-shell`, component `agent`, roles
`session` and `session_agent`). At login it starts the login items and reopens the applications the last session saved;
at the end of the session it shows the log out, restart and shut down panels, the power-button panel and the panel that
interrupts an end an application holds up, and asks the server's `org.cupertino.Session1` to carry the end out; throughout
it times the power key, posts the low-battery notifications and publishes the session menu's state for the menu bar.

## Contents

[Component](#component) · [Control interface](#control-interface) · [Session menu](#session-menu) ·
[Confirmation panel](#confirmation-panel) · [Quit sequence](#quit-sequence) · [Low battery](#low-battery) ·
[Login items](#login-items) · [Power key](#power-key) · [Reopening applications](#reopening-applications) ·
[What a restart loses](#what-a-restart-loses) · [Values applied](#values-applied) · [Modules](#modules) · [Checks](#checks)

## Component

`Agent` is a Kit application (`Application`, `Program`, `Component` of `cupertino-kit-shell`) the host runs on a thread
of its own with the CPU renderer: panels shown now and then need no GPU context. Its windows are overlay layer surfaces of
the server's first output. Its session bus connection is the one the host announces to the server (D20); the system bus
connection is Kit's (`cupertino_kit::bus::connect`), ticked by the component's loop. Every bus call is awaited on the loop
and every file is read or written on Kit's I/O pool (`cupertino_kit::task::io`); the component starts no thread and blocks
nowhere, which its `clippy.toml` (written by `cupertino-layers`) enforces. The followed signal streams are futures of
the loop, so they end with the component.

The server sends the session's log-out request (`cupertino_session_v1.log_out_requested`) to the holder of the
`session_agent` role, which the agent's connection holds: it arrives as `Event::Session(Change::LogOutRequested)` and
shows the log out panel. `Change::Locked(true)` cancels the panels.

## Control interface

`org.cupertino.SessionAgent1` at `/org/cupertino/SessionAgent1`, served on the component's session connection, which also
owns the name (`control`). The server calls it on the agent's accepted unique name from the connection that owns
`org.cupertino.Session1`; a call from any other connection gets `org.freedesktop.DBus.Error.AccessDenied` and nothing
happens (`cupertino_kit_shell::bus::caller`).

| Method | What the agent does |
|---|---|
| `RequestLogOut`, `RequestRestart`, `RequestShutDown` | shows the confirmation panel, in place of one that is open; ignored on a locked session |
| `RequestPowerDialog` | shows the power-button panel |
| `PowerKey(b pressed)` | the power key went down or up ([Power key](#power-key)) |
| `RefreshCapabilities` | reads again what the machine allows ([Session menu](#session-menu)) |

The agent calls `org.cupertino.Session1`'s `LogOut`, `Restart`, `ShutDown`, `Sleep`, `OpenApplications`,
`PendingInterrupt`, `RetrySessionAction`, `CancelSessionAction` and `ForceQuitApplication` on its accepted connection,
which the server authorizes as the agent component's.

## Session menu

The agent publishes `bus::Message::SessionMenu { user_name, lock, log_out, switch_user, sleep, restart, shut_down }` on
the host bus (retained, topic `Session`), which the menu bar takes from the `agent` sender alone. `user_name` is the
account's full name, else its login. `sleep`, `restart` and `shut_down` are on when logind's `CanSuspend`,
`CanReboot` and `CanPowerOff` answer yes or challenge, `switch_user` when the session manager
(`org.cupertino.LoginManager1`) owns its name, `log_out` and `lock` always (`capabilities`). It is read once both the
name and logind's answers are known, again on `RefreshCapabilities` and whenever the manager's name changes owner, one
reading at a time; what is off is logged with its reason when it changes (`cupertino-shell-agent: menu item off: …`).
Without a system bus only `log_out` and `lock` are on.

### Confirmation panel

A layer surface on the overlay layer (`Role::Layer`), anchored at the top-left corner of the first output and placed by its
margins: centred when it shows, with exclusive keyboard. Pressing on its background (not on a control) and dragging moves it, the
margins following the pointer and kept on the output. It is no modal dialog: when the focus leaves its window (a press outside it gives the
keyboard to what was under the press) the panel takes the keyboard on demand from then on, when it is pressed again. The server draws
the glass's shadow deeper while the panel has the keyboard (24 pt blur, 10 pt down, black at 0.7, 0.8 in the dark
appearance) than while it does not (16 pt, 4 pt down, 0.55 and 0.65). The countdown is 60 s.

The panel is an alert on glass with 26 pt corners, as wide as its checkbox row needs on one line (the label, its 22 pt
box and gap, and the 22 pt text inset on either side) and at least 260 pt. The height follows the vertical chain: 20 pt,
the 64 pt icon (drawn by the panel: a power symbol, a circling arrow for restart, a door with an arrow for log out) at the
text column's leading edge, 16 pt, the title (bold 13 on 16 pt lines, wrapped in the text column), 10 pt, the info line
(13 on 16 pt lines, tabular figures), 16 pt, the "Open my windows again after I log in" checkbox (a 16 pt row), 16 pt, the
buttons, 16 pt. Each element sits at the previous one's bottom edge, using the line counts the theme's text system
measures (`view::panel_frames`; the info line at the most lines it takes while counting down, so the size stays put). The
buttons are 28 pt capsules: Cancel and the confirming button side by side 8 pt apart, sharing the row between the 16 pt
side padding. The panel is a Kit window (`view::panel`): an alert dialog on the glass window variant's glass, the regular glass of an alert with the alert's drop shadow the compositor draws (the popover surface's fill
where the desktop shows none), Kit capsule buttons at the frames (the default one with the accent), and Kit's `CheckBox`
in the checkbox row, which has the focus when the panel shows.

- A second request replaces the open panel, and a lock closes it.
- Return does the default action, Escape cancels, a button click does that button's, and the end of the countdown does the
  default action. Tab moves the focus through the checkbox and the buttons; assistive technology clicks a button.
- The checkbox starts at the choice the person last confirmed (on until then). A click on the box or its label, Space, or a
  click from assistive technology (a `CheckBox` node with its checked state) flips it; the countdown goes on. Confirming
  remembers the choice, cancelling forgets a change. See [Reopening applications](#reopening-applications).
- Every confirmed action goes to the server's `org.cupertino.Session1` (below). Sleep quits no application; log out,
  restart and shut down first quit the applications, then the server asks the manager to log out (`LogOut`: the login
  screen takes the console first, then the session ends, so the desktop goes straight to the login screen; logind's
  `TerminateSession` when the manager does not take it) or logind to `Reboot` or `PowerOff`. Only a sleep `Session1` does
  not take is asked for by the agent itself: the session manager (`RequestPower`), or logind's `Suspend` without one. A restart or shut down logind refuses (another user's session is open, a block inhibitor) is logged by the server, which emits `SessionActionFailed(action, reason)`: the agent leaves its
  logging-out state (the power-button panel and log out work again), drops the saved list and posts "<action> did not
  start" with the reason, and the session goes on. A sleep the manager or logind refuses, or a call that fails, is logged by the agent.
- The words are in `panel::Kind::text`. The countdown is `panel::format_countdown`: seconds up to a minute ("60 seconds"),
  above that full units, at most two, with a no-break space between each number and its unit.

**Power-button panel.** `panel::Kind::PowerButton`, the same alert with the power icon and the checkbox, four capsules
stacked full width 6 pt apart, 16 pt under the checkbox: Shut Down (the default, Return), Restart (`r`), Sleep (`s`) and
Cancel (Escape) last. The title is the Shut Down panel's. There is no info line and no countdown, so the panel waits for
an answer.

### Quit sequence

A confirmed log out, restart or shut down goes to the server's `org.cupertino.Session1` (`LogOut`, `Restart`,
`ShutDown`; D13, `crates/cupertino-server/README.md`), which quits the applications and then carries out the action: it
brings the most recently used application with windows forward, asks each of its windows to close, gives it 45 s, asks
the next 0.1 s after one quits, and once none is left asks `cupertino-sessiond` to end the session or logind to restart
or power off. Sleep goes to `Session1.Sleep`, which quits no application; when `Session1` does not take it (not on the
bus, or the session is locked) the agent asks the session manager or logind directly, as `Session1` would.

Just before it asks for a log out, restart or shut down, the agent reads the applications open now
(`OpenApplications`, in the order they will quit) and saves them once for the next login
([Reopening applications](#reopening-applications)). A cancelled sequence, or a request `Session1` did not take, drops
the saved list again, since the session goes on.

Without `org.cupertino.Session1` (another compositor, the server's bus thread down) the agent quits nothing and does
nothing: it logs the failure and posts a notification ("Log out did not start", "Restart did not start", "Shut down did
not start") and the session goes on.

An application still there after 45 s makes the server emit `QuitInterrupted(app_id, reason)`, which brings the interrupt
panel (`interrupt`), on the same regular glass with 26 pt corners as the confirmation it interrupts: content 413 pt wide
and at least 128 high, a 64 pt icon at (20, 13), the title (bold 13) at (102, 13) and the info line 10 pt under it, both
wrapping at 293 pt in 16 pt lines, the buttons 12 pt under the info line (y 83 at the least) as 28 pt capsules 8 pt
apart, ending at the text column's end. Like the confirmation panels it moves when its background is dragged and is no
modal dialog. The server sends the signal once, so an agent that starts while an interrupt waits (it restarted) asks `PendingInterrupt` once its signals are subscribed, and one that gets an interrupt it did not start asks it too: the panel is worded as the action it names (a log out when the call fails). The panel is worded by the reason, the application's quit status:

- `kept-windows`: the application cancelled the log out. Try Again (the default) and Cancel.
- `closing`: it did not finish quitting. Force Quit, Cancel and Try Again.
- `not-responding`: it does not respond. Force Quit, Cancel and Try Again.

- Try Again or Return calls `RetrySessionAction`: the applications are asked to quit again, from the most recently used.
- Cancel or Escape calls `CancelSessionAction`: the sequence ends and the session goes on. The applications that quit
  stay closed.
- Force Quit asks first (Cancel, focused, goes back; Return presses nothing); its Force Quit calls
  `ForceQuitApplication(app_id)` (SIGKILL to the processes of its windows), and the server gives it another 45 s.
- An application that quits while its panel is up closes the panel (`QuitInterruptClosed`). A lock while the panel is
  up, or an interrupt that comes while the session is locked, cancels the action.

### Low battery

The agent reads `WarningLevel` and `Percentage` of UPower's display device on the system bus (served by `cupertino-powerd`,
which decides the level) and follows their changes and the daemon leaving and coming back. At `low` (3) and `critical` (4) it
posts a notification through `org.freedesktop.Notifications` (app name "Battery", icon `battery-low` / `battery-empty`, urgency
1 / 2), replacing the one it posted for the previous level. When the level drops back to `none` (external power) it withdraws it
(`low_battery::LowBattery`). A missing notification service or power daemon is logged, and the agent goes on.

### Login items

At start the agent runs the XDG autostart entries once per logind session. A marker `$XDG_RUNTIME_DIR/cupertino-autostart-<session>`
claims the run, so a restarted agent starts nothing. The directories are `$XDG_CONFIG_DIRS/autostart` (default `/etc/xdg`) then
`$XDG_CONFIG_HOME/autostart` (default `~/.config`). A user entry with the same file name shadows a system one. System items
start first, then the user's, each set by file name.

An entry is skipped when it is not `Type=Application`, has `Hidden=true` or `X-GNOME-Autostart-enabled=false`, has an
`OnlyShowIn` without, or a `NotShowIn` with, one of the desktops in `XDG_CURRENT_DESKTOP`, has a `TryExec` that is not found or
`Terminal=true` (no terminal to start it in), or has an `Exec` that does not parse. The field codes are expanded or dropped as
the Desktop Entry spec says. Each item is spawned through the host (`cupertino_kit_shell::host::spawn`) with `Path` as its directory and stdin
closed, and the host reaps it. The entries are read and the run claimed on Kit's I/O pool; the session is logind's
(`XDG_SESSION_ID`, else `GetSessionByPID`), and without one the login items and reopened applications wait for none and
never run.

### Power key

At start the agent takes a logind `handle-power-key` **block** inhibitor and holds it, so logind does not power off on the key.
The server binds the key (`XF86PowerOff`) to the `power-key` action, calls `org.cupertino.SessionAgent1.PowerKey(true)` on its press and `PowerKey(false)` on its release, in that
order, on the agent's accepted bus name, and drops the repeats of a held key (`crates/cupertino-server/README.md`). The timing is
the pure state machine `power_button::PowerButton`, fed with the calls and the component's clock:

| Key held | `PowerButtonSleepsSystem` off (`component::POWER_BUTTON_SLEEPS_SYSTEM`, the design's default) | on |
|---|---|---|
| released within 0.35 s | nothing | nothing |
| released between 0.35 s and 1.5 s | nothing | sleep (`Session1.Sleep`) |
| 1.5 s, still down | the power-button panel shows | the power-button panel shows |

The panel does not show over another panel or a log-out that was confirmed; the hold is logged and dropped. Once the panel is up
the release does nothing. With the session locked the press and the release are ignored and a key that was down is forgotten.
A release at or after 1.5 s shows the panel even when the loop has not yet seen the timer fire. The greeter has no session
agent and takes no inhibitor, so a hold there shows nothing: the server logs that no agent component is accepted, and logind
handles the key by its own configuration.

### Reopening applications

With "Open my windows again after I log in" on, the applications open at a log out, restart or shut down start again at the
next login (`restore`).

**What is saved.** The identities `Session1` names as open (`OpenApplications`) just before the agent asks for the action, in quit order, each resolved to an installed desktop entry of
the XDG application directories: the entry whose desktop file ID is the identity (with or without `.desktop`), else the first
whose `StartupWMClass` is it, ignoring case. Left out, each with a log line (`application not saved for the next login:
<identity>: <reason>`): an application without an entry, an entry hidden from listings (`NoDisplay`, `Hidden`, `OnlyShowIn`,
`NotShowIn`, a failing `TryExec`), one with `Terminal=true`, one whose `Exec` does not split, and the shell's own programs
(`restore::SHELL_PROGRAMS`: the server, the session manager, the shell host `cupertino-shell` and every component, shell
process and daemon, matched by identity, desktop file ID and the program of `Exec`). An entry two identities resolve to is saved once; at most 64 are kept.

**The file.** `$XDG_STATE_HOME/cupertino/restore` (default `~/.local/state/cupertino/restore`), replaced atomically, mode
0600. It holds the format version, the choice (`reopen = true|false`), the boot (`/proc/sys/kernel/random/boot_id`) and logind
session the list was saved in, and `app = <desktop file ID>` lines. Only desktop file IDs of letters, digits, `.`, `-` and `_`
ending in `.desktop` are read; never a path or a command. A missing file is the choice on and no list; a malformed line is
skipped, and a file of another version keeps only the choice. Without a boot ID the list is not saved.

**At login.** After the login items, the agent reads the list and writes the file back without it, keeping the choice, *before*
it starts anything; when that write fails nothing is started (`the saved list could not be emptied`). So a crash, a restart of
the agent and an unexpected end of the session never start a list twice, and a session that ends without a confirmed panel
(a crash, a power cut, `Super+Shift+Q` while the agent restarts) leaves no list. Dropped without starting anything: a list
saved with the choice off, and a list saved in this very boot and session (the agent restarted after a refused restart, then a
failure). Each ID is looked up again among the installed entries and started through its `Exec` with the file field codes
dropped, without a terminal or an activation token, stdin closed, through the host, which reaps it (`cupertino-apps`'s
`exec`). The file is read and emptied on Kit's I/O pool; the start is on the component's loop. Not started, each logged as `application not reopened: <id>: <reason>`: a removed or now hidden entry, a
shell program, a duplicate, an ID a login item started already (same file name) and a start that fails; the others go on.

**Windows and documents.** The agent starts applications; it does not restore windows, their places or the documents in
them, and Wayland gives it no way to. An application that wants to come back as it was keeps that state itself and restores
it when it starts.

## What a restart loses

The host restarts a failed agent alone (D16). A restart loses an open panel and its countdown (the request is not
replayed: the person asks again), a power key held at that moment, the low-battery notification's id (the next warning
posts a new one) and the agent's knowledge of a quit sequence under way: the server's interrupt is asked for again with
`PendingInterrupt` and adopted. It never starts the login items or the saved applications twice: the run is claimed per
session and the list is emptied before anything starts.

## Values applied

| Item | Value | Where |
|---|---|---|
| Confirmation panels | An alert on regular glass with 26 pt corners, at least 260 pt wide, padding 20 pt above and 16 pt around, the 64 pt icon, the title Bold 13 and the info line 13 pt on 16 pt lines, 28 pt capsule buttons 8 pt apart. | `panel`, `view::panel`, `view::widgets::AlertBody` |
| Checkbox row | 12 pt below the info line, 18 pt high on one line and 16 pt per line when its label wraps in the text column, 16 pt above the buttons. The power-button panel has none, so its buttons sit 16 pt below its title. | `panel::INFO_CHECKBOX_GAP`, `panel::CHECKBOX_HEIGHT`, `panel::CHECKBOX_LINE`, `panel::TEXT_BUTTONS_GAP` |
| Countdown formatter | A zero unit is dropped; two units join with ", ". The reference output was not captured. | `panel::format_countdown` |
| Panel text | Kit text: the title as body text in Bold, the info line as body text, wrapped at the text column. | `view::panel` |
| Icon disc | A neutral grey (0.60, 0.62, 0.66) under our own glyphs (approximate). | `view::panel_parts::icon_ops` |
| Interrupt panel buttons, words | Cancel 83 pt, Try Again 90 pt, Force Quit 100 pt, capsules 28 pt high and 8 pt apart, ending with the text column; no default button on the force quit question (Return does nothing). The words are ours; the design fills them in at run time. | `interrupt` |
| Quit order | Most recently used first; the design walks its process list from the end, order not read. | server `quit.rs` |
| Force Quit | No confirmation sheet. SIGKILL to the processes of the application's windows. | server `quit.rs` |
| Low-battery words | Ours; the design text is not in the strings read. | `low_battery` |
| Login items | Started without an activation token (no input event to ask for one). The design's delayed list (10 s) has no XDG counterpart. | `startup` |
| Reopened applications | Started without an activation token, after the login items, all at once. On until the person turns it off; the power-button panel has no checkbox and uses the saved choice. | `restore`, `startup` |

## Modules

| Module | Content |
|---|---|
| `panel` | `Kind`, `Action`, `Decision`, `Panel`, `Key`, `Placement`, `PanelCheckbox`: the countdown, the keys, the button and checkbox frames, the place on the output and the words of the confirmation panels and the power-button panel |
| `interrupt` | `Frames`, `Reason`, `Stage`, `Choice`, `title`, `info`, `press`: the interrupt panel |
| `power_button` | `PowerButton`, `Effect`: the power key's hold timing |
| `low_battery` | `LowBattery`, `Level`, `Notice`, `Action`: which notification to post, replace or withdraw |
| `capabilities` | `Capabilities`, `Inputs`, `Answer`: which menu actions work, from logind's answers and the manager's presence |
| `autostart` | `directories`, `list`, `parse`, `exec_argv`, `claim_run`: the XDG autostart entries |
| `restore` | `SavedState`, `parse`, `to_text`, `load`, `store`, `save`, `discard`, `choice`, `state_path`, `Applications` (`restorable`, `by_id`), `saved_list`, `take_at_login`, `launch`, `SessionMark`, `SHELL_PROGRAMS`: reopening applications at the next login |
| `session` | `org.cupertino.Session1` (async): `log_out`, `restart`, `shut_down`, `sleep`, `open_applications`, `pending_interrupt`, `retry_session_action`, `cancel_session_action`, `force_quit_application`, `follow_interrupts` (`Interrupt`), `report_failure` |
| `system` | logind, the session manager, the power daemon and the notifications (async): `session_id`, `inputs`, `block_power_key`, `request_power`, `follow_battery`, `follow_manager`, `post_notice`, `close_notice` |
| `control` | `org.cupertino.SessionAgent1`: `serve`, `Request` |
| `view` | the Kit windows: `panel`, `interrupt`, `panel_frames`, `interrupt_frames`, `Input` |
| `Agent` | the component (`component`; `component::actions`: carrying out, the interrupt and the battery; `startup`: the file work on the I/O pool) |

## Checks

- `cargo nextest run -p cupertino-shell-agent`: the panels, the interrupt, the power key's timing, the low-battery
  notices, the menu's capabilities, restore and autostart through the public interface.
- `crates/cupertino-session-tests/tests/shell-agent.rs`: the agent in the shared host; fifty restarts under injected panics
  (painting, dispatching, a helper thread) with the host's memory flat; a log out `Session1` asks for showing the agent's
  panel; the power key reaching it; the lock screen's `org.cupertino.Login1` with `Lock` alone.
- `cargo run -p cupertino-shell-agent --example agent_capture -- agent/SURFACE --sheet`: each panel in light and dark
  (`log-out`, `restart`, `shut-down`, `power-button`, `interrupt`), never with the real `HOME`.
