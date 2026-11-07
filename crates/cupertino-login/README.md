# cupertino-login

The lock screen, the greeter and the session agent of the Cupertino desktop, in one process. It stands in for the design's
login window. The lock screen locks the session on request, shows the clock, the user and a password field on one lock surface
per output, and unlocks only after PAM accepts the password. The greeter is the same screen as the login screen of the session
manager (`cupertino-sessiond`), with a user picker. The session agent shows the confirmation panels for log out, restart and shut down.

## Contents

[Running](#running) · [Lock screen](#lock-screen) · [Session agent](#session-agent) ·
[Reopening applications](#reopening-applications) · [Greeter](#greeter) ·
[Values applied](#reference-values-applied) · [Open issues](#open-issues) · [Modules](#modules) · [Tests](#tests)

The crate has no rendering or compositor dependency: `cupertino-seat` supplies the logind lookup, and it builds against Smithay's
session trait. The state machine (`model`), layout (`layout`), CPU renderer (`render`) and user and clock helpers (`user`) are
separate from the process, so a host can drive the screen without it. Sizes, positions, the shake, the fades and the hide timer
follow the analysis. Items marked approximate are authored
and replaceable.

This is the one shell process that links a library beyond the C runtime: `libpam`, through `pam-client2` 0.5.5 (MPL-2.0, the
maintained fork of `pam-client`, over `pam-sys2`), for the lock screen. The greeter links no PAM code: it talks to the session
manager through `cupertino-login-ipc` (`models/`), and the manager runs the authentication in a root worker.

## Running

Feature `app` builds the binary: a client of `cupertino-server` on `cupertino-wayland-client`, `cupertino-shell-client` and a `cupertino-ui-wayland`
presenter per surface, that needs the server's
`ext_session_lock_manager_v1` and `ext_idle_notifier_v1` globals besides the toolkit's usual ones. The server starts and
restarts it through the `shell` key of its configuration:

```json
{"shell": [{"name": "login", "command": ["cupertino-login"]}]}
```

```
cargo build -p cupertino-login --features app
cupertino-login [--idle-timeout SECONDS] [--retries-until-hint N] [--power-button-sleeps-system BOOL]
cupertino-login --greeter
```

| Option | Mode | Default | Effect |
|---|---|---|---|
| `--idle-timeout SECONDS` | lock screen | absent | Lock after the seat has been idle this long; at least 1. Absent means no idle lock of its own: the server asks for the lock after the display sleep. Refused with `--greeter`. |
| `--retries-until-hint N` | lock screen | 0 | `RetriesUntilHint`: failures until the password help shows. 0 shows it at 3. The greeter gets the setting from the session manager (`retries-until-hint` in `login.conf`). |
| `--power-button-sleeps-system BOOL` | lock screen | `false` | `PowerButtonSleepsSystem`, `true` or `false`; `false` is the design's default. See Power key. |
| `--greeter` | | off | Run as the login screen of the session manager. Takes no other option: the accounts, the power items, the hint setting and the session command are the manager's (`/etc/cupertino/login.conf`, see `cupertino-sessiond`). |

`XDG_SESSION_ID` names the logind session (else the process id, as in `cupertino-seat`). The greeter reads `CUPERTINO_LOGIN_SOCK`, the path of the manager's socket.

### What locks the session

Every trigger goes through the same request, which does nothing when the session is locked already.

| Trigger | How |
|---|---|
| `org.cupertino.Login1.Lock | Method on the session bus. The server asks for the lock over its own connection (`cupertino_session_v1.lock_requested`), not by this method; a log out the server asks for arrives the same way (`log_out_requested`) and shows the same panel as `RequestLogOut`. A second process cannot take the name: the lock screen then runs without it and logs an error. |
| logind `Lock` | Signal on this process's session: `loginctl lock-session`. |
| `PrepareForSleep(true) | The process holds a `sleep` **delay** inhibitor while the session is unlocked, drops it when the server confirms the lock, and takes it again after the unlock. |
| `LockNow | The lock shortcut (a lock the person asked for, request type 8): the same lock, but the interface hides after 5 s without input instead of 30 s. |
| display sleep | The server's idle policy calls `Lock` once the display has slept for the password delay (`power.conf`, below). |
| idle | With `--idle-timeout` only: `ext-idle-notify-v1`, which honours idle inhibitors. |

**Idle lock.** The design locks when the display sleeps (request type 5) once the password
delay has passed. The server owns that timing, since it knows the power source and the idle inhibitors: its idle policy dims,
sleeps the display and calls `Lock` after `require_password_after` (`cupertino-defaults` `power.conf`; default 300 s).
The display sleeps after 10 minutes on mains power and 2 on battery. `--idle-timeout` stays for a session
without that server.

### Log lines

The lines go to stderr, and the QEMU check matches them. The greeter logs the same `authentication failed` and
`authentication error: <reason>` lines as the lock screen.

| Line | When |
|---|---|
| `cupertino-login: locked` | Lock confirmed, once per lock. |
| `cupertino-login: authentication failed` | Wrong password. |
| `cupertino-login: authentication error: <reason> | PAM could not decide or refused the account; the screen stays locked and shows the reason. |
| `cupertino-login: unlocked` | After the unlock. |
| `cupertino-login: greeter ready` | First greeter frame. |
| `cupertino-login: picker: <n> users, other tile: <yes\|no>, <k> logged in` | Picker built, before `greeter ready`, and again when `Users` arrives; `k` rows carry the logged-in mark. |
| `cupertino-login: unlocked by a fast user switch` | The manager's `Unlock` reached a locked session: the person at the login screen proved the password, so the lock screen fades out without asking again. Logged when the screen is not locked: `unlock ignored: the session is not locked`. |
| `cupertino-login: reset` | The manager showed the screen again after a session ended. |
| `cupertino-login: power action: <restart\|shut-down\|sleep> | Greeter power menu item ran. |
| `cupertino-login: power items: <titles> | The power menu's titles, at start and when `Users` arrives: `Restart…, Shut Down…, Sleep` while a user session is open, else without the ellipsis. |
| `cupertino-login: power action authorised` | The administrator's password was accepted for a restart or shut down (see the power menu); the manager carries the action out, or sends `power action refused` and `reset`. |
| `cupertino-login: power action refused: <reason> | The manager sent `PowerRefused` for a greeter menu item. |
| `cupertino-login: <Restart\|ShutDown\|Sleep> refused: <error> | The manager refused the agent's `RequestPower` (another user's session is open, an inhibitor); the session goes on. |
| `cupertino-login: session announced to the session manager` | `SessionReady` was accepted at start (the manager started this session); a manager that does not know the session logs the refusal instead. |
| `cupertino-login: session announced again to the restarted session manager` | The manager's bus name got a new owner (it restarted or crashed): the agent calls `SessionReady` again so the manager knows whom to send `Unlock`. |
| `cupertino-login: panel shown: <log-out\|restart\|shut-down\|power-button> | A panel appeared. |
| `cupertino-login: panel cancelled` | Escape, Cancel, the lock, another panel or the server. |
| `cupertino-login: panel confirmed: <log-out\|restart\|shut-down\|sleep> | Return, a confirming button, `r` or `s`, or the countdown end; the manager call follows. |
| `cupertino-login: power key down`, `cupertino-login: power key up` | Server forwarded the key to an unlocked session. |
| `cupertino-login: power button: sleep` | Release in 0.35 s to 1.5 s with `--power-button-sleeps-system true`. |
| `cupertino-login: power button hold ignored: a panel or a log-out is running` | Held 1.5 s during a panel or a confirmed log-out. |
| `cupertino-login: panel request ignored: the session is locked` | Request during the lock. |
| `cupertino-login: quitting application: <app_id> (<n> windows) | Server asked to quit an application. |
| `cupertino-login: application quit: <app_id> | Last window gone. |
| `cupertino-login: application did not quit: <app_id> | 45 s passed; interrupt panel shows. |
| `cupertino-login: force quit: <app_id> (<n> processes) | Force Quit pressed. |
| `cupertino-login: <log-out\|restart\|shut-down> cancelled while quitting the applications` | Cancel, Escape or a lock on the interrupt panel. |
| `cupertino-login: no application left: <log-out\|restart\|shut-down> | logind's `TerminateSession` (log out) or the manager's `RequestPower` follows. |
| `cupertino-login: applications not quit before the <action>: <error> | `org.cupertino.Session1` did not answer; the call follows. |
| `cupertino-login: low battery: <Low\|Critical> at <p> %, notification <id> | Warning posted. |
| `cupertino-login: low battery cleared, notification <id> withdrawn` | Level back to none. |
| `cupertino-login: login item started: <id>`, `cupertino-login: login item skipped: <id>: <reason> | At start, per entry. |
| `cupertino-login: login items ran already in this session` | Restarted agent. |

## Lock screen

**Lock and unlock.** On a request the process asks for the lock and creates one lock surface per output (one more for an output
that appears later; it destroys the surface of one that goes away). It logs `cupertino-login: locked` once the server confirms,
and the surfaces draw from the first configure on. After a successful attempt the interface fades out, the surfaces are
destroyed, the server is told to unlock and the process logs `cupertino-login: unlocked`. Every error ends the process instead,
so the lock stays, as the protocol requires. A restarted process finds the session locked, gets `finished` and exits again until
the server offers a way back; that policy is the server's.

Everything but the wallpaper is one layer whose opacity is animated. Times are on the process's clock, and the state machine
(`model::LockScreen`) takes them as arguments.

| State | Meaning | Leaves when |
|---|---|---|
| `Locked` | Waiting for a password. | Return with a non-empty field: `Authenticating`. |
| `Authenticating` | The attempt is at PAM. The field takes a retry; Return waits. | The answer: success `Unlocking`, wrong password `Failed`, error `Locked` with a notice. |
| `Failed` | The field shakes for 0.4 s (not under Reduce Motion) and takes a retry; the pill is outlined in red until the person edits the field. | 0.4 s later: `Locked`. Return with a non-empty field: `Authenticating`. |
| `Unlocking` | The interface fades out. | The fade ends: `Unlocked`. |
| `Unlocked` | The process unlocks the session. | |

Keys: printable characters append (not with Ctrl, Alt or Super held), Backspace deletes, Return submits, Escape clears. An empty
field is not submitted. After 30 s without input the interface fades out; the next key or pointer input shows it again and is
not typed. A notice (an error that is not a wrong password, or text from PAM or the session manager) shows under the field until the next edit.

The only way to `Unlocked` is `LockScreen::finish_auth` with `AuthOutcome::Success` for an attempt that `LockScreen::press`
handed out. `finish_auth` without an attempt in flight is an error.

### Authentication

PAM service `cupertino-login`, file `/etc/pam.d/cupertino-login`. One PAM transaction per attempt runs on a worker thread:
`pam_authenticate`, then `pam_acct_mgmt`, so an expired or refused account does not unlock the screen. `AUTH_ERR` and `MAXTRIES`
from the first are a wrong password. Any other failure is an error: the screen stays locked and shows the reason, never an unlock. Text from PAM
modules (`PAM_TEXT_INFO`, `PAM_ERROR_MSG`: the `pam_faillock` lockout, a password about to expire) shows as the notice under the
field until the next edit, also after a wrong password. Every prompt but the password one is refused.

The process holds the typed password, so it starts by setting `PR_SET_DUMPABLE=0` and `RLIMIT_CORE=0` (`harden`): it writes no core file and its owner cannot read its memory or environment through `/proc`. Memory the process has freed is not zeroed here, and swap can hold a password (no `mlockall`).

The password lives in a 512-byte buffer (`PAM_MAX_RESP_SIZE`, allocated once so typing never copies it), zeroed when the attempt
is done. One copy escapes: `pam-client2` takes the prompt answer as a `CString` by value and drops it after copying it for PAM,
without zeroing it.

Service file for Arch Linux (checked against `/etc/pam.d` on the development host, which has `system-auth` with `pam_faillock`
and `pam_unix`):

```
#%PAM-1.0
auth       include    system-auth
-auth      optional   pam_gnome_keyring.so
account    include    system-auth
```

It includes `system-auth` and not `system-login`, which adds `pam_shells`, `pam_nologin` (refuses every user but root while
`/etc/nologin` exists, for example during shutdown) and the login session modules, none of which belongs to unlocking. Failures
count towards the account's `pam_faillock` limit, as for a login. `pam_unix` checks the process's own user through
`unix_chkpwd`, so the process needs no privilege. `pam_gnome_keyring` (skipped when it is not installed) runs only after the
password stack accepted the password and unlocks the user's keyring with it when it is locked (see
`cupertino-sessiond`'s README, "Secret Service"). A new drawing of this screen keeps passing the typed password to
`Authenticator`: that is where the keyring is unlocked, not in the UI.

## Session agent

The process that holds the lock screen is also the session agent. It serves `org.cupertino.Login1` on the session bus, at path
`/org/cupertino/Login1`.

Every method checks its caller (`cupertino-peer`): only the server, this process's parent, and this process itself may call.
The server also starts the applications, so a process with the same parent is refused. The menu bar does not call this
interface: it sends `cupertino_menu_bar_v1.session_action` and the server calls on its behalf (it needs the menu bar role,
the least privilege that fits). An anonymous, sandboxed (Flatpak, Snap) or unrelated caller gets
`org.cupertino.Error.AnonymousCaller`, `FlatpakCaller`, `SnapCaller` or `UntrustedCaller`, one warning is logged and nothing
happens. The properties stay readable by every caller.

| Member | Meaning |
|---|---|
| `Lock | Lock the session (the display slept, logind asked). |
| `LockNow | Lock the session because the person asked for it (the lock shortcut); the interface hides after 5 s without input. |
| `RequestLogOut`, `RequestRestart`, `RequestShutDown | Show the confirmation panel and return at once. Fail with `org.freedesktop.DBus.Error.Failed` while the session is locked. |
| `Sleep | Lock and put the computer to sleep, without a panel (the sleep preparation locks first). Fails while the session is locked. |
| `RefreshCapabilities | Read logind's policy and the session manager again; returns at once and `PropertiesChanged` for `Capabilities` follows when something changed. The menu calls it when it opens. Allowed on a locked session. |
| `RequestPowerDialog | Show the power panel (Restart, Sleep, Cancel, Shut Down), the one a held power button shows. Fails while the session is locked. |
| `SwitchToLoginWindow | Fast user switching: lock the session, and once the server confirms the lock, ask the session manager to give the console to the login screen (`cupertino-login: switching to the login window`). A session that is locked already switches at once. |
| `PowerKey(pressed: b) | The power key went down or up; the process times the hold. Takes effect on an unlocked session only and never fails. |
| `Locked` (`b`, read only, `PropertiesChanged`) | True from the request for the lock until the unlock. |
| `UserName` (`s`, read only) | The account's full name, else its login: the menu's "Log Out <name>…". Cleaned as untrusted text: control and bidirectional-override characters dropped, at most 64 characters. |
| `Capabilities` (`a{sb}`, read only, `PropertiesChanged`) | `sleep`, `restart`, `shut-down` (logind's `CanSuspend`, `CanReboot`, `CanPowerOff` answer yes or challenge), `log-out` and `lock` (always), `switch-user` (the session manager is on the system bus). Read at start, when the manager's name gets an owner and on every `RefreshCapabilities`, on a thread of its own (`probe`: the questions to logind never hold the agent's loop; requests in a burst are one read); what is off is logged with the reason when it changes (`cupertino-login: menu item off: …`). A manager that goes away is noticed at the next refresh. |

### Confirmation panel

A layer-shell surface on the overlay layer of the first output, without anchors so the server centres it, with exclusive
keyboard. The width, the buttons' x and widths (32 pt high) and the 60 s countdown are. The height
follows 's vertical chain: 20 pt, the title (bold 13 on 16 pt lines, wrapped at 305 pt), 8 pt, the info line (11 on 14 pt
lines), 7 pt, the "Reopen windows when logging back in" checkbox (an 18 pt row: a 14 pt box at the text column and its
label), 14 pt, the buttons, 13 pt. Each element sits at the previous one's bottom edge, using the line counts the renderer
measures (`render::Renderer::panel_frames`; the info line at the most lines it takes while counting down, so the size stays put).
With the design strings (two lines each) the log out, restart and shut down panels are 417 x 172 pt. The resize rule is.

- A second request replaces the open panel, and a lock closes it.
- Return does the default action, Escape cancels, a button click does that button's, and the end of the countdown does the
  default action.
- The checkbox starts at the choice the person last confirmed (on until then). A click on the box or its label, Space, or a
  click from assistive technology (a `CheckBox` node with its checked state) flips it; the countdown goes on. Confirming
  remembers the choice, cancelling forgets a change. See [Reopening applications](#reopening-applications).
- Sleep asks the session manager at once (`RequestPower`, see below). Without a session manager on the bus (a session another display manager such as GDM or SDDM started) restart, shut down and sleep call logind's `Reboot`, `PowerOff` and `Suspend` instead (not interactive: what the person may not do, or what an inhibitor blocks, is refused and logged like the manager's refusals). Log out, restart and shut down first quit the applications
  (below); then log out calls logind's `TerminateSession` with this process's session id, and restart and shut down ask the
  manager. The manager refuses a restart or shut down while another user's session is open and the caller is no administrator
  (`OtherUsersLoggedIn`), and while a program holds a block inhibitor (`Inhibited`): the agent logs the refusal and the session
  goes on. Any other failed call ends the process.
- The words are the design strings of, in `panel::Kind::text`. The countdown is `panel::format_countdown`:
  full units, at most two ("1 minute" at the start, "59 seconds" a second later).

**Power-button panel.** `panel::Kind::PowerButton`, 466 pt wide (143 pt high with the two-line title), no checkbox (its
restart and shut down follow the choice last confirmed on the other panels), four
100 pt buttons at the x origins 59, 157, 255 and 353: Restart (`r`), Sleep (`s`), Cancel (Escape) and Shut Down
(Return, the default button). The title is the Shut Down panel's. There is no info line and no countdown, so the panel waits for
an answer. The register's origins are 98 pt apart for 100 pt buttons, so neighbours overlap by 2 pt and a click there goes to
the left button. The title wraps at 354 pt and the buttons sit 21 pt below it (approximate), but never nearer than 14 pt to the
icon's bottom edge: the title chain alone would put the buttons under the icon (bottom edge 84 against a 73 pt chain).

### Quit sequence

Before a confirmed log out, restart or shut down the agent calls the server's `org.cupertino.Session1.QuitNextApplication
(`crates/cupertino-server/README.md`). The server brings the most recently used application with windows forward, asks
each of its windows to close, and emits `ApplicationQuit(app_id) when the last one is gone. The agent times it
(`logout::LogOut`): 45 s per application, the next asked 0.1 s after one quits, the logind call once the answer is
`("", 0).

An application still there after 45 s brings the interrupt panel (`interrupt`): content 413 x 128 pt, a 64 pt icon at (20, 13),
the title (bold 13) at (102, 13) and the info line (10 pt) at (102, 55), both wrapping at 293 pt, the buttons on y 83
(frames); Cancel and Force Quit.

- Cancel or Escape ends the sequence and the session goes on. The applications that quit stay closed.
- Force Quit calls `ForceQuitApplication(app_id)` (SIGKILL to the processes of its windows) and gives it another 45 s.
- An application that quits while its panel is up closes the panel. A lock while the panel is up cancels the log out.
- Without `org.cupertino.Session1` on the bus (another compositor) the agent logs it and goes straight to the call.

- While the applications quit, the agent records the identity the server names for each (`QuitNextApplication`'s `app_id`).
  Once none is left, and before the logind call or the power request, it saves them for the next login
  ([Reopening applications](#reopening-applications)). A cancelled sequence saves nothing; a restart or shut down the
  manager or logind refuses drops what was saved, since the session goes on.

### Low battery

The agent reads `WarningLevel` and `Percentage` of UPower's display device on the system bus (served by `cupertino-powerd`,
which decides the level) and follows their changes and the daemon leaving and coming back. At `low` (3) and `critical` (4) it
posts a notification through `org.freedesktop.Notifications` (app name "Battery", icon `battery-low` / `battery-empty`, urgency
1 / 2), replacing the one it posted for the previous level. When the level drops back to `none` (external power) it withdraws it
(`low_battery::LowBattery`). A missing notification service or power daemon is logged, and the lock screen goes on.

### Login items

At start the agent runs the XDG autostart entries once per logind session. A marker `$XDG_RUNTIME_DIR/cupertino-autostart-<session>`
claims the run, so a restarted agent starts nothing. The directories are `$XDG_CONFIG_DIRS/autostart` (default `/etc/xdg`) then
`$XDG_CONFIG_HOME/autostart` (default `~/.config`). A user entry with the same file name shadows a system one. System items
start first, then the user's, each set by file name.

An entry is skipped when it is not `Type=Application`, has `Hidden=true` or `X-GNOME-Autostart-enabled=false`, has an
`OnlyShowIn` without, or a `NotShowIn` with, one of the desktops in `XDG_CURRENT_DESKTOP`, has a `TryExec` that is not found or
`Terminal=true` (no terminal to start it in), or has an `Exec` that does not parse. The field codes are expanded or dropped as
the Desktop Entry spec says. Each item is spawned with `Path` as its directory and stdin closed, and its exit is reaped on a
thread.

### Power key

At start the process takes a logind `handle-power-key` **block** inhibitor and holds it, so logind does not power off on the key.
The server binds the key (`XF86PowerOff`) to the `power-key` action, calls `PowerKey(true)` on its press and `PowerKey(false) on
its release, in that order, and drops the repeats of a held key (`crates/cupertino-server/README.md`). The timing is
the pure state machine `power_button::PowerButton`, fed with the calls and the process's clock:

| Key held | `--power-button-sleeps-system` off (default) | on |
|---|---|---|
| released within 0.35 s | nothing | nothing |
| released between 0.35 s and 1.5 s | nothing | sleep (`Suspend(false)) |
| 1.5 s, still down | the power-button panel shows | the power-button panel shows |

The panel does not show over another panel or a log-out that was confirmed; the hold is logged and dropped. Once the panel is up
the release does nothing. With the session locked the press and the release are ignored and a key that was down is forgotten
('s lock assertions). A release at or after 1.5 s shows the panel even when the loop has not yet seen the timer fire. The
greeter has no session agent and takes no inhibitor, so a hold there shows nothing: the server logs a warning that nobody owns
`org.cupertino.Login1`, and logind handles the key by its own configuration.

## Reopening applications

With "Reopen windows when logging back in" on, the applications open at a log out, restart or shut down start again at the
next login (`restore`).

**What is saved.** The identities the quit sequence recorded, in that order, each resolved to an installed desktop entry of
the XDG application directories: the entry whose desktop file ID is the identity (with or without `.desktop`), else the first
whose `StartupWMClass` is it, ignoring case. Left out, each with a log line (`application not saved for the next login:
<identity>: <reason>`): an application without an entry, an entry hidden from listings (`NoDisplay`, `Hidden`, `OnlyShowIn`,
`NotShowIn`, a failing `TryExec`), one with `Terminal=true`, one whose `Exec` does not split, and the shell's own programs
(`restore::SHELL_PROGRAMS`: the server, the session manager and every shell process and daemon, matched by identity, desktop
file ID and the program of `Exec`). An entry two identities resolve to is saved once; at most 64 are kept.

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
dropped, without a terminal or an activation token, stdin closed, the exit reaped on a thread (`cupertino-apps`'s `exec` and
`launch::spawn`). Not started, each logged as `application not reopened: <id>: <reason>`: a removed or now hidden entry, a
shell program, a duplicate, an ID a login item started already (same file name) and a start that fails; the others go on.

**Windows and documents.** The agent starts applications; it does not restore windows, their places or the documents in
them, and Wayland gives it no way to. An application that wants to come back as it was keeps that state itself and restores
it when it starts.

## Greeter

`cupertino-login --greeter` is the login screen of the session manager, [`cupertino-sessiond`](../cupertino-sessiond/README.md).
The manager starts `cupertino-server --udev` as the account `cupertino-greeter` (`greeter-command` in `/etc/cupertino/login.conf`),
in a logind session of class `greeter` on the login VT. The server's configuration (`CUPERTINO_SERVER_CONFIG`, in the harness
`/etc/cupertino/greeter.json`) starts this process as its only shell process:

```json
{"shell": [{"name": "login", "command": ["cupertino-login", "--greeter"]}]}
```

The process never ends by itself: the server restarts it when it fails. It links no PAM code and reads no file about accounts. It
connects to the manager's socket (`$CUPERTINO_LOGIN_SOCK`, `/run/cupertino-login/greeter.sock`, mode 0600, owned by the greeter
account), greets it (`Hello`) and draws what the manager's `Welcome` says: the accounts of the picker with their full names and
pictures, whether "Other…" follows, which power items show and the hint setting. The protocol and its caps are in
`models/cupertino-login-ipc`. Pictures are PNG files: the manager reads `~/.face` (it does not decode) and the screen decodes it; a
picture that is not a PNG is logged (`cupertino-login: the picture of "<login>": ...`) and the tile shows initials.

The connection is tried once a second for 30 s when the process starts (the manager may be restarting); after that, and on a
broken or refused connection, the process ends with failure and the server restarts it after 1 s. A half-typed password is lost
then, which is intended.

**Conversation.** Return on the password screen sends `Authenticate` for the account (`Listed { uid }` for a tile, `Typed { name }`
for "Other…") and keeps the password until the manager asks for it. The screen holds no PAM state.

- A `Prompt` takes the password: a secret prompt, and a visible one too (its text shows as the notice). The password goes out as
  `Answer` in a zeroed buffer. A second `Prompt` after the password was used is more than the screen can answer: it sends `Cancel`
  and shows `authentication needs more than a password`.
- `Notice` (PAM's info and error text) shows under the field until the next edit.
- `Result { Accepted }` logs `session starting` and leaves the spinner running: the screen stays as it is until the manager shows
  it again with `Reset` (back to the picker). `Denied` is a wrong password or an unknown account (the same, so the screen cannot
  enumerate accounts): the field shakes, the failure counts for the chosen account and the screen logs `authentication failed`.
  The other outcomes show a notice and log `authentication error: <text>`: `Locked`, `Expired` and `Refused` show PAM's own text
  when it sent one, else ours (`This account is locked.`, `The password for this account has expired.`, `This account is not
  allowed to log in.`; approximate, the design's words are not in the strings read); `Busy` and `Internal` show ours only.
- `Reset` (the manager shows the screen again after a log out) clears every field and goes back to the picker; the screen logs
  `reset`. `Users` (the list changed) builds the tiles again with the new rows; a password screen being typed on stays.
- A broken connection ends the process with failure.

**Surface.** One full-output overlay layer surface per output with exclusive keyboard (`ext-session-lock` needs a session to
lock). The register has no difference between the login and the lock screen for the clock, the user view and the field (are the login window's own layout), so the greeter draws the lock screen's for the password, including the hide after
30 s without input (the picker does not hide). Messages from PAM within one attempt follow one another in the notice (a lock-out is two: that the account is locked and for how long). The manager reads the pictures, because the greeter's user cannot read a home directory.

**Power menu.** A power glyph sits at the top right of every greeter screen. A click or Tab opens a menu of Restart,
Shut Down and Sleep. Up and Down move, Return does the selected item, Escape closes. An item shows unless the manager's `Welcome`
says otherwise (`restart-disabled`, `shut-down-disabled`, `sleep-disabled` in `login.conf`, with `power-off-disabled` for an item
whose own key is absent). When no item shows, the glyph is hidden. The items send `Request::Power` to the manager (the greeter calls no logind) and
log `cupertino-login: power action: <restart|shut-down|sleep>`. The manager refuses an item the settings hide (`Disabled`), a
restart or shut down while any user session is open without an administrator's authentication (`OtherUsersLoggedIn`) and one a
program blocks (`Inhibited`). The greeter then logs `cupertino-login: power action refused: <reason>, shows the
reason as the notice on the password screen (the picker has no notice line) and stays up. There is no confirmation panel on the
login screen.

**Ellipsis and administrator's authentication.** While a user session is open (`will_prompt` of the manager's
`Welcome` and `Users`) the titles are `Restart…` and `Shut Down… (the design's, set from
). Choosing one then opens the name screen (the "Other…" tile's) with the notice "Other users are
logged in. To restart, enter an administrator's name and password." (ours, approximate), then the password screen; the screen sends
`Request::AuthenticatePower { name, action }` instead of `Authenticate`. Escape goes back to the picker. The manager accepts a
member of `wheel` with the right password, carries the action out and logs `power action authorised` on the screen; a name that is
no administrator, a wrong password and a lock-out are the login screen's usual failures. The design passes this through the
system's authorisation rights (`RestartShutdownAuthRefMgr`), whose dialog is not in the payload; the dialog here is ours. The
in-session agent has none: its restart and shut down keep being refused for a non-administrator while another user is logged in.

### User picker

The greeter starts on the picker: one tile per row of the manager's list, sorted by name without regard to case, then the
"Other…" tile when it shows. The manager decides who is a login user (an account of `/etc/passwd` whose user id lies in
`UID_MIN` … `UID_MAX` of `/etc/login.defs` and whose shell is not `nologin` or `false`) and applies the filters of. There
is no Guest tile: the design shows it only with its remote-locate feature on. Tiles sit in the grid of: n = min(M, tiles) with M = 25 on an output larger
than 1024 x 768 points and 12 otherwise, rows and columns from the register's tables, 5 x 5 for 25. Tile i is on page i / M, at
column k % columns and row k / columns for k = i % M. One page shows at a time.

| Input | Effect |
|---|---|
| Arrow keys | Move the selection one tile (left, right: across pages too) or one row (up, down: within the page). |
| Return, a click on a tile | Choose the selected account: the password screen for it. |
| Pointer over a tile | Select it. |
| Escape on the password screen | Back to the picker, unless an attempt is being decided. |

"Other…" opens the name screen: the "Other…" avatar and a field that takes the name as text (no whitespace, 32 bytes at most,
Return to go on, Escape to go back). Nothing is looked up for it, so the screen does not say whether the account exists; the
manager decides at the attempt. A failed attempt counts for the account
that was chosen (the password help, below).

### Account filters

The filters moved to the session manager, which applies them before it sends the list (`filter.rs` in `cupertino-sessiond`; the
keys of `login.conf` and the rules are in its README). This screen only draws the result and logs
`cupertino-login: picker: <n> users, other tile: <yes|no>, <k> logged in` once, before `greeter ready`. A list the filters leave empty without an
"Other…" tile ends the process with an error: nothing could log in.

Not built, because there is no equivalent on this system: `Hide500Users` (the picker already lists only the `login.defs` id
range), `HideLocalUsers`, `HideMobileAccounts`, `LocalUserLoginEnabled`, `IncludeNetworkUser` and `MaxNetworkUsers` (there are no
directory-bound, mobile or network accounts), and the design's third "Other…" reason (a directory-bound machine that allows
network users).

## Values applied

| Item | Value |
|---|---|
| Shake | 0.4 s, `v = 0.5 + 0.5 sin(6 pi p) d`, `d = 1` for `p < 0.5` else `exp(-0.8 p)`, `x = (v - 0.5) * 2 * 14` pt, zero at `p = 1`; none under the session's Reduce Motion (the lock screen; the greeter has no person's options) |
| Failure outline | 2 pt red around the password pill after a wrong password, until the person's next edit, with or without Reduce Motion (approximate) |
| Fade of the interface | linear, `\|to - from\| * 0.25 s` |
| Interface hides after | 30 s without input, 5 s after a lock the person asked for (the lock shortcut: server `LockNow`, lock request type 8); the greeter always 30 s |
| Date | size 0.028 S, first baseline 0.09 H, `EEEEMMMMd` as `%A, %B %-d` |
| Time | size 0.12 S, first baseline 4 date sizes below the date's, `Jmm` as `%H:%M` |
| User view | 20 pt below the time's bottom; avatar 70 pt with 7 pt above and below; name baseline 23 pt below the view, 15 pt, at most 150 pt wide |
| Avatar halo and ring | shadow radius 15 opacity 0.25 (a radial fade), ring 1 pt white 0.1 |
| Password field | 160 x 28 pt, fully rounded, system 13 Medium, placeholder white 0.5 |
| Wallpaper treatment | black 0.25, then black 0.3, then a radial black 0.2 to 0 from the middle to the corner with the darken blend |
| Idle lock | after the display sleep plus the password delay, timed by the server |
| Quit sequence | one application at a time, 45 s each, the next 0.1 s after one quits; interrupt panel 413 x 128 |
| Login items | global items before the user's, one after the other |
| Longest password | 512 bytes |
| Password help | failed attempts are counted per user and restart at 1 when another user fails; with `RetriesUntilHint` R = 0 (absent) the help shows at 3 failures and without hint text, with R > 0 at R failures (`hint::FailedAttempts`); the shake plays on every failure |
| Picker | grid rows and columns by n = min(M, count), M = 25 above 1024 x 768 else 12, 5 x 5 for 25 and outside 1 … 24; item i at page i / M, column k % columns, row k / columns; users sorted by case-insensitive name, then "Other…" |
| Spinner | drawn from numbers, no artwork: 12 spokes from r 11.77 to r 26.57 and 5.26 wide on a 64 unit canvas, white; 24 frames of 1/24 s, frame `round(frac(t) * 24) mod 24`; head spoke `floor((f - 1) / 2) mod 12` clockwise from 12 o'clock, spoke k steps behind it has `ramp[11 - k]` (ramp A on even frames, B on odd); shown in the field's trailing slot (in place of the badge) once an attempt has been pending for 1.0 s, from frame 0 (`spinner`, `model::LockScreen::spinner`) |
| Caps lock badge | shown while the field has the focus and Caps Lock is on (xkb `Lock` modifier from `wl_keyboard.modifiers`, read from the transport's `Connection::modifiers`); an 18 pt slot at the pill's trailing edge, a 16 x 16 pt rounded square (radius 4) stretched by `min(h / 17, 1.2) = 1.2 in the 28 pt pill, white, glyph cut out; no animation |

### Approximated values

| Item | Value | Where |
|---|---|---|
| Wallpaper | Procedural gradient with two blobs in the server's dark desktop colours. The design shows the user's wallpaper, which the process cannot ask for yet. | `render::wallpaper` |
| Password field fill | White 0.2. The design uses material 27, a backdrop that lock surfaces do not get. | `render::PILL_FILL` |
| Dots | 6 pt, 11 pt apart, at most 11 shown (the room left by the badge's slot on both sides). | `render` |
| Gap between name and field, notice position | 10 pt (the spinner gap of), 20 pt. | `layout` |
| Initials, text | Grey fill, initials 0.4 of the avatar; white text, date Medium, time Semibold of the substitute family. | `render` |
| Picture | `~/.face` as PNG, cropped to a circle. A file that does not decode is an error; a missing one means initials. | `user` |
| Panel body, icon, buttons | Opaque grey body with 12 pt corners, a plain disc, the accent blue on the default button. | `render`, `panel` |
| Administrator | Members of `wheel` in `/etc/group`; the design's admin group has no counterpart. | `users::ADMIN_GROUP` in `cupertino-sessiond` |
| Checkbox row | 7 pt below the info line, 18 pt high with a 14 pt box (3 pt corners; the default button's blue with a white check when on, the cancel button's fill with an edge when off) and a 13 pt label 6 pt right of it, 14 pt above the buttons. The power-button panel has none, so its buttons sit 21 pt below its title. | `panel::INFO_CHECKBOX_GAP`, `panel::CHECKBOX_HEIGHT`, `panel::CHECKBOX_BOX`, `panel::CHECKBOX_BUTTONS_GAP`, `panel::TITLE_BUTTONS_GAP`, `render::CHECKBOX_*` |
| Icon to buttons gap | At least 14 pt below the icon, so the power-button panel's buttons (from x 59) clear it. | `panel::ICON_BUTTONS_GAP` |
| Greeter placeholders | Plain English. | `render::PLACEHOLDER` |
| Countdown formatter | A zero unit is dropped; two units join with ", ". The reference output was not captured. | `panel::format_countdown` |
| Panel text baselines | First baseline 13 pt (title) and 11 pt (info) below the line box top; the design font metrics are not available. Columns, line boxes, wrap widths and the height rule are (the height rule ). | `render::PANEL_TITLE_BASELINE`, `render::PANEL_MESSAGE_BASELINE` |
| Caps lock glyph | The arrow-over-bar cut-out is ours; gives path constants but not their order. No tooltip (text not decoded). | `render::CAPS_GLYPH` |
| Spinner size and cap | 16 pt square in the 18 pt slot (intrinsic size not in the payload read); round caps at both ends, the design has a flat inner end. | `render::SPINNER_SIZE`, `render::Canvas::spinner` |
| Password help | One line 32 pt above the bottom edge (offset, section 2), our words. No hint text: a threshold above 0 only moves the failure count at which the help shows. | `render::PASSWORD_HELP` |
| Logged-in mark | A 9 pt disc (green, 0.20 0.72 0.35) with a white check at the lower right of the avatar of a user who has a session. approximate: the design marks such users in its picker, the shape and place are not read. | `render::LOGGED_IN_FILL` |
| Picker tiles | 186 x 112 pt cells (150 pt name plus 18 pt borders, avatar, name line), grid centred from the password screen's user view. Selected tile: white 0.2 rounded square of 86 pt (the register's selection size), 20 pt corners. "Other…" avatar: grey disc with an ellipsis. "Larger than 1024 x 768" means both sides larger. | `layout::TILE_WIDTH`, `layout::TILE_HEIGHT`, `render`, `picker::max_per_page` |
| Power item and menu | The status bar is only the power item: 22 pt square, 20 pt from the trailing edge, 6 pt from the top; a ring with a 60 degree gap and a stem, 5.5 pt radius, 1.6 pt stroke. Menu 150 pt wide, 24 pt rows, 4 pt padding and gap, 8 pt corners. Items. | `layout`, `render` |
| Interrupt panel buttons, words | Cancel 83 pt at x 214, Force Quit 100 pt at x 295, 32 pt high, ending with the text column; no default button (Return does nothing). The words are ours; the design fills them in at run time. | `interrupt` |
| Quit order | Most recently used first; the design walks its process list from the end, order not read. | server `quit.rs` |
| Force Quit | No confirmation sheet. SIGKILL to the processes of the application's windows. | server `quit.rs` |
| Low-battery words | Ours; the design text is not in the strings read. | `low_battery` |
| Login items | Started through `Command` without an activation token (no input event to ask for one). The design's delayed list (10 s) has no XDG counterpart. | `app` |
| Reopened applications | Started without an activation token, after the login items, all at once. On until the person turns it off; the power-button panel has no checkbox and uses the saved choice. | `restore`, `app` |

**Text.** The `ui` font is a variable font whose optical size follows the point size, so every `TextRenderer` owner calls `set_scale` with the output scale before it draws and measures (`wrap` measures in points, the drawing helpers at the canvas scale): text is laid out in points at the same optical size it is drawn with, at every output scale.

## Open issues

- **Not implemented.** The name shadow, the rest of the status bar, the grace period, auto log-out, display sleep after the hide, the picker's
  page indicator and the animation between the picker and the password screen ('s 0.5 s layout change, peekaboo and the
  cooldown), 's status bar, and the sessions the manager does not know (a hidden account's) in the power menu's ellipsis.
## Modules

| Module | Content |
|---|---|
| `model` | `LockScreen`, `Phase`, `Key`, `AuthOutcome`, `Password`, `shake_offset` and the timing constants |
| `hint` | `FailedAttempts`: the per-user failure count and when the password help shows |
| `layout` | `Layout`: the frames and baselines of the screen in points |
| `power` | `Action`, `Menu`, `MenuKey`: the power menu's items and its selection |
| `picker` | `Picker` (`new` takes the accounts and whether "Other…" shows; `NoTiles` when nothing is left), `Tile`, `Grid`, `cell_of`, `max_per_page`, `NameEntry` |
| `render` | `Renderer`, `View`, `PickerView` |
| `spinner` | the spinner's frame, head spoke and spoke opacities |
| `user` | `User::current` (account, full name, picture), `User::from_row` (a row of the manager's list), `initials`, `format_time` |
| `panel` | `Kind`, `Action`, `Decision`, `Panel`, `PanelCheckbox`: the countdown, the keys, the button and checkbox frames and the words of the confirmation panels and the power-button panel |
| `power_button` | `PowerButton`, `Effect`: the power key's hold timing |
| `logout` | `LogOut`, `Do`: the timing of the quit sequence |
| `interrupt` | `Frames`, `Choice`, `title`, `info`, `press`: the interrupt panel |
| `low_battery` | `LowBattery`, `Level`, `Notice`, `Action`: which notification to post, replace or withdraw |
| `autostart` | `directories`, `list`, `parse`, `exec_argv`, `claim_run`: the XDG autostart entries |
| `restore` | `SavedState`, `parse`, `to_text`, `load`, `store`, `save`, `discard`, `choice`, `state_path`, `Applications` (`restorable`, `by_id`), `saved_list`, `take_at_login`, `launch`, `SessionMark`: reopening applications at the next login |
| `control` (feature `dbus`) | the `org.cupertino.Login1` interface (`serve`, `Service::set_locked`, `Service::set_capabilities`) |
| `probe` | `run`: the thread that reads logind and the manager on request and publishes the capabilities |
| `capabilities` | `Capabilities`, `Inputs`, `Answer`: which menu actions work, from logind's answers and the manager's presence |
| `logind` (feature `dbus`) | the session's `Lock` and `PrepareForSleep` signals, the sleep and power key inhibitors, `TerminateSession` |
| `login_manager` (feature `dbus`) | the manager's `org.cupertino.LoginManager1`: `request_power`, `session_ready`, `switch_to_login_window`, `watch_unlock` (the manager's unicast `Unlock`; `Error::refusal` names the expected refusals) |
| `upower` (feature `dbus`) | `watch` (the display device's `WarningLevel` and `Percentage`), `post` and `close` (the notification calls) |
| `session` (feature `dbus`) | `quit_next_application`, `force_quit_application`, `watch_quit`: the server's `org.cupertino.Session1` |
| `auth` (feature `app`) | `Authenticator`: PAM on a worker thread |
| `manager` (feature `app`) | `Connection`, `Message`: the greeter's connection to the session manager and the thread that reads its events |

The process (`main`, `options`, `screen`, `app`, `greeter`) is the binary's.

## Tests

```
cargo test -p cupertino-login --features app
```

| Test | Proves |
|---|---|
| Shake, fade, state machine | The shake curve (peaks of 14 pt, zero outside), the linear fade, typing and the byte limit, wrong password, PAM error and success paths, a result with no attempt in flight, the hide after 30 s, the layout of a 1920 x 1080 output. |
| Exhaustive inputs | Every sequence of up to five inputs and failures never reaches `Unlocking` or `Unlocked`. |
| Panels and power key | Countdown, keys, button and checkbox frames, clicks, the checkbox's start state, Space and its node for assistive technology, the power-button panel's four buttons, its keys and its lack of a countdown and checkbox; key timing at 0.2, 0.5 and 1.5 s with and without sleeping. |
| Saved state (`restore`) | The file round-trips with IDs only, mode 0600; a missing file is the choice on; malformed lines, other versions, invalid IDs, duplicates and the 64 cap; identities resolve by ID then `StartupWMClass`; shell programs, terminal entries and bad `Exec` lines are refused; the list is emptied on disk before it is handed out and only once, dropped when saved in the same boot and session or with the choice off, and not handed out when it cannot be emptied; launching skips removed entries, duplicates, login items and failed starts. |
| `tests/restore.rs` | A stand-in `org.cupertino.Session1` on a private `dbus-daemon` names the applications as they quit; the saved list in a fixture home holds only their desktop file IDs, and the next login starts the one still installed once through its `Exec`; a failed start leaves the rest going. |
| Options | The session options, and that the greeter takes none. |
| `tests/harden.rs` | The running greeter (waiting on a compositor that never answers) has core file size 0 and refuses its owner the read of `/proc/<pid>/environ`; each of the two calls removed fails it. The check of the environment is skipped as root. |
| `org.cupertino.Login1` on a private `dbus-daemon` | `Lock`, a second process cannot take the name (`serve` fails with `NameTaken`; `serve_unnamed` serves the object on the unique name), `Request*` refused while locked, `Locked` announced once per change; a host process (`dbus-send`) is refused every method with `UntrustedCaller`, and a process with the login process's parent (a stand-in for an application the server started) is refused `Sleep` while the parent is not. |
| Quit sequence, low battery, autostart | One application at a time, the 0.1 s pause, the 45 s interrupt, Force Quit, the interrupt panel; the notification posted, replaced and withdrawn; the autostart parser (`Hidden`, `OnlyShowIn`, `NotShowIn`, `X-GNOME-Autostart-enabled`, `TryExec`, `Exec` quoting, shadowing, order, the once-per-session marker). |

Not covered by a test:

- The bus calls of the quit sequence and the notifications. The server side of `org.cupertino.Session1` is `cupertino-server`'s
  `tests/quit.rs`, and the low-battery path runs in the QEMU harness. The process's start, with a saved list in a fixture
  home, is `cupertino-server`'s `tests/session_restore.rs`.
- A real log out, restart or shut down with the checkbox on, and the next real login.
- The logind calls and signals. Against a private bus with a stand-in logind they showed the two inhibitors.
  A real `Lock` signal would lock the developer's session. The manager's side of `RequestPower` and `SessionReady` is tested in
  `cupertino-sessiond` (`tests/service.rs`) and by the QEMU checks `28` and `29`.
- The drawing of the panel and of the greeter, and the PAM success path (it needs the user's password).
- The lock and unlock cycle against a server with `ext-session-lock`: the phase 5 QEMU check, and `cupertino-shell-client`'s
  `tests/protocol-flow.rs` for the lock it uses.

On the development host a wrong password through the real PAM stack (`pam_unix`, service `vlock`) returned `Denied`, and the sleep
inhibitor showed up in `systemd-inhibit --list`.
