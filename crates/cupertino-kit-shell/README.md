# cupertino-kit-shell

The shell's extension to CupertinoKit (`shellkit` layer). `ComponentName` names a component in the server's table.
`bus::Message` carries a publication, its topic and whether the bus retains it. A publication can name an output
by its registry name or a transaction by its creator, instance and serial. `bus::Sender` distinguishes the host from
a component on the wire.

`test_clock` gives the clocks for what a component draws, including the debug build's fixed display clock.
The [shell host specification](../../docs/architecture/shell-host.md#cupertino-kit-shell) describes the component interface.
