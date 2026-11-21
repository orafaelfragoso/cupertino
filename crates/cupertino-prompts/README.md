# cupertino-prompts

The session's prompts (`cupertino-prompts`): one Kit program, always running, that holds every prompt the shell asks
the person on another program's behalf. Password entry stays in this small process, away from the components that
handle untrusted content and from every window title and screen content (D10).

- **The authentication prompt.** When another program asks polkitd for an action that needs a password (`pkexec`,
  mounting a disk, changing the date), polkitd asks this program's agent. The agent shows the prompt, starts polkit's
  own setuid helper for the chosen user, and passes on what the person types. Only the helper checks the password, and
  it tells polkitd the result itself; the agent never reports success. No Cupertino daemon asks polkit (architecture
  rule 5).
- **The keyring's prompt.** The keyring daemon (gnome-keyring) asks for passwords through a prompter on the session
  bus: to unlock a keyring, for a new keyring's password and its confirmation, or to confirm an action. This program
  takes the prompter's names, so in a Cupertino session the questions appear in the same alert.
- **The portal's consent questions.** The portal (`cupertino-portal`) asks over `org.cupertino.Prompts1` whether an
  application may take a screenshot, and whether it may use keyboard shortcuts while another application has the
  keyboard. The portal decides what to ask and keeps the answers; this program shows the question and says what the
  person answered.
- **Desktop control's consent question.** When an app asks to control or capture the desktop and the person has not
  decided for it yet, the server asks whether to allow it through the consent authority this program holds. The server
  decides when to ask and keeps the answer; this program shows the question and says what the person answered.

## Running

The server starts and supervises the program from `/usr/bin/cupertino-prompts` in every user session, on a Wayland
connection with the `session`, `prompt` and `consent` roles (`cupertino shell list` names it `prompts`); the `consent`
role is the one that sees the server's consent authority (`cupertino_control_authority_v1`). Nothing in
`compositor.json` names it. With the `prompt` role every surface of the program is a prompt surface: above every other
surface, left out of every capture and out of desktop control, and while one is shown no application holds the pointer
or the shortcuts and desktop-control runs are suspended.

```
cargo build -p cupertino-prompts
```

The process:

- makes itself undumpable and sets its core size to zero before anything else (`cupertino_kit::process::harden`), after
  installing its diagnostic recorder;
- runs on CupertinoKit's loop (`cupertino_kit_shell::run`): every bus call, the reads of polkit's helper and the
  writes to it, and the consent alerts' arming delay are futures of that loop; finding and starting the helper, reading
  the password database and `/proc`, the keyring offer's files and the applications' icons run on Kit's I/O pool. No
  thread of its own, no polling;
- keeps no copy of what is typed beyond the field's own buffer and one zeroing copy for the button that submits it:
  answers and the lines written to the helper are zeroed when dropped, an answer prints as `Answer(<secret>)` and a
  field's text as `Secret(..)`, and the process's global allocator (`cupertino_login_ipc::zeroing::ZeroOnFree`)
  zeroes every block it frees, which covers what the toolkit's keyboard handling allocates;
- exits with a failure, and the server starts it again, when one of its bus connections or its Wayland connection is
  lost.

[`run`](src/lib.rs) takes [`Options`]: where polkit's helper is reached (found where polkit installs it, or given) and
whether to make the keyring offer. A program test runs the program in its process with its own options.

## The authentication agent

- finds its logind session (`XDG_SESSION_ID`, else by process id) and registers at `/org/cupertino/PolkitAgent` with
  polkitd for that session, again whenever polkitd starts anew. A registration polkitd refuses is recorded
  (`polkit.register.failed`) and the program goes on serving the keyring's prompts; without a logind session or without
  polkit's helper the agent does not register, so polkitd asks another agent;
- answers only polkitd: a `BeginAuthentication` or `CancelAuthentication` from any other bus name is refused, and at
  most four authentications may wait at once;
- believes a signal only from its sender: polkitd's restart only from the bus daemon's own `NameOwnerChanged`, and the
  session's `Active` only from the connection that owns logind's name. The bus delivers a signal addressed to the
  program whatever its match rules say, so it reads plain match-rule streams and checks each message itself; its
  session proxy keeps no property cache;
- reaches the helper through polkit's socket (`/run/polkit/agent-helper.socket`) when it answers, else starts
  `polkit-agent-helper-1` from `/usr/lib/polkit-1/` or `/usr/libexec/` with an empty environment and the cookie on its
  standard input.

