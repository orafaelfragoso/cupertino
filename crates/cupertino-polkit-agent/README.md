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
- reaches the helper through polkit's socket (`/run/polkit/agent-helper.socket`) when it answers, else starts
  `polkit-agent-helper-1` from `/usr/lib/polkit-1/` or `/usr/libexec/` with an empty environment and the cookie on its
  standard input.

It exits with a failure (and the server restarts it) when a connection is lost, when polkitd refuses the registration
(another agent holds the session), or when polkit's helper is not installed.

## The prompt

One prompt is on screen at a time; the others wait in polkitd's order. Each output gets a scrim on the overlay layer,
above the menu bar and the dock, and the panel sits above them on the active output, where it takes the keyboard
exclusively. A click on the scrim does nothing. The server keeps the prompt's surfaces out of every capture.

The panel shows the lock symbol, "Authentication Required", polkit's message (at most six lines), a user pop-up when
polkitd offers more than one user, the field, the error, PAM's information and Caps Lock, a Details disclosure with the
program and the action, and Cancel and Authenticate. Return authenticates, Escape cancels, and Tab cycles inside the
panel. After three failed attempts polkitd gets the failure and the program shows its own error.

While the session is locked (`cupertino_app_effects_v1` version 5 `session_lock`) or isn't logind's active one,
nothing is shown: a prompt on screen is cancelled, and waiting ones stay until the session is back.

Assistive technology sees a modal alert dialog labelled by the title and described by the message. A secret field's
value is protected, so neither what is typed nor its length leaves the process; the error line is announced.

## Modules

| Module | What |
|---|---|
| `agent` | the agent on the system bus and its registration |
| `helper` | the conversation with polkit's helper |
| `model` | the prompt and the waiting requests, as a pure model |
| `panel` | the panel's layout and keyboard focus |
| `render` | the scrim and the panel drawn with `tiny-skia` |
| `access` | the tree assistive technology gets |
| `users` | how the prompt names polkitd's users |
| `surfaces` | the layer surfaces (feature `app`) |

The colours are the design tokens of the session's appearance; the scrim is `authentication-scrim`. Values marked
approximate in `panel` and `render` are Cupertino's own.

## Not yet

- The user pop-up steps through the users (click, Space, Up, Down) instead of opening a menu.
- The panel draws no shadow, and the scrim fades in but not out.
