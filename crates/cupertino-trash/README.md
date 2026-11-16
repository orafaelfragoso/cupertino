# cupertino-trash

The trash as the freedesktop.org Trash specification 1.0 defines it, with no toolkit and no process of its own: the home
trash and the trash of a mount point, `trashinfo` records, name clashes, the `directorysizes` file, listing, restoring,
deleting one item and emptying the trash. Emptying takes the home trash and the user's trash folder on every mounted
volume that can be written (`.Trash/$uid` under a real, sticky `.Trash`, and `.Trash-$uid`, each a real directory the
user owns), deletes a link found there without following it, and reports what it could not delete, which stays in its
trash. `EMPTY_TITLE` and its neighbours are the one wording and picture of the question asked first, and of the alert
after an emptying that left something. The file manager (`cupertino-files`, which re-exports it as `trash`) and the dock
(its trash tile's Empty Trash) use the same code, so both delete under the same rules.

Absolute locations are injectable through `Trash`, so tests use temporary directories.
