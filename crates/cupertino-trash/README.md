# cupertino-trash

The trash as the freedesktop.org Trash specification 1.0 defines it, with no toolkit and no process of its own: the home
trash and the trash of a mount point, `trashinfo` records, name clashes, the `directorysizes` file, listing, restoring,
deleting one item and emptying the trash. Emptying takes the home trash and the user's trash folder on every mounted
volume that can be written (`.Trash/$uid` under a real, sticky `.Trash`, and `.Trash-$uid`, each a real directory the
user owns), deletes a link found there without following it, and reports what it could not delete, which stays in its
trash. `EMPTY_TITLE` and its neighbours are the one wording and picture of the question asked first, and of the alert
after an emptying that left something. The file manager (`cupertino-files`, which re-exports it as `trash`) and the dock
(its trash tile's Empty Trash) use the same code, so both delete under the same rules.

A mounted volume may not answer: a network mount whose server went away blocks every call on it for minutes. Every
function here blocks and starts no thread; a program runs the ones that touch the volumes as tasks (in a Kit program
`kit::task::io`, whose deadline reports a look held up by a volume). Each look at the volumes (`volume_trash_dirs`,
`entries`, `holds_items`, `list_all`, `empty` and their neighbours) reads them one after another on the calling
thread, one look at a time per volume in the process: a look that finds another still out on a volume waits
`VOLUME_ANSWERS_WITHIN` (2 s) at most for it, then counts that volume as holding no trash for that look
(`silent_volumes` names it, and `empty` reports it as one failure by its mount point), so looks cannot pile up behind
a silent volume. The look that is out waits for the volume and gives it back when it answers after all. The home
trash never waits for a volume: `home_holds_items` and `home_entries` touch none, `holds_items` asks the volumes only
when the home trash is empty, and `empty` and `list_all` take the home trash first. A deletion on a volume that stops
answering while it is being emptied is not bounded: it holds up the volumes after it, never the home trash.

`Look` is the trash for a program's UI thread (the dock's tile, the file manager's windows): `full` is the home trash
as it is now or the volumes' as last looked, and asks for the next look at the volumes; `next` hands that look out,
unless one is still out, as blocking work for a task, which says whether the volumes' answer changed.

Absolute locations are injectable through `Trash`, so tests use temporary directories; `with_volume_look` is called
with each mount point before it is read (a test stands in for a silent volume by not returning from it) and
`with_volume_wait` sets how long a volume is given.
