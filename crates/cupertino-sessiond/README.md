# cupertino-sessiond

The session manager: the root-owned half of the login. It stands in for the console role of the design's login window and
replaces greetd. The design is; this file says
what is built.

The manager owns `/run/cupertino-login/greeter.sock`, where the login screen (`cupertino-login --greeter`, an unprivileged
compositor client) draws the picker and sends passwords. It starts the login screen's compositor, and later one worker per
authentication attempt and per session. It links no rendering, font, image or Wayland code, and decodes nothing the screen sends
beyond the typed messages of [`cupertino-login-ipc`](../cupertino-login-ipc/README.md).

## Service

| | |
|---|---|
| Runs as | root (the main process and the workers until they drop privileges) |
| Binary | `cupertino-sessiond` (feature `app`): the manager, or with the argument `worker` one worker, started with `exec` from `/proc/self/exe` |
| Configuration | [`/etc/cupertino/login.conf`](../cupertino-defaults/src/login.rs) (`CUPERTINO_LOGIN_CONF` overrides the path); a bad or missing file ends the manager |
| Socket | `/run/cupertino-login/greeter.sock`, mode 0600, owned by `greeter-user`; one connection at a time; `SO_PEERCRED` uid must be the greeter's |
| Bus | `org.cupertino.LoginManager1` on the system bus, root-owned (policy: `org.cupertino.conf`); see "The bus interface" |
| Unit file (QEMU image) | [`cupertino-login.service`] |
| Image files | [`login.conf`], PAM services `cupertino-greeter`, `cupertino-session` and `cupertino-autologin`, [sysusers] |

**The package's unit** (`cupertino-login.service`, `Alias=display-manager.service`) differs from the harness's in two lines only:
`Conflicts=getty@tty1.service` and the alias (`Type=notify`: the manager sends `READY=1` once its socket listens, the login screen's
session is starting and the bus name is taken), because the harness keeps its getty session on VT 1.

```
[Unit]
After=systemd-user-sessions.service systemd-logind.service dbus.socket getty@tty1.service
Conflicts=getty@tty1.service
[Service]
Type=notify
ExecStart=/usr/lib/cupertino/cupertino-sessiond
Restart=always
RestartSec=1
KillMode=process
LimitCORE=0
RuntimeDirectory=cupertino-login
RuntimeDirectoryMode=0755
RuntimeDirectoryPreserve=restart
[Install]
Alias=display-manager.service
```

`KillMode=process`: workers and the sessions they supervise survive a crash of the manager (losing every user's session
because the small daemon crashed is the worst outcome available); the new process adopts them (see "What the manager does"). `RuntimeDirectoryPreserve=restart` keeps `/run/cupertino-login` (the once-per-boot auto login flag) over a restart or a crash and removes it on a stop. There are no sandboxing options: the workers' children inherit
the unit's restrictions and a session must be able to run `sudo`.

**PAM services.** `cupertino-greeter`: `auth` and `account` `pam_permit.so`, `session` `pam_loginuid.so` and `pam_systemd.so`.
`cupertino-session`: `auth`, `account` and `session` include `system-local-login` (`pam_faillock`, `pam_unix`, `pam_loginuid`,
`pam_systemd`, `pam_limits`, `pam_env`), then `pam_gnome_keyring` for `auth` and `session` (see "Secret Service"). `cupertino-autologin`: `auth` `pam_permit.so`, then `pam_faillock.so authsucc` (optional: it makes the tally file the lock screen, which runs as the user, records failures in), `account` and `session` include `system-local-login`, then `pam_gnome_keyring`'s `session` line. `cupertino-login` (the lock screen's) adds `pam_gnome_keyring`'s `auth` line. **Accounts.** `u cupertino-greeter -
"Cupertino login screen" / /usr/bin/nologin` (sysusers; home `/` because the worker's `chdir($HOME)` must not fail and the
account has no private directory), no group.

## Administrators

An administrator is a member of any of the administrators' groups. They are `admin-groups` of `login.conf` (group names
separated by commas, spaces around them allowed; empty names nobody) when that file is root-owned and writable by root alone, the same rule as auto
login; otherwise the build's: `CUPERTINO_ADMIN_GROUPS` at compile time (the Arch packages leave it unset), else
`wheel`. An untrusted file's `admin-groups` is logged and the build's groups apply. The bus checks ask
the account's groups at every call (`getgrouplist`, so a primary group and a directory service's groups count); the login
screen's administrator list and the power dialog read the members `/etc/group` lists, so an account whose primary group is
an administrators' group, and is not listed there, is not found by them.

## `login.conf`

One `key = value` per line, `#` starts a comment. Keys (`cupertino-defaults::login`): `vt` (1), `greeter-user`
(`cupertino-greeter`), `greeter-command` and `session-command` (required, absolute program path, split at spaces),
`hidden-users`, `deny-list`, `allow-list`, `hide-admin-users`, `show-other-users`, `restart-disabled`, `shut-down-disabled`,
`sleep-disabled`, `power-off-disabled`, `retries-until-hint`, `auto-login` (an account name; off when absent: see "Auto login"),
`admin-groups` (see "Administrators"). A bad line is logged and its key takes its safe value (the table in
`cupertino-defaults::login`: a bad filter lists nobody and offers "Other…", a bad power switch disables the item, a bad
`admin-groups` names no administrator, a bad `auto-login` is off); only a missing or bad greeter or session command, or a
missing file, keeps the manager from starting. A line without `=` is a bad line of the key it starts with
(`hide-admin-users false` hides the administrators). A misspelled key is only logged, with one summary line after the
bad lines: after editing the file run `cupertino-sessiond --check-config`, which prints each bad line and exits non-zero
when there is one, and also names `auto-login` or `admin-groups` the manager would ignore because the file is not root's alone (it reads the file and starts nothing; a file only root can read needs root), or check the journal. The filter and power keys are the login window preferences of the
reference; they were options of `cupertino-login --greeter`.

## What the manager does

