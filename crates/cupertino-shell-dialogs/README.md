# cupertino-shell-dialogs

The portal's dialogs: the file chooser, which is the file family's open and save panel
(`cupertino-ui-files`' `panel::OpenSavePanel`), and the app chooser. The portal (`cupertino-portal`) answers the
`FileChooser` and `AppChooser` portal requests by asking these dialogs over `org.cupertino.Dialogs1`.

The crate is a library: one component, `Dialogs`, which the shell host (`cupertino-shell`) runs on a thread of its
own as `dialogs` of the server's component table.

| Module | What it holds |
|---|---|
| `ask` | what the portal asks for (`FileAsk`, `AppAsk`), decoded from the wire and bounded once more, and how each dialog ends (`FileAnswer`, `AppAnswer`) |
| `chooser` | one file chooser without drawing: the folder it shows, the filter (globs and MIME types), the selection, the name of a save, the choices, and the answer they make |
| `service` | `org.cupertino.Dialogs1`, served to the portal alone |
| `file`, `apps` | what an open file chooser and app chooser keep, and what each event of their windows does |
| `view` | the windows: the panel around a `FileList` of the chooser's rows, and the app chooser's list under its question |

## Running

The server lists `dialogs` to the shell host, which runs it; nothing starts it on its own. `compositor.json` names it
among the `components` of its `shell` section (`docs/architecture/shell-host.md`), and `alone` runs it in a host of
its own (`cupertino-shell --only dialogs`). A development build of the host makes it panic on request
(`cupertino shell crash dialogs --at paint|dispatch|helper`).

```
cargo run -p cupertino-shell-dialogs --example dialogs_capture -- dialogs/open --sheet   # also save, apps
```

The capture lists example folders under the temporary directory and runs with a home of its own under `target/`.

## org.cupertino.Dialogs1

Served at `/org/cupertino/Dialogs1` on the component's own session bus connection, which owns the name.

| Method | Answer |
|---|---|
| `ChooseFile(o handle, s kind, s app_id, s parent_window, s title, a{sv} options)` | `(u response, aay paths, i filter, a(ss) choices)` |
| `ChooseApplication(o handle, s app_id, s parent_window, s target, a(sss) choices, s last_choice, b wants_token)` | `(u response, s choice, s token)` |
| `UpdateChoices(o handle, a(sss) choices)` | nothing |
| `Close(o handle)` | nothing |

The response is 0 chosen, 1 cancelled, 2 ended without an answer (closed, or the component restarted). `kind` is
`open`, `save` or `save-files`. The options are those of the portal's request, already checked by it:
`accept_label s`, `modal b`, `multiple b`, `directory b`, `filters a(sa(us))`, `current_filter i` (an index into
`filters`, -1 or absent for none), `choices a(ssa(ss)s)`, `current_name s`, `current_folder ay`, `current_file ay` and
`files aay`. The answer's `filter` is an index into `filters` (-1 for none) and its `choices` each choice's id and
value. An application is `(id, name, icon)`, the icon empty for none. A `Choose` call's reply waits for the person's
answer; `Close` takes the dialog of `handle` down and its call answers 2.

Every method accepts only the program the server started as `portal` (D20, `Session1.ProgramOf`, asked through
`cupertino-peer` on a lookup connection of the component's own); any other caller gets
`org.freedesktop.DBus.Error.AccessDenied` and nothing is shown. The component bounds every value again: at most 16
dialogs open, 4 of one app, 1024 bytes of a text, 4096 of a path, 64 filters, 64 patterns of a filter, 16 choices,
64 options of a choice, 1024 files and 256 applications; text it shows loses control and bidirectional-override
characters, and an icon name with a `/` names none.

## What a restart loses

A failed component is started again alone by its host (D16, D21). The dialogs keep nothing of their own across a
restart:

- every open dialog goes, without an answer: each `Choose` call answers 2 (or fails, when the connection that served it
  went first), so the portal ends the app's request as cancelled and the app may ask again;
- a call of `org.cupertino.Dialogs1` made while the name has no owner is refused by the bus, and the portal answers it
  the same way;
- the places, the item icons and the folders read, which the new instance reads again.

## How it works

**Windows.** Each dialog is a Kit toplevel window (`Role::Window`), the child of the app's window the portal names
(`wayland:HANDLE`, through `xdg-foreign`), modal to it when the request is. A file chooser is 720 x 460 pt, at least
560 x 340, with no title bar: the panel's sidebar of places (the user folders), the Enclosing Folder row, the list of
the folder (name and kind, the item icons of the file family), the Save As field of a save, the filters, the choices,
and Cancel and the accept button. Saving over a file asks first, in the panel. An app chooser is a prompt window: its
question ("Open “Notes.txt” with:"), the applications with their icons, Cancel and Open. Return is the default button
(or opens the selected folder of a file chooser) and Escape cancels; a window the server closes cancels.

**Reading.** The places, the user folders, the MIME database and the icon themes are read once, as one task of Kit's
I/O pool; every folder is read as a task too, past which a folder on a hung mount is reported and the dialog keeps the
folder it showed. Nothing reads the disk on the UI thread: the item icons load lazily inside Kit's `FileIcons`.

**Activation.** When the app asks for an activation token, the app chooser asks the server for one made for the key or
click that chose, on its window, and answers once it arrives, so the opened application may take the focus.
