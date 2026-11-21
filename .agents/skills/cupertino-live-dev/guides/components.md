# Components and what a restart costs

## Eligible components

A restart stops the component and starts it again at once: a component the development folder names runs in a host of its
own, which the server ends (SIGTERM, SIGKILL after 5 s) and starts again from the folder's file. Applications and the
other components stay connected; the component loses what it held only in memory.

| Name | What the person sees during the restart | What is lost |
| --- | --- | --- |
| `dock` | the dock disappears for a moment | an open overview or app switcher closes and its windows go back to their places |
| `menubar` | the menu bar disappears for a moment | an open menu closes |
| `launcher` | an open launcher panel closes | what was typed in it |
| `notifications` | banners and an open notification centre close | what the process held only in memory |
| `controlcenter` | an open control panel closes | what the process held only in memory |
| `osd` | an overlay on screen disappears | nothing the person set |
| `windowmanager` | windows stay where they are until the new host arranges them again | arrangements it held only in memory |
| `desktop` | the desktop's items disappear for a moment | a selection or an open rename |
| `overview` | an open overview closes | nothing the person set |
| `dialogs` | an open file or app chooser closes and its portal call ends with nothing chosen | the chooser's state |

Find a component's live host with `cupertino shell list` (the `COMPONENT` table gives its `HOST` and pid; pid and start time
are in the process table), not by the program name.

## Not eligible

These never start from the development folder:

- the server (`cupertino-server`) and anything linked into it, including vendored crates;
- `agent` (the session agent, `cupertino-shell-agent`), `login` (`cupertino-login`: the lock screen) and the session manager;
- the portal (`cupertino-portal`), the prompts (`cupertino-prompts`: authentication, keyring and consent) and desktop
  control (`cupertino-control`);
- data-control clients and the system daemons (network, Bluetooth, power).

Work on them nested or headless: `cupertino desktop start --backend nested --spawn foot` from inside the session, or
`cargo run -p cupertino-server -- --headless --screenshot out.png --exit-after-screenshot` for an offscreen capture. Their changes reach the
installed desktop through a package update, which the person decides on. After it, a process the server supervises
(`agent`, `login`, `portal`, `prompts`, `desktop-control`) runs the new program from `cupertino shell restart NAME`:

| Name | Defer the restart while | What is lost |
| --- | --- | --- |
| `agent` | a log out, restart or shut down panel is open or an action is under way | the open panel and what the agent held only in memory |
| `login` | the session is locked | what the lock screen held only in memory |
| `portal` | a screen share or a portal request is active | active screen-sharing sessions and outstanding requests |
| `prompts` | an authentication, keyring or consent prompt is open | the open prompt |
| `desktop-control` | an agent run you or someone else still needs is active | every run, yours included |

Only the server (and what it links) and the session manager need a new login; the system daemons follow their
system service units. If an
investigation truly needs a manual replacement, follow `docs/src/building.md` ("Components that are not eligible"):
identify the process through the server's runtime record, its parent and start time, keep a rollback copy, stage and
rename with the required owner and mode, and record the override.

## Regular applications

Files (`cupertino-files`) and Settings (`cupertino-settings`) are applications, not supervised components. Build them
with `--features app` and run `target/debug/cupertino-files` (or `-settings`) directly. When the change
concerns how the application starts or its single instance, quit the installed copy first; nothing in `/usr` changes.