| Step | Detail |
|---|---|
| Start | `PR_SET_DUMPABLE` 0 and `RLIMIT_CORE` 0; read `login.conf`; resolve the greeter account (not root); bind the socket; hold `/dev/ttyN` of the login VT open (`VT_OPENQRY` skips a terminal something has open); start the greeter worker |
| Greeter | the worker authenticates `cupertino-greeter` through PAM, opens a session of class `greeter` on the login VT, and runs `greeter-command` as that account with `CUPERTINO_LOGIN_SOCK` set |
| Hello | the screen's first message must be `Hello` with this protocol version; the manager answers `Hello` and `Welcome` (roster, power items, `will_prompt`, hint setting). Anything else is refused and the connection closed |
| Roster | (`logged_in` marks the accounts that have a session the manager started: see "Fast user switching") `/etc/passwd` rows with a uid in `UID_MIN..=UID_MAX` of `login.defs` (1000 and 60000 when unset) and a shell other than `nologin` or `false`; the filters of (`filter.rs`); at most 1024 rows and 1 MiB of pictures. Directory-service accounts are not listed; "Other…" reaches them |
| Pictures | `~/.face` is read on a short-lived thread with `setfsuid(uid)`, `O_NOFOLLOW` and `O_NONBLOCK`, and `fstat` insisting on a regular file of that user of at most 256 KiB. Not decoded. A bad file means no picture, logged once per user per start |
| Attempt | (an account that has a session already is only checked, see "Fast user switching") `Authenticate` names a listed account (it must be a row of the last list) or a typed name (validated again by `is_login_name`); anything else is answered `Denied` after 2 s, the time `pam_unix` takes for a wrong password (`pam_faildelay`; approximate). Otherwise the manager holds the first free terminal above the login VT (`VT_OPENQRY`; none free is `Busy`) and starts a worker with the PAM service `cupertino-session` and `session-command` from `login.conf`. The screen's choices never reach the worker's command, environment or terminal |
| Conversation | the worker's prompts become `Prompt` events, its info and error text `Notice` events; each `Prompt` carries a `conversation` id (unique over the manager's life) and the screen's `Answer` repeats it: only the answer to the prompt that is open goes back to the worker, once; an answer for a finished or cancelled attempt, a replay, or one that follows the screen's reconnect is dropped with a log line and its secret is zeroed. A prompt of any other style, or a prompt when no attempt is open, is answered with a cancel |
| Outcome | `pam_authenticate` and `pam_acct_mgmt` map to `Denied` (`AUTH_ERR`, `USER_UNKNOWN`), `Locked` (`MAXTRIES`, and `AUTH_ERR` when a module printed a message before the first password prompt: `pam_faillock`'s `preauth` says "The account is locked due to 3 failed logins." and the stack goes on to ask and fail with `AUTH_ERR`, so the worker tells the lock-out from a wrong password by the conversation, not by the code; the text is not matched), `Expired` (`ACCT_EXPIRED`, `NEW_AUTHTOK_REQD`), `Refused` (`PERM_DENIED`), else `Internal` with the cause in the journal only |
| Limits | at most 10 `Authenticate` per rolling 60 s (the 11th gets `Busy` and starts nothing; `limits::RateLimit`); an attempt that waits 60 s for the screen to answer a prompt, or for its worker to answer an answer or to open the session, is cancelled: the worker is killed and the screen gets `Internal` (or `Reset` once the attempt was accepted). The design has no back-off or lock-out on this class of machine, so these bound what a compromised screen costs and are not parity; the account lock-out is `pam_faillock`'s |
| Cancel | before the attempt is accepted the worker is killed; once it is accepted the session starts regardless |
| Session start | after `Accepted` the screen gets `SessionStarting { uid }`; the worker opens the session and reports `Started { session_id, leader, uid }`; the manager then closes its end of the channel (the worker supervises the session alone), reads whether logind left the new session inactive (a session on a terminal that is not in the foreground stays inactive) and calls `ActivateSession` |
| Ready gate | the screen keeps its spinner until the session's agent calls `SessionReady` on the bus, at most 15 s; then the manager sends `Reset` and `Users`. **After the switch, not before** (deviation from the design, which gates the activation): the compositor of a session on a background terminal waits for the terminal and starts none of its shell processes (`backend/udev.rs`), so the agent can announce nothing until the session is active. A session that was not announced in 15 s is logged (`sent no ready signal within 15 s`) and the screen is reset all the same |
| Power | `Request::Power` from the screen and `RequestPower` on the bus end in one function: an item the settings hide is `Disabled` (the screen only), a restart or shut down while another user's session is open is `OtherUsersLoggedIn` (the screen: any user session, unless an administrator authenticated, see "Administrator's authentication"; an agent: a session of another user, unless the caller is an administrator, see "Administrators"), else logind's `RebootWithFlags`, `PowerOffWithFlags` or `SuspendWithFlags` with `ROOT_CHECK_INHIBITORS`, so a block inhibitor of another user stops it (`Inhibited`; logind ignores the requester's own inhibitors, root's included) |
| Session ends | the leader exits, so its worker closes the PAM session and exits; the manager then calls `TerminateSession` for what the leader left in the session (logind removes a session only when its scope is empty) and watches logind's `SessionRemoved`. That signal, and the seat's `ActiveSession` changes, count only from the connection that owns `org.freedesktop.login1` when they arrive (`logind::Logind::watch`): the bus delivers a signal addressed to the manager whatever its match rules say, and a forged `SessionRemoved` would drop the record of a running session (test `only_logind_removes_a_session_or_moves_the_seat`). When the session whose leader exited is the seat's `ActiveSession`, the manager returns to the login screen at once (activates its session, sends `Reset` and `Users`): an app that ignores the end can keep the scope, and so the removal, for the stop timeout, and a crashed compositor must not leave the console on a terminal nothing draws on. On the removal of a session it started it releases the terminal and reads the seat's `ActiveSession`: with none (the session that ended was the one in the foreground) it activates the login screen's logind session and sends the screen `Reset` and `Users` (an attempt that is not accepted yet ends first); otherwise the session ended in the background and the screen only gets `Users` (what is typed on it stays). A session that dies with its server is therefore an ended session; the user's server is not restarted (the design's session ends too) |
| Fast user switching | `UserRow.logged_in` is true for an account with a session in the manager's table (deviation from the design, which reads logind's `ListSessions`: a getty's session of the same user is no session of the manager's and has no agent to unlock). `Authenticate` for such an account runs the worker in `Mode::Verify`: `pam_authenticate` and `pam_acct_mgmt`, then the transaction ends with the worker (no credentials, no session). On `Accepted` the manager sends the agent registered by `SessionReady` the unicast signal `Unlock` (the person has just proved the password: unlock result 4,), activates the session and shows the login screen again for the next time. `SwitchToLoginWindow` (bus, from the foreground session's agent after it locked its screen) activates the login screen's session and resets the screen. While logind has not made the target the foreground (polled every 100 ms, 5 s at most) a second `SwitchToLoginWindow` is ignored and an `Authenticate` is `Busy` |
| Adoption | at start the manager lists the sessions of `seat0` in logind and adopts those a PAM service of its own opened (`Service` is `cupertino-greeter` for a `greeter` session of the greeter account, `cupertino-session` or `cupertino-autologin` for a `user` session; a getty's session is not ours, and a `closing` one is skipped; deviation from the design, which adopts every `user` session, so that the roster and the bus interface treat the harness's getty session as before). The login screen's session is not started again; a user's goes into the session table (ready, its terminal held again, `VTNr`) and its agent registers by calling `SessionReady` again, which the agent does when the manager's bus name gets a new owner. An adopted worker is not a child of this process, so nothing reports that the session's command exited: logind's `Leader` is the worker that opened the session, and its process descriptor (`pidfd_open`) tells the manager the worker ended, which happens when the command exited and the PAM session is closed. Then the manager calls `TerminateSession` for what is left, as it does after a worker it started, and logind's `SessionRemoved` does the rest (return to the login screen, or a new login screen session after 1 s for an adopted one, within the five-in-30-s rule). A leader that is gone at adoption has its session terminated |
| Auto login | see "Auto login" |
| Administrator's authentication | `will_prompt` in `Welcome` and `Users` is true while logind has a user session (the same rule as the refusal above): the screen titles Restart and Shut Down with an ellipsis and, instead of sending `Power`, sends `AuthenticatePower { name, action }`. The manager answers a name that is no administrator (see "Administrators"), an invalid name, an `action` of `Sleep` or a hidden item like a wrong password (`Denied` after 2 s; `Refused` for the last two); otherwise it runs a `Mode::Verify` worker for the typed account (PAM service `cupertino-session`, so `pam_faillock` counts the failures). On `Accepted` it answers `Result`, then carries the action out as `Requester::Administrator` (the open sessions no longer refuse it; the settings and block inhibitors still do) and, when logind refuses, sends `PowerRefused` and `Reset`. The in-session agent has no such dialog (the manager takes no secret on the bus): a non-administrator's agent is still refused with `OtherUsersLoggedIn`, and the person can switch to the login window and use the dialog there. The password and the name are never logged |
| Greeter ends | its worker exits and the login screen's session is started again after 1 s; five exits within 30 s stop that (logged, the manager stays up, the other terminals keep their getty) |
| logind | the manager ends with failure when logind's signal stream ends (logind restarted); systemd restarts the unit. The design's single in-process reconnect is not built |

## Auto login

`auto-login = <account>` in `login.conf` logs that account in once per boot without a password (the semantics are unknown in the
reference and the design is 2.10; off by default, user decision 2026-10-25). It
skips authentication, so:

1. **The settings file must be trusted** (`auto_login::settings_are_trusted`): a regular file (a link is refused) that root owns and
   that group and others cannot write, in a directory with the same properties. Otherwise the manager logs `auto login refused: <reason>`
   and starts as if the key were absent (the login screen is the safe state; the manager does not stop).
2. **Once per boot.** When the login screen's session is up (started, or adopted after a restart) the manager takes the claim
   `/run/cupertino-login/auto-login-done` (`O_CREAT|O_EXCL`, so a crash racing a restart cannot log in twice). The file lives on a
   tmpfs and survives a restart or a crash of the manager (`RuntimeDirectoryPreserve=restart`), so those, and a log out, do not log in
   again. The claim is taken before the login, so a failed auto login is not repeated either.
3. **The account** must be an ordinary account with a login shell (the roster's rule: `UID_MIN..=UID_MAX`, not `nologin`), and must have no
   session already. **Deviation from the design:** the design skips auto login when any user session exists; the check here is the
   account's own sessions, because the QEMU harness keeps a getty session of its own account on VT 1 for the other checks. A
   session of another account does not prevent it.
4. The worker runs as `Mode::Open` with the PAM service `cupertino-autologin` and the same session command, terminal allocation,
   privilege drop and ready gate as a typed login; the login screen is told nothing (no attempt is open).

If the login screen's session cannot be started (five failures in 30 s) the auto login runs at that point instead, so a broken
login screen does not keep the computer from its user.

## The bus interface

`org.cupertino.LoginManager1` at `/org/cupertino/LoginManager1` (`service.rs`). Every method authorises the caller's process, and
none takes a session argument: it acts for the caller's own session. The bus daemon names the process (`GetConnectionCredentials`,
asked on a second connection to the bus, `cupertino_peer::lookup::Lookup`, so that its answers, each with a process
descriptor, never queue behind a flood of calls until the bus daemon refuses every other caller of the manager),
logind names its session (`GetSessionByPID`) and `caller::authorise` decides, the rule of `cupertino-powerd` (no polkit,
the architecture notes in `docs/architecture.md`). A caller passes when its session is of class `user` on `seat0`, the process runs as the session's user (root
inside a user's session is refused: it has no use for the manager), and for `RequestPower` the session is in the foreground.

| Method | Needs | Does |
|---|---|---|
| `RequestPower(s) | the active user session | `restart`, `shut-down` or `sleep`; errors `AccessDenied`, `InvalidArgs`, `OtherUsersLoggedIn`, `Inhibited`, `Failed` (all `org.cupertino.LoginManager1.Error.*) |
| `SwitchToLoginWindow | the active user session | the login screen takes the console; the agent has locked its screen first. Ignored while a switch is under way. Errors: `Failed` (no login screen, logind refused) |
| `SessionReady | a user session, active or not | the session's agent is up: the manager registers the caller's unique name as the session's agent and ends the ready gate. A session the manager did not start (a getty's) gets `UnknownSession`; a repeated call (the agent restarted) registers the new caller |
| `SetSystemAccessibility(a{ss})` | an administrator at the console, through the installed settings application | writes the machine's accessibility defaults, `/etc/cupertino/accessibility.conf` (`system_accessibility.rs`); see below |
| `CanSetSystemAccessibility → b` | — | whether the caller would pass the same checks now: a hint for the settings application, a method because the answer depends on the caller; the setter checks again |