The alert shows the lock symbol, "Authentication Required", polkit's message, a User pop-up button with its menu when
polkitd offers more than one user, the field (a password field, or a plain one when PAM asks for text it shows), the
error, PAM's information and Caps Lock, a Details disclosure, and Cancel and Authenticate. The details name the
application the person used (polkitd's subject), the program that asked polkitd when it is another one (its caller),
and the action. Both are the executables the kernel names for polkitd's process ids, never what a process says about
itself; a process the user may not inspect (one running as root) or that has ended gets no row. Return authenticates,
Escape cancels, and Tab cycles inside the alert. After three failed attempts polkitd gets the failure.

## The keyring's prompter

The prompter has a session bus connection of its own, on which it serves no object: it reads every call itself, so
nothing answers a caller before the prompter decided. It

- requests `org.gnome.keyring.SystemPrompter` and `org.gnome.keyring.PrivatePrompter`, replacing a prompter that allows
  it and allowing no replacement; when another prompter holds a name it waits in the bus's queue for it;
- serves `org.gnome.keyring.internal.Prompter` at `/org/gnome/keyring/Prompter` and takes one caller at a time
  (`BeginPrompting` with the path of its callback object); the others wait in order, at most eight. Only the active
  caller may ask (`PerformPrompt`, type `password` or `confirm`, with the properties it changed); another call is
  refused with `org.gnome.keyring.Prompter.InProgress`, a broken exchange with `…Failed`;
- answers through the caller's `org.gnome.keyring.internal.Prompter.Callback`: `PromptReady("", {}, exchange)` once a
  caller is active, `PromptReady("yes" | "no", {choice-chosen, password-strength}, exchange)` for each question (each
  value in a variant of its own inside the dictionary's, which is where the daemon reads it), and `PromptDone()` when it
  ends a prompt itself (the session locks, the server closes the alert, or Escape for a caller that set
  `escape-dismisses`). The reply to a call always goes before its callbacks: one outbox carries both in order. A caller
  that leaves the bus (the bus daemon's `NameOwnerChanged`) loses its prompt;
- returns the password only through the secret exchange (`sx-aes-1`): Diffie-Hellman in the 1536-bit MODP group of
  RFC 3526, HKDF with SHA256 of the shared value's bytes without leading zeros into a 16-byte key, 128-bit AES in CBC
  mode with PKCS#7 padding and a fresh IV. A cancel or a confirmation carries no secret.

The alert is the authentication prompt's: the caller's message in bold (its title when there is no message) and its
description, the password field, the confirmation field when `password-new` is set, a check box when `choice-label` is
set, the caller's `warning`, and Cancel and the continue button (`cancel-label`, `continue-label`, their mnemonic
marks removed). A confirmation that differs keeps the answer back and says so. After an answer the alert stays, busy,
until the caller asks again or stops. `password-strength` goes back as 0 for an empty password, else 1.
`caller-window` names an X11 window, which a Wayland surface can't be parented to: the alert is modal over the active
output whatever it names.

## The default keyring that login does not unlock

Login unlocks the keyring named `login` and no other. When an application has made another keyring the default
collection, that keyring stays locked after login. Cupertino never changes keyrings silently: the prompter asks once
whether that keyring's items should move into the login keyring.

- **When.** The prompter that owns the prompter's names starts the check once it serves, as a future of its loop on
  the program's main session bus connection. It waits up to 20 seconds for `org.freedesktop.secrets` to appear
  (following the bus's `NameOwnerChanged`, never starting it), then reads the `default` alias and the login
  collection's lock.
- **Nothing is asked** when there is no Secret Service, when the default collection is the login one or there is none,
  when the login collection does not exist or is locked, after a remembered answer, or a second time in one login
  (`$XDG_RUNTIME_DIR/cupertino-keyring-offer` names the logind session that asked).
- **The question** is asked as the keyring daemon asks its own: the offer is a caller of the prompter inside the
  process, so it is the same alert, behind any prompt already waiting and never over a locked session. Its buttons are
  Move Items and Don't Move.
- **Don't Move** is kept in `$XDG_STATE_HOME/cupertino/keyring-offer` (`declined`) and the question never comes back.
  Escape means not now, as does a question the session took away: nothing is kept, this login does not ask again and
  the next one does.
- **Move Items** unlocks the old keyring through the daemon's own unlock prompt, copies every item into the login
  collection, reads every copy back, and only then points the `default` alias at the login collection and reads it
  back. The old keyring keeps its items. A failure deletes the copies this run made and leaves the alias as it was;
  the alert says why and offers Try Again and Not Now.
- **Secrets** cross the bus under the Secret Service's session encryption (`dh-ietf1024-sha256-aes128-cbc-pkcs7`), are
  held in buffers that are zeroed, and appear in no log line, error or `Debug` output.

To be asked again after Don't Move, delete `~/.local/state/cupertino/keyring-offer` and log in again.

## The portal's consent questions

The program serves `org.cupertino.Prompts1` at `/org/cupertino/Prompts1` on its main session bus connection, which
owns the name (`consent::service`):

| Method | Answer |
|---|---|
| `AskScreenshot(o handle, s app_id, s app_name, s app_icon, s parent_window)` | `u`: 0 granted, 1 cancelled |
| `AskShortcuts(o handle, s app_id, s app_name, s app_icon, s parent_window, a(sss) shortcuts, u allowed)` | `u`: 0 allowed, 1 denied, 2 dismissed |
| `Close(o handle)` | the question of the portal's request `handle` goes; its call answers as dismissed |

- **Only the portal asks.** Every method accepts only the program the server started as `portal` (D20:
  `Session1.ProgramOf`, asked through `cupertino-peer`'s `Lookup` on a session bus connection of its own, which reads
  nothing else). Any other caller gets `org.freedesktop.DBus.Error.AccessDenied` and nothing is shown. At most 16
  questions wait at once (`LimitsExceeded` past that), 64 shortcuts a call, and every text is at most 1024 bytes and
  loses its control and bidirectional-override characters.
- **The reply waits for the person.** An `Ask` call is answered once the question is: by the person, by `Close`, or
  because the session went away.
- **The portal words the rows.** Each shortcut is `(description, keys, note)`, worded by the portal; the note says why
  the shortcut gets no key (`not available: it needs ⌃, ⌥ or ⌘`) and is empty when it gets its key. A row shows as
  `⌃⌥T  Push to talk`, with `(note)` after it in secondary text when there is one. `allowed` counts the app's
  shortcuts allowed before and not listed: with any, the alert asks for "more" shortcuts.
- **The alerts.** The application's icon (its `Icon=` name, read from the host's icon themes on Kit's I/O pool; the
  applications symbol until it is read or when there is none) stands where the lock symbol goes, then the question in
  bold, worded as the portal's windows did: "“<app>” wants to take a screenshot of your screen." with Cancel and Take
  Screenshot (one above the other, Take Screenshot first, as the label is too long for the pair), and "“<app>” wants
  to use these keyboard shortcuts, also while you work in other apps." with the shortcut list, which grows by its rows'
  lines up to ten and scrolls past them, and Don't Allow and Allow. The alerts are named "Screenshot" and "Keyboard
  Shortcuts" to assistive technology. `app_name` empty shows "An application"; `parent_window` is read and bounded,
  but an alert on the prompt layer has no parent.
- **Arming.** The keyboard focus starts on the deny button, so the answer a stray key reaches is the one that denies.
  The allow button takes a press only 500 ms (`consent::ARMING`) after the alert presented its first frame, and until
  then Return, Enter and Space press no button, the deny button included: neither a key nor a click meant for the
  application can consent. Once armed, Tab to the allow button and Return or Space grants. Escape, the deny button and
  the server closing the alert deny at once, armed or not: Escape is cancelled for a screenshot and dismissed for
  shortcuts, which the portal does not remember.

## Desktop control's consent question

The program binds the server's consent authority as it starts (`cupertino_kit_shell::consent`). The server asks one
run at a time, after naming the app it comes from, and says when the run stops waiting (`desktop_control`,
`program::desktop_control`):

- **The alert.** The app's icon (its desktop entry's, read on Kit's I/O pool; the applications symbol until it is read,
  or when the app names an executable rather than a desktop entry), "Allow “<app>” to control the desktop?" (or to
  capture the screen, or both) in bold, the rights the run asks for, the display, the executable's path for an app
  without a desktop entry, whether the answer is remembered for the app, and that Ctrl+Alt+Escape stops control; Don't
  Allow and Allow. A controller that names no app is asked as "the desktop controller", for this request only. Names
  lose their control and bidirectional characters and are cut (48 characters for an app's name, 120 for a path, which
  keeps its end, 64 for a display). The alert goes on the display the run would control when the server describes it,
  else on the active output, and its surface is the one the server takes the answer on (`set_window`).
- **Arming.** As for the portal's questions, the focus starts on Don't Allow, and Allow takes a press only 500 ms
  (`consent::ARMING`) after the alert presented its first frame. Don't Allow, Escape and the server closing the alert
  answer at once: Don't Allow is remembered for the app, Escape and a closed alert refuse this request only.
- **Who can answer.** The server takes Allow, and a remembered Don't Allow, only with fresh physical input the person
  gave this surface after the run asked; scripted input and desktop control's own input never reach the alert or
  count (`crates/cupertino-server/src/control.rs`). The alert stays until the server says the run no longer waits, so
  an answer the server did not take can be given again.
- **While the session is away** the request asked is dismissed, and one asked then is dismissed at once: nothing is
  remembered for an answer nobody gave. The server ends the run as the session locks.

## The alert

One prompt is on screen at a time: the one shown first stays until it ends, an authentication comes before a keyring
question that waits, the keyring before the portal's consent questions, which come in the order they were asked, and
they before desktop control's question. Each output gets a scrim on the overlay layer (`authentication-scrim`, fading in over
0.2 s along `Timing::AUTHENTICATION_SCRIM_FADE_IN`, at once with Reduce Motion), and the alert sits above them on the
active output, where it takes the keyboard exclusively. A click on the scrim does nothing. The alert is 260 points
wide with 26-point corners on the regular glass, the popover surface's fill where glass is off, and an outline with
Increase Contrast; the 64-point lock symbol, 13-point text, 28-point capsule buttons 8 points apart, 16 points in.

