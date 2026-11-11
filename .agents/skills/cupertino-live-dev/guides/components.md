# Components and what a restart costs

## Eligible components

A restart ends the process (SIGTERM, SIGKILL after 5 s) and starts it again at once. Applications stay connected; the
component loses what it held only in memory.

| Name | What the person sees during the restart | What is lost |
| --- | --- | --- |
| `dock` | the dock disappears for a moment | an open overview or app switcher closes and its windows go back to their places |
| `menubar` | the menu bar disappears for a moment | an open menu closes |
| `launcher` | an open launcher panel closes | what was typed in it |
| `notifications` | banners and an open notification centre close | what the process held only in memory |
| `controlcenter` | an open control panel closes | what the process held only in memory |
| `osd` | an overlay on screen disappears | nothing the person set |
| `windowmanager` | windows stay where they are until the new process arranges them again | arrangements it held only in memory |

Find a component's live process with `cupertino shell list` (pid and start time), not by its program name.

## Not eligible

These never start from the development folder:

- the server (`cupertino-server`) and anything linked into it, including vendored crates;
- `login` (`cupertino-login`: the lock screen and the session agent) and the session manager;
- the portal (`cupertino-portal`), the authentication agent (`cupertino-polkit-agent`) and desktop control
  (`cupertino-control`);
- data-control clients and the system daemons (network, Bluetooth, power).

Work on them nested or headless: `cupertino desktop start --backend nested --spawn foot` from inside the session, or
the integration tests and `crates/cupertino-server/scripts/capture.sh` for offscreen captures. Their changes reach the
installed desktop through a package update, which the person decides on. After it, a process the server supervises
(`login`, `portal`, `polkit-agent`, `desktop-control`) runs the new program from `cupertino shell restart NAME`:

| Name | Defer the restart while | What is lost |
| --- | --- | --- |
| `login` | the session is locked | what the lock screen and session agent held only in memory |
| `portal` | a screen share or a portal request is active | active screen-sharing sessions and outstanding requests |
| `polkit-agent` | an authentication prompt is open | the open prompt |
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
