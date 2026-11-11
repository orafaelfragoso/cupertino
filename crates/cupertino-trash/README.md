# cupertino-trash

The trash as the freedesktop.org Trash specification 1.0 defines it, with no toolkit and no process of its own: the home
trash and the trash of a mount point, `trashinfo` records, name clashes, the `directorysizes` file, listing, restoring,
deleting one item and emptying the trash. The file manager (`cupertino-files`, which re-exports it as `trash`) and the dock
(its trash tile's Empty Trash) use the same code, so both delete under the same rules.

Absolute locations are injectable through `Trash`, so tests use temporary directories.