While the session is locked or isn't logind's active one nothing is shown: an authentication on screen is cancelled
and waiting ones stay until the session is back; a keyring question is answered `no`, the active caller hears
`PromptDone`, and waiting callers wait; every consent question, shown or waiting, is answered cancelled or dismissed,
and one asked while the session is away is answered so at once, so nothing is granted or remembered.

Assistive technology sees a modal alert dialog labelled by the title and described by the message. A password field's
value is protected, so neither what is typed nor its length leaves the process.

## What a restart loses

The program keeps nothing on disk but the keyring offer's answer. A restart loses the prompt on screen and the waiting
ones: polkitd sees the agent's bus name go and fails the authentications it asked for, and registers the agent again
when it starts; a keyring caller loses its prompter and gets an error from its next call; the portal's consent calls
fail, which the portal takes as Cancel for a screenshot and as dismissed for shortcuts, so nothing is granted or
remembered; the server ends a desktop-control run that waited for the person's answer and asks again once the restarted
program binds the authority, when the app asks again. Nothing typed survives.

## Modules

| Module | What |
|---|---|
| `polkit` | an authentication request, its outcome and the rows of its details |
| `polkit::agent` | the agent on the system bus and its registration |
| `polkit::helper` | the conversation with polkit's helper |
| `polkit::model` | the prompt and the waiting requests, as a pure model |
| `logind` | the process's session and whether it is the active one |
| `users` | how the prompt names polkitd's users |
| `keyring::exchange` | the keyring's secret exchange: DH, HKDF and AES |
| `keyring::model` | the keyring's callers and its prompt, as a pure model |
| `keyring::prompter` | the prompter on its own session bus connection |
| `keyring::offer` | the offer about the default keyring: its flow and the move, as a pure model |
| `keyring::secrets` | the Secret Service the offer works on |
| `keyring::asking` | the offer carried out, asking through the prompter inside the process |
| `consent` | the portal's consent questions, their answers and the arming delay |
| `consent::model` | the waiting consent questions, as a pure model |
| `consent::service` | `org.cupertino.Prompts1` and who may call it |
| `desktop_control` | desktop control's consent question and the texts its alert says |
| `program` | the Kit program: the model of every prompt, the buses and the futures |
| `view` | the alerts and the scrims |
| `view::consent` | the consent alerts and their wording |
| `view::desktop_control` | desktop control's consent alert |