The program check is defence in depth, not the boundary: a program of the same user can still start the real
`cupertino-settings` with `LD_PRELOAD` or trace its own child (as Yama's `ptrace_scope` allows), and a root-owned
interpreter hard-linked into a user-writable prefix would pass where `fs.protected_hardlinks` is off. The boundary is an
administrator at the console in the active session, and what it guards is bounded to `SYSTEM_KEYS`. A Settings window that
was open while a package upgrade replaced the program runs a deleted file (`/proc/<pid>/exe` ends in ` (deleted)`), so its
writes are refused until it is opened again.

**The machine's accessibility defaults.** The first write from a user to a file of root's, so its checks are stricter than the
other methods':
- The caller is the process behind the `ProcessFD` the bus daemon gives (`cupertino_peer::Caller::from_process_fd`); a daemon
  that gives none (older than dbus 1.16) is refused, and the service says so once at start (`cupertino_peer::NO_PROCESS_FD`). It must be unsandboxed and run `<install root>/bin/cupertino-settings` (`/usr`, and the build's
  `CUPERTINO_INSTALL_PREFIX`), and the file it runs (stat through `/proc/<pid>/exe`) must be a regular file owned by root and
  writable by no group or other user (`service::only_root_changes`), so a program of the user's under a writable prefix is
  refused. Its session (logind `GetSessionByPID` by that process) must be the active user session of
  `seat0`, opened there (`Remote` false) and in `State` `active` (`Need::Console`), else `NotAtConsole`; its user must be in
  one of the administrators' groups (see "Administrators") by `getgrouplist`, asked at every call (`users::is_administrator`), else `NotAdministrator`. The process must still be
  the one the descriptor holds after these lookups.
- The call is refused whole before anything else when it has more entries than the allowed keys or a key or value over 64
  bytes, then when a key is not one of `SYSTEM_KEYS` or a value is not one the file takes (`InvalidArgs`). Only then does it
  count against the limit: one write a second per user and five a second in all (`TooManyWrites`).
- The keys it names replace the file's; the others keep the file's values. `/etc/cupertino` is made (755) when missing (also
  at the manager's start, so every compositor, the login screen's first, can watch the file in it) and a link or other file
  there is refused; the file is written as a new file beside it (exclusive, not through a link, 644),
  flushed, renamed over `accessibility.conf`, and the directory is flushed. The detailed reason of a refusal is in the
  journal; the error names are what the settings application shows (its own sentences).
- A person who deletes their own `accessibility.conf` while signed in keeps their settings until their next login: the
  compositor reads whether they have one at start.

**Signal `Unlock()`**, sent by the manager to one agent only: the unique name that called `SessionReady` for the session (a `dbus-daemon` delivers a message with a destination to that destination alone). The agent checks each `Unlock` against the current owner of the manager's name, and only root may own that name, so no other process can send it (the subscription's `sender` alone keeps out nothing addressed to the agent's unique name). A session without a registered agent is only activated (`session N has no agent to unlock it`).

The methods run on the connection's thread and wait at most 10 s for the main loop's answer (the loop never blocks longer than a
logind call). The main loop is fed by a channel and an eventfd, the way logind's signals are.

**polkit stays in the QEMU image (deviation).** The design removes it in this unit. With polkit masked logind answers `access denied`
for what the compositor and the agent still ask of it as an unprivileged user: `Seat.SwitchTo` (Ctrl+Alt+Fn; measured: `switching the
virtual terminal failed ... AccessDenied`), the server's system sleep and `Inhibit` (measured: `Failed to inhibit: Access denied`).
The first two belong to `cupertino-seat` and `cupertino-server`, which this unit does not own. The agent's restart, shut down and
sleep no longer call logind; removing polkit needs a `SwitchVt` method here (greeter and user sessions), the server's sleep through
`RequestPower`, and a brokered inhibitor (a fixed set of two locks, returned as a descriptor), in that order.

## The worker

`cupertino-sessiond worker` runs the privileged path in this order and no other:

1. `PR_SET_DUMPABLE` 0, `RLIMIT_CORE` 0.
2. (`Mode::Verify` ends after step 3: the account has a session, and only the password is checked.) `pam_start` with the service and login of the manager's `Begin`, `PAM_TTY`, `pam_authenticate`, `pam_acct_mgmt`.
3. `XDG_SESSION_TYPE=wayland`, `XDG_SESSION_CLASS`, `XDG_SEAT=seat0`, `XDG_VTNR` into the PAM environment with `pam_putenv`;
   `pam_setcred(ESTABLISH_CRED)`, `pam_open_session` (`pam_systemd` registers the logind session and exports `XDG_SESSION_ID`,
   `XDG_RUNTIME_DIR`).
4. `fork`. The child: `setsid`, `initgroups`, `setgid`, `setuid`, then it checks that `setuid(0)` fails (and that the real and
   effective ids are the user's), `chdir($HOME)` (a failure is an error, no fallback to `/`), `umask(022)`, default signal
   dispositions and an empty mask (this resets the ignored `SIGPIPE`), standard input from `/dev/null`, `close_range` above
   descriptor 2, `execve` with exactly the PAM environment, `HOME`, `USER`, `LOGNAME`, `SHELL`, `PATH` (`ENV_PATH` of
   `login.defs`) and the configured extras. A failure in the child comes back through a close-on-exec pipe.
5. The worker reports `Started { session_id, leader, uid }`, waits for the leader (it forwards `SIGTERM`, `SIGHUP` and `SIGINT` to
   it), closes the PAM session and exits with the leader's status. The login screen's worker also supervises its screen reader
   (below) while it waits.

### The screen reader at the login screen

The machine's `/etc/cupertino/accessibility.conf` may set `screen_reader_at_login = on` (a `SYSTEM_KEYS` boolean, never a command
or a path); the manager reads it, with the system-file rules, each time it starts the login screen and passes it in `Begin`.
For the login screen only, the worker makes a `SOCK_SEQPACKET` socketpair before the fork, and the leader gets its end as
descriptor 3 (`CUPERTINO_READER_FD=3`). The compositor sends `ready <socket>` once and `toggle <socket>` for Super+Alt+S from a
keyboard (`wayland-<digits>`, at most 64 bytes, anything else refused); the worker answers a toggle `on`, `off` or `unavailable`.

The reader is the file of the first of `reader::READERS` (`cupertino_defaults::accessibility::SCREEN_READERS`, `/usr/bin/orca`; a person's session runs the same one, started by its compositor) that only root can change, never one from `$PATH`; a link on the way is followed only when it is root's and in a directory only root can change (`cupertino_defaults::system::root_only_target`), so a distribution that ships orca as a link still has a reader. It runs while it
is wanted (the machine's setting, flipped by each toggle) and the login screen is on the console: the manager sends
`ToWorker::Shown { shown }` before its own switch to another session and after the login screen takes the console back, and
again on every change of the seat's `ActiveSession` (logind's `PropertiesChanged`), whoever switched: shown only while the
seat's active session is the login screen's, hidden when logind does not answer. A manager that is gone counts as not shown for
good. The worker forks it as a second child, so it is in the greeter's logind session and
cgroup. The child: `setsid`, the privilege drop and check of step 4, `PR_SET_PDEATHSIG(SIGKILL)` (after the drop, which clears
it), `PR_SET_NO_NEW_PRIVS`, then, as the greeter, `$XDG_RUNTIME_DIR/cupertino-reader-<worker>-<n>/{config,data,cache}` made with
`mkdirat` under an `O_DIRECTORY | O_NOFOLLOW` descriptor (mode 0700), standard input from `/dev/null`, `close_range` above 2, and
`execve` with only `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`,
`WAYLAND_DISPLAY` and `HOME` (the private directory). The bus is `unix:path=$XDG_RUNTIME_DIR/bus`, or PAM's value when it is
exactly a `unix:path=` directly in that directory.

Stopping it (a toggle, `Shown { shown: false }`, the leader's end, the manager's channel closing) sends `SIGTERM` to its process
group, `SIGKILL` after 2 s, reaps it, then kills every process of the greeter in the session scope's `cgroup.procs` that does not
descend from the leader (a daemon that left the group with `setsid`; only when the worker's cgroup is `session-<id>.scope`), and
removes the private directory in a child that dropped to the greeter: root never follows a path the greeter can write.

`greeter.sock` takes only the greeter's user, and, when this run started the login screen, only the leader or a descendant of
it: the peer is held by `SO_PEERPIDFD`, its parents are read from `/proc`, and both it and the leader must still run after. The
reader, a child of the worker, can never connect, and the login screen's compositor starts no program (cupertino-server's
README), so no process but the login screen's own descends from the leader.

The worker runs no threads (so `fork` is safe) and makes no D-Bus calls. It starts with an empty environment and its only
channel is one socketpair end as standard input.

## Logging rule

A log line never holds what the screen sent. Decoding errors are logged by class (`DecodeError::class`), listed accounts by uid,
a typed name not at all (`attempt for a typed name`: people type passwords there), and `ChannelError` / `FrameError` /
`DecodeError` carry no message content. PAM modules log account names themselves.

## Secret Service

Applications keep passwords and tokens (a browser's saved passwords, a mail account, an AI provider's key) in the
Secret Service, `org.freedesktop.secrets` on the user bus. Cupertino writes no provider of its own: the session uses
gnome-keyring's `secrets` component and unlocks it through PAM with the password this manager's worker checks.

**What an installed session had before** (Arch package and live image at `20c98802`, by reading the packages, PAM files,
units and the session script): no package depended on a provider, and the live image listed none, so most machines had no
`org.freedesktop.secrets` at all and libsecret clients failed or fell back to a plain store. Where gnome-keyring came in
through another package, its socket unit and D-Bus activation started it on first use, but none of the PAM services
called `pam_gnome_keyring` (Arch's `system-local-login` does not), so the login keyring stayed locked and the first
application asked for the password in a separate window. Nothing locked it at logout, and the portal routing sent the
Secret portal to the GTK backend, which has none, so sandboxed applications had no Secret portal.

**The choice.** gnome-keyring, because every target distribution packages it with its PAM module, it is the provider
libsecret clients are tested against, it brings the prompt for a locked keyring (gcr's system prompter) and a Secret
portal backend, and Arch enables its socket unit for every user when it is installed. KeePassXC or KWallet implement the
same interface; a user who installs one instead gets whichever takes the name first, but the packages and the tests
target gnome-keyring.

| What | Owned by |
|---|---|
| The daemon (`gnome-keyring-daemon --components=pkcs11,secrets`) | the gnome-keyring package: user units `gnome-keyring-daemon.socket` and `.service` (its install script runs `systemctl --global enable gnome-keyring-daemon.socket`), and D-Bus activation of `org.freedesktop.secrets`. The session starts it no other way; `cupertino-session.target` does not pull it in, so a daemon PAM started is never replaced by the socket |
| Unlock at login | PAM service `cupertino-session`: `-auth optional pam_gnome_keyring.so` right after the stack that checks the password keeps the password the stack accepted; `-session optional pam_gnome_keyring.so auto_start`, last, after `pam_systemd` has made `XDG_RUNTIME_DIR`, hands it to the daemon through its control socket (`$XDG_RUNTIME_DIR/keyring/control`, which starts the socket-activated service) or, when nothing listens there, starts the daemon with it. Both run in the worker's single PAM transaction (`Mode::Open`), so the password never leaves PAM |
| Auto login | PAM service `cupertino-autologin`: the session line only. There is no password, so the daemon runs locked |
| Unlock at the lock screen | PAM service `cupertino-login` (`cupertino-login`'s lock screen): `-auth optional pam_gnome_keyring.so` after `system-auth` unlocks a running daemon with the password that unlocked the screen |
| Lock at logout | the session command `cupertino-session` (below) |
| The prompt for a locked keyring | gcr's system prompter (`org.gnome.keyring.SystemPrompter`, a GTK window started on demand). It is not a Cupertino surface |
| Sandboxed applications | xdg-desktop-portal's Secret portal, routed to gnome-keyring in `cupertino-portals.conf` |
| Packages | Arch (`cupertino`, `cupertino-git`) depends on `gnome-keyring` and `libsecret` (`secret-tool`); the live image lists both. The `-` before each PAM line skips it silently when the module is not installed |

| Case | What happens |
|---|---|
| First login with a password | the daemon creates the login keyring, encrypted with that password, as the default collection, unlocked |
| Login | the keyring is unlocked before the session command starts |
| Authentication fails | the password stack fails first (`pam_faillock`'s `authfail` ends it), so `pam_gnome_keyring` never sees a wrong password: no session, the keyring is not touched |
| The password is accepted but is not the keyring's (changed with `passwd`, whose PAM file does not update the keyring) | the session opens with the keyring locked; the first client is refused (`IsLocked`) or its unlock opens the prompter, which asks for the keyring's own password |
| Locked keyring | reading or storing is refused with `org.freedesktop.Secret.Error.IsLocked`, a search lists the items as locked, and `Unlock` returns a prompt. Running the prompt opens the prompter; when the person dismisses it the prompt completes as dismissed and the keyring stays locked. Nothing is unlocked without the password |
| Auto login | locked, as above, until the person unlocks it in the prompter or unlocks the lock screen with the password |
| Screen locked | the keyring stays unlocked, as on other desktops; unlocking the screen unlocks a keyring that was locked |
| Fast user switching, the administrator's restart | `Mode::Verify` runs the same `auth` stack, but the worker sets no `XDG_RUNTIME_DIR` before `pam_authenticate`, so the module finds no daemon and keeps nothing: the session's keyring is not changed |
| Logout | after the compositor ends and `cupertino-session.target` is stopped, the session command lists the provider's `Collections` and calls `Lock` on all of them through `busctl --user --auto-start=no`, so a provider that is not running is not started. It logs `locked the keyring (N collections)` in `session.log`; a path that is not `/org/freedesktop/secrets/collection/…` stops it with a log line. The daemon itself ends with the user manager, after the user's last session. A session that did not own the user manager (another graphical session of the user was active) locks nothing |

Neither the password nor any secret is logged: PAM modules do not print the password, the daemon does not print values
(the tests check its output), and the session command never reads a secret.

**Hand-off to the login screen's UI** (#110 moves it to Kit). Nothing here is in UI code. The boundary is the PAM
transaction: the greeter answers the worker's prompts through `greeter.sock` (`Answer`), and the lock screen passes the
typed password to `cupertino_login::auth::Authenticator` (PAM service `cupertino-login`). A new UI keeps those two calls;
it needs to know nothing about the keyring.

**Seeing and deleting saved secrets.** No Cupertino window lists them yet. From a terminal in the session:

```
busctl --user get-property org.freedesktop.secrets /org/freedesktop/secrets/collection/login org.freedesktop.Secret.Collection Items
busctl --user get-property org.freedesktop.secrets ITEM org.freedesktop.Secret.Item Label Attributes
secret-tool search --all ATTRIBUTE VALUE      # label, attributes and the secret itself, printed on the terminal
secret-tool clear ATTRIBUTE VALUE             # deletes every item with that attribute
secret-tool lock --collection=/org/freedesktop/secrets/collection/login
```

Seahorse ("Passwords and Keys", not installed by the packages) shows and deletes items in a window. Removing
`~/.local/share/keyrings/login.keyring` while logged out deletes every saved secret; the next login makes a new keyring.

**Tests.** `tests/secret_service.rs` runs the real daemon on a private bus with no activatable services and a home of its
own, started as `pam_gnome_keyring` starts it (the password on its standard input): a client stores, reads and deletes a
secret over the D-Bus API and through `secret-tool`; a locked keyring refuses reads and writes and an unlock asks the
system prompter (a stand-in that refuses, as a dismissed window does); a wrong or missing password leaves it locked; the
keyring file is the account's alone; no secret is in the daemon's output. It is skipped with `SKIPPED:
gnome-keyring-daemon is not installed` when the daemon is missing, and its `secret-tool` part without `secret-tool`
(`CUPERTINO_KEYRING_REQUIRED=1`, set in CI, fails instead). `packaging/test_session.py` checks the lock at logout (with a
fake `busctl`, and with the real daemon when it is installed, required in CI the same way) and `packaging/test_release.py` the PAM lines and the packages. PAM itself is not run: that needs root.

**Not verified on an installed session.** On a machine installed from the live image, as a user with a password:

1. Log in at the login screen. `busctl --user get-property org.freedesktop.secrets /org/freedesktop/secrets/collection/login org.freedesktop.Secret.Collection Locked` prints `b false`; `systemctl --user status gnome-keyring-daemon.service` is active.
2. `printf secret | secret-tool store --label=check service cupertino-check`, then `secret-tool lookup service cupertino-check` prints `secret`; save a password in Chromium and see it after a restart of the browser.
3. Lock the screen (`Super+Ctrl+Q`), `secret-tool lock --collection=/org/freedesktop/secrets/collection/login` from an ssh login or a timer, then unlock the screen with the password: `Locked` is `b false` again.
4. Lock the keyring again and run `secret-tool lookup service cupertino-check`: the prompter window asks for the password; Cancel makes the lookup print nothing and exit 1; the right password prints `secret`.
5. Log out. From another login of the same user (a text console), `Locked` is `b true` and `~/.local/state/cupertino/session.log` has `locked the keyring`.
6. With auto login on, the session starts with `Locked` `b true` and the first lookup opens the prompter.
7. `secret-tool clear service cupertino-check`; the lookup prints nothing. `journalctl --user -b` and `session.log` hold neither the password nor `secret`.

## Security

| Measure | Where | Proved by |
|---|---|---|
| No core file, no ptrace attach while root: `PR_SET_DUMPABLE` 0 and `RLIMIT_CORE` 0 (and `LimitCORE=0` in the unit) | manager start, worker step 1 | by reading; the unit |
| Only the greeter account connects, one connection at a time: mode 0600 on the socket, `SO_PEERCRED` uid check | `manager::bind`, `accept` | unit test `the_socket_is_private_to_its_owner`; QEMU `29-auth-failure` (the guest account cannot connect, root is refused by the peer check, a second connection is refused) |
| Frame caps: 2 KiB from the screen, checked on the length prefix before any body is read; 4 MiB to it; the decoder asks for exact amounts | `cupertino-login-ipc::Decoder`, `Channel` | `cupertino-login-ipc` `tests/protocol.rs` (any chunking, over the cap, zero length, overrun, encode over the cap); `channel` tests; QEMU `29` (a 3 KiB frame closes the connection) |
| A strict state machine and `deny_unknown_fields`: any unknown type or field, any message the state does not allow, closes the connection; an `Answer` that does not name the open prompt is dropped (SEC-06) | `State::next`, `Attempt::answered`, `screen_readable` | `tests/protocol.rs` (every refused pair); `manager` tests `an_answer_is_taken_once_and_only_for_the_prompt_that_is_open`, `an_answer_of_a_finished_attempt_does_not_fit_the_next_attempt`; QEMU `29` |
| No secret in a log, panic or `Debug` output: `Secret` prints `<secret>`, decode errors keep only their class, the wire form is hex so the JSON parser never copies it into its unzeroed scratch buffer, buffers are `Zeroizing` (decoder bodies, encoders, the 4 KiB read buffer after each read) | `cupertino-login-ipc`, `Channel` | `tests/protocol.rs` (`a_secret_round_trips_and_its_text_never_shows_in_debug_output`, `a_malformed_secret_is_a_decode_error_that_does_not_echo_it`, `a_decode_error_names_the_class_and_never_the_value`); `channel` test `an_error_never_holds_the_text_that_caused_it`; QEMU `29` (five typed or sent strings are not in the journal) |
| The password does not outlive its use in memory: the binary's global allocator (`zeroing::ZeroOnFree`, shared with `cupertino-login` through `cupertino-login-ipc`) zeroes every block it frees, so the copy PAM's conversation receives as a `CString`, the frame it came from and the decoder's scratch are all cleared when dropped; PAM clears its own copy. A comment that says so would not count | `zeroing` | `zeroing` test `no_copy_of_the_answer_is_freed_with_the_password_in_it` (a watching allocator finds no freed block that still holds the answer; fails with the zeroing removed) |
| A signal is believed only from its service: `SessionRemoved` and the seat's `PropertiesChanged` from the owner of logind's name when each arrives (`cupertino_peer::signal::from_owner`), since a match rule's well-known `sender` filters nothing sent to the manager's unique name | `logind::Logind::watch` | `logind` test `only_logind_removes_a_session_or_moves_the_seat` (forged from another connection, to the manager and to everyone, then logind's own) |
| Not covered by the allocator: the kernel's socket buffers (the frame between the screen and the manager, and between the manager and the worker) are cleared on free only with `init_on_free=1`; the process does not call `mlockall`, so swap can hold a password (core dumps are off: `harden` sets `PR_SET_DUMPABLE=0` and `RLIMIT_CORE=0`) | none | none (known limitation) |
| A screen that dies mid-attempt ends the attempt and kills its worker; an accepted attempt survives its screen | `manager::cancel_attempt` | `manager` tests `a_screen_that_dies_mid_attempt_ends_the_attempt_and_kills_its_worker`, `an_accepted_attempt_survives_its_screen` (the wiring from `screen_readable`'s close to `drop_screen` is not covered: `Manager` cannot be built in a unit test) |
| No `unwrap`, `expect` or `panic! on anything from the socket or a worker: every failure closes the connection or fails the attempt | `manager`, `channel` | by reading; the tests above |
| Account input: a listed uid must be a row of the last list; a typed name passes `is_login_name`; anything else is `Denied` after the time a wrong password takes (no account enumeration) | `authenticate` | `tests/protocol.rs` (`typed_account_names_follow_the_limit_of_the_name_entry`); QEMU `29` (a typed unknown account: same line, 2.3 s against 1.8 s) |
| Attempt limit and timeout | `limits`, `abort_attempt` | `limits` test; QEMU `29` (the 11th attempt is `Busy`; a prompt left alone is cancelled after 60 s) |
| The worker starts with an empty environment (`env_clear`), one socketpair end as standard input, no other descriptor (all are close-on-exec) | `spawn_worker` | by reading |
| The session leader gets exactly the PAM environment, the login variables and the configured extras; its descriptors above 2 are closed, standard input is `/dev/null`, signal dispositions and mask are reset | `worker::spawn_leader` | QEMU `11-login-session` reads `/proc/<leader>/environ` (the PAM and login variables are there, the manager's `INVOCATION_ID` and the rest are not) |
| The privilege drop is `initgroups`, `setgid`, `setuid`, then `setuid(0)` must fail (and the real and effective ids must be the user's); `chdir($HOME) failing is an error | `worker::become_user_and_exec` | QEMU `11-login-session` reads `/proc/<leader>/status` (all four ids are the account's, no capability left) |
| The screen never chooses a command, an environment, a terminal, a class or an uid it was not shown | `ToWorker::Begin` is built from `login.conf` and the manager's own allocation | by the protocol: there is no message for any of them |
| Every bus method is authorised by the caller's process: a process in no session, in a session of another class or seat, in a background session (for `RequestPower`) or running as another user than its session is refused with `AccessDenied` before anything reaches the main loop; `SessionReady` also needs the session to be one the manager started for that uid | `caller::authorise`, `service::LoginManager::caller` | `caller` tests (every refusal and the one pass); `tests/service.rs` on a private bus (denials reach no `Call`, refusals come back by error name; on a bus with the system bus's limits, 3000 calls sent at once while logind is slow do not make the bus daemon refuse another caller, which gets the manager's own refusal); QEMU `29` (a service outside every session and root inside the session are refused, the harness's getty session is `UnknownSession`) |
| `Unlock` goes to the one agent that announced the session, and the agent unlocks only on a signal from the owner of the manager's name | `service::unlock`, `login_manager::watch_unlock` | `tests/service.rs` (a signal sent to one connection does not reach another); QEMU `30` (the wrong password leaves the session locked, the right one unlocks it) |
| A power action is checked by the manager, not trusted from the caller: the settings, other users' sessions (from logind, not from the manager's own table), the administrator rule and block inhibitors with root honouring them | `Manager::power` | QEMU `29` (`Disabled`, `OtherUsersLoggedIn` on the socket, `Inhibited` on the bus), `28` (an agent of a non-administrator is refused while another user's session is open) |
| A restart or shut down that ends other users' sessions needs the person at the login screen to be an administrator who proves her password: the account must be in an administrators' group, the password goes through PAM like a login's, and the action is the manager's own call to logind (honouring block inhibitors) | `Manager::authenticate_power`, `caller::may_end_other_sessions` | `caller` tests (every asker); QEMU `30` (a non-administrator with her right password, an administrator with a wrong one, and an administrator with hers while a program blocks shutdown: only the last is authorised, and logind refuses it) |
| Auto login needs a settings file only root can change, and happens once: the file and its directory are root-owned and not group- or world-writable, the file is not a link, the claim is exclusive and kept over restarts | `auto_login` | `auto_login` tests (every refusal, the exclusive claim); `cupertino-defaults` test (the key); QEMU `32-auto-login` (a 0664 file is refused and logs nobody in; a restart and a log out log nobody in again) |
| The administrators' groups come from a settings file only root can change, else from the build: `admin-groups` of an untrusted `login.conf` is ignored and logged; an empty list names no administrator; no listed group that exists means no administrator | `Manager::start`, `users::is_administrator`, `users::admin_logins` | `users` tests (any of the groups by `getgrouplist`, a missing group, no groups, the listed members each once); `cupertino-defaults` test (the key and the build's list); the trust filter by reading (the same rule as auto login's) |
| The machine's settings are written only for the installed settings program: its path is under an install root and the file the process runs is root-owned and writable by root alone | `service::LoginManager::console_administrator`, `service::only_root_changes` | `service` test `only_a_program_file_root_alone_can_change_passes_for_the_installed_one`; the rest by the `service` bus tests |
| Only sessions a PAM service of the manager opened are adopted, and the login screen's only when it is the greeter account's | `Manager::adopt_sessions` | QEMU `31-login-crash` (adopted), `29` (the getty's session stays `UnknownSession`) |
| A picture file is read as the user (`setfsuid`), without following links, from a regular file of that user of at most 256 KiB | `picture` | `picture` tests (symlink, directory, foreign owner, size cap) |

## Modules

| Module | Contents |
|---|---|
| `users` | `Account`, `login_accounts`, `admin_logins`, `by_name`, `by_uid`, `session_path` |
| `filter` | `Filters::apply`: the hidden, admin, allow and deny lists and the "Other…" rule |
| `roster` | `build` (rows, pictures within the budget, the row cap), `power_menu` ('s rule), `filters` |
| `picture` | `read(uid, home)`: the `setfsuid` read |
| `auto_login` | `settings_are_trusted` (the mode and owner rule), `claim` (the once-per-boot flag) |
| `caller` | `CallerSession`, `Need`, `Denial`, `authorise`: who may call the bus interface; `Asker`, `may_end_other_sessions`: who may restart or shut down while other sessions are open (pure) |
| `channel` | `Channel`: one read per call over a socket, frames of `cupertino-login-ipc` |
| `worker_protocol` | `ToWorker::Begin` (with `Mode::{Open, Verify}` and `screen_reader`), `ToWorker::Shown`, `FromWorker::{Started, Failed}`, `SessionClass` |
| `reader` (feature `app`) | the login screen's screen reader: `READERS`, `resolve`, `bus_address`, `parse_request`, `Wish`, `descends_from`, `kill_strays`, `Reader` |
| `manager`, `worker`, `vt`, `logind` (`SeatSession`: the sessions adoption reads), `service` (feature `app`) | the main loop, the PAM transaction and privilege drop, the terminals, the logind calls, the bus interface |

## Tests

`cargo test -p cupertino-sessiond --features app`: the roster (filters, picture budget, row cap, the `logged_in` mark, power rule), the account
database parsers, the picture read (missing file, symlink, directory, foreign owner, size cap), auto login's mode rule and exclusive claim, the socket's mode, the authoriser and the rule for ending other users' sessions
(`caller`), the screen reader (`reader`: the program list, the bus address, the requests, the ancestry walk, and a fake reader run as
the test's user that sees only its environment, descriptors 0 to 2 and its 0700 directories, and whose `setsid` daemon ends with it;
`manager`: only the leader's descendants may connect) and the bus interface on a private `dbus-daemon` (skipped when the binary is missing; `SwitchToLoginWindow` and the unicast `Unlock` included). The manager and
the worker run as root in the QEMU harness (`11-login-session`, `12-greeter-picker`, `14-greeter-filters`, `28`, `30-two-sessions`, `31-login-crash`, `32-auto-login`).

`tests/worker.rs` runs the worker as the manager starts it (`cupertino-sessiond worker`, an empty environment, a socketpair as
standard input) under a private PAM policy (`cupertino-test-pam`): no transaction reaches `/etc/pam.d`, the machine's accounts,
the journal or the buses, and the policy's account is the test's own user under a name of its own, so the leader runs without
root. It covers the hidden prompt relayed and answered; an accepted account's credentials, session and credentials again
around the leader, then the session closed and the credentials deleted, all on the session's terminal; the leader's
environment (the PAM environment, the login variables and the configured extra, nothing of the worker's own); the login
screen's class and screen reader socket; a user with a session already only checked; a wrong password and an unknown account
both `Denied`; too many tries, an expired account or password, a refused account, an account the check does not know, a stack
that fails to run and insufficient credentials each with its outcome; a lock-out message before the prompt relayed and
`Locked`; the account check's message relayed; a cancelled prompt and a yes/no question denied, a prompt with echo relayed; a
manager gone during the prompt; credentials or a session the stack refuses, or no login session registered, ending the worker
with nothing left established; and a session that fails to close reported.