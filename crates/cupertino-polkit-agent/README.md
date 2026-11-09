# cupertino-polkit-agent

The session's polkit authentication agent. When another program asks polkitd for an action that needs a password
(`pkexec`, mounting a disk, changing the date), polkitd asks this agent. The agent shows the prompt, starts polkit's
own setuid helper for the chosen user, and passes on what the person types. Only the helper checks the password, and
it tells polkitd the result itself; the agent never reports success. No Cupertino daemon asks polkit (architecture
rule 5).

## Running

Feature `app` builds the `cupertino-polkit-agent` binary. The server starts and supervises it from its installed path,
on a Wayland connection that may only draw the authentication prompt. Nothing in `compositor.json` names it.

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

The colours are the design tokens of the session's appearance; the scrim is `authentication-scrim`. Values marked
approximate in `panel` and `render` are Cupertino's own.

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