## Captures

`cargo run -p cupertino-prompts --example prompts_capture -- prompts/SURFACE [flags]` (`crates/cupertino-kit/README.md`):
`authentication` (`--details hidden|shown`, `--users one|two`), `keyring` (`--kind password|new|confirm`), and the consent
alerts `screenshot`, `shortcuts` and `desktop-control` (armed, with the applications symbol for the app's icon).

## Diagnostic events

The program records versioned diagnostic events to the user's event store (`$XDG_STATE_HOME/cupertino/events`); `main`
installs the recorder first. No event names the user, polkitd's identities or anything typed.

| Event | Severity | Fields | Where |
|---|---|---|---|
| `polkit.register.failed` | error | `when` (`start`, or `again` after polkitd started anew), `error_name` (the D-Bus error name), `error` | polkitd does not take the agent's registration |
| `polkit.auth.failed` | warn | `action` (polkit's action id), `reason` (`refused`: PAM refused what was typed; `helper_failed`, `helper_start`, `helper_answer`: polkit's helper failed), `attempt` (1 to 3), `last` (the authentication ends with it), `error` (for the helper's failures) | each failed attempt of a prompt |
| `prompts.exit_error` | error | `error` | the program ends with an error, or cannot harden itself |
| `prompts.panic` | fatal | `panic`, `location` | any panic |
