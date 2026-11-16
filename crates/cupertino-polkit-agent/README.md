# cupertino-polkit-agent

The session's password prompts: the polkit authentication agent (`cupertino-polkit-agent`) and the keyring's prompter
(`cupertino-keyring-prompter`, see [The keyring's prompter](#the-keyrings-prompter)), drawn alike.

The polkit authentication agent: When another program asks polkitd for an action that needs a password
(`pkexec`, mounting a disk, changing the date), polkitd asks this agent. The agent shows the prompt, starts polkit's
own setuid helper for the chosen user, and passes on what the person types. Only the helper checks the password, and
it tells polkitd the result itself; the agent never reports success. No Cupertino daemon asks polkit (architecture
rule 5).

## Running

Feature `app` builds the `cupertino-polkit-agent` and `cupertino-keyring-prompter` binaries. The server starts and
supervises each from its installed path, on a Wayland connection that may only draw the authentication prompt. Nothing
in `compositor.json` names them.

```
cargo build -p cupertino-polkit-agent --features app
```

The process:

- makes itself undumpable and sets its core size to zero before anything else;
- finds its logind session (`XDG_SESSION_ID`, else by process id) and registers at `/org/cupertino/PolkitAgent` with
  polkitd for that session, again whenever polkitd restarts;
- answers only polkitd: a `BeginAuthentication` or `CancelAuthentication` from any other bus name is refused, and at
  most four authentications may wait at once;
- believes a signal only from its sender: polkitd's restart only from the bus daemon's own `NameOwnerChanged`, and the
  session's `Active` only from the connection that owns logind's name when the change arrives. The bus delivers a signal
  addressed to the agent whatever its match rules say, so the agent reads plain match-rule streams and checks each message
  itself rather than rely on the filtering of zbus's proxy streams; its session proxy keeps no property cache;
- keeps no copy of what is typed: the field is allocated at its limit (512 bytes) so typing never moves it, the answer and
  the line written to the helper are zeroed when dropped, an answer prints as `Answer(<secret>)`, and the process's global
  allocator (`zeroing::ZeroOnFree`) zeroes every block it frees, which covers what the toolkit's keyboard handling allocates;
- reaches the helper through polkit's socket (`/run/polkit/agent-helper.socket`) when it answers, else starts
  `polkit-agent-helper-1` from `/usr/lib/polkit-1/` or `/usr/libexec/` with an empty environment and the cookie on its
  standard input.

It exits with a failure (and the server restarts it) when a connection is lost, when polkitd refuses the registration
(another agent holds the session), or when polkit's helper is not installed.

## The prompt

One prompt is on screen at a time; the others wait in polkitd's order. Each output gets a scrim on the overlay layer,
above the menu bar and the dock, and the panel sits above them on the active output, where it takes the keyboard
exclusively. A click on the scrim does nothing. A scrim is one pixel of its colour the compositor stretches over the output
(`ShmPresenter::draw_solid`), so its fade draws and uploads nothing; a compositor without one-pixel buffers gets
frames filled with the colour. The server keeps the prompt's surfaces out of every capture.

The panel shows the lock symbol, "Authentication Required", polkit's message (at most six lines), a user pop-up when
polkitd offers more than one user (a collapsed pop-up button to a screen reader), the field, the error, PAM's information and Caps Lock, a Details disclosure, and
Cancel and Authenticate. The details name the application the person used (polkitd's subject), the program that asked
polkitd when it is another one (its caller), and the action. Both are the executables
the kernel names for polkitd's process ids, never what a process says about itself; a process the user may not inspect
(one running as root) or that has ended gets no row. Return authenticates, Escape cancels, and Tab cycles inside the
panel. After three failed attempts polkitd gets the failure and the program shows its own error.

While the session is locked (`cupertino_app_effects_v1` version 5 `session_lock`) or isn't logind's active one,
nothing is shown: a prompt on screen is cancelled, and waiting ones stay until the session is back.

Assistive technology sees a modal alert dialog labelled by the title and described by the message. A secret field's
value is protected, so neither what is typed nor its length leaves the process; the error line is announced.

## Modules

| Module | What |
|---|---|
| `agent` | the agent on the system bus and its registration; the prompt's detail rows |
| `conversation` | the model's effects carried out: the helper started and answered, polkitd answered |
| `helper` | the conversation with polkit's helper |
| `model` | the prompt and the waiting requests, as a pure model |
| `panel` | the panel's layout and keyboard focus |
| `render` | the scrim and the panel drawn with `tiny-skia` |
| `access` | the tree assistive technology gets |
| `users` | how the prompt names polkitd's users |
| `zeroing` | the process's global allocator, which zeroes every block it frees |
| `surfaces` | the layer surfaces (feature `app`) |
| `keyring::exchange` | the keyring's secret exchange: DH, HKDF and AES |
| `keyring::model` | the keyring's callers and its prompt, as a pure model |
| `keyring::prompter` | the prompter on the session bus |
| `keyring::panel`, `keyring::render`, `keyring::access` | the keyring prompt's layout, drawing and accessible tree |
| `keyring::offer` | the offer to move a default keyring that login does not unlock into the login keyring: its flow and the move, as a pure model |
| `keyring::secrets` | the Secret Service the offer works on: the default and the login collection, an unlock, items read and stored over an encrypted session |
| `keyring::asking` | the offer carried out: its questions asked through the prompter, its answer kept |

The colours are the design tokens of the session's appearance; the scrim is `authentication-scrim`. Values marked
approximate in `panel` and `render` are Cupertino's own.

## The keyring's prompter

The keyring daemon (gnome-keyring) asks for passwords through a prompter on the session bus: to unlock a keyring, for a
new keyring's password and its confirmation, or to confirm an action. Its D-Bus activation files start a GTK window;
`cupertino-keyring-prompter` takes the two names first, so in a Cupertino session the questions appear in the
authentication prompt's panel instead. The server starts and supervises it like the polkit agent, from its installed
path and on a connection that may only draw the prompt, so its surfaces stay out of every capture and suspend desktop
control. Nothing under `/usr` changes: outside a Cupertino session the stock prompter still answers.

The process hardens itself, zeroes what it frees and keeps its fields at their limit (512 bytes) as the agent does, then:

- serves `org.gnome.keyring.internal.Prompter` at `/org/gnome/keyring/Prompter` and requests
  `org.gnome.keyring.SystemPrompter` and `org.gnome.keyring.PrivatePrompter`, replacing a prompter that allows it and
  allowing no replacement; when another prompter holds a name it waits in the bus's queue for it;
- takes one caller at a time (`BeginPrompting` with the path of its callback object); the others wait in order, at most
  eight. Only the active caller may ask (`PerformPrompt`, type `password` or `confirm`, with the properties it changed);
  another call is refused with `org.gnome.keyring.Prompter.InProgress`, a broken exchange with `…Failed`;
- answers through the caller's `org.gnome.keyring.internal.Prompter.Callback`: `PromptReady("", {}, exchange)` once a
  caller is active, `PromptReady("yes" | "no", {choice-chosen, password-strength}, exchange)` for each question (each
  value in a variant of its own inside the dictionary's, which is where the daemon reads it: a bare value is dropped,
  and the daemon then takes a new password for an empty one and the choice for not ticked), and
  `PromptDone()` when it ends a prompt itself (the session locks, the server closes the panel, or Escape for a caller that set `escape-dismisses`: a `no` followed by
  `PromptDone` is the prompter's answer, not the person's). The reply to a call is always sent before its
  callback is called, even for an answer given at once: the loop sends the reply itself. A caller that leaves the bus (the bus daemon's `NameOwnerChanged`) loses its prompt;
- returns the password only through the secret exchange (`sx-aes-1`): Diffie-Hellman in the 1536-bit MODP group of
  RFC 3526, HKDF with SHA256 of the shared value's bytes without leading zeros into a 16-byte key, 128-bit AES in CBC mode with PKCS#7
  padding and a fresh IV. A cancel or a confirmation carries no secret.

The panel is the agent's alert: 260 points wide, 26-point corners, the panel window's glass, the 64-point lock, the
caller's message in bold (its title when there is no message) and its description, the password field, the
confirmation field when `password-new` is set, a check box when `choice-label` is set, the caller's `warning` in red,
and Cancel and the continue button (`cancel-label`, `continue-label`, their mnemonic marks removed; Continue by
default) as 28-point capsules 8 points apart, 16 points in. Return in the first field of a new password goes on to the
confirmation; a confirmation that differs keeps the answer back and says so. After an answer the panel stays, busy,
until the caller asks again (the daemon's warning after a wrong password) or stops. `password-strength` goes back as 0
for an empty password, else 1. `caller-window` names an X11 window, which a Wayland surface can't be parented to: the
panel is modal over the active output whatever it names. While the session is locked or isn't logind's active one,
nothing shows: a question is answered `no`, the active caller hears `PromptDone`, and waiting callers wait.

Assistive technology sees a modal alert dialog; both fields are password inputs with a protected value.

## The default keyring that login does not unlock

Login unlocks the keyring named `login` and no other. When an application has made another keyring the default
collection, that keyring stays locked after login and asks for its own password in every session. Cupertino never
changes keyrings silently: the prompter asks once whether that keyring's items should move into the login keyring.

- **When.** The prompter that owns the prompter's names starts the check on a thread and a bus connection of its own
  once it serves, so neither its loop nor login waits for the Secret Service. It looks for `org.freedesktop.secrets`
  for 20 seconds without starting it, then reads the `default` alias and the login collection's lock.
- **Nothing is asked** when there is no Secret Service, when the default collection is the login one or there is
  none, when the login collection does not exist or is locked, after a remembered answer, or a second time in one
  login (`$XDG_RUNTIME_DIR/cupertino-keyring-offer` names the logind session that asked, so a prompter that starts
  again does not ask again).
- **The question** is asked as the keyring daemon asks its own: the check is a caller of the prompter (a `confirm`
  prompt), so it is the same alert, on a connection the server protects from capture and control, behind any prompt
  already waiting and never over a locked session. Its buttons are Move Items and Don't Move.
- **Don't Move** is kept in `$XDG_STATE_HOME/cupertino/keyring-offer` (`declined`) and the question never comes back.
  Only the button, clicked or pressed from the keyboard while it has the focus, declines. Escape means not now, as
  does a question the session took away (it locked, or the alert could not be shown): nothing is kept, this login
  does not ask again and the next one does. The check asks for that with a property of its own, `escape-dismisses`,
  on its questions: Escape then ends the prompt as the prompter itself does (`no`, then `PromptDone`). On the keyring
  daemon's questions, which do not set it, Escape is the cancel button.
- **Move Items** unlocks the old keyring through the daemon's own unlock prompt, which asks for its password once.
  Cancelling there is not a decline: the next login asks again. Then every item (label, attributes, secret, content
  type) is copied into the login collection beside what it holds, every copy is read back and compared, and only then
  the `default` alias is pointed at the login collection and read back. The old keyring keeps its items and is not
  deleted. `moved` is kept in the answer's file.
- **A failure** at any step deletes the copies this run made and leaves the alias as it was; the alert says that
  nothing was moved and why (an item could not be read, saved or did not match, the default changed meanwhile or could
  not be changed) and offers Try Again and Not Now. Nothing is remembered, so the next login asks again.
- **An alias that cannot be read back** after it was set is set to the old keyring again, and only when that call
  succeeds are the copies deleted as after any failure. When it fails too, the default may be the login keyring, so
  the copies stay (the login keyring is never left the default without the items) and the alert says so: the login
  keyring may now be the default, and the items are in both keyrings. Try Again, or the next login when the default is
  still the old keyring, finishes the move without copying an item twice; with the login keyring the default there
  is nothing left to ask.
- **A move that stopped halfway** (the process ended) can be repeated: an item the login keyring already holds
  identical is not copied again. An item of the login keyring with the same attributes and another secret is never
  replaced; the copy is stored beside it.
- **Secrets** cross the bus under the Secret Service's session encryption (`dh-ietf1024-sha256-aes128-cbc-pkcs7`),
  are held in buffers that are zeroed, and appear in no log line, error or `Debug` output: an error names the call
  and the service's error name only.

To be asked again after Don't Move, delete `~/.local/state/cupertino/keyring-offer` and log in again.

## Diagnostic events

The agent records versioned diagnostic events to the user's event store (`$XDG_STATE_HOME/cupertino/events`), each
beside the line it already writes; `main` installs the recorder first, before the process hardens itself. No event
names the user, polkitd's identities or anything typed.

| Event | Severity | Fields | Where |
|---|---|---|---|
| `polkit.register.failed` | error | `when` (`start`, or `again` after polkitd started anew), `error_name` (the D-Bus error name), `error` | polkitd does not take the agent's registration |
| `polkit.auth.failed` | warn | `action` (polkit's action id), `reason` (`refused`: PAM refused what was typed; `helper_failed`, `helper_start`, `helper_answer`: polkit's helper failed), `attempt` (1 to 3), `last` (the authentication ends with it), `error` (for the helper's failures) | each failed attempt of a prompt |
| `polkit.exit_error` | error | `error` | the agent ends with an error, or cannot harden itself |
| `polkit.panic` | fatal | `panic`, `location` | any panic |

## Tests

The keyring's prompter: `src/keyring/*/tests.rs` the exchange (both directions, padding, refused keys and messages),
the model, the panel's layout and keys, the drawing (warning and outline red, glass, check box) and the accessibility
audit; `tests/keyring.rs` the prompter on a private bus with a caller of the test's own, which decrypts the typed
password from the exchange: a password round trip, a wrong password asked again with the warning, a new password with
its confirmation and the choice, a confirmation, a cancel, one caller at a time, refusals, a locked session, and both
names on one connection; `tests/keyring_daemon.rs` the installed gnome-keyring daemon on a private bus unlocking its
login keyring with the password typed into the prompt, after a cancel and a wrong password, and making a keyring an
application asks for (`CreateCollection`) with the new password typed twice: a cancel, a confirmation that differs,
the keyring made and opening with that password only, and the ticked choice letting the daemon open it without a
question afterwards (skipped when the daemon is not installed; `CUPERTINO_KEYRING_REQUIRED=1` fails instead).

The offer about the default keyring: `src/keyring/offer/tests.rs` the model without a bus: when the question is
asked, accept, decline by the button and its memory, a question that went away without a button, a cancelled
password, each failure of the move leaving the default and both keyrings as they were, a default that cannot be read
back (put back, or the copies kept and the alert saying so) or reads back as the old or another keyring, a move
repeated after it stopped halfway, and no secret in what is printed; `src/keyring/model/tests.rs` Escape as a cancel
and, for a caller that asks for it, as the end of its prompt; `src/keyring/render/tests.rs` the alert shows the question's and each failure's whole text;
`tests/keyring_offer.rs` the installed daemon on a private bus with a second, locked keyring as the default: nothing
asked without a service, with the login keyring the default or locked; a cancelled password, a locked session, Escape
and a decline leave everything as it was and only the decline is kept; accepting copies every item (read back over a
session of the test's own), skips the one already copied, keeps a differing item, changes the alias and keeps the old
keyring.

The polkit agent:

`cargo test -p cupertino-polkit-agent`: the model, the panel, the text and the helper's protocol as unit tests;
`tests/agent.rs` the agent on a private bus with a fake polkitd (registration, refusals, limits, a forged restart that
registers nothing); the `logind` unit test a forged `Active` change ignored and logind's own followed; `tests/zeroing.rs` a
password typed, answered to the fixture helper and dropped, with a watching allocator that finds no freed block holding any
piece of it (the field never grows, the model test `typing_up_to_the_limit_never_moves_the_field`); `tests/flow.rs` a whole
authentication without a display: a fake polkitd asks, the model and `conversation` run the fixture helper
(`tests/fixtures/polkit-agent-helper-1`, which accepts "right"), and the test types. It checks the reason and the
application the prompt shows, that a valid password answers polkitd, that three wrong ones are retried and then end the
authentication, and that Escape and polkitd's cancel answer `Cancelled`. polkitd and its real helper are not run: they
need the system bus and root. The panel's unit tests run `cupertino-ui-test`'s accessibility audit on every phase of
the prompt and check that no typed password reaches its accessible tree.

**Not verified on an installed session.** Logged in on a machine installed from the live image:

1. In a terminal, `pkexec true`: the prompt shows "Authentication is needed to run `/usr/bin/true' as the super user"
   (polkit's own text); Details shows the shell as the application and `org.freedesktop.policykit.exec`.
2. Type a wrong password three times: "Incorrect password. Try again." twice, then the prompt closes and `pkexec` prints
   that the authorization failed (`echo $?` is 127).
3. `pkexec true` again, Escape: `pkexec` reports the request dismissed (`echo $?` is 126). Again with the right password:
   exit 0.
4. Mount a disk from the file manager as a user that needs a password (or `udisksctl mount -b DEVICE`): the prompt
   shows udisks' reason and the file manager as the application.
5. Lock the screen while a prompt is up: it is dismissed; after unlocking, a new `pkexec true` prompts again.

## Not yet

- The user pop-up steps through the users (click, Space, Up, Down) instead of opening a menu.
- The panel draws no shadow, and the scrim fades in but not out.
